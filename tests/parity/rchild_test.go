// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// A child of the handshake check: its own identity, its parents, a 3 s `[stream] timeout` (never 0, which waits
// forever) and the 5 s reconnect delay floor, so a first connection comes 5-10 s after the first collection.
func handshakeChild(t *testing.T, bin string, role Role, destination, logs, extra string) *daemon.Daemon {
	t.Helper()
	id := daemon.Identity{Hostname: "handshake-child", StreamKey: "5a1e0000-0000-4000-8000-0000000000c3",
		MachineGUID: "5a1e0000-0000-4000-8000-0000000000c4"}
	o := daemon.Options{Binary: bin, RunDir: runDir(t, role), Identity: &id, DBMode: "alloc", StorageTiers: 1,
		NoStreamKey: true, LogsExtra: logs, StreamTo: &daemon.StreamTo{Destination: destination,
			APIKey: parentIdentity.StreamKey, Extra: "    reconnect delay = 5\n    timeout = 3\n" + extra}}
	d, err := daemon.Start(o)
	if err != nil {
		t.Fatalf("parity: start %s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	return d
}

// stubs are a child's scripted parents by name: their addresses name them in the records.
type stubs struct {
	parents map[string]*stream.Parent
	names   map[string]string // "127.0.0.1:port" -> name
}

func startStubs(t *testing.T, scripts map[string]func(*stream.Parent)) *stubs {
	t.Helper()
	s := &stubs{parents: map[string]*stream.Parent{}, names: map[string]string{}}
	for name, setup := range scripts {
		p, err := stream.StartParent(nil)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { p.Close() })
		setup(p)
		s.parents[name] = p
		s.names[p.Addr()] = name
	}
	return s
}

// destination lists the stubs in name order.
func (s *stubs) destination() string {
	var names []string
	for n := range s.parents {
		names = append(names, n)
	}
	sort.Strings(names)
	var addrs []string
	for _, n := range names {
		addrs = append(addrs, s.parents[n].Addr())
	}
	return strings.Join(addrs, " ")
}

var (
	// optionalRecordRe are the records a shutdown may leave while an attempt runs (the map's §2 "Shutdown"); the
	// connector thread's end is waited for by both agents since D110
	optionalRecordRe = regexp.MustCompile(`last error: thread cancelled|Thread is cancelled while connecting`)
	dstPortRe        = regexp.MustCompile(` dst_port=(\d+)`)
	retryAtRe        = regexp.MustCompile(`will retry in (\d+) secs, at (\S+?)"`)
	postponedRe      = regexp.MustCompile(`POSTPONED FOR \d+ SECS MORE`)
	fdRe             = regexp.MustCompile(`, fd \d+\)`)
	stubPortRe       = regexp.MustCompile(`127\.0\.0\.1(:|', port '| port )(\d+)`)
	recordTimeRe     = regexp.MustCompile(`^time=(\S+) `)
	requestPortRe    = regexp.MustCompile(`127\.0\.0\.1:\d+`)
)

// handshakeRecords are a child's connector records (thread SNDR-CN[0]) and its collector's streaming records, each
// once, normalized: the stubs' ports by name, descriptors and postponements masked. Each "will retry ... at" is
// checked against its record's time (the delay is at least 5 s and at most the row's, give or take the second the
// time is printed in) and masked.
func handshakeRecords(t *testing.T, d *daemon.Daemon, s *stubs) []string {
	t.Helper()
	seen := map[string]bool{}
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		th := threadOf(l)
		// the start trigger's records and the connector's calls from the collector (PULSE)
		collector := th == "PULSE" && (strings.Contains(l, "STREAM SND") || strings.Contains(l, "STREAM CONNECT"))
		if th != "SNDR-CN[0]" && !collector || optionalRecordRe.MatchString(l) {
			continue
		}
		if m := retryAtRe.FindStringSubmatch(l); m != nil {
			checkRetryAt(t, l, m[1], m[2])
		}
		n := normalizeLog(l, d.Opts.RunDir, "")
		n = retryAtRe.ReplaceAllString(n, `will retry in ${1} secs, at T"`)
		n = postponedRe.ReplaceAllString(n, "POSTPONED FOR N SECS MORE")
		n = fdRe.ReplaceAllString(n, ", fd N)")
		n = stubPortRe.ReplaceAllStringFunc(n, func(m string) string {
			sub := stubPortRe.FindStringSubmatch(m)
			if name, ok := s.names["127.0.0.1:"+sub[2]]; ok {
				return "127.0.0.1" + sub[1] + name
			}
			return "127.0.0.1" + sub[1] + "PORT"
		})
		for addr, name := range s.names {
			n = strings.ReplaceAll(n, addr, name)
		}
		n = dstPortRe.ReplaceAllStringFunc(n, func(m string) string {
			if name, ok := s.names["127.0.0.1:"+dstPortRe.FindStringSubmatch(m)[1]]; ok {
				return " dst_port=" + name
			}
			return m
		})
		if !seen[n] {
			seen[n] = true
			out = append(out, n)
		}
	}
	sort.Strings(out)
	return out
}

