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
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// teeReplies holds what each parent of the pair sent back to the child over the tee's connections.
type teeReplies struct {
	mu    sync.Mutex
	sides [2]bytes.Buffer // oracle, candidate
}

type teeReplyWriter struct {
	r    *teeReplies
	side int
}

func (w teeReplyWriter) Write(b []byte) (int, error) {
	w.r.mu.Lock()
	defer w.r.mu.Unlock()
	return w.r.sides[w.side].Write(b)
}

// get returns a copy of what one side sent so far.
func (r *teeReplies) get(side int) []byte {
	r.mu.Lock()
	defer r.mu.Unlock()
	return bytes.Clone(r.sides[side].Bytes())
}

// startTee accepts streaming connections and relays each to both daemons of the pair: what the child sends goes
// to both, byte for byte (compressed or not), and only the oracle's answers go back to the child. What each parent
// answers is recorded in replies.
func startTee(t *testing.T, p *Pair, replies *teeReplies) string {
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
			go tee(child, p.Oracle.Addr, p.Candidate.Addr, replies)
		}
	}()
	return ln.Addr().String()
}

func tee(child net.Conn, oracle, candidate string, replies *teeReplies) {
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
		_, _ = io.Copy(child, io.TeeReader(a, teeReplyWriter{replies, 0}))
		_ = child.Close()
	}()
	go func() { _, _ = io.Copy(teeReplyWriter{replies, 1}, b) }()
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

// streamPathBlocks returns the payloads of the JSON STREAM_PATH blocks a parent sent to its child.
func streamPathBlocks(b []byte) [][]byte {
	const begin, end = "JSON STREAM_PATH\n", "\nJSON_PAYLOAD_END\n"
	var out [][]byte
	for {
		i := bytes.Index(b, []byte(begin))
		if i < 0 {
			return out
		}
		b = b[i+len(begin):]
		j := bytes.Index(b, []byte(end))
		if j < 0 {
			return out
		}
		out = append(out, b[:j])
		b = b[j+len(end):]
	}
}

// parentSince matches the parent's entry up to its `since`: the second each parent accepted the connection.
var parentSince = regexp.MustCompile(`("host_id":"` + parentIdentity.MachineGUID + `","node_id":[^,]*,"claim_id":[^,]*,"hops":-?\d+,"since":)\d+`)

func maskParentSince(b []byte) []byte {
	return parentSince.ReplaceAll(b, []byte("${1}0"))
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
			replies := &teeReplies{}
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
					Destination: startTee(t, p, replies),
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
			// The stream path each parent sends back (knowledge/brief-stream-path.md §10 in the status repository): the
			// first answers the child's path, the last carries the settled retention start. How many there are
			// depends on when the RRDCONTEXT thread widens the retention, so the counts are only logged.
			t.Run("stream-path", func(t *testing.T) {
				var blocks [2][][]byte
				for i := range blocks {
					blocks[i] = streamPathBlocks(replies.get(i))
				}
				t.Logf("JSON STREAM_PATH blocks: oracle %d, candidate %d", len(blocks[0]), len(blocks[1]))
				if len(blocks[0]) == 0 || len(blocks[1]) == 0 {
					t.Fatalf("a parent sent no stream path")
				}
				for _, which := range []struct {
					name string
					at   func(b [][]byte) []byte
				}{
					{"first", func(b [][]byte) []byte { return b[0] }},
					{"last", func(b [][]byte) []byte { return b[len(b)-1] }},
				} {
					o, c := maskParentSince(which.at(blocks[0])), maskParentSince(which.at(blocks[1]))
					if !bytes.Equal(o, c) {
						t.Errorf("%s stream path differs\n%s", which.name, firstDifference(o, c))
					}
				}
			})
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
