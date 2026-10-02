// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// rawExchange sends request as-is and returns every byte the server sends back until it closes the connection
// or stays silent for timeout (a request C never completes shows up as "<timeout>").
func rawExchange(addr string, request []byte, timeout time.Duration) ([]byte, error) {
	return rawExchangeFrom("", addr, request, timeout)
}

// rawExchangeFrom is rawExchange from a given local IP (empty: any).
func rawExchangeFrom(localIP, addr string, request []byte, timeout time.Duration) ([]byte, error) {
	var dialer net.Dialer
	if localIP != "" {
		dialer.LocalAddr = &net.TCPAddr{IP: net.ParseIP(localIP)}
	}
	conn, err := dialer.Dial("tcp", addr)
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	if _, err := conn.Write(request); err != nil {
		return nil, err
	}
	var out bytes.Buffer
	buf := make([]byte, 64*1024)
	for {
		if err := conn.SetReadDeadline(time.Now().Add(timeout)); err != nil {
			return nil, err
		}
		n, err := conn.Read(buf)
		out.Write(buf[:n])
		if err != nil {
			var ne net.Error
			if errors.As(err, &ne) && ne.Timeout() {
				out.WriteString("<timeout>")
			}
			return out.Bytes(), nil
		}
	}
}

// rawRequest is an HTTP/1.1 request: the method, the target, the header lines, then the body with its
// Content-Length (no body and no length when body is nil; `Content-Length: 0` when it is empty).
func rawRequest(method, target string, headers []string, body []byte) []byte {
	var b bytes.Buffer
	b.WriteString(method + " " + target + " HTTP/1.1\r\n")
	for _, h := range headers {
		b.WriteString(h + "\r\n")
	}
	if body != nil {
		fmt.Fprintf(&b, "Content-Length: %d\r\n", len(body))
	}
	b.WriteString("\r\n")
	b.Write(body)
	return b.Bytes()
}

// contentLengthOf is a response head's Content-Length (-1 when it has none).
func contentLengthOf(head []byte) int {
	m := contentLengthRe.Find(head)
	if m == nil {
		return -1
	}
	n, err := strconv.Atoi(string(bytes.TrimPrefix(m, []byte("Content-Length: "))))
	if err != nil {
		return -1
	}
	return n
}

// readResponse reads one response from conn after the bytes already read (pending): its head, then the
// Content-Length's bytes, or everything until the server closes or stays silent for timeout when the head has no
// length (C drops keep-alive then). A silence before the response ends appends "<timeout>"; closed tells whether the
// connection ended. rest are the bytes read past the response.
func readResponse(conn net.Conn, pending []byte, timeout time.Duration) (resp, rest []byte, closed bool) {
	buf := make([]byte, 64*1024)
	out := pending
	for {
		if i := bytes.Index(out, []byte("\r\n\r\n")); i >= 0 {
			if n := contentLengthOf(out[:i]); n >= 0 && len(out) >= i+4+n {
				return out[:i+4+n], out[i+4+n:], false
			}
		}
		_ = conn.SetReadDeadline(time.Now().Add(timeout))
		n, err := conn.Read(buf)
		out = append(out, buf[:n]...)
		if err != nil {
			var ne net.Error
			if errors.As(err, &ne) && ne.Timeout() {
				out = append(out, "<timeout>"...)
			}
			return out, nil, true
		}
	}
}

// rawExchanges sends the requests on one connection, each after the previous response came whole (keep-alive), and
// returns the responses; the exchange ends early when the server closes the connection.
func rawExchanges(addr string, requests [][]byte, timeout time.Duration) ([][]byte, error) {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	var out [][]byte
	var rest []byte
	for _, req := range requests {
		if _, err := conn.Write(req); err != nil {
			return out, err
		}
		var resp []byte
		var closed bool
		resp, rest, closed = readResponse(conn, rest, timeout)
		out = append(out, resp)
		if closed {
			break
		}
	}
	return out, nil
}

// rawHoldAndClose sends request, runs wait (until the server reached a point, e.g. a plugin got the call), then
// closes the connection: whole (the client is gone, nothing is read) or, with half, only its writing side, reading
// what the server sends back until it closes or stays silent for timeout.
func rawHoldAndClose(addr string, request []byte, wait func(), half bool, timeout time.Duration) ([]byte, error) {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	if _, err := conn.Write(request); err != nil {
		return nil, err
	}
	wait()
	if !half {
		return nil, conn.Close()
	}
	if err := conn.(*net.TCPConn).CloseWrite(); err != nil {
		return nil, err
	}
	resp, _, _ := readResponse(conn, nil, timeout)
	return resp, nil
}

