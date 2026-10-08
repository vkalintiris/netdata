// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// stubAnswer is an answer as a stub agent writes it (a 200 unless `status` names another), in the shape of C's head
// (H35's probe P1): after `delay`, the Date of the second it is written in, the expiry `expires` seconds after it,
// the Content-Length `short` bytes short of the body, a random transaction id's shape. body and date are given that
// second (date nil: C's HTTP date).
type stubAnswer struct {
	body    func(now int64) string
	date    func(now int64) string
	expires int64
	short   int
	delay   time.Duration
	status  string
}

func (s stubAnswer) raw() string {
	time.Sleep(s.delay)
	now := time.Now()
	body := s.body(now.Unix())
	date := now.UTC().Format(http.TimeFormat)
	if s.date != nil {
		date = s.date(now.Unix())
	}
	status := "200 OK"
	if s.status != "" {
		status = s.status
	}
	return "HTTP/1.1 " + status + "\r\nConnection: close\r\nDate: " + date + "\r\n" +
		"Content-Type: application/json; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\n" +
		"Expires: " + now.Add(time.Duration(s.expires)*time.Second).UTC().Format(http.TimeFormat) + "\r\n" +
		fmt.Sprintf("Content-Length: %d\r\n", len(body)-s.short) +
		"X-Transaction-ID: 0123456789abcdef0123456789abcdef\r\n\r\n" + body
}

// dashNormStub is a stub agent for the harness's own units: a loopback listener that answers every request with
// what answer returns at that moment and closes, until the test ends. No netdata is started.
func dashNormStub(t *testing.T, answer func() string) *daemon.Daemon {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = l.Close() })
	go func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				return
			}
			go func() {
				defer conn.Close()
				var request []byte
				buf := make([]byte, 4096)
				for !bytes.Contains(request, []byte("\r\n\r\n")) {
					n, err := conn.Read(buf)
					if err != nil {
						return
					}
					request = append(request, buf[:n]...)
				}
				_, _ = conn.Write([]byte(answer()))
			}()
		}
	}()
	return &daemon.Daemon{Addr: l.Addr().String(), Opts: daemon.Options{RunDir: "/nonexistent/dash-norm-stub"}}
}

