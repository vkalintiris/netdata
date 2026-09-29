// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"crypto/tls"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// localhostPortRe is a stub's port after `localhost` or `::1` in a record.
var localhostPortRe = regexp.MustCompile(`(localhost|::1)(:|', port '| port )\d+`)

// tlsStub is one TLS stub parent (StartParentTLS) serving `key` and `cert`, set up by `setup`.
func tlsStub(t *testing.T, key, cert []byte, setup func(*stream.Parent)) *stream.Parent {
	t.Helper()
	pair, err := tls.X509KeyPair(cert, key)
	if err != nil {
		t.Fatal(err)
	}
	p, err := stream.StartParentTLS(nil, &tls.Config{Certificates: []tls.Certificate{pair}})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { p.Close() })
	if setup != nil {
		setup(p)
	}
	return p
}

// tlsStubRecords are a child's streaming records (rchildRecords) with an SSL ERROR record's peers and the stub's
// port after `localhost` or `::1` masked.
func tlsStubRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, r := range rchildRecords(t, d) {
		r = tlsPeersRe.ReplaceAllString(r, "[[${1}]:P]")
		out = append(out, localhostPortRe.ReplaceAllString(r, "${1}${2}P"))
	}
	return out
}

// waitLog waits up to `timeout` for a record containing `text` in a child's log.
func waitLog(t *testing.T, d *daemon.Daemon, text string, timeout time.Duration) bool {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for !logContains(t, d, text) {
		if time.Now().After(deadline) {
			return false
		}
		time.Sleep(500 * time.Millisecond)
	}
	return true
}

// hellos are a stub's ClientHellos as text.
func hellos(p *stream.Parent) []string {
	var out []string
	for _, h := range p.Hellos() {
		out = append(out, h.String())
	}
	return out
}

