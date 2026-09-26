// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// bindCase is one `[web] bind to` value. In bindTo and targets, {port} is the readiness port, {pN} the case's N-th
// extra port, {pN+65536} that port plus 65536, and {shared} a directory the two runs use in turn (unix paths must be
// the same length on both sides, since C cuts them at 107 bytes).
type bindCase struct {
	bindTo string
	// ports is how many extra ports the case needs.
	ports int
	// extra is appended to [web].
	extra string
	prep  func(shared string) error
	// targets get GET /index.html: "unix:<path>" or "tcp:<host>:<port>".
	targets []string
	// service, when set, is an /etc/services name the case binds; it is skipped when that port is taken.
	service string
}

// bindResult is what one daemon did with a case.
type bindResult struct {
	start     string
	listeners []string
	records   []string
	statuses  []string
	unix      []string
}

var extraPortRe = regexp.MustCompile(`\{p(\d+)(\+65536)?\}`)

// freePorts picks n ports free for TCP and UDP on 127.0.0.1 and TCP on ::1, all distinct.
func freePorts(t *testing.T, n int) []int {
	t.Helper()
	var ports []int
	var held []net.Listener
	defer func() {
		for _, l := range held {
			l.Close()
		}
	}()
	for len(ports) < n {
		l, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		held = append(held, l)
		p := l.Addr().(*net.TCPAddr).Port
		if l6, err := net.Listen("tcp", fmt.Sprintf("[::1]:%d", p)); err == nil {
			l6.Close()
		} else {
			continue
		}
		if u, err := net.ListenPacket("udp", fmt.Sprintf("127.0.0.1:%d", p)); err == nil {
			u.Close()
		} else {
			continue
		}
		ports = append(ports, p)
	}
	return ports
}

func expandBind(s string, port int, extra []int, shared string) string {
	s = strings.ReplaceAll(s, "{port}", strconv.Itoa(port))
	s = strings.ReplaceAll(s, "{shared}", shared)
	return extraPortRe.ReplaceAllStringFunc(s, func(m string) string {
		g := extraPortRe.FindStringSubmatch(m)
		i, _ := strconv.Atoi(g[1])
		p := extra[i-1]
		if g[2] != "" {
			p += 65536
		}
		return strconv.Itoa(p)
	})
}

// portMasks name the case's ports in normalized output.
func portMasks(port int, extra []int) []func(string) string {
	masks := []func(string) string{}
	add := func(p int, name string) {
		re := regexp.MustCompile(`\b` + strconv.Itoa(p) + `\b`)
		masks = append(masks, func(s string) string { return re.ReplaceAllString(s, name) })
	}
	add(port, "<PORT>")
	for i, p := range extra {
		add(p+65536, fmt.Sprintf("<P%d+65536>", i+1))
		add(p, fmt.Sprintf("<P%d>", i+1))
	}
	return masks
}

func applyPortMasks(s string, masks []func(string) string) string {
	for _, m := range masks {
		s = m(s)
	}
	return s
}

var (
	ssRcvBufRe = regexp.MustCompile(`\brb(\d+)`)
	localPort  = regexp.MustCompile(`:(\d+)$`)
)

// ssListeners are the process's listening sockets that belong to the case: its ports and the shared directory.
func ssListeners(t *testing.T, pid int, ports map[int]bool, shared string, masks []func(string) string) []string {
	out, err := exec.Command("ss", "-H", "-O", "-lnpxtuwm").Output()
	if err != nil {
		t.Fatalf("parity: ss: %v", err)
	}
	var set []string
	for _, line := range strings.Split(string(out), "\n") {
		if !strings.Contains(line, fmt.Sprintf("pid=%d,", pid)) {
			continue
		}
		f := strings.Fields(line)
		if len(f) < 5 {
			continue
		}
		local := f[4]
		m := localPort.FindStringSubmatch(local)
		mine := strings.Contains(local, shared) || strings.HasPrefix(local, "@")
		if m != nil {
			p, _ := strconv.Atoi(m[1])
			mine = ports[p]
		}
		if !mine {
			continue
		}
		rb := ""
		if m := ssRcvBufRe.FindStringSubmatch(line); m != nil {
			rb = "rb" + m[1]
		}
		local = strings.ReplaceAll(local, shared, "<SHARED>")
		set = append(set, applyPortMasks(strings.Join([]string{f[0], f[1], f[3], local, rb}, " "), masks))
	}
	sort.Strings(set)
	return set
}