// checkRetryAt checks a retry record's time against the record's own.
func checkRetryAt(t *testing.T, line, secs, at string) {
	t.Helper()
	tm := recordTimeRe.FindStringSubmatch(line)
	if tm == nil {
		return
	}
	logged, err1 := time.Parse(time.RFC3339Nano, tm[1])
	retry, err2 := time.Parse(time.RFC3339, at)
	if err1 != nil || err2 != nil {
		t.Errorf("retry record times %q %q: %v %v", tm[1], at, err1, err2)
		return
	}
	var max int
	fmt.Sscan(secs, &max)
	if max < 5 {
		max = 5
	}
	delay := retry.Sub(logged)
	if delay < 4*time.Second || delay > time.Duration(max+1)*time.Second {
		t.Errorf("retry at %s is %v after its record (row %s s): %s", at, delay, secs, line)
	}
}

var mlCapableRe = regexp.MustCompile(`ml_capable=[^&]*`)

// requestLines are a STREAM request's line as sent (its parameters' order and encoding), then its parameters and
// headers, the stub's port masked; `ml_capable` is masked (no ML here, D101.5).
func requestLines(r stream.Request) []string {
	out := []string{mlCapableRe.ReplaceAllString(r.Line, "ml_capable=M")}
	for k, v := range r.Params {
		if k == "ml_capable" {
			v = []string{"M"}
		}
		out = append(out, k+"="+strings.Join(v, ","))
	}
	sort.Strings(out)
	for _, h := range r.Headers {
		out = append(out, requestPortRe.ReplaceAllString(h, "127.0.0.1:P"))
	}
	return out
}

func diffLines(t *testing.T, what string, a, b []string) {
	t.Helper()
	if !slices.Equal(a, b) {
		t.Errorf("%s differ:\noracle:\n%s\ncandidate:\n%s", what, strings.Join(a, "\n"), strings.Join(b, "\n"))
	}
}

// runBoth runs the scenario against each binary in parallel, each with its own stubs.
func runBoth(t *testing.T, run func(i int, bin string, role Role)) {
	bins := binaries(t)
	var wg sync.WaitGroup
	for i, role := range []Role{"handshake-oracle", "handshake-candidate"} {
		wg.Add(1)
		go func() {
			defer wg.Done()
			run(i, bins[i], role)
		}()
	}
	wg.Wait()
}

// waitRecords waits up to `timeout` until the child's connector has logged, for each stub name, a record naming it
// that contains its text.
func waitRecords(t *testing.T, d *daemon.Daemon, s *stubs, want map[string]string, timeout time.Duration) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for {
		missing := map[string]string{}
		for k, v := range want {
			missing[k] = v
		}
		for _, l := range handshakeRecords(t, d, s) {
			for name, text := range missing {
				if strings.Contains(l, name) && strings.Contains(l, text) {
					delete(missing, name)
				}
			}
		}
		if len(missing) == 0 {
			return
		}
		if time.Now().After(deadline) {
			t.Errorf("%s: no records for %v within %v", d.Opts.RunDir, missing, timeout)
			return
		}
		time.Sleep(500 * time.Millisecond)
	}
}