// rawMasks hide the header values that differ between any two responses (clock and random transaction ids).
var rawMasks = []*regexp.Regexp{
	regexp.MustCompile(`(?m)^(Date|Expires): [^\r]*`),
	regexp.MustCompile(`(?m)^X-Transaction-ID: [0-9a-f]*`),
}

func maskRaw(b []byte) []byte {
	for _, re := range rawMasks {
		b = re.ReplaceAllFunc(b, func(m []byte) []byte {
			name, _, _ := bytes.Cut(m, []byte(":"))
			return append(name, []byte(": <masked>")...)
		})
	}
	return b
}

// TestRawProtocol sends byte-exact requests that exercise the HTTP layer (method checks, OPTIONS, header
// parsing, limits) and compares the complete raw responses.
func TestRawProtocol(t *testing.T) {
	p := StartPair(t, daemon.Options{}, parentIdentity)
	cases := map[string][]byte{
		"unsupported-method":                   []byte("PATCH / HTTP/1.1\r\n\r\n"),
		"lowercase-method":                     []byte("get / HTTP/1.1\r\n\r\n"),
		"head-is-unsupported":                  []byte("HEAD /api/v1/info HTTP/1.1\r\n\r\n"),
		"options":                              []byte("OPTIONS /api/v1/info HTTP/1.1\r\nOrigin: http://x\r\n\r\n"),
		"options-mcp":                          []byte("OPTIONS /mcp HTTP/1.1\r\n\r\n"),
		"header-without-colon-never-completes": []byte("GET / HTTP/1.1\r\nFoo\r\n\r\n"),
		"uri-too-long": append(append([]byte("GET /"), bytes.Repeat([]byte("a"), 1<<20)...),
			[]byte(" HTTP/1.1\r\n\r\n")...),
	}
	compareRaw(t, p, cases)
}

// compareRaw sends each request to both daemons and compares the masked raw responses.
func compareRaw(t *testing.T, p *Pair, cases map[string][]byte) {
	t.Helper()
	for name, request := range cases {
		t.Run(name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, request, 2*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = maskRaw(b)
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\noracle:    %q\ncandidate: %q", truncateBytes(got[0]), truncateBytes(got[1]))
			}
		})
	}
}

// TestStaticAndRouting covers URL routing (API versions and commands, dashboard version prefixes, host switching)
// and static files, with both daemons serving the oracle's web directory.
func TestStaticAndRouting(t *testing.T) {
	webDir := oracleWebDir(t)
	p := StartPair(t, daemon.Options{WebDir: webDir}, parentIdentity)
	get := func(path string) []byte { return []byte("GET " + path + " HTTP/1.1\r\n\r\n") }
	paths := []string{
		"/", "/index.html", "/v3", "/v3/", "/v3/?x=1", "/v2/", "/v2/index.html", "/nonexistent",
		"/nonexistent.js", "/static/", "/netdata-swagger.json", "/bad%20name", "/a/../index.html",
		"/host/other/api/v1/info", "/host/", "/host/parity-parent", "/host/parity-parent/v3?y",
		"/api", "/api/v9", "/api/v1", "/api/v1//info", "/api/v1/info/x", "/api/v1/nope%2Fx", "/v1/v2/",
		"/node/5A1E0000-0000-4000-8000-0000000000AA/x.js", "/host/localhost/x.js", "/host/localhost",
		"/node/00000000-0000-0000-0000-000000000000/x.js", "/host/5A1E00000000400080000000000000AA/x.js",
		"/host/5a1e0000-0000-4000-8000-0000000000aaxyz/x.js",
	}
	cases := map[string][]byte{}
	for _, path := range paths {
		cases[strings.ReplaceAll(path, "/", "_")] = get(path)
	}
	// gzip bodies go out in one chunk per 16 KiB of zlib output, so a large file takes several
	for _, path := range []string{"/registry-hello.html", "/index.html", "/netdata-swagger.json", largestFile(t, webDir)} {
		cases["gzip"+strings.ReplaceAll(path, "/", "_")] = []byte("GET " + path + " HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n")
	}
	compareRaw(t, p, cases)
}

