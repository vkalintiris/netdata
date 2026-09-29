// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// tlsPeersRe are the sockets of an SSL ERROR record's peers.
var tlsPeersRe = regexp.MustCompile(`\[\[([^\]]*)\]:\d+\]`)

// TestRChildTLS (check `stream.rchild-tls`, milestone 7 commit 7, D107, D113; map `knowledge/map-m7-commit7-tls.md`
// §11): a C child and a Rust child with the same identity each stream over TLS (`destination = …:SSL`) to its own C
// parent (the oracle on both sides) whose certificate is a self-signed one for 127.0.0.1.
//   - cafile (the certificate as the child's `CAfile`), capath (a hashed `CApath`) and skip (`ssl skip certificate
//     verification = yes`): the comparison of `stream.rchild` (paths, stream_info, the parents' charts, the data
//     within each side, the children's records, which carry `dst_transport=https`), and the parents' receiver records
//     (`src_transport=https`); skip also requires C's NOTICE that the senders skip the verification and its web
//     server's INFO, which reads the senders' flag.
//   - invalid (no CA, verification on): the probe fails its certificate test and the parent is postponed, no session;
//     the record of the failure comes once the parent closes the idle connection the close waits on (D107.1).
//   - plain-port (a parent without certificates): the probe's handshake fails.
//     Both failures compare the children's streaming records, the SSL ERROR record's peers masked.
func TestRChildTLS(t *testing.T) {
	key, cert := selfSigned(t)
	oracle := os.Getenv("PARITY_ORACLE")
	caFile := filepath.Join(t.TempDir(), "ca.pem")
	if err := os.WriteFile(caFile, cert, 0o644); err != nil {
		t.Fatal(err)
	}
	certificates := map[string][]byte{"key.pem": key, "cert.pem": cert}
	parents := func(t *testing.T, name string, files map[string][]byte) (*Pair, [2]tlsListeners) {
		return tlsPairOf(t, "", files, [2]string{oracle, oracle},
			[2]Role{Role("rtls-" + name + "-parent-oracle"), Role("rtls-" + name + "-parent-candidate")})
	}
	to := func(ls [2]tlsListeners, extra string) func(i int) *daemon.StreamTo {
		return func(i int) *daemon.StreamTo {
			return &daemon.StreamTo{Destination: ls[i].standard + ":SSL", APIKey: parentIdentity.StreamKey,
				Extra: "    reconnect delay = 5\n" + extra}
		}
	}
	// CApath: a directory hashed as OpenSSL looks it up (D107.8)
	caPath := t.TempDir()
	if err := os.WriteFile(filepath.Join(caPath, "ca.pem"), cert, 0o644); err != nil {
		t.Fatal(err)
	}
	rehashed := exec.Command("openssl", "rehash", caPath).Run() == nil
	for name, extra := range map[string]string{
		"cafile": "    CAfile = " + caFile + "\n",
		"capath": "    CApath = " + caPath + "\n",
		"skip":   "    ssl skip certificate verification = yes\n",
	} {
		t.Run(name, func(t *testing.T) {
			if name == "capath" && !rehashed {
				t.Skip("no `openssl rehash`")
			}
			p, ls := parents(t, name, certificates)
			children := rchildCompare(t, p, "tls-"+name, to(ls, extra))
			if name == "skip" {
				for i, c := range children {
					if !logContains(t, c, "SSL: streaming senders will skip SSL certificates verification.") {
						t.Errorf("child %d: no NOTICE that the senders skip the verification", i)
					}
					// C's web server tests the senders' flag (static-threaded.c:466)
					if !logContains(t, c, "SSL: web server will skip SSL certificates verification.") {
						t.Errorf("child %d: no web server INFO that it skips the verification", i)
					}
				}
			}
			var received [2][]string
			for i, side := range p.Each() {
				for _, r := range parentRecords(t, side.Daemon, "STREAM RCV", nil) {
					// a Rust child is not ML capable (D101.5)
					received[i] = append(received[i], mlCapableRe.ReplaceAllString(r, "ml_capable=M"))
				}
			}
			diffLines(t, "the parents' receiver records", received[0], received[1])
			if !strings.Contains(strings.Join(received[0], "\n"), "src_transport=https") {
				t.Errorf("the oracle's parent records no TLS child: %v", received[0])
			}
		})
	}
	failures := map[string]struct {
		files map[string][]byte
		// want is the record every child's log must have
		want string
	}{
		"invalid":    {certificates, "invalid SSL certification"},
		"plain-port": {nil, "cannot establish SSL connection"},
	}
	for name, tc := range failures {
		t.Run(name, func(t *testing.T) {
			p, ls := parents(t, name, tc.files)
			bins := binaries(t)
			var records [2][]string
			runBoth(t, func(i int, bin string, role Role) {
				id := daemon.Identity{Hostname: rchildHostname, StreamKey: cChildKey, MachineGUID: rchildGUID}
				d, err := daemon.Start(daemon.Options{Binary: bins[i], RunDir: runDir(t, Role("rtls-"+name+"-"+string(role))),
					StorageTiers: 1, DBMode: "alloc", Identity: &id, NoStreamKey: true, StreamTo: to(ls, "")(i)})
				if err != nil {
					t.Errorf("start child %d: %v", i, err)
					return
				}
				t.Cleanup(func() { _ = d.Stop() })
				// after a failed certificate test the close's second SSL_shutdown waits for the parent to close the
				// idle connection (its web timeout), as C's (D107.1)
				deadline := time.Now().Add(120 * time.Second)
				for !logContains(t, d, tc.want) && time.Now().Before(deadline) {
					time.Sleep(time.Second)
				}
				_ = d.Stop()
				for _, r := range rchildRecords(t, d) {
					records[i] = append(records[i], tlsPeersRe.ReplaceAllString(r, "[[${1}]:P]"))
				}
			})
			if !strings.Contains(strings.Join(records[0], "\n"), tc.want) {
				t.Fatalf("the oracle has no %q record: %v", tc.want, records[0])
			}
			diffLines(t, "children's records", records[0], records[1])
			for i, side := range p.Each() {
				if n := strings.Count(strings.Join(logLines(t, side.Daemon.Opts.RunDir, "daemon.log"), "\n"),
					"connected and ready to receive data"); n != 0 {
					t.Errorf("side %d: the parent took %d sessions", i, n)
				}
			}
			t.Logf("records:\n%s", strings.Join(records[0], "\n"))
		})
	}
}
