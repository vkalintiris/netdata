// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// startTee accepts streaming connections and relays each to both daemons of the pair: what the child sends goes
// to both, byte for byte (compressed or not), and only the oracle's answers go back to the child.
func startTee(t *testing.T, p *Pair) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	go func() {
		for {
			child, err := ln.Accept()
			if err != nil {
				return
			}
			go tee(child, p.Oracle.Addr, p.Candidate.Addr)
		}
	}()
	return ln.Addr().String()
}

func tee(child net.Conn, oracle, candidate string) {
	defer child.Close()
	a, err := net.Dial("tcp", oracle)
	if err != nil {
		return
	}
	defer a.Close()
	b, err := net.Dial("tcp", candidate)
	if err != nil {
		return
	}
	defer b.Close()
	go func() {
		_, _ = io.Copy(child, a)
		_ = child.Close()
	}()
	go func() { _, _ = io.Copy(io.Discard, b) }()
	_, _ = io.Copy(io.MultiWriter(a, b), child)
}

// v3Retention reads db.first_entry/last_entry of a v3 answer.
func v3Retention(body []byte) (int64, int64, int) {
	var doc struct {
		DB struct {
			First int64 `json:"first_entry"`
			Last  int64 `json:"last_entry"`
		} `json:"db"`
		View struct {
			Dimensions struct {
				IDs []string `json:"ids"`
			} `json:"dimensions"`
		} `json:"view"`
	}
	if json.Unmarshal(body, &doc) != nil {
		return 0, 0, 0
	}
	return doc.DB.First, doc.DB.Last, len(doc.View.Dimensions.IDs)
}

func httpBody(b []byte) []byte {
	if i := bytes.Index(b, []byte("\r\n\r\n")); i >= 0 {
		return b[i+4:]
	}
	return nil
}

// TestCChild streams a real C agent (the oracle binary: its internal charts only) through a tee into both parents
// (ram, one tier) and compares their v1 and v3 answers about it, once per compression the parents may pick for it:
// the slice-1 goal, a C child in the Rust parent.
func TestCChild(t *testing.T) {
	algorithms := []string{"zstd", "lz4", "brotli", "gzip", "none"}
	guid := func(i int) string { return fmt.Sprintf("5a1e0000-0000-4000-8000-00000000c0%02d", i) }
	var extra strings.Builder
	for i, algorithm := range algorithms {
		if algorithm == "none" {
			fmt.Fprintf(&extra, "\n[%s]\n    enable compression = no\n", guid(i))
		} else {
			fmt.Fprintf(&extra, "\n[%s]\n    compression algorithms order = %s\n", guid(i), algorithm)
		}
	}
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, StreamExtra: extra.String()}, parentIdentity)
	for i, algorithm := range algorithms {
		t.Run(algorithm, func(t *testing.T) {
			hostname := "parity-cchild-" + algorithm
			child, err := daemon.Start(daemon.Options{
				Binary:       os.Getenv("PARITY_ORACLE"),
				RunDir:       runDir(t, Role("child-"+algorithm)),
				StorageTiers: 1,
				Identity: &daemon.Identity{
					Hostname:    hostname,
					StreamKey:   "5a1e0000-0000-4000-8000-00000000c1ff",
					MachineGUID: guid(i),
				},
				StreamTo: &daemon.StreamTo{
					Destination: startTee(t, p),
					APIKey:      parentIdentity.StreamKey,
					Compression: true,
				},
			})
			if err != nil {
				t.Fatalf("start child: %v", err)
			}
			t.Cleanup(func() { _ = child.Stop() })

			get := func(addr, path string) []byte {
				b, err := rawExchange(addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 5*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", path, err)
				}
				return b
			}
			// Wait for 20 s of data on the oracle, then settle a window both parents have whole.
			scope := "/api/v3/data?scope_nodes=" + hostname
			var first, last int64
			deadline := time.Now().Add(60 * time.Second)
			for {
				f, l, dims := v3Retention(httpBody(get(p.Oracle.Addr, scope+"&after=-60&before=0&points=1")))
				if dims > 0 && f > 0 && l-f >= 20 {
					first, last = f, l
					break
				}
				if time.Now().After(deadline) {
					t.Fatalf("no data from the C child: retention [%d,%d], %d dimensions", f, l, dims)
				}
				time.Sleep(time.Second)
			}
			time.Sleep(3 * time.Second)
			before, after := last-2, last-17
			if after < first {
				after = first
			}
			win := fmt.Sprintf("&after=%d&before=%d", after, before)
			cases := map[string]string{
				"v3":          scope + win + "&points=5",
				"v3-natural":  scope + win,
				"v3-instance": scope + win + "&points=3&group_by=instance",
				"v3-raw":      scope + win + "&points=3&options=raw",
				"v3-details":  scope + win + "&points=2&options=details",
				"v1-context":  "/host/" + hostname + "/api/v1/data?context=netdata.*&options=jsonwrap" + win,
				"v1-csv":      "/host/" + hostname + "/api/v1/data?context=netdata.*&format=csv&options=seconds&points=4" + win,
			}
			for name, path := range cases {
				t.Run(name, func(t *testing.T) {
					var got [2][]byte
					for i, side := range p.Each() {
						from := time.Now().Unix()
						b := get(side.Daemon.Addr, path)
						got[i] = maskNowEntries(maskTimings(maskRaw(b)), from, time.Now().Unix())
					}
					if name == "v3-details" {
						// C prints instance labels in heap-address order (decision D22.2): compare values.
						if a, b := labelsOnly(t, got[0]), labelsOnly(t, got[1]); !reflect.DeepEqual(a, b) {
							t.Errorf("responses differ\n%s", firstDifference(got[0], got[1]))
						}
						return
					}
					if !bytes.Equal(got[0], got[1]) {
						t.Errorf("responses differ\n%s", firstDifference(got[0], got[1]))
					}
				})
			}
		})
	}
}