func truncateBytes(b []byte) string {
	s := string(b)
	if len(s) > 600 {
		return s[:600] + "..."
	}
	return strings.ToValidUTF8(s, "?")
}

// oracleWebDir is the web directory of the oracle's install, which both daemons serve in the static-file checks.
func oracleWebDir(t *testing.T) string {
	t.Helper()
	webDir := filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", "share", "netdata", "web")
	if _, err := os.Stat(filepath.Join(webDir, "index.html")); err != nil {
		t.Fatalf("parity: the oracle's web directory: %v", err)
	}
	return webDir
}

// largestFile is the web path of the biggest file under dir.
func largestFile(t *testing.T, dir string) string {
	t.Helper()
	var best string
	var size int64
	err := filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		info, err := d.Info()
		if err == nil && info.Size() > size {
			best, size = path, info.Size()
		}
		return err
	})
	if err != nil || best == "" {
		t.Fatalf("parity: no file under %s: %v", dir, err)
	}
	rel, _ := filepath.Rel(dir, best)
	return "/" + filepath.ToSlash(rel)
}

// TestRequestDuringLargeResponse sends a second request while the response to the first (the largest static file)
// is still being written: both daemons must answer both, in order.
func TestRequestDuringLargeResponse(t *testing.T) {
	webDir := oracleWebDir(t)
	big := largestFile(t, webDir)
	p := StartPair(t, daemon.Options{WebDir: webDir}, parentIdentity)
	var got [2][]byte
	for i, side := range p.Each() {
		conn, err := net.Dial("tcp", side.Daemon.Addr)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		if _, err := conn.Write([]byte("GET " + big + " HTTP/1.1\r\nConnection: keep-alive\r\n\r\n")); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		time.Sleep(200 * time.Millisecond)
		if _, err := conn.Write([]byte("GET /nonexistent.js HTTP/1.1\r\n\r\n")); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		var out bytes.Buffer
		buf := make([]byte, 1<<20)
		for {
			_ = conn.SetReadDeadline(time.Now().Add(3 * time.Second))
			n, err := conn.Read(buf)
			out.Write(buf[:n])
			if err != nil {
				break
			}
		}
		conn.Close()
		got[i] = maskRaw(out.Bytes())
	}
	for i, side := range []string{"oracle", "candidate"} {
		if !bytes.HasSuffix(got[i], []byte("File does not exist, or is not accessible: nonexistent.js")) {
			t.Errorf("%s: the second request was not answered", side)
		}
	}
	if !bytes.Equal(got[0], got[1]) {
		t.Errorf("responses differ (lengths %d and %d)\noracle tail:    %q\ncandidate tail: %q", len(got[0]), len(got[1]),
			truncateBytes(got[0][max(0, len(got[0])-400):]), truncateBytes(got[1][max(0, len(got[1])-400):]))
	}
}

// TestNoReadWhileWriting sends junk behind a request whose response the client does not read: the server must stop
// reading (C polls only for writing while it sends), so the client's writes block after the socket buffers fill.
func TestNoReadWhileWriting(t *testing.T) {
	webDir := oracleWebDir(t)
	big := largestFile(t, webDir)
	p := StartPair(t, daemon.Options{WebDir: webDir}, parentIdentity)
	const junk = 64 << 20
	for _, side := range p.Each() {
		conn, err := net.Dial("tcp", side.Daemon.Addr)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		if _, err := conn.Write([]byte("GET " + big + " HTTP/1.1\r\nConnection: keep-alive\r\n\r\n")); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		time.Sleep(200 * time.Millisecond)
		_ = conn.SetWriteDeadline(time.Now().Add(2 * time.Second))
		written, _ := conn.Write(make([]byte, junk))
		conn.Close()
		if written >= 16<<20 {
			t.Errorf("%s accepted %d bytes of junk while writing a response; want backpressure", side.Role, written)
		}
	}
}

// TestIgnoredSignals sends signals C neither handles nor lets kill it (it blocks everything but the deadly ones):
// both daemons must keep serving.
func TestIgnoredSignals(t *testing.T) {
	p := StartPair(t, daemon.Options{}, parentIdentity)
	for _, side := range p.Each() {
		for _, sig := range []syscall.Signal{syscall.SIGUSR1, syscall.SIGALRM, syscall.SIGPIPE} {
			if err := syscall.Kill(side.Daemon.PID(), sig); err != nil {
				t.Fatalf("%s: kill %v: %v", side.Role, sig, err)
			}
		}
	}
	time.Sleep(500 * time.Millisecond)
	for _, side := range p.Each() {
		b, err := rawExchange(side.Daemon.Addr, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 2*time.Second)
		if err != nil || !bytes.HasPrefix(b, []byte("HTTP/1.1 200 OK")) {
			t.Errorf("%s stopped serving after ignored signals: %v %q", side.Role, err, truncateBytes(b))
		}
	}
}

