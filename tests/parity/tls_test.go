// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// TestWebTLS (check `web.tls`, milestone 6, D96): the web server's TLS against C's.
//   - stream-force: a `^SSL=force` listener refuses a plain STREAM even with no certificate configured: 400 "HTTP
//     method requested is not supported..." and C's ERR record naming the child's `hostname=` (up to its `&`, else
//     "not available"). A plain GET is served there (no TLS context, no redirect).
func TestWebTLS(t *testing.T) {
	t.Run("stream-force", func(t *testing.T) {
		opts := daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, LogsExtra: "    level = debug\n",
			BindTo: "127.0.0.1:{port}=dashboard|registry|badges|management|netdata.conf|streaming|mcp^SSL=force"}
		p := StartPair(t, opts, parentIdentity)
		requests := []string{
			"STREAM key=%s&hostname=forced-child&registry_hostname=forced-child&machine_guid=" +
				"b6b6b6b6-3333-4333-8333-000000000001&update_every=1&os=linux&ver=1 HTTP/1.1\r\n" +
				"User-Agent: netdata/v0\r\nAccept: */*\r\n\r\n",
			"STREAM key=%s&hostname=last-param HTTP/1.1\r\n\r\n",
			"GET /api/v1/info HTTP/1.1\r\n\r\n",
		}
		for i, format := range requests {
			var got [2][]byte
			for s, side := range p.Each() {
				req := format
				if strings.Contains(format, "%s") {
					req = fmt.Sprintf(format, side.Daemon.StreamKey)
				}
				b, err := rawExchange(side.Daemon.Addr, []byte(req), 10*time.Second)
				if err != nil {
					t.Fatalf("%s: request %d: %v", side.Role, i, err)
				}
				head, _, _ := bytes.Cut(b, []byte("\r\n"))
				if strings.HasPrefix(format, "GET") {
					// the info body is `api.v1-info`'s; here only that it is served
					b = head
				}
				got[s] = maskTimings(maskRaw(b))
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("request %d: responses differ\n%s", i, firstDifference(got[0], got[1]))
			}
		}
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		var records [2][]string
		for i, side := range p.Each() {
			port := strconv.Itoa(side.Daemon.Opts.Port)
			for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
				if strings.Contains(l, "please enable the SSL on child") {
					records[i] = append(records[i], normalizeLog(l, side.Daemon.Opts.RunDir, port))
				}
			}
			slices.Sort(records[i])
		}
		if !slices.Equal(records[0], records[1]) {
			t.Errorf("refusal records:\noracle:\n%s\ncandidate:\n%s", strings.Join(records[0], "\n"),
				strings.Join(records[1], "\n"))
		}
		if len(records[0]) != 2 {
			t.Errorf("the oracle logged %d refusals, want 2", len(records[0]))
		}
	})
}