// rejecting answers every STREAM request with `reply` and closes.
func rejecting(reply string) func(*stream.Parent) {
	return func(p *stream.Parent) {
		p.SetScript(func(stream.Request) stream.Answer { return stream.Answer{Reply: reply, Close: true} })
	}
}

// probing answers probes with `answer` (nil closes without one) and refuses access.
func probing(answer []byte) func(*stream.Parent) {
	return func(p *stream.Parent) {
		p.Probe = func(string) []byte { return answer }
		rejecting(stream.RejectNotPermitted)(p)
	}
}

// jsonAnswer is a 200 answer carrying `body`.
func jsonAnswer(body string) []byte {
	return []byte(fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s", len(body), body))
}

// streamInfo is a parent's stream_info answer for a known host.
func streamInfo(hostID, ingestType, ingestStatus string) []byte {
	return jsonAnswer(fmt.Sprintf(`{"version":1,"status":200,"host_id":"%s","nodes":2,"receivers":1,"nonce":7,`+
		`"db_status":"online","db_liveness":"live","ingest_type":"%s","ingest_status":"%s","first_time_s":1000,`+
		`"last_time_s":2000}`, hostID, ingestType, ingestStatus))
}

// battery runs `scripts` as one child's parents on both sides, until each side logged `want` (or `timeout`), and
// compares the connector's records.
func battery(t *testing.T, scripts map[string]func(*stream.Parent), extraDest []string, names map[string]string,
	want map[string]string, timeout time.Duration) {
	var records [2][]string
	runBoth(t, func(i int, bin string, role Role) {
		s := startStubs(t, scripts)
		for k, v := range names {
			s.names[k] = v
		}
		dest := strings.Join(append([]string{s.destination()}, extraDest...), " ")
		d := handshakeChild(t, bin, role, dest, "", "")
		waitRecords(t, d, s, want, timeout)
		_ = d.Stop()
		records[i] = handshakeRecords(t, d, s)
	})
	diffLines(t, "records", records[0], records[1])
	t.Logf("records:\n%s", strings.Join(records[0], "\n"))
}

// TestRChildHandshake (check `stream.rchild-handshake`, milestone 7 commit 3, D102): a child's connection to scripted
// parents. Compared between C and Rust children: the STREAM request, the probes' bytes, and the connector's records
// with their fields (a record's errno included), as sets.
//   - accept: one parent that probes 404 and accepts in plaintext; the child connects and hands the link over.
//   - debug: accept at `level = debug`, so the connector's debug records are compared too (D102.7).
//   - h2o-404, h2o-101: `parent using h2o = yes` against a parent that answers the upgrade 404 ("Parent version too
//     old"), and one that upgrades, then takes the STREAM request on the same connection.
//   - rejections: one parent per answer a child can get (the seven rejections, garbage, silence, a close, a VN
//     prompt without a version); each postpones its parent within the row's bounds (checked on every retry record).
//   - probes: one parent per stream_info outcome (a C parent's 404, the origin server with and without the child in
//     its path, a session ban, bad JSON, no Content-Length, missing and malformed members, a close), a refused port
//     and a name that does not resolve; every parent that stays a candidate refuses access.
func TestRChildHandshake(t *testing.T) {
	t.Run("accept", func(t *testing.T) {
		var reqs, probes, records [2][]string
		runBoth(t, func(i int, bin string, role Role) {
			s := startStubs(t, map[string]func(*stream.Parent){"P-accept": func(*stream.Parent) {}})
			d := handshakeChild(t, bin, role, s.destination(), "", "")
			sess := s.parents["P-accept"].WaitSession(1, 40*time.Second)
			if sess == nil {
				t.Errorf("%s: no STREAM connection within 40 s", role)
				return
			}
			time.Sleep(2 * time.Second)
			_ = d.Stop()
			reqs[i] = requestLines(sess.Request)
			for _, p := range s.parents["P-accept"].ProbeRequests() {
				probes[i] = append(probes[i], requestPortRe.ReplaceAllString(p, "127.0.0.1:P"))
			}
			records[i] = handshakeRecords(t, d, s)
		})
		diffLines(t, "requests", reqs[0], reqs[1])
		diffLines(t, "probes", probes[0], probes[1])
		diffLines(t, "records", records[0], records[1])
		t.Logf("records:\n%s", strings.Join(records[0], "\n"))
	})
	t.Run("debug", func(t *testing.T) {
		var records [2][]string
		runBoth(t, func(i int, bin string, role Role) {
			s := startStubs(t, map[string]func(*stream.Parent){"P-accept": func(*stream.Parent) {}})
			d := handshakeChild(t, bin, role, s.destination(), "    level = debug\n", "")
			if s.parents["P-accept"].WaitSession(1, 40*time.Second) == nil {
				t.Errorf("%s: no STREAM connection within 40 s", role)
			}
			time.Sleep(2 * time.Second)
			_ = d.Stop()
			records[i] = handshakeRecords(t, d, s)
		})
		diffLines(t, "records", records[0], records[1])
		t.Logf("records:\n%s", strings.Join(records[0], "\n"))
	})
	// one parent per run: with two, the child's shuffle decides which it tries first
	h2o := func(t *testing.T, name string, answer string, want string) {
		var records, upgrades [2][]string
		runBoth(t, func(i int, bin string, role Role) {
			s := startStubs(t, map[string]func(*stream.Parent){
				name: func(p *stream.Parent) { p.Upgrade = func(string) []byte { return []byte(answer) } },
			})
			d := handshakeChild(t, bin, role, s.destination(), "", "    parent using h2o = yes\n")
			waitRecords(t, d, s, map[string]string{"": want}, 60*time.Second)
			_ = d.Stop()
			records[i] = handshakeRecords(t, d, s)
			for _, u := range s.parents[name].Upgrades() {
				if !slices.Contains(upgrades[i], u) {
					upgrades[i] = append(upgrades[i], u)
				}
			}
		})
		diffLines(t, "upgrade requests", upgrades[0], upgrades[1])
		diffLines(t, "records", records[0], records[1])
		t.Logf("records:\n%s", strings.Join(records[0], "\n"))
	}
	t.Run("h2o-404", func(t *testing.T) {
		h2o(t, "H-404", stream.NotFound, "Parent version too old")
	})
	t.Run("h2o-101", func(t *testing.T) {
		h2o(t, "H-101", "HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: netdata_stream/2.0\r\n\r\n",
			"established link")
	})
	t.Run("rejections", func(t *testing.T) {
		scripts := map[string]func(*stream.Parent){
			"R-localhost": rejecting(stream.RejectSameLocalhost),
			"R-vnode":     rejecting(stream.RejectLocalVnode),
			"R-already":   rejecting(stream.RejectAlreadyStreaming),
			"R-denied":    rejecting(stream.RejectNotPermitted),
			"R-busy":      rejecting(stream.RejectBusy),
			"R-internal":  rejecting(stream.RejectInternalError),
			"R-init":      rejecting(stream.RejectInitializing),
			"R-garbage":   rejecting("HTTP/1.1 200 OK\r\n\r\n"),
			"R-version0":  rejecting("Hit me baby, push them over with the version=0"),
			"R-silent": func(p *stream.Parent) {
				p.Script = func(stream.Request) stream.Answer { return stream.Answer{Silent: true} }
			},
			"R-close": func(p *stream.Parent) {
				p.Script = func(stream.Request) stream.Answer { return stream.Answer{CloseNow: true} }
			},
		}
		want := map[string]string{
			"R-denied": "will retry", "R-busy": "will retry", "R-internal": "will retry", "R-init": "will retry",
			"R-garbage": "will retry", "R-version0": "will retry", "R-silent": "does not respond",
			"R-close": "does not respond",
		}
		battery(t, scripts, nil, nil, want, 90*time.Second)
	})
	t.Run("probes", func(t *testing.T) {
		const child, other = "5a1e0000-0000-4000-8000-0000000000c4", "11111111-2222-3333-4444-555555555555"
		refused := freePort(t)
		scripts := map[string]func(*stream.Parent){
			"Q-404json": probing(jsonAnswer(`{"version":1,"status":404,"host_id":"` + other +
				`","nodes":3,"receivers":0,"nonce":7}`)),
			"Q-origin":       probing(streamInfo(child, "localhost", "online")),
			"Q-origin-other": probing(streamInfo(other, "localhost", "online")),
			"Q-initializing": probing(streamInfo(other, "child", "initializing")),
			"Q-inpath":       probing(streamInfo(child, "child", "online")),
			"Q-badjson":      probing(jsonAnswer("not json")),
			"Q-nocl":         probing([]byte("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}")),
			"Q-nonodes": probing(jsonAnswer(`{"version":1,"status":200,"host_id":"` + other +
				`","receivers":0,"nonce":7}`)),
			"Q-statusabc": probing(jsonAnswer(`{"version":1,"status":"abc"}`)),
			"Q-closed":    probing(nil),
		}
		want := map[string]string{
			"Q-404json": "will retry", "Q-origin": "banned permanently", "Q-origin-other": "banned permanently",
			"Q-inpath": "banned for this session", "Q-badjson": "will retry", "Q-nocl": "will retry",
			"Q-nonodes": "will retry", "Q-statusabc": "will retry", "Q-closed": "socket receive error",
			"Q-refused": "connection refused", "nonexistent.invalid": "cannot resolve hostname",
		}
		battery(t, scripts, []string{refused, "nonexistent.invalid:1"}, map[string]string{refused: "Q-refused"},
			want, 90*time.Second)
	})
}

// freePort is an address on 127.0.0.1 nothing listens on.
func freePort(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	addr := ln.Addr().String()
	ln.Close()
	return addr
}

var (
	// the parents listen on their own ports; the records name them
	anyLocalPortRe = regexp.MustCompile(`127\.0\.0\.1(:|', port '| port )\d+`)
	anyDstPortRe   = regexp.MustCompile(` dst_port=\d+`)
	// how much a connection has carried when it closes
	carriedRe = regexp.MustCompile(`\d+ bytes (transmitted )?in \d+ operations|sent \d+ bytes in \d+ operations`)
	threadNRe = regexp.MustCompile(`(STREAM (?:SND|THREAD|RCV))\[\d+\]`)
)

// rchildRecords are a child's streaming records, each once: the connector's (SNDR-CN[0]) and its stream thread's
// (STREAM[n]) about its parent, with the parent's port, the counts and the stream thread's number masked.
func rchildRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	seen := map[string]bool{}
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		th := threadOf(l)
		if th != "SNDR-CN[0]" && !strings.HasPrefix(th, "STREAM[") || optionalRecordRe.MatchString(l) {
			continue
		}
		n := normalizeLog(l, d.Opts.RunDir, "")
		n = anyLocalPortRe.ReplaceAllString(n, "127.0.0.1${1}P")
		n = anyDstPortRe.ReplaceAllString(n, " dst_port=P")
		n = carriedRe.ReplaceAllString(n, "N bytes in N operations")
		n = threadNRe.ReplaceAllString(n, "${1}[n]")
		n = fdRe.ReplaceAllString(n, ", fd N)")
		n = postponedRe.ReplaceAllString(n, "POSTPONED FOR N SECS MORE")
		// a parent's stream_info answer carries a random nonce
		n = probeNonceRe.ReplaceAllString(n, `nonce\":N`)
		if !seen[n] {
			seen[n] = true
			out = append(out, n)
		}
	}
	sort.Strings(out)
	return out
}

