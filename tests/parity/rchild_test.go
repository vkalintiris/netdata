// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net"
	"regexp"
	"slices"
	"sort"
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
	// runtimeRecordRe are the records of the sender runtime (milestone 7 commit 4): a Rust child of commit 3 hands
	// its link to a stream thread that only holds it (D102.2), so neither side's copy is compared until then
	runtimeRecordRe = regexp.MustCompile(`streaming is ready, sending metrics to parent|streaming connector removed ` +
		`host: DISCONNECTED SHUTDOWN REQUESTED|running on-disconnect hooks`)
	// optionalRecordRe are the records a shutdown may leave while an attempt runs (the map's §2 "Shutdown"), and the
	// connector thread's end, which neither agent waits for
	optionalRecordRe = regexp.MustCompile(`last error: thread cancelled|Thread is cancelled while connecting|` +
		`thread=SNDR-CN\[0\] .*msg="thread with task id \d+ finished"`)
	dstPortRe     = regexp.MustCompile(` dst_port=(\d+)`)
	retryAtRe     = regexp.MustCompile(`will retry in (\d+) secs, at (\S+?)"`)
	postponedRe   = regexp.MustCompile(`POSTPONED FOR \d+ SECS MORE`)
	fdRe          = regexp.MustCompile(`, fd \d+\)`)
	stubPortRe    = regexp.MustCompile(`127\.0\.0\.1(:|', port '| port )(\d+)`)
	recordTimeRe  = regexp.MustCompile(`^time=(\S+) `)
	requestPortRe = regexp.MustCompile(`127\.0\.0\.1:\d+`)
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
		if th != "SNDR-CN[0]" && !(th == "PULSE" && strings.Contains(l, "STREAM SND")) ||
			runtimeRecordRe.MatchString(l) || optionalRecordRe.MatchString(l) {
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

// requestLines are a STREAM request's parameters and headers, the stub's port masked; `ml_capable` is masked (no ML
// here, D101.5).
func requestLines(r stream.Request) []string {
	var out []string
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
		p.Script = func(stream.Request) stream.Answer { return stream.Answer{Reply: reply, Close: true} }
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
