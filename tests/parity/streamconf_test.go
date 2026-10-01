// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// cvGUID is the child of the stream.conf variants.
const cvGUID = "5a1e0000-0000-4000-8000-00000000c0f1"

// stockDir lays out each side's stock config directory in its run directory, with `conf` as its stream.conf (none
// when empty).
func stockDir(conf string) func(*testing.T, string) {
	return func(t *testing.T, runDir string) {
		dir := filepath.Join(runDir, "stock")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		if conf != "" {
			if err := os.WriteFile(filepath.Join(dir, "stream.conf"), []byte(conf), 0o644); err != nil {
				t.Fatal(err)
			}
		}
	}
}

// TestStreamConfVariants (check `stream.conf-variants`, M7 10f, D122.11): parents whose stream.conf is not the
// harness's. Each variant starts a pair, sends one STREAM request, and compares the answer, the stream.conf load's
// NOTICEs and the request's records:
//   - stock: no stream.conf in the user directory, so the stock directory's (here the run directory's `stock`)
//     is read; it enables the key: accepted after one NOTICE.
//   - defaults: neither file: internal defaults, the key not enabled; both NOTICEs.
//   - compression-off: `[stream] enable compression = no` is the key section's default: compression is offered and
//     none answered.
func TestStreamConfVariants(t *testing.T) {
	key := parentIdentity.StreamKey
	bins := binaries(t)
	variants := []struct {
		name    string
		opts    daemon.Options
		prepare func(*testing.T, string)
		caps    uint32
		accept  bool
		want    []string // texts the oracle's records must hold
	}{
		{name: "stock", opts: daemon.Options{NoStreamConf: true, StockConfigDir: "{run}/stock"},
			prepare: stockDir("[" + key + "]\n    enabled = yes\n    type = api\n    db = ram\n"), caps: stream.CapsLive,
			accept: true, want: []string{"CONFIG: cannot load user config '<RUN>/etc/stream.conf'. Will try stock config."}},
		{name: "defaults", opts: daemon.Options{NoStreamConf: true, StockConfigDir: "{run}/stock"},
			prepare: stockDir(""), caps: stream.CapsLive,
			want: []string{"CONFIG: cannot load stock config '<RUN>/stock/stream.conf'. Running with internal defaults.",
				"API key is not enabled in stream.conf"}},
		{name: "compression-off", opts: daemon.Options{StreamSection: "    enable compression = no\n"},
			caps: stream.CapsLive | stream.CapsCompression, accept: true, want: []string{"Host 'cv-compression-off'"}},
	}
	for _, v := range variants {
		t.Run(v.name, func(t *testing.T) {
			opts := v.opts
			opts.StreamMemoryMode, opts.StorageTiers = "ram", 1
			p := startPairWith(t, opts, parentIdentity, bins, [2]string{}, [2]Role{Oracle, Candidate}, v.prepare)
			host := "cv-" + v.name
			req := streamRequest(fmt.Sprintf("key=%s&hostname=%s&machine_guid=%s&update_every=1&ver=%d", key, host,
				cvGUID, v.caps))
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, req, time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = b
			}
			if bytes.HasPrefix(got[0], []byte(hsPrompt)) != v.accept {
				t.Errorf("the oracle answered %q", got[0])
			}
			if v.caps&stream.CapsCompression != 0 && bytes.Contains(got[0], []byte(hsPrompt)) {
				c, _ := strconv.ParseUint(string(hsVersionRe.FindSubmatch(got[0])[1]), 10, 32)
				if uint32(c)&stream.CapsCompression != 0 {
					t.Errorf("the oracle answered compression: %q", got[0])
				}
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\noracle:    %q\ncandidate: %q", got[0], got[1])
			}
			time.Sleep(time.Second)
			var recs [2][]string
			for i, side := range p.Each() {
				for _, l := range parentRecords(t, side.Daemon, "msg=", nil) {
					if strings.Contains(l, "CONFIG: cannot load") && strings.Contains(l, "stream.conf") ||
						strings.Contains(l, "STREAM RCV '"+host+"'") || strings.Contains(l, "Host '"+host+"'") {
						recs[i] = append(recs[i], hsHostKeyRe.ReplaceAllString(l, "with api key 'K'"))
					}
				}
			}
			for _, w := range v.want {
				if !strings.Contains(strings.Join(recs[0], "\n"), w) {
					t.Errorf("the oracle's records lack %q:\n%s", w, strings.Join(recs[0], "\n"))
				}
			}
			diffLines(t, "records", recs[0], recs[1])
			t.Logf("records:\n%s", strings.Join(recs[0], "\n"))
		})
	}
}