// TestRChildTLSStub (check `stream.rchild-tls/stub`, milestone 7 commit 7 part b, D107.6-8; map
// `knowledge/map-m7-commit7-tls.md` §11): a C child and a Rust child, in parallel, each against its own TLS stub
// parent (`stream.StartParentTLS`, which records every ClientHello).
//   - hello, hello-localhost (`localhost:P:SSL`), h2o (`parent using h2o = yes`, an upgrade over TLS): the probe's and
//     the STREAM connection's ClientHellos (server name, ALPN, versions, suites, groups, signature schemes, extension
//     order), the STREAM request, the probe and the upgrade requests, the children's records.
//   - connect-fail, connect-fail-debug: the probe's handshake passes and the STREAM connection's is refused (the
//     connect loop's TLS path: "stream connection … failed …" and "can't connect to a parent").
//   - eof-close-notify, eof-raw: the stub ends a streaming session with a TLS close_notify (its TCP close a second
//     later) and without one; the child's records (at `level = debug`, so the requeue's errno after the TLS close
//     compares) until it connects again.
//   - wrong-ca (20), expired (10), missing-cafile (the CAfile INFO record, then 18): the certificate test's records.
//   - postpones (PARITY_LONG): the time between a stub's ClientHellos, [300, 601) s after an invalid certificate and
//     [60, 181) s after a refused handshake (C's postpone plus a connector pass).
func TestRChildTLSStub(t *testing.T) {
	key, cert := selfSigned(t)
	caFile := writeTemp(t, "ca.pem", cert)
	accept := func(extra string, dest func(p *stream.Parent) string) func(t *testing.T) {
		return func(t *testing.T) {
			var hs, reqs, probes, ups, records [2][]string
			runBoth(t, func(i int, bin string, role Role) {
				p := tlsStub(t, key, cert, func(p *stream.Parent) {
					p.Upgrade = func(string) []byte {
						return []byte("HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: netdata_stream/2.0\r\n\r\n")
					}
				})
				d := handshakeChild(t, bin, role, dest(p), "", "    CAfile = "+caFile+"\n"+extra)
				sess := p.WaitSession(1, 40*time.Second)
				if sess == nil {
					t.Errorf("%s: no STREAM connection within 40 s", role)
					return
				}
				time.Sleep(2 * time.Second)
				_ = d.Stop()
				hs[i], reqs[i] = hellos(p), requestLines(sess.Request)
				for _, r := range p.ProbeRequests() {
					probes[i] = append(probes[i], localhostPortRe.ReplaceAllString(requestPortRe.ReplaceAllString(r,
						"127.0.0.1:P"), "${1}${2}P"))
				}
				for _, u := range p.Upgrades() {
					ups[i] = append(ups[i], localhostPortRe.ReplaceAllString(requestPortRe.ReplaceAllString(u,
						"127.0.0.1:P"), "${1}${2}P"))
				}
				records[i] = tlsStubRecords(t, d)
			})
			if len(hs[0]) < 2 {
				t.Fatalf("the oracle's stub saw %d ClientHellos: %v", len(hs[0]), hs[0])
			}
			diffLines(t, "ClientHellos", hs[0], hs[1])
			diffLines(t, "requests", reqs[0], reqs[1])
			diffLines(t, "probes", probes[0], probes[1])
			diffLines(t, "upgrades", ups[0], ups[1])
			diffLines(t, "records", records[0], records[1])
			t.Logf("ClientHellos:\n%s\nrecords:\n%s", strings.Join(hs[0], "\n"), strings.Join(records[0], "\n"))
		}
	}
	ssl := func(p *stream.Parent) string { return p.Addr() + ":SSL" }
	t.Run("hello", accept("", ssl))
	t.Run("hello-localhost", accept("", func(p *stream.Parent) string {
		return "localhost" + strings.TrimPrefix(p.Addr(), "127.0.0.1") + ":SSL"
	}))
	t.Run("h2o", accept("    parent using h2o = yes\n", ssl))

	// until `want` is logged, then the children's records
	failing := func(t *testing.T, key, cert []byte, setup func(*stream.Parent), logs, extra, want string,
		also func(d *daemon.Daemon) []string) {
		var records [2][]string
		runBoth(t, func(i int, bin string, role Role) {
			p := tlsStub(t, key, cert, setup)
			d := handshakeChild(t, bin, role, ssl(p), logs, extra)
			if !waitLog(t, d, want, 90*time.Second) {
				t.Errorf("%s: no %q within 90 s", role, want)
			}
			_ = d.Stop()
			records[i] = tlsStubRecords(t, d)
			if also != nil {
				records[i] = append(records[i], also(d)...)
			}
			if n := len(p.Sessions()); n != 0 {
				t.Errorf("%s: the stub took %d sessions", role, n)
			}
		})
		if !strings.Contains(strings.Join(records[0], "\n"), want) {
			t.Fatalf("the oracle has no %q record: %v", want, records[0])
		}
		diffLines(t, "children's records", records[0], records[1])
		t.Logf("records:\n%s", strings.Join(records[0], "\n"))
	}
	refuseStream := func(p *stream.Parent) { p.RefuseHandshake = func(n int) bool { return n >= 2 } }
	t.Run("connect-fail", func(t *testing.T) {
		failing(t, key, cert, refuseStream, "", "    CAfile = "+caFile+"\n", "can't connect to a parent", nil)
	})
	t.Run("connect-fail-debug", func(t *testing.T) {
		failing(t, key, cert, refuseStream, "    level = debug\n", "    CAfile = "+caFile+"\n",
			"can't connect to a parent", nil)
	})

	ca, caKey, caPEM := newCA(t, "parity CA")
	_, _, otherPEM := newCA(t, "another CA")
	leafKey, leafCert := caSigned(t, ca, caKey, time.Now().Add(24*time.Hour))
	oldKey, oldCert := caSigned(t, ca, caKey, time.Now().Add(-time.Hour))
	t.Run("wrong-ca", func(t *testing.T) {
		failing(t, leafKey, leafCert, nil, "", "    CAfile = "+writeTemp(t, "other.pem", otherPEM)+"\n",
			"invalid SSL certification", nil)
	})
	t.Run("expired", func(t *testing.T) {
		failing(t, oldKey, oldCert, nil, "", "    CAfile = "+writeTemp(t, "ca.pem", caPEM)+"\n",
			"invalid SSL certification", nil)
	})
	t.Run("missing-cafile", func(t *testing.T) {
		failing(t, key, cert, nil, "", "    CAfile = /nonexistent/ca.pem\n", "invalid SSL certification",
			func(d *daemon.Daemon) []string {
				var out []string
				for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
					if strings.Contains(l, "can not verify custom CAfile") {
						out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
					}
				}
				return out
			})
	})

	for name, eof := range map[string]func(*stream.Session) error{
		// the close_notify a second before the TCP close: together they arrive as one readable event (C's HUP
		// record) or two (its EOF record), a race between two C children too
		"eof-close-notify": func(s *stream.Session) error {
			err := s.CloseNotify()
			time.Sleep(time.Second)
			_ = s.CloseRaw()
			return err
		},
		"eof-raw": (*stream.Session).CloseRaw,
	} {
		t.Run(name, func(t *testing.T) {
			var records [2][]string
			runBoth(t, func(i int, bin string, role Role) {
				p := tlsStub(t, key, cert, nil)
				// at debug the requeue after the close carries the errno the TLS close left (R46 m2)
				d := handshakeChild(t, bin, role, ssl(p), "    level = debug\n", "    CAfile = "+caFile+"\n")
				sess := p.WaitSession(1, 40*time.Second)
				if sess == nil {
					t.Errorf("%s: no STREAM connection within 40 s", role)
					return
				}
				time.Sleep(3 * time.Second)
				_ = eof(sess)
				if p.WaitSession(2, 40*time.Second) == nil {
					t.Errorf("%s: no second STREAM connection within 40 s", role)
				}
				time.Sleep(time.Second)
				_ = d.Stop()
				records[i] = tlsStubRecords(t, d)
			})
			if !strings.Contains(strings.Join(records[0], "\n"), "adding host in connector queue") {
				t.Errorf("the oracle logged no requeue after the close: %v", records[0])
			}
			diffLines(t, "children's records", records[0], records[1])
			t.Logf("records:\n%s", strings.Join(records[0], "\n"))
		})
	}

	t.Run("postpones", func(t *testing.T) {
		if os.Getenv("PARITY_LONG") != "1" {
			t.Skip("PARITY_LONG=1 times the postponements (about 11 minutes)")
		}
		kinds := []struct {
			name     string
			setup    func(*stream.Parent)
			min, max time.Duration
		}{
			{"invalid", nil, 300 * time.Second, 601 * time.Second},
			{"refused", func(p *stream.Parent) { p.RefuseHandshake = func(int) bool { return true } },
				60 * time.Second, 181 * time.Second},
		}
		runBoth(t, func(i int, bin string, role Role) {
			stubs := make([]*stream.Parent, len(kinds))
			for j, k := range kinds {
				stubs[j] = tlsStub(t, key, cert, k.setup)
				// no CAfile: the self-signed certificate fails its test
				handshakeChild(t, bin, Role(string(role)+"-"+k.name), ssl(stubs[j]), "", "")
			}
			start := time.Now()
			for j, k := range kinds {
				for len(stubs[j].Hellos()) < 2 && time.Since(start) < k.max+30*time.Second {
					time.Sleep(time.Second)
				}
				h := stubs[j].Hellos()
				if len(h) < 2 {
					t.Errorf("%s %s: %d ClientHellos by the deadline", role, k.name, len(h))
					continue
				}
				gap := h[1].At.Sub(h[0].At)
				if gap < k.min || gap >= k.max {
					t.Errorf("%s %s: %v between the first two ClientHellos, want [%v, %v)", role, k.name, gap,
						k.min, k.max)
				}
				t.Logf("%s %s: %v", role, k.name, gap.Round(time.Second))
			}
		})
	})
}

// writeTemp writes `content` to `name` in a new temporary directory and returns its path.
func writeTemp(t *testing.T, name string, content []byte) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, content, 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}