// TestStaticEdgeFiles serves a scratch web directory: an empty file requested with gzip (C sends the gzip and chunked
// header lines and closes without a chunk) and a file dated in the year 10000 (C's Date header is empty). Neither
// the Date nor the Expires value of these responses is masked when it is empty.
func TestStaticEdgeFiles(t *testing.T) {
	// tmpfs keeps 64-bit timestamps; ext4 wraps a year-10000 mtime.
	web := t.TempDir()
	if shm, err := os.MkdirTemp("/dev/shm", "parity-web-"); err == nil {
		web = shm
		t.Cleanup(func() { _ = os.RemoveAll(shm) })
	}
	for name, content := range map[string]string{"index.html": "<html></html>", "empty.txt": "", "future.txt": "x"} {
		if err := os.WriteFile(filepath.Join(web, name), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	// utimensat() directly: os.Chtimes goes through int64 nanoseconds, which overflow in the year 10000.
	const future = 253402300800
	futureOK := false
	ts := []syscall.Timespec{{Sec: future}, {Sec: future}}
	if syscall.UtimesNano(filepath.Join(web, "future.txt"), ts) == nil {
		var st syscall.Stat_t
		futureOK = syscall.Stat(filepath.Join(web, "future.txt"), &st) == nil && st.Mtim.Sec == future
	}
	p := StartPair(t, daemon.Options{WebDir: web}, parentIdentity)
	cases := map[string][]byte{
		"empty-gzip":  []byte("GET /empty.txt HTTP/1.1\r\nAccept-Encoding: gzip\r\nConnection: keep-alive\r\n\r\n"),
		"empty-plain": []byte("GET /empty.txt HTTP/1.1\r\n\r\n"),
	}
	if futureOK {
		cases["year-10000"] = []byte("GET /future.txt HTTP/1.1\r\n\r\n")
	} else {
		t.Log("the filesystem cannot store a year-10000 mtime; that case is skipped")
	}
	compareRaw(t, p, cases)
	if futureOK {
		// The mask hides Date values; check the empty one directly.
		for _, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, cases["year-10000"], time.Second)
			if err != nil || !bytes.Contains(b, []byte("\r\nDate: \r\n")) {
				t.Errorf("%s: want an empty Date header: %v %q", side.Role, err, truncateBytes(b))
			}
		}
	}
}

// TestInfoBeforeReady polls /api/v1/info from the moment each daemon starts. Until startup completes C answers 503
// with the request as the body (api_v1_info() returns before flushing the buffer the request was read into); the
// candidate must answer the same (check api.info-ready). The window can be a few milliseconds, so each side gets a
// few starts to show it. M8 commit 6 (D157) adds the Functions endpoints C gates the same way (api_v1_function.c:6-7,
// api_v1_functions.c:6-7) and v2's list, which it does not gate (contexts v2): never a 503 while startup runs.
func TestInfoBeforeReady(t *testing.T) {
	for _, c := range [][2]string{{"info", "/api/v1/info"}, {"fn-v1", "/api/v1/function?function=x"},
		{"fn-v3", "/api/v3/function?function=x"}, {"fns-v1", "/api/v1/functions"}} {
		t.Run(c[0], func(t *testing.T) { compareBeforeReady(t, c[1]) })
	}
	t.Run("fns-v2", func(t *testing.T) { neverBeforeReady(t, "/api/v1/functions", "/api/v2/functions") })
}