// TestStreamSenderConf (check `stream.sender-conf`, M7 10f, D122.11): a child's [stream] keys against stub parents.
//   - required: `enabled = yes` without a destination, or without an API key: the ERR record (present/missing), and
//     no connection in 20 s (a first attempt comes 5-10 s after the first collection).
//   - default-port: a destination without a port connects to `default port`, and to 19999 when it is unset (a stub
//     on 127.0.0.2:19999, one side after the other; skipped when the address is taken).
//   - compression-default: no `enable compression`: the request offers every compression (yes by default).
//   - reconnect: see reconnectRun.
func TestStreamSenderConf(t *testing.T) {
	t.Run("required", func(t *testing.T) {
		rows := map[string]func(o *daemon.Options){
			"nodest": func(o *daemon.Options) { o.StreamTo.Destination = "" },
			"nokey":  func(o *daemon.Options) { o.StreamTo.APIKey = "" },
		}
		for name, adjust := range rows {
			t.Run(name, func(t *testing.T) {
				var recs [2][]string
				var tries [2]int
				runBoth(t, func(i int, bin string, role Role) {
					parent, err := stream.StartParent(nil)
					if err != nil {
						t.Error(err)
						return
					}
					t.Cleanup(func() { parent.Close() })
					d := senderChild(t, bin, Role(string(role)+"-"+name), parent, "", adjust)
					time.Sleep(20 * time.Second)
					_ = d.Stop()
					tries[i] = len(parent.Sessions()) + len(parent.Probes())
					for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
						if strings.Contains(l, "cannot enable sending thread") {
							recs[i] = append(recs[i], normalizeLog(l, d.Opts.RunDir, ""))
						}
					}
				})
				if len(recs[0]) != 1 || tries[0] != 0 {
					t.Errorf("the oracle logged %d records and reached its parent %d times", len(recs[0]), tries[0])
				}
				if tries[1] != tries[0] {
					t.Errorf("the candidate reached its parent %d times", tries[1])
				}
				diffLines(t, "records", recs[0], recs[1])
				t.Logf("records:\n%s", strings.Join(recs[0], "\n"))
			})
		}
	})
	t.Run("default-port", func(t *testing.T) {
		var reqs, recs [2][]string
		runBoth(t, func(i int, bin string, role Role) {
			p, err := stream.StartParent(nil)
			if err != nil {
				t.Error(err)
				return
			}
			t.Cleanup(func() { p.Close() })
			_, port, _ := net.SplitHostPort(p.Addr())
			s := &stubs{parents: map[string]*stream.Parent{"P-port": p}, names: map[string]string{p.Addr(): "P-port"}}
			d := senderChild(t, bin, Role(string(role)+"-port"), p, "    default port = "+port+"\n",
				func(o *daemon.Options) { o.StreamTo.Destination = "127.0.0.1" })
			sess := p.WaitSession(1, 40*time.Second)
			_ = d.Stop()
			if sess == nil {
				t.Errorf("%s: no STREAM connection within 40 s", role)
				return
			}
			reqs[i], recs[i] = requestLines(sess.Request), handshakeRecords(t, d, s)
		})
		diffLines(t, "requests", reqs[0], reqs[1])
		diffLines(t, "records", recs[0], recs[1])
		bins := binaries(t)
		for i, role := range []Role{"port-oracle", "port-candidate"} {
			p, err := stream.StartParentOn("127.0.0.2:19999", nil)
			if err != nil {
				t.Skipf("a stub cannot listen on 127.0.0.2:19999: %v", err)
			}
			s := &stubs{parents: map[string]*stream.Parent{}, names: map[string]string{}}
			d := senderChild(t, bins[i], Role(string(role)+"-19999"), p, "",
				func(o *daemon.Options) { o.StreamTo.Destination = "127.0.0.2" })
			sess := p.WaitSession(1, 40*time.Second)
			_ = d.Stop()
			p.Close()
			if sess == nil {
				t.Fatalf("%s: no STREAM connection to 127.0.0.2:19999 within 40 s", role)
			}
			reqs[i], recs[i] = requestLines(sess.Request), handshakeRecords(t, d, s)
		}
		if !slices.ContainsFunc(recs[0], func(l string) bool { return strings.Contains(l, "dst_port=19999") }) {
			t.Errorf("the oracle's records name no port 19999:\n%s", strings.Join(recs[0], "\n"))
		}
		diffLines(t, "requests (19999)", reqs[0], reqs[1])
		diffLines(t, "records (19999)", recs[0], recs[1])
		t.Logf("records:\n%s", strings.Join(recs[0], "\n"))
	})
	t.Run("compression-default", func(t *testing.T) {
		var reqs [2][]string
		var offered [2]uint32
		runBoth(t, func(i int, bin string, role Role) {
			p, err := stream.StartParent(nil) // plaintext whatever is offered
			if err != nil {
				t.Error(err)
				return
			}
			t.Cleanup(func() { p.Close() })
			d := senderChild(t, bin, Role(string(role)+"-comp"), p, "",
				func(o *daemon.Options) { o.StreamTo.CompressionUnset = true })
			sess := p.WaitSession(1, 40*time.Second)
			_ = d.Stop()
			if sess == nil {
				t.Errorf("%s: no STREAM connection within 40 s", role)
				return
			}
			offered[i], reqs[i] = sess.Request.Caps(), requestLines(sess.Request)
		})
		if offered[0]&stream.CapsCompression != stream.CapsCompression {
			t.Errorf("the oracle offered %d: not every compression", offered[0])
		}
		diffLines(t, "requests", reqs[0], reqs[1])
	})
	t.Run("reconnect", func(t *testing.T) {
		// [stream] reconnect delay: 15 by default (a window of [7, 20) s), at least 5 (`1` gives [5, 10) s, not [5, 6))
		for _, c := range []struct {
			name, extra     string
			lo, hi, atLeast float64
		}{
			{"default", "", 6.5, 23, 14},
			{"floor", "    reconnect delay = 1\n", 4.5, 13, 8},
		} {
			t.Run(c.name, func(t *testing.T) {
				var samples [2][]float64
				runBoth(t, func(i int, bin string, role Role) {
					samples[i] = reconnectRun(t, bin, Role(string(role)+"-delay-"+c.name), c.extra, 14)
				})
				for i, role := range []Role{Oracle, Candidate} {
					t.Logf("%s: %v", role, samples[i])
					if len(samples[i]) != 14 {
						t.Errorf("%s: %d of 14 hosts reached the stub", role, len(samples[i]))
						continue
					}
					if slices.Min(samples[i]) < c.lo || slices.Max(samples[i]) > c.hi || slices.Max(samples[i]) < c.atLeast {
						t.Errorf("%s: the waits %v are not C's window (each in [%v, %v], one at least %v)", role,
							samples[i], c.lo, c.hi, c.atLeast)
					}
				}
			})
		}
	})
}