var probeNonceRe = regexp.MustCompile(`nonce\\":\d+`)

// TestRChild (check `stream.rchild`, milestone 7 commit 4, D103): a C child and a Rust child with the same identity,
// each streaming to its own C parent (the oracle on both sides, `ram`, one tier). Compared while connected: both
// parents' `/api/v3/stream_path` (the child's entry and its capabilities), each child's own path once its parent's
// came down, the parents' `stream_info` for the child with its retention masked (a Rust child sends no charts before
// commit 5), and each child's `_net_default_iface`; after the children stop, their streaming records as sets.
// Variants: compression off (the harness default) and on (the C parent decodes the Rust child's zstd).
func TestRChild(t *testing.T) {
	const hostname, guid = "parity-rchild", "5a1e0000-0000-4000-8000-00000000c0bb"
	bins := binaries(t)
	for _, compression := range []bool{false, true} {
		name := "plain"
		if compression {
			name = "compressed"
		}
		t.Run(name, func(t *testing.T) {
			p := startPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity,
				[2]string{bins[0], bins[0]}, [2]string{}, [2]Role{"rchild-parent-oracle", "rchild-parent-candidate"})
			var children [2]*daemon.Daemon
			for i, side := range p.Each() {
				id := daemon.Identity{Hostname: hostname, StreamKey: cChildKey, MachineGUID: guid}
				d, err := daemon.Start(daemon.Options{Binary: bins[i], RunDir: runDir(t, Role("rchild-"+name+"-"+strconv.Itoa(i))),
					StorageTiers: 1, DBMode: "alloc", Identity: &id, NoStreamKey: true,
					StreamTo: &daemon.StreamTo{Destination: side.Daemon.Addr, APIKey: parentIdentity.StreamKey,
						Compression: compression, Extra: "    reconnect delay = 5\n"}})
				if err != nil {
					t.Fatalf("start child %d: %v", i, err)
				}
				t.Cleanup(func() { _ = d.Stop() })
				children[i] = d
			}
			parents := [2]string{p.Oracle.Addr, p.Candidate.Addr}
			// a C parent calls a child online only once it holds data for it: a Rust child's charts come with commit 5
			for _, addr := range parents {
				waitReceiver(t, addr, guid)
			}
			compareStreamPath(t, "parents", parents, "/api/v3/stream_path", entryTimes, "_streams_to")
			compareStreamPath(t, "children", [2]string{children[0].Addr, children[1].Addr}, "/api/v3/stream_path",
				entryTimes, "_streams_to")
			var info [2][]byte
			for i, addr := range parents {
				b, err := rawExchange(addr, []byte("GET /api/v3/stream_info?machine_guid="+guid+" HTTP/1.1\r\n\r\n"),
					5*time.Second)
				if err != nil {
					t.Fatal(err)
				}
				info[i] = streamInfoRetentionRe.ReplaceAll(httpBody(b), []byte(`"${1}":T`))
				// what the parent's data for the child decide (commit 5)
				info[i] = streamInfoDataRe.ReplaceAll(info[i], []byte(`"${1}":"D"`))
				info[i] = streamInfoNonceRe.ReplaceAll(info[i], []byte(`"nonce":N`))
			}
			if !bytes.Equal(info[0], info[1]) {
				t.Errorf("stream_info differs:\noracle:    %s\ncandidate: %s", info[0], info[1])
			}
			// the charts each parent holds for its child, as their definitions made them (commit 5): the data wait
			// for the replication answers (commit 6), so retention and values are masked
			var defs [2][]byte
			for i, addr := range parents {
				b, err := rawExchange(addr, []byte("GET /host/"+hostname+"/api/v1/charts HTTP/1.1\r\n\r\n"),
					5*time.Second)
				if err != nil {
					t.Fatal(err)
				}
				defs[i] = parentCharts(t, httpBody(b))
				if os.Getenv("PARITY_KEEP") == "1" {
					_ = os.WriteFile(filepath.Join(children[i].Opts.RunDir, "parent-charts.json"), defs[i], 0o644)
				}
			}
			if len(defs[0]) < 1000 || !bytes.Equal(defs[0], defs[1]) {
				t.Errorf("the parents' charts of the child differ (%d, %d bytes; PARITY_KEEP=1 keeps them)",
					len(defs[0]), len(defs[1]))
			}
			for i, c := range children {
				b, err := rawExchange(c.Addr, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 5*time.Second)
				if err != nil {
					t.Fatal(err)
				}
				if !strings.Contains(string(b), `"_net_default_iface":"lo"`) {
					t.Errorf("child %d: no _net_default_iface lo in /api/v1/info", i)
				}
			}
			var records [2][]string
			for i, c := range children {
				_ = c.Stop()
				records[i] = rchildRecords(t, c)
			}
			diffLines(t, "children's records", records[0], records[1])
			// what each C parent's parser said of its child's stream (a line it tolerates with an error)
			var parsed [2][]string
			for i, side := range p.Each() {
				parsed[i] = pluginsdRecords(t, side.Daemon)
			}
			diffLines(t, "the parents' PLUGINSD records", parsed[0], parsed[1])
			t.Logf("records:\n%s", strings.Join(records[0], "\n"))
		})
	}
}

