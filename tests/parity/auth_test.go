// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// responseBytesRe finds an access record's response sizes.
var responseBytesRe = regexp.MustCompile(`((?:sent|size)_bytes=)\d+`)

// apiMemberRe is the `api` object `/api/v2/info` and `/api/v3/info` print in each agent's entry.
var apiMemberRe = regexp.MustCompile(`"api":\{[^}]*\}`)

// TestWebAuth (check `web.auth`, milestone 6 commit 2, D96): what an unauthenticated local client may do, with
// `[web] bearer token protection` off and on. Compared raw: `/api/v3/me`; the commands only the Cloud reaches (451,
// or 400 with a subpath); the 412 of a command needing anonymous data under protection. Compared by their `api`
// member: `/api/v2/info` and `/api/v3/info`. Then both logs, whose records carry each request's role and access.
func TestWebAuth(t *testing.T) {
	for name, extra := range map[string]string{"off": "", "on": "    bearer token protection = yes\n"} {
		t.Run(name, func(t *testing.T) {
			p := StartPair(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1,
				WebExtra: extra, LogsExtra: "    level = debug\n"}, parentIdentity)
			raw := []string{"/api/v3/me", "/api/v3/me/x", "/api/v1/info", "/api/v2/data?points=1",
				"/api/v3/stream_path", "/api/v2/rtc_offer", "/api/v2/bearer_protection", "/api/v2/bearer_get_token",
				"/api/v3/rtc_offer", "/api/v3/bearer_protection", "/api/v3/bearer_get_token",
				"/api/v2/rtc_offer/x", "/api/v1/bearer_get_token"}
			for _, path := range raw {
				var got [2][]byte
				for i, side := range p.Each() {
					b, err := rawExchange(side.Daemon.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 10*time.Second)
					if err != nil {
						t.Fatalf("%s: %s: %v", side.Role, path, err)
					}
					if name == "off" && (strings.HasPrefix(path, "/api/v1/info") ||
						strings.HasPrefix(path, "/api/v2/data") || strings.HasPrefix(path, "/api/v3/stream_path")) {
						// served: the bodies are other checks'; here only that they are
						b, _, _ = bytes.Cut(b, []byte("\r\n"))
					}
					got[i] = maskTimings(maskRaw(b))
				}
				if !bytes.Equal(got[0], got[1]) {
					t.Errorf("%s: responses differ\n%s", path, firstDifference(got[0], got[1]))
				}
			}
			for _, path := range []string{"/api/v2/info", "/api/v3/info"} {
				var got [2]string
				for i, side := range p.Each() {
					b, err := rawExchange(side.Daemon.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 10*time.Second)
					if err != nil {
						t.Fatalf("%s: %s: %v", side.Role, path, err)
					}
					head, _, _ := bytes.Cut(b, []byte("\r\n"))
					got[i] = string(head) + " " + strings.Join(apiMemberRe.FindAllString(string(b), -1), " ")
				}
				if got[0] != got[1] {
					t.Errorf("%s:\noracle:    %s\ncandidate: %s", path, got[0], got[1])
				}
				t.Logf("%s: %s", path, got[0])
			}
			for _, side := range p.Each() {
				if err := side.Daemon.Stop(); err != nil {
					t.Fatalf("stop %s: %v", side.Role, err)
				}
			}
			// the served bodies are other checks'
			compareLogFilesWith(t, p, []logMask{{responseBytesRe, "${1}N"}})
		})
	}
}