// probeStatus is the status of GET /index.html over one listener, or the error class. A static file, because the
// candidate's /api/v1/info is not byte-identical yet and its size is in the access records.
func probeStatus(target string) string {
	network, addr, _ := strings.Cut(target, ":")
	c := &http.Client{
		Timeout: 10 * time.Second,
		Transport: &http.Transport{
			// identity bodies read to the end: the sent bytes are in the access records, and the gzip backends differ
			DisableCompression: true,
			DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
				var d net.Dialer
				return d.DialContext(ctx, network, addr)
			},
		},
	}
	resp, err := c.Get("http://localhost/index.html")
	if err != nil {
		return "error"
	}
	_, _ = io.Copy(io.Discard, resp.Body)
	resp.Body.Close()
	return strconv.Itoa(resp.StatusCode)
}

var connectedRe = regexp.MustCompile(` CONNECTED"$`)

func runBindCase(t *testing.T, role Role, bin string, c bindCase, port int, extra []int, shared string) bindResult {
	t.Helper()
	entries, err := os.ReadDir(shared)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if err := os.RemoveAll(filepath.Join(shared, e.Name())); err != nil {
			t.Fatal(err)
		}
	}
	if c.prep != nil {
		if err := c.prep(shared); err != nil {
			t.Fatal(err)
		}
	}
	id := parentIdentity
	o := daemon.Options{
		Binary:           bin,
		RunDir:           runDir(t, role),
		Port:             port,
		BindTo:           expandBind(c.bindTo, port, extra, shared),
		WebDir:           oracleWebDir(t),
		WebExtra:         expandBind(c.extra, port, extra, shared),
		LogsExtra:        "    level = debug\n",
		StreamMemoryMode: "ram",
		StorageTiers:     1,
		Identity:         &id,
	}
	masks := portMasks(port, extra)
	var res bindResult
	d, err := daemon.Start(o)
	if err != nil {
		res.start = "failed"
		if m := regexp.MustCompile(`exited during startup: (exit status \d+)`).FindStringSubmatch(err.Error()); m != nil {
			res.start = m[1]
		}
	} else {
		ports := map[int]bool{port: true}
		for _, p := range extra {
			ports[p] = true
		}
		res.listeners = ssListeners(t, d.PID(), ports, shared, masks)
		for _, target := range c.targets {
			res.statuses = append(res.statuses, target+" "+probeStatus(expandBind(target, port, extra, shared)))
		}
		if err := d.Stop(); err != nil {
			t.Errorf("parity: stop %s: %v", role, err)
		}
	}
	norm := func(l string) string {
		return applyPortMasks(strings.ReplaceAll(normalizeLog(l, o.RunDir, strconv.Itoa(port)), shared, "<SHARED>"), masks)
	}
	for _, l := range logLines(t, o.RunDir, "daemon.log") {
		if threadOf(l) == "" && (strings.Contains(l, `msg="LISTENER: `) || strings.Contains(l, "Cannot setup listen port")) {
			res.records = append(res.records, norm(l))
		}
	}
	for _, l := range logLines(t, o.RunDir, "access.log") {
		if strings.Contains(l, "src_port=UNIX") {
			l = norm(l)
			// the peek after accept may find the request not yet sent (EAGAIN): timing, not a contract
			if connectedRe.MatchString(l) {
				l = errnoRe.ReplaceAllString(l, "")
			}
			res.unix = append(res.unix, l)
		}
	}
	return res
}