// pluginsdRecords are a parent's parser records (PLUGINSD) about its children's streams, each once, normalized.
func pluginsdRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	seen := map[string]bool{}
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if !strings.Contains(l, `msg="PLUGINSD`) {
			continue
		}
		n := normalizeLog(l, d.Opts.RunDir, "")
		n = anyLocalPortRe.ReplaceAllString(n, "127.0.0.1${1}P")
		n = threadNRe.ReplaceAllString(n, "${1}[n]")
		if !seen[n] {
			seen[n] = true
			out = append(out, n)
		}
	}
	sort.Strings(out)
	return out
}

// waitReceiver waits until the parent has a receiver for the child (its stream_info names the child's host and
// counts the receiver), then a few seconds for the session's metadata.
func waitReceiver(t *testing.T, addr, guid string) {
	t.Helper()
	deadline := time.Now().Add(60 * time.Second)
	for {
		b, err := rawExchange(addr, []byte("GET /api/v3/stream_info?machine_guid="+guid+" HTTP/1.1\r\n\r\n"), 5*time.Second)
		if err == nil && bytes.Contains(b, []byte(`"receivers":1`)) && bytes.Contains(b, []byte(`"ingest_type":"child"`)) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("%s: no receiver for the child: %s", addr, b)
		}
		time.Sleep(time.Second)
	}
	time.Sleep(5 * time.Second)
}

