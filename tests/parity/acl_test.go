// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// TestWebACL compares the [web] access lists and response options. The lists keep "localhost" (127.0.0.1, which
// the launcher's readiness probe uses) and the requests come from 127.0.0.2, which they exclude.
//
// The management route (M9 commit 7, D210; `/api/v1/manage/health`, web_api_v1.c:214-221) is asked from 127.0.0.2
// too. Where the management list takes every address (`features-denied`: the client has that right alone) the route
// itself answers, and refuses a request without a token (403). Where the management list alone leaves the address
// out (`management-denied`: its default, `localhost`, beside the other lists' defaults) the web server lets the
// request in for the client's other rights and the route's own access bit refuses it (451, web_api.c:82-84),
// whatever token it carries. The oracle must give both answers (aclWant). A client with no right at all
// (`everything-denied`) is refused before any route is looked up (web_client.c:1528-1537), a route that does not
// exist too: no row asks the management route there.
//
// The badge route (M9 commit 10, D217; `/api/v1/badge.svg`, web_api_v1.c:37-44) asks for the `badges` right and no
// other. Where the badges list leaves the address out (`features-denied`) the request is let in for the client's
// management right and the route's own access bit refuses it (451). Where the dashboard list alone leaves it out
// (`badges-only`: the badges list's default takes every address) the route answers (a badge, `chart not found`: the
// agent has no chart of that name), while a data request of the same client is refused for the right it lacks (451).
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
				"    enable gzip compression = no\n" +
				"    allow management from = *\n",
			cases: map[string][]byte{
				"static":   get("/"),
				"info":     get("/api/v1/info"),
				"contexts": get("/api/v1/contexts"),
				// ACL NODES, as info
				"dbengine-stats": get("/api/v1/dbengine_stats"),
				"unknown":        get("/api/v1/nope"),
				"stream":         get("/stream?key=x&hostname=y&machine_guid=z"),
				// the STREAM method itself, which the streaming list denies before any key check
				"stream-method": []byte("STREAM key=11111111-2222-3333-4444-555555555555&hostname=y&machine_guid=" +
					"66666666-7777-8888-9999-000000000000 HTTP/1.1\r\n\r\n"),
				"options":  []byte("OPTIONS / HTTP/1.1\r\n\r\n"),
				"gzip-off": get("/nonexistent", "Accept-Encoding: gzip"),
				// the management list takes this client: the route asks for the key
				"manage-allowed": get("/api/v1/manage/health?cmd=LIST"),
				// the badges list does not: the route's own access bit
				"badge": get("/api/v1/badge.svg?chart=x"),
			},
		},
		"badges-only": {
			extra: "    allow dashboard from = localhost\n",
			cases: map[string][]byte{
				"badge": get("/api/v1/badge.svg?chart=x"),
				"data":  get("/api/v1/data?chart=x"),
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
				"static":       get("/"),
				"options":      []byte("OPTIONS / HTTP/1.1\r\n\r\n"),
				"netdata.conf": get("/netdata.conf"),
			},
		},
		"management-denied": {
			extra: "    allow management from = localhost\n",
			cases: map[string][]byte{
				// the management list alone leaves this client out: no token is looked at
				"manage": get("/api/v1/manage/health?cmd=LIST", "X-Auth-Token: 00000000-0000-0000-0000-000000000000"),
			},
		},
		"connections-denied": {
			extra: "    allow connections from = localhost\n",
			cases: map[string][]byte{
				"info": get("/api/v1/info"),
			},
		},
	}
	// what the oracle must answer the management route's rows and the badge route's with: the answer's start and its
	// end
	const denied = "\r\n\r\nYou need to be authorized to access this resource"
	aclWant := map[string][2]string{
		"features-denied/manage-allowed": {"HTTP/1.1 403 Forbidden\r\n", "\r\n\r\nAuth Error\n"},
		"management-denied/manage":       {"HTTP/1.1 451 Unavailable For Legal Reasons\r\n", denied},
		"features-denied/badge":          {"HTTP/1.1 451 Unavailable For Legal Reasons\r\n", denied},
		"badges-only/badge":              {badgeOK, "</script></svg>"},
		"badges-only/data":               {"HTTP/1.1 451 Unavailable For Legal Reasons\r\n", denied},
	}
	// a part the oracle's answer must hold besides
	aclHolds := map[string]string{"badges-only/badge": ">chart not found</text>"}
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
					if want, ok := aclWant[name+"/"+cname]; ok && !(strings.HasPrefix(string(got[0]), want[0]) && strings.HasSuffix(string(got[0]), want[1])) {
						t.Fatalf("oracle: answered %q, want %q at its start and %q at its end", truncateBytes(got[0]), want[0], want[1])
					}
					if part := aclHolds[name+"/"+cname]; !strings.Contains(string(got[0]), part) {
						t.Fatalf("oracle: answered %q, which does not hold %q", truncateBytes(got[0]), part)
					}
					if !bytes.Equal(got[0], got[1]) {
						t.Errorf("responses differ\noracle:    %q\ncandidate: %q", truncateBytes(got[0]), truncateBytes(got[1]))
					}
				})
			}
		})
	}
}