// testDashNormWiring pins that the comparers apply what the units of `tight` pin one by one: each is asked of two stub
// agents whose answers have C's shape (stubAnswer), with one thing wrong on one side at a time. A comparer that left
// a judge out (the agent's clock for a family without `now` members, the length, the escapes, the head read against
// the flight) reports nothing where a problem is wanted here; one that read the candidate by the oracle's flight
// reports a problem on C's own answers (the row whose candidate answers a second later), and so does one that read
// the agent's clock without the family's shift whenever the request stays within one second (nearly always: a
// mutation's kill, not this unit's verdict, rests on it).
//
// The stubs answer by the wall clock, so nothing here turns on which side of a second's boundary a request falls, or
// on a stalled box: a wrong clock is a minute off, and the exact bounds are the units' (v2Clock, v2NowInFlight,
// v2TiersInFlight, maskHead).
func testDashNormWiring(t *testing.T) {
	// an info answer: the agent's clock `behind` seconds before the second it is written in, its tier ending `ended`
	// seconds before it, and its cloud URL as written
	info := func(behind, ended int64, url string) func(int64) string {
		return func(now int64) string {
			return fmt.Sprintf(`{"api":2,"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent",`+
				`"now":%d,"ai":0,"cloud":{"url":"%s"},"db_size":[{"tier":0,"to":%d}]}]}`, now-behind, url, now-ended)
		}
	}
	const url = "https://app.netdata.cloud"
	good := stubAnswer{body: info(0, 0, url)}
	windowed := stubAnswer{body: info(1, 0, url)}
	behind := stubAnswer{body: info(60, 0, url)}
	pair := func(o, c stubAnswer) *Pair {
		return &Pair{Oracle: dashNormStub(t, o.raw), Candidate: dashNormStub(t, c.raw)}
	}
	const plain, window = "/api/v2/info", "/api/v2/info?after=-60"
	for name, c := range map[string]struct {
		o, c   stubAnswer
		target string
		want   string
	}{
		"C's answers": {good, good, plain, ""},
		// the walk's clock is the second before with a window, its tiers' end the wall's all the same
		"C's answers, with a window": {windowed, windowed, window, ""},
		// each side by its own flight: the candidate is asked, and answers, more than a second after the oracle
		"a candidate that answers a second later": {good, stubAnswer{body: info(0, 0, url), delay: 1100 * time.Millisecond},
			plain, ""},
		// D1: a family without `now` members holds the agent's clock too, each side's own
		"a clock of 0":              {good, stubAnswer{body: func(int64) string { return info(0, 0, url)(0) }}, plain, "candidate: agents[0].now"},
		"a clock a minute behind":   {good, behind, plain, "candidate: agents[0].now"},
		"the oracle's clock behind": {behind, good, plain, "oracle: agents[0].now"},
		"a tier that ended a minute ago": {good, stubAnswer{body: info(0, 60, url)}, plain,
			"candidate: agents[0].db_size[0].to"},
		// D7: each side's length is its body's, and the strings' escapes are compared
		"a length a byte short": {good, stubAnswer{body: info(0, 0, url), short: 1}, plain, "candidate: Content-Length"},
		"a slash escaped":       {good, stubAnswer{body: info(0, 0, `https:\/\/app.netdata.cloud`)}, plain, "escapes differ"},
		// D5: each side's Date is its flight's and its expiry at the oracle's distance
		"an expiry a second after the Date": {good, stubAnswer{body: info(0, 0, url), expires: 1}, plain, "headers differ"},
		"a date in another format": {good, stubAnswer{body: info(0, 0, url), date: func(now int64) string {
			return time.Unix(now, 0).UTC().Format(time.RFC3339)
		}}, plain, "headers differ"},
		"a date of a minute ago": {good, stubAnswer{body: info(0, 0, url), date: func(now int64) string {
			return time.Unix(now-60, 0).UTC().Format(http.TimeFormat)
		}}, plain, "headers differ"},
	} {
		a := v2Round(pair(c.o, c.c), v2Req{name: name, target: c.target, status: "200"}, infoV2Family())
		got := strings.Join(a.problems, "\n")
		if (c.want == "" && got != "") || !strings.Contains(got, c.want) {
			t.Errorf("v2Round, %s: problems %q, want %q", name, got, c.want)
		}
	}
	// a family whose unordered paths are flat maps: the labels' order is each side's, another shape is not
	nodes := func(labels, hw string) stubAnswer {
		return stubAnswer{body: func(int64) string {
			return `{"api":2,"nodes":[{"mg":"m","labels":` + labels + `,"hw":` + hw + `}]}`
		}}
	}
	for name, c := range map[string]struct {
		c    stubAnswer
		want string
	}{
		"the labels in another order": {nodes(`{"b":"2","a":"1"}`, `{"x":["1"],"y":"2"}`), ""},
		"another shape":               {nodes(`{"a":"1","b":"2"}`, `{"x":"1","y":["2"]}`), "layouts differ"},
	} {
		a := v2Round(pair(nodes(`{"a":"1","b":"2"}`, `{"x":["1"],"y":"2"}`), c.c), v2Req{name: name, target: "/api/v3/nodes",
			status: "200"}, v2Family{unordered: []string{"nodes.[].labels"}, flat: true,
			masks: []Mask{{"nodes.[].hw", "the shape is the layout's to judge"}}})
		got := strings.Join(a.problems, "\n")
		if (c.want == "" && got != "") || !strings.Contains(got, c.want) {
			t.Errorf("v2Round of a flat family, %s: problems %q, want %q", name, got, c.want)
		}
	}

	// the exact rows, the access rows and the data rows read each side's head against its own flight
	tree := stubAnswer{body: func(now int64) string { return fmt.Sprintf(`{"version":1,"agent":{"now":%d}}`, now) }}
	late := stubAnswer{body: tree.body, expires: 1}
	masked := "Date: now\r\nContent-Type: application/json; charset=utf-8\r\n" +
		"Cache-Control: no-cache, no-store, must-revalidate\r\nExpires: Date+0\r\n" +
		fmt.Sprintf("Content-Length: %d\r\n", len(tree.body(dashNormBase))) + "X-Transaction-ID: <masked>\r\n\r\n"
	exact, err := exactAnswers(pair(tree, late), exactReq{target: "/api/v1/config", mask: exactNow})
	if want := "HTTP/1.1 200 OK\r\nConnection: close\r\n" + masked + `{"version":1,"agent":{"now":"NOW"}}`; err != nil ||
		string(exact[0]) != want || string(exact[1]) != strings.Replace(want, "Expires: Date+0", "Expires: Date+1", 1) {
		t.Errorf("exactAnswers (%v):\n%q\n%q\nwant\n%q, and the candidate's with Date+1", err, exact[0], exact[1], want)
	}
	acl := aclAnswers(pair(tree, late), "", []byte("GET /api/v1/config HTTP/1.1\r\n\r\n"))
	if !bytes.Contains(acl[0], []byte(masked)) || bytes.Equal(acl[0], acl[1]) ||
		!bytes.Contains(acl[1], []byte("Expires: Date+1\r\n")) {
		t.Errorf("aclAnswers:\n%q\n%q", acl[0], acl[1])
	}
	short := stubAnswer{body: tree.body, short: 2}
	for name, c := range map[string]struct {
		answer stubAnswer
		head   string
		bad    bool
	}{
		"C's answer":            {tree, "Date: now\r\n", false},
		"a length two short":    {short, "Date: now\r\n", true},
		"an expiry a second on": {late, "Expires: Date+1\r\n", false},
	} {
		got, length, err := dataAnswer(dashNormStub(t, c.answer.raw).Addr, []byte("GET /api/v1/data HTTP/1.1\r\n\r\n"))
		if err != nil || (length != nil) != c.bad || !bytes.Contains(got, []byte(c.head)) ||
			!bytes.Contains(got, []byte("Content-Length: <masked>\r\n")) {
			t.Errorf("dataAnswer, %s: %q, length %v (%v)", name, got, length, err)
		}
	}
	for name, c := range map[string]struct {
		o, c  stubAnswer
		want  string
		fatal bool
	}{
		"C's answers":                 {tree, tree, "", false},
		"a length two short":          {tree, short, "candidate: Content-Length", false},
		"an expiry a second on":       {tree, late, "headers differ", false},
		"an oracle that says no JSON": {stubAnswer{body: func(int64) string { return "refused" }}, tree, "", true},
		// the oracle must serve the client: JSON under another status is not that
		"an oracle whose JSON is a 404":   {stubAnswer{body: tree.body, status: "404 Not Found"}, tree, "", true},
		"a candidate whose JSON is a 404": {tree, stubAnswer{body: tree.body, status: "404 Not Found"}, "headers differ", false},
	} {
		problems, fatal := aclServedProblems(pair(c.o, c.c), "", []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"))
		got := strings.Join(problems, "\n")
		if (fatal != "") != c.fatal || (c.want == "" && got != "") || !strings.Contains(got, c.want) {
			t.Errorf("aclServedProblems, %s: problems %q, fatal %q", name, got, fatal)
		}
	}
}