// parentCharts is a parent's /api/v1/charts of a child as its definitions made it: each chart's retention, the
// totals, and the chart of the points the child's replication answers generate (commit 6) are left out.
func parentCharts(t *testing.T, body []byte) []byte {
	t.Helper()
	var v map[string]any
	if err := json.Unmarshal(body, &v); err != nil {
		t.Fatalf("charts: %v: %.200s", err, body)
	}
	for _, k := range []string{"charts_count", "dimensions_count", "rrd_memory_bytes"} {
		delete(v, k)
	}
	charts, _ := v["charts"].(map[string]any)
	delete(charts, "netdata.db_points_results")
	for _, c := range charts {
		for _, k := range []string{"first_entry", "last_entry", "duration"} {
			delete(c.(map[string]any), k)
		}
	}
	out, _ := json.MarshalIndent(v, "", " ")
	return out
}

var (
	streamInfoDataRe      = regexp.MustCompile(`"(db_status|db_liveness|ingest_status)":"[^"]*"`)
	streamInfoRetentionRe = regexp.MustCompile(`"(first_time_s|last_time_s)":\d+`)
	streamInfoNonceRe     = regexp.MustCompile(`"nonce":\d+`)
)

// runtimeScript are the lines a scripted parent sends down after its prompt: functions this agent does not have, the
// incomplete forms, bodies, cancels and progress requests, replication requests the sender rejects, NODE_IDs it
// rejects, JSON bodies and unknown commands (the map's §11 item 1). Each answer or record is the child's.
var runtimeScript = []string{
	`FUNCTION tx-1 10 "no-such-function" 0x13 "method=api"`,
	`FUNCTION tx-2`,
	`FUNCTION_PAYLOAD tx-3 10 "no-such-function" 0x13 "method=api" application/json`,
	`{"a":1}`,
	`{"b":2}`,
	`FUNCTION_PAYLOAD_END`,
	`FUNCTION_PAYLOAD`,
	`FUNCTION_PAYLOAD_END`,
	`FUNCTION_CANCEL tx-9`,
	`FUNCTION_PROGRESS tx-9`,
	`REPLAY_CHART "no.such.chart"`,
	`NODE_ID 'not-a-uuid' 'x' 'https://example.invalid'`,
	`NODE_ID '00000000-0000-0000-0000-000000000000' '11111111-2222-3333-4444-555555555555' 'https://example.invalid'`,
	`NODE_ID '11111111-2222-3333-4444-555555555555' '00000000-0000-0000-0000-000000000000' 'https://example.invalid'`,
	`NODE_ID '11111111-2222-3333-4444-555555555555' 'x' 'https://example.invalid'`,
	`JSON STREAM_PATH`,
	`{not json`,
	`JSON_PAYLOAD_END`,
	`JSON FOO`,
	`some payload`,
	`JSON_PAYLOAD_END`,
	`GARBAGE x "y z"`,
	``,
}