// neverBeforeReady polls a gated path and a free one in turn while each binary starts (up to 10 starts per side, until
// a free request follows a 503 of the gated path at once, inside the window or just past it: C vs C, about 5 starts in
// 8): the free path must never answer 503.
func neverBeforeReady(t *testing.T, gated, free string) {
	t.Helper()
	bins := binaries(t)
	for i, role := range []Role{Oracle, Candidate} {
		var after []string
		for attempt := 1; attempt <= 10 && len(after) == 0; attempt++ {
			r := beforeReadyFree(t, bins[i], Role(fmt.Sprintf("%s-%d", role, attempt)), gated, free)
			for _, s := range r.free {
				if strings.HasPrefix(s, "HTTP/1.1 503 ") {
					t.Errorf("%s: %s answered %q while startup ran", role, free, s)
					break
				}
			}
			after = r.after
			t.Logf("%s start %d: %d answers of %s, %d right after a 503 of %s: %q", role, attempt, len(r.free), free,
				len(r.after), gated, r.after)
		}
		if len(after) == 0 {
			t.Errorf("%s: no request of %s right after a 503 of %s in 10 starts", role, free, gated)
		}
	}
}

// beforeReadyAnswers are the status lines of a free path's answers while a daemon started, and of those requested
// right after a 503 of the gated path.
type beforeReadyAnswers struct{ free, after []string }

// beforeReadyFree starts the binary while polling gated and free in turn, until the daemon is ready.
func beforeReadyFree(t *testing.T, bin string, role Role, gated, free string) beforeReadyAnswers {
	t.Helper()
	request := func(path string) []byte {
		return []byte("GET " + path + " HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
	}
	status := func(b []byte) string {
		l, _, _ := bytes.Cut(b, []byte("\r\n"))
		return string(l)
	}
	port := freePorts(t, 1)[0]
	addr := "127.0.0.1:" + strconv.Itoa(port)
	stop, done := make(chan struct{}), make(chan struct{})
	var r beforeReadyAnswers
	go func() {
		defer close(done)
		for {
			select {
			case <-stop:
				return
			default:
			}
			after503 := false
			if b, err := rawExchange(addr, request(gated), time.Second); err == nil {
				after503 = bytes.HasPrefix(b, []byte("HTTP/1.1 503 "))
			}
			if b, err := rawExchange(addr, request(free), time.Second); err == nil {
				r.free = append(r.free, status(b))
				if after503 {
					r.after = append(r.after, status(b))
				}
			}
			time.Sleep(500 * time.Microsecond)
		}
	}()
	d, err := daemon.Start(daemon.Options{Binary: bin, Port: port, RunDir: runDir(t, role), Identity: &parentIdentity})
	close(stop)
	<-done
	if err != nil {
		t.Fatalf("start %s: %v", role, err)
	}
	if err := d.Stop(); err != nil {
		t.Errorf("stop %s: %v", role, err)
	}
	return r
}

// compareBeforeReady compares the first 503 each binary answers for path while its startup runs.
func compareBeforeReady(t *testing.T, path string) {
	t.Helper()
	bins := binaries(t)
	var first [2][]byte
	for i, role := range []Role{Oracle, Candidate} {
		for attempt := 1; attempt <= 5 && first[i] == nil; attempt++ {
			first[i] = beforeReady(t, bins[i], Role(fmt.Sprintf("%s-%d", role, attempt)), path)
		}
		if first[i] == nil {
			t.Fatalf("%s: no 503 before startup completed in 5 starts", role)
		}
	}
	if !bytes.Equal(first[0], first[1]) {
		t.Errorf("the answers before startup completed differ\n%s", firstDifference(first[0], first[1]))
	}
}

// beforeReady starts the binary while polling path; the first 503 it answers, masked, or nil.
func beforeReady(t *testing.T, bin string, role Role, path string) []byte {
	t.Helper()
	request := []byte("GET " + path + " HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
	port := freePorts(t, 1)[0]
	addr := "127.0.0.1:" + strconv.Itoa(port)
	stop := make(chan struct{})
	got := make(chan []byte, 1)
	go func() {
		defer close(got)
		for {
			select {
			case <-stop:
				return
			default:
			}
			if b, err := rawExchange(addr, request, time.Second); err == nil && bytes.HasPrefix(b, []byte("HTTP/1.1 503 ")) {
				got <- b
				return
			}
			time.Sleep(500 * time.Microsecond)
		}
	}()
	d, err := daemon.Start(daemon.Options{Binary: bin, Port: port, RunDir: runDir(t, role), Identity: &parentIdentity})
	close(stop)
	if err != nil {
		t.Fatalf("start %s: %v", role, err)
	}
	b := <-got
	if err := d.Stop(); err != nil {
		t.Errorf("stop %s: %v", role, err)
	}
	if b == nil {
		return nil
	}
	return maskRaw(b)
}