// TestWebBindTo compares `[web] bind to` (check web.bind, D53). A case's two runs happen one after the other,
// since several cases need the same fixed ports. Datagrams are never sent: C crashes on them (D53.1).
func TestWebBindTo(t *testing.T) {
	cap52 := ""
	for i := 1; i <= 52; i++ {
		cap52 += fmt.Sprintf(" 127.0.0.1:{p%d}", i)
	}
	cases := map[string]bindCase{
		"unix": {
			bindTo:  "unix:{shared}/nd.sock 127.0.0.1:{port}",
			targets: []string{"unix:{shared}/nd.sock"},
		},
		"unix-acl-is-path": {
			bindTo:  "unix:{shared}/nd.sock=dashboard 127.0.0.1:{port}",
			targets: []string{"unix:{shared}/nd.sock=dashboard"},
		},
		"unix-no-dir": {bindTo: "unix:{shared}/nodir/nd.sock 127.0.0.1:{port}"},
		"unix-abstract": {
			bindTo:  "unix: 127.0.0.1:{port}",
			targets: []string{"unix:@" + strings.Repeat("\x00", 107)},
		},
		"unix-over-file": {
			bindTo:  "unix:{shared}/f.sock 127.0.0.1:{port}",
			prep:    func(shared string) error { return os.WriteFile(filepath.Join(shared, "f.sock"), []byte("x"), 0o644) },
			targets: []string{"unix:{shared}/f.sock"},
		},
		"unix-long": {bindTo: "unix:{shared}/" + strings.Repeat("s", 120) + " 127.0.0.1:{port}"},
		"udp":       {bindTo: "udp:127.0.0.1:{p1} 127.0.0.1:{port}", ports: 1},
		"udp-tcp-only-service": {
			bindTo: "udp:127.0.0.1:x11 127.0.0.1:{port}",
		},
		"iface-port": {
			bindTo:  "[::1]%lo:{p1} 127.0.0.1:{port}",
			ports:   1,
			targets: []string{"tcp:[::1]:{p1}"},
		},
		"iface-unknown":    {bindTo: "[::1]%nosuchif:{p1} 127.0.0.1:{port}", ports: 1},
		"wildcard-bad-svc": {bindTo: "*:nosuchsvc 127.0.0.1:{port}"},
		"host-bad-svc":     {bindTo: "127.0.0.1:nosuchsvc 127.0.0.1:{port}"},
		"port-wraps": {
			bindTo:  "127.0.0.1:{p1+65536} 127.0.0.1:{port}",
			ports:   1,
			targets: []string{"tcp:127.0.0.1:{p1}"},
		},
		"vertical-tab": {
			bindTo:  "127.0.0.1:{port}\v127.0.0.1:{p1}",
			ports:   1,
			targets: []string{"tcp:127.0.0.1:{p1}"},
		},
		"brackets-keep-equals": {bindTo: "[::1=x] 127.0.0.1:{port}"},
		"junk-after-bracket": {
			bindTo:  "[::1]x=badges 127.0.0.1:{port}",
			ports:   1,
			extra:   "    default port = {p1}\n",
			targets: []string{"tcp:[::1]:{p1}"},
		},
		"duplicate": {bindTo: "127.0.0.1:{port} 127.0.0.1:{port}"},
		"acl-words": {
			bindTo:  "127.0.0.1:{p1}=badges|registry^SSL=optional tcp:127.0.0.1:{port}",
			ports:   1,
			targets: []string{"tcp:127.0.0.1:{p1}"},
		},
		"cap": {bindTo: "127.0.0.1:{port}" + cap52, ports: 52},
		"service-name": {
			bindTo:  "127.0.0.1:x11 127.0.0.1:{port}",
			service: "127.0.0.1:6000",
			targets: []string{"tcp:127.0.0.1:6000"},
		},
		"nothing-opens": {bindTo: "udp:127.0.0.1:x11"},
	}
	if lladdr, iface := linkLocal(); lladdr != "" {
		cases["link-local"] = bindCase{
			bindTo:  "[" + lladdr + "]%" + iface + ":{p1} 127.0.0.1:{port}",
			ports:   1,
			targets: []string{"tcp:[" + lladdr + "%" + iface + "]:{p1}"},
		}
	}
	bins := map[Role]string{Oracle: os.Getenv("PARITY_ORACLE"), Candidate: os.Getenv("PARITY_CANDIDATE")}
	if bins[Oracle] == "" || bins[Candidate] == "" {
		t.Fatal("parity: set PARITY_ORACLE and PARITY_CANDIDATE")
	}
	names := make([]string, 0, len(cases))
	for name := range cases {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		c := cases[name]
		t.Run(name, func(t *testing.T) {
			if c.service != "" {
				l, err := net.Listen("tcp", c.service)
				if err != nil {
					t.Skipf("%s is taken", c.service)
				}
				l.Close()
			}
			ports := freePorts(t, 1+c.ports)
			shared := t.TempDir()
			var got [2]bindResult
			for i, role := range []Role{Oracle, Candidate} {
				got[i] = runBindCase(t, role, bins[role], c, ports[0], ports[1:], shared)
			}
			compare := func(what string, o, c []string) {
				if strings.Join(o, "\n") != strings.Join(c, "\n") {
					t.Errorf("%s differ\noracle:\n  %s\ncandidate:\n  %s", what, strings.Join(o, "\n  "), strings.Join(c, "\n  "))
				}
			}
			o := got[0]
			t.Logf("oracle: startup %q, listeners %q, %d LISTENER records, responses %q, %d unix access records",
				o.start, o.listeners, len(o.records), o.statuses, len(o.unix))
			compare("startup", []string{got[0].start}, []string{got[1].start})
			compare("listeners", got[0].listeners, got[1].listeners)
			compare("LISTENER records", got[0].records, got[1].records)
			compare("responses", got[0].statuses, got[1].statuses)
			compare("unix clients' access records", got[0].unix, got[1].unix)
		})
	}
}

// linkLocal is an IPv6 link-local address of an interface that is up, and the interface's name.
func linkLocal() (string, string) {
	ifaces, err := net.Interfaces()
	if err != nil {
		return "", ""
	}
	for _, ifc := range ifaces {
		if ifc.Flags&net.FlagUp == 0 || ifc.Flags&net.FlagLoopback != 0 {
			continue
		}
		addrs, _ := ifc.Addrs()
		for _, a := range addrs {
			if n, ok := a.(*net.IPNet); ok && n.IP.To4() == nil && n.IP.IsLinkLocalUnicast() {
				return n.IP.String(), ifc.Name
			}
		}
	}
	return "", ""
}