var expiresRe = regexp.MustCompile(`^(FUNCTION_RESULT_BEGIN "[^"]*" \d+ "[^"]*") \d+$`)

// runtimeUpstream are the lines a child sent back that the capture does not otherwise parse (its function results),
// with their expiry masked.
func runtimeUpstream(c capture) []string {
	var out []string
	for _, l := range c.other {
		out = append(out, expiresRe.ReplaceAllString(l, "${1} E"))
	}
	sort.Strings(out)
	return out
}

// TestRChildRuntime (check `stream.rchild-runtime`, milestone 7 commit 4, D103): C and Rust children against
// scripted recording parents that accept in plaintext. Compared: the lines each child sends back, and the child's
// streaming records as sets.
//   - executor: the parent sends `runtimeScript` down after its prompt.
//   - long-line: a 15487-byte line with no newline fills the child's receive buffer; the child restarts the
//     connection ("error during receive") and connects again.
//   - parent-close: the parent closes the connection after the session starts; the child sees the hangup and connects
//     again.
func TestRChildRuntime(t *testing.T) {
	cases := map[string]struct {
		down  []string
		close bool
		// later are sent after the child read the first lines and went quiet: C's first record then carries
		// the errno its last read left (review R41 m2)
		later []string
	}{
		"executor": {down: runtimeScript},
		"executor-later": {down: []string{`GARBAGE a`},
			later: []string{`GARBAGE b`, `FUNCTION tx-2`, `NODE_ID 'not-a-uuid' 'x' 'https://example.invalid'`}},
		// the line fills the child's 15488-byte buffer before its newline arrives
		"long-line":    {down: []string{strings.Repeat("x", 15487)}},
		"parent-close": {close: true},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			var up, records [2][]string
			runBoth(t, func(i int, bin string, role Role) {
				parent, err := stream.StartParent(func(r stream.Request) stream.Answer {
					a := stream.PlaintextAnswer(r)
					a.Down = tc.down
					a.DownAfter = time.Second
					return a
				})
				if err != nil {
					t.Error(err)
					return
				}
				t.Cleanup(func() { parent.Close() })
				d := senderChild(t, bin, Role(string(role)+"-"+name), parent, "")
				s := parent.WaitSession(1, 60*time.Second)
				if s == nil {
					t.Errorf("%s: no STREAM connection within 60 s", role)
					return
				}
				time.Sleep(3 * time.Second)
				if tc.later != nil {
					_ = s.Send(tc.later...)
					time.Sleep(2 * time.Second)
				}
				if tc.close {
					_ = s.Close()
				}
				if !strings.HasPrefix(name, "executor") && parent.WaitSession(2, 30*time.Second) == nil {
					t.Errorf("%s: no second connection within 30 s", role)
				}
				_ = d.Stop()
				up[i] = runtimeUpstream(parseCapture(s.Request, s.Data()))
				records[i] = rchildRecords(t, d)
			})
			diffLines(t, "upstream lines", up[0], up[1])
			diffLines(t, "records", records[0], records[1])
			t.Logf("upstream:\n%s\nrecords:\n%s", strings.Join(up[0], "\n"), strings.Join(records[0], "\n"))
		})
	}
}
