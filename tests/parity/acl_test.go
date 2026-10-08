// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
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
// whatever token it carries. The oracle must give every row's answer (aclWant). A client with no right at all
// (`everything-denied`) is refused before any route is looked up (web_client.c:1528-1537), a route that does not
// exist too: no row asks the management route there.
//
// The badge route (M9 commit 10, D217; `/api/v1/badge.svg`, web_api_v1.c:37-44) asks for the `badges` right and no
// other. Where the badges list leaves the address out (`features-denied`) the request is let in for the client's
// management right and the route's own access bit refuses it (451). Where the dashboard list alone leaves it out
// (`badges-only`: the badges list's default takes every address) the route answers (a badge, `chart not found`: the
// agent has no chart of that name), while a data request of the same client is refused for the right it lacks (451).
//
// The do-not-track policy (`do-not-track`: `respect do not track policy = yes`, the lists at their defaults): every
// answer without cookies says it does not track (`Tk: N`, web_client.c:996-1009), whether the client sent `DNT: 1`
// or not (the header only marks the client for the registry's cookies, http_header.c:74-79).
//
// A list with no word in it (`dashboard-empty`, `connections-empty`: the key with an empty value, D233; R100's 2.2)
// is no list: C's simple_pattern_create returns NULL for it and a NULL list allows every client (socket.c:574-575;
// netdata-conf-web.c:76-102). The oracle's `/netdata.conf` must show the empty value (aclEmpty), then the client the
// other configurations refuse is served the dashboard's page whole and `/api/v1/info` (aclServed).
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
		"do-not-track": {
			extra: "    respect do not track policy = yes\n",
			cases: map[string][]byte{
				"no-dnt": get("/"),
				"dnt":    get("/", "DNT: 1"),
			},
		},
	}
	// what the oracle must answer each row with: the answer's start and its end (C's answers, H35's probe P1). A
	// client a route's list leaves out gets the refusal (accessDenied; web_api.c:82-84, web_client.c:1528-1537), the
	// STREAM method its text without a head (the streaming list, before any key); a route that does not exist is
	// looked up for a client with some right (404) and not for one with none; OPTIONS answers `OK` to a client with
	// some right; a client the connections list leaves out gets no answer at all (aclClosed).
	const refusal = "You need to be authorized to access this resource"
	aclWant := map[string][2]string{
		"features-denied/static":         accessDenied,
		"features-denied/info":           accessDenied,
		"features-denied/contexts":       accessDenied,
		"features-denied/dbengine-stats": accessDenied,
		"features-denied/unknown":        {"HTTP/1.1 404 Not Found\r\n", "\r\n\r\nUnsupported API command: nope"},
		"features-denied/stream":         accessDenied,
		"features-denied/stream-method":  {refusal, refusal},
		"features-denied/options":        {"HTTP/1.1 200 OK\r\n", "\r\n\r\nOK"},
		"features-denied/gzip-off":       accessDenied,
		"features-denied/manage-allowed": {"HTTP/1.1 403 Forbidden\r\n", "\r\n\r\nAuth Error\n"},
		"features-denied/badge":          accessDenied,
		"badges-only/badge":              {badgeOK, "</script></svg>"},
		"badges-only/data":               accessDenied,
		"everything-denied/static":       accessDenied,
		"everything-denied/options":      accessDenied,
		"everything-denied/netdata.conf": accessDenied,
		"management-denied/manage":       accessDenied,
		"connections-denied/info":        aclClosed,
		"do-not-track/no-dnt":            {"HTTP/1.1 200 OK\r\n", "</script></html>"},
		"do-not-track/dnt":               {"HTTP/1.1 200 OK\r\n", "</script></html>"},
	}
	// a part the oracle's answer must hold besides
	aclHolds := map[string]string{
		"features-denied/options": "\r\nX-Frame-Options: SAMEORIGIN\r\n",
		"badges-only/badge":       ">chart not found</text>",
		"do-not-track/no-dnt":     "\r\nTk: N\r\n",
		"do-not-track/dnt":        "\r\nTk: N\r\n",
	}
	for name, cfg := range configs {
		t.Run(name, func(t *testing.T) {
			p := StartPair(t, daemon.Options{WebDir: webDir, WebExtra: cfg.extra, StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
			for cname, request := range cfg.cases {
				t.Run(cname, func(t *testing.T) {
					want, pinned := aclWant[name+"/"+cname]
					if !pinned {
						t.Fatalf("harness: %s/%s has no pinned answer", name, cname)
					}
					aclRow(t, p, "127.0.0.2", request, want, aclHolds[name+"/"+cname])
				})
			}
		})
	}
	for name, key := range map[string]string{"dashboard-empty": "allow dashboard from",
		"connections-empty": "allow connections from"} {
		t.Run(name, func(t *testing.T) {
			p := StartPair(t, daemon.Options{WebDir: webDir, WebExtra: "    " + key + " =\n", StreamMemoryMode: "ram",
				StorageTiers: 1}, parentIdentity)
			aclEmpty(t, p, key)
			t.Run("static", func(t *testing.T) {
				aclRow(t, p, "127.0.0.2", get("/"), [2]string{"HTTP/1.1 200 OK\r\n", "</script></html>"}, "")
			})
			t.Run("info", func(t *testing.T) { aclServed(t, p, "127.0.0.2", get("/api/v1/info")) })
		})
	}
}

// aclEmpty holds the oracle to having read `[web]`'s key with an empty value: its `/netdata.conf` prints the key so
// (the dump's own line of a key that was set, inicfg_conf_file.c:267-275 keeps the empty value).
func aclEmpty(t *testing.T, p *Pair, key string) {
	t.Helper()
	b, err := rawExchange(p.Oracle.Addr, []byte("GET /netdata.conf HTTP/1.1\r\n\r\n"), 5*time.Second)
	if line := "\n\t" + key + " = \n"; err != nil || !bytes.HasPrefix(b, []byte("HTTP/1.1 200 OK\r\n")) ||
		!bytes.Contains(b, []byte(line)) {
		t.Fatalf("oracle: /netdata.conf does not show %q (%v): %q", line, err, truncateBytes(b))
	}
}

// aclServed sends request from `from` to both agents and compares the answers' heads, each side's clock and expiry
// read against its own flight (maskHead) and its length against its own body (rawLength), once the oracle's answer is
// a 200 with a JSON body: that the client is served, where the body's bytes are another check's (`/api/v1/info`:
// `api.localhost-identity`, `api.v1-info-tail`).
func aclServed(t *testing.T, p *Pair, from string, request []byte) {
	t.Helper()
	problems, fatal := aclServedProblems(p, from, request)
	if fatal != "" {
		t.Fatal(fatal)
	}
	for _, problem := range problems {
		t.Error(problem)
	}
}

// aclServedProblems is aclServed's judgement: what is wrong with the oracle's answer (the row did not run), or what
// differs.
func aclServedProblems(p *Pair, from string, request []byte) (problems []string, fatal string) {
	var head [2][]byte
	for i, side := range p.Each() {
		start := time.Now().Unix()
		b, _ := rawExchangeFrom(from, side.Daemon.Addr, request, 2*time.Second)
		h, body, _ := bytes.Cut(b, []byte("\r\n\r\n"))
		if side.Role == Oracle {
			if _, err := ParseJSON(body); err != nil || !bytes.HasPrefix(h, []byte("HTTP/1.1 200 OK\r\n")) {
				return nil, fmt.Sprintf("oracle: answered %q (%v), want a 200 with a JSON body", truncateBytes(b), err)
			}
		}
		if err := rawLength(b); err != nil {
			problems = append(problems, fmt.Sprintf("%s: %v", side.Role, err))
		}
		head[i] = contentLengthRe.ReplaceAll(maskHead(h, [2]int64{start, time.Now().Unix()}),
			[]byte("Content-Length: <masked>"))
	}
	if !bytes.Equal(head[0], head[1]) {
		problems = append(problems, fmt.Sprintf("headers differ\noracle:    %q\ncandidate: %q", head[0], head[1]))
	}
	return problems, ""
}

// aclRow sends request from `from` to both agents and compares the raw answers, each side's clock and expiry read
// against its own flight (maskAnswer), after the oracle's answer is checked: it must start with want[0], end with
// want[1] and hold `holds` (an empty text checks nothing; exactJudge); with aclClosed, it must be no answer at all.
func aclRow(t *testing.T, p *Pair, from string, request []byte, want [2]string, holds string) {
	t.Helper()
	got := aclAnswers(p, from, request)
	if want == aclClosed && len(got[0]) != 0 {
		t.Fatalf("oracle: answered %q, want the connection closed without an answer", truncateBytes(got[0]))
	}
	exactJudge(t, got, want, []string{holds}, nil)
}

// aclAnswers sends request from `from` to both agents and hands back each side's answer after maskAnswer for the
// seconds its own request was in flight. A refused connection is closed at once: nothing, or a reset, comes back,
// and the answer is empty.
func aclAnswers(p *Pair, from string, request []byte) [2][]byte {
	var got [2][]byte
	for i, side := range p.Each() {
		start := time.Now().Unix()
		b, _ := rawExchangeFrom(from, side.Daemon.Addr, request, 2*time.Second)
		got[i] = maskAnswer(b, [2]int64{start, time.Now().Unix()})
	}
	return got
}

// aclClosed is aclRow's want for a client the agent lets no further than its accept: the connection is closed
// without an answer.
var aclClosed = [2]string{}