// reconnectRun starts a proxy whose [stream] sends every child's host to a stub with `extra` (its reconnect delay),
// connects `n` raw children 200 ms apart (the waiting list admits one per 100 ms) and returns, per child, how many
// seconds after its first block the proxy's sender for it reached the stub: the window stream_parents_host_reset()
// draws when the sender starts, [max(5, delay/2), delay+5) s, plus the connector's one-second pass.
func reconnectRun(t *testing.T, bin string, role Role, extra string, n int) []float64 {
	t.Helper()
	var mu sync.Mutex
	arrived := map[string]time.Time{}
	stub, err := stream.StartParent(func(r stream.Request) stream.Answer {
		mu.Lock()
		if _, ok := arrived[r.Params.Get("machine_guid")]; !ok {
			arrived[r.Params.Get("machine_guid")] = time.Now()
		}
		mu.Unlock()
		return stream.PlaintextAnswer(r)
	})
	if err != nil {
		t.Error(err)
		return nil
	}
	t.Cleanup(func() { stub.Close() })
	id := proxyIdentity
	d, err := daemon.Start(daemon.Options{Binary: bin, RunDir: runDir(t, role), Identity: &id, StorageTiers: 1,
		StreamMemoryMode: "ram",
		StreamSection:    fmt.Sprintf("    destination = %s\n    api key = %s\n%s", stub.Addr(), proxyUpKey, extra),
		StreamExtra:      fmt.Sprintf("\n[%s]\n    proxy enabled = yes\n", proxyIdentity.StreamKey)})
	if err != nil {
		t.Errorf("start %s: %v", role, err)
		return nil
	}
	t.Cleanup(func() { _ = d.Stop() })
	first := map[string]time.Time{}
	for k := range n {
		host := stream.HostInfo{Hostname: fmt.Sprintf("delay-%02d", k),
			MachineGUID: fmt.Sprintf("5a1e0000-0000-4000-8000-0000000de%03d", k)}
		c, err := stream.Connect(d.Addr, d.StreamKey, host, stream.CapsLive)
		if err != nil {
			t.Errorf("%s: child %d: %v", role, k, err)
			return nil
		}
		t.Cleanup(func() { _ = c.Close() })
		c.Linef("CHART 'delay.c' '' 't' 'u' 'f' 'delay.c' line 1000 1 '' delay corpus")
		c.Linef("DIMENSION 'd' '' absolute 1 1 ''")
		c.Linef("BEGIN2 'delay.c' 1 %d #", time.Now().Unix())
		c.Linef("SET2 'd' 1 1 A")
		c.Linef("END2")
		if err := c.Flush(); err != nil {
			t.Errorf("%s: child %d: %v", role, k, err)
			return nil
		}
		first[host.MachineGUID] = time.Now()
		time.Sleep(200 * time.Millisecond)
	}
	deadline := time.Now().Add(30 * time.Second)
	for {
		mu.Lock()
		got := len(arrived)
		mu.Unlock()
		if got >= n || time.Now().After(deadline) {
			break
		}
		time.Sleep(200 * time.Millisecond)
	}
	mu.Lock()
	defer mu.Unlock()
	var out []float64
	for guid, at := range arrived {
		if f, ok := first[guid]; ok {
			out = append(out, at.Sub(f).Seconds())
		}
	}
	slices.Sort(out)
	return out
}
