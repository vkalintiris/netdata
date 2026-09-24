// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// TestWebACL compares the [web] access lists and response options. The lists keep "localhost" (127.0.0.1, which
// the launcher's readiness probe uses) and the requests come from 127.0.0.2, which they exclude.
func TestWebACL(t *testing.T) {
	webDir := oracleWebDir(t)
	get := func(path string, headers ...string) []byte {
		h := ""
		for _, x := range headers {
			h += x + "\r\n"
		}
		return []byte("GET " + path + " HTTP/1.1\r\n" + h + "\r\n")
	}
	configs := map[string]struct {
		extra string
		cases map[string][]byte
	}{
		"features-denied": {
			extra: "    allow dashboard from = localhost\n" +
				"    allow streaming from = localhost\n" +
				"    allow badges from = localhost\n" +
				"    x-frame-options response header = SAMEORIGIN\n" +
				"    enable gzip compression = no\n",
			cases: map[string][]byte{
				"static":   get("/"),
				"info":     get("/api/v1/info"),
				"contexts": get("/api/v1/contexts"),
				"unknown":  get("/api/v1/nope"),
				"stream":   get("/stream?key=x&hostname=y&machine_guid=z"),
				"options":  []byte("OPTIONS / HTTP/1.1\r\n\r\n"),
				"gzip-off": get("/nonexistent", "Accept-Encoding: gzip"),
			},
		},
		"everything-denied": {
			extra: "    allow dashboard from = localhost\n" +
				"    allow badges from = localhost\n" +
				"    allow management from = localhost\n" +
				"    allow netdata.conf from = localhost\n" +
				"    allow mcp from = localhost\n" +
				"[registry]\n" +
				"    allow from = localhost\n",
			cases: map[string][]byte{
				"static":  get("/"),
				"options": []byte("OPTIONS / HTTP/1.1\r\n\r\n"),
			},
		},
		"connections-denied": {
			extra: "    allow connections from = localhost\n",
			cases: map[string][]byte{
				"info": get("/api/v1/info"),
			},
		},
	}
	for name, cfg := range configs {
		t.Run(name, func(t *testing.T) {
			p := StartPair(t, daemon.Options{WebDir: webDir, WebExtra: cfg.extra, StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
			for cname, request := range cfg.cases {
				t.Run(cname, func(t *testing.T) {
					var got [2][]byte
					for i, side := range p.Each() {
						// A refused connection is closed at once: nothing, or a reset, comes back.
						b, _ := rawExchangeFrom("127.0.0.2", side.Daemon.Addr, request, 2*time.Second)
						got[i] = maskRaw(b)
					}
					if !bytes.Equal(got[0], got[1]) {
						t.Errorf("responses differ\noracle:    %q\ncandidate: %q", truncateBytes(got[0]), truncateBytes(got[1]))
					}
				})
			}
		})
	}
}
