// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/fixture"
)

// dashNormBase is the fixture child's base in the C-against-C run the pins of the node rows' parameters are taken
// from (H35's probe P2, 2026-10-08 05:58Z).
const dashNormBase int64 = 1791438960

// testDashNormTight pins what the harness holds each side's answer to on its own, beside the comparison of the two
// (the tightening pass after reviews R95 and R96): the answering agent's clock and its tiers' ends against the
// request's flight, the head's Date, expiry and length, the strings' escapes, the layout of a family whose unordered
// paths are flat maps, the streaming table's relations, runR's sample counts, the weights' parallel walk, and the
// harness's own rails. The values are C's, of H35's probes P1 and P2 (C against C, 2026-10-08), but most documents
// here are composed from them, a member or a line each, so that the one planted member is all that differs; a pin
// says "recorded" only where the text is an answer C gave, trimmed (the alert rows' are whole,
// dash_norm_alerts_data_test.go). How the comparers apply these judges is pinned by testDashNormWiring.
func testDashNormTight(t *testing.T) {
	parse := func(b string) Value {
		t.Helper()
		v, err := ParseJSON([]byte(b))
		if err != nil {
			t.Fatalf("%v: %s", err, b)
		}
		return v
	}
	problem := func(err error) string {
		if err != nil {
			return err.Error()
		}
		return ""
	}

	// a window as C's v2 walk reads one: an `after` or a `before` that is a number other than 0, the last of each
	for target, want := range map[string]bool{
		"/api/v2/info":           false,
		"/api/v2/info?after=-60": true,
		"/api/v2/info?after=-60&before=0&options=debug":      true,
		"/api/v2/nodes?after=1791438960&before=1791439020":   true,
		"/api/v2/info?before=-1":                             true,
		"/api/v2/info?after=0&before=0":                      false,
		"/api/v2/info?after=&before=":                        false,
		"/api/v2/info?after=x":                               false,
		"/api/v2/info?after=-60&after=0":                     false,
		"/api/v2/info?after=0&after=7":                       true,
		"/api/v2/info?after=+5":                              true,
		"/api/v2/info?after=--5":                             false,
		"/api/v2/info?after=007":                             true,
		"/api/v2/alert_transitions?anchor_gi=5&last=3":       false,
		"/api/v2/contexts?scope_nodes=after=5":               false,
		"/api/v2/contexts?options=after&scope_nodes=before":  false,
		"/api/v3/weights?after=1791438960&before=1791439020": true,
	} {
		if got := v2Windowed(target); got != want {
			t.Errorf("v2Windowed(%s) is %v", target, got)
		}
	}
	// the agent's clock: the flight's seconds; a second earlier at each end for the walk asked with a window; the
	// wall's for the weights family whatever it is asked
	flight := [2]int64{1791438323, 1791438324}
	for name, c := range map[string]struct {
		fam    v2Family
		target string
		want   [2]int64
	}{
		"the walk":               {contextsFamily, "/api/v2/contexts?scope_nodes=*", flight},
		"the walk with a window": {contextsFamily, "/api/v2/contexts?after=-600", [2]int64{1791438322, 1791438323}},
		"info with a window":     {infoV2Family(), "/api/v2/info?after=-60", [2]int64{1791438322, 1791438323}},
		"weights with a window":  {weightsFamily, "/api/v3/weights?after=1&before=2", flight},
	} {
		if got := c.fam.v2Clock(c.target, flight); got != c.want {
			t.Errorf("v2Clock, %s: %v, want %v", name, got, c.want)
		}
	}

	// D1: the first agent's `now` is held to the agent's clock for every family, and its tiers' `to` to the flight.
	// C's `/api/v2/info`, asked in second 1791438323: its `now` that second and, with `after=-60`, the one before
	// (the tier's end the wall's either way); with `options=rfc3339` both are dates.
	info := func(now, to string) Value {
		return parse(`{"api":2,"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent",` +
			`"now":` + now + `,"ai":0,"db_size":[{"tier":0,"granularity":"1s","metrics":0,"samples":0,"from":1791438320,` +
			`"to":` + to + `,"retention":3,"retention_human":"1m"}]}]}`)
	}
	asked := [2]int64{1791438323, 1791438323}
	before := [2]int64{1791438322, 1791438322}
	for name, c := range map[string]struct {
		now   string
		clock [2]int64
		bad   bool
	}{
		"the second of the flight":            {"1791438323", asked, false},
		"a second late":                       {"1791438324", asked, true},
		"a second soon":                       {"1791438322", asked, true},
		"a clock of 0":                        {"0", asked, true},
		"a clock in milliseconds":             {"1791438323000", asked, true},
		"the agent's start":                   {"1791438300", asked, true},
		"a clock that is no number":           {`"now"`, asked, true},
		"a clock that is null":                {"null", asked, true},
		"a date in flight":                    {`"2026-10-08T05:45:23Z"`, asked, false},
		"a date a second late":                {`"2026-10-08T05:45:24Z"`, asked, true},
		"a date without its zone":             {`"2026-10-08T05:45:23"`, asked, true},
		"with a window, the second before":    {"1791438322", before, false},
		"with a window, the wall's second":    {"1791438323", before, true},
		"with a window, the date before":      {`"2026-10-08T05:45:22Z"`, before, false},
		"with a window, two seconds before":   {"1791438321", before, true},
		"a flight of two seconds, its second": {"1791438324", flight, false},
	} {
		if err := v2NowInFlight(info(c.now, "1791438323"), c.clock); (err != nil) != c.bad {
			t.Errorf("v2NowInFlight, %s: %v", name, err)
		}
	}
	for name, c := range map[string]struct {
		v   Value
		bad bool
	}{
		"the second of the flight": {info("1791438323", "1791438323"), false},
		"the walk's second before": {info("1791438322", "1791438322"), true},
		"an end of 0":              {info("1791438323", "0"), true},
		"an end in milliseconds":   {info("1791438323", "1791438323000"), true},
		"a date in flight":         {info("1791438323", `"2026-10-08T05:45:23Z"`), false},
		"a date a second late":     {info("1791438323", `"2026-10-08T05:45:24Z"`), true},
		"an end that is no clock":  {info("1791438323", `"now"`), true},
		"no tier":                  {parse(`{"agents":[{"now":1791438323,"db_size":[]}]}`), false},
		"a tier without retention": {parse(`{"agents":[{"now":1791438323,"db_size":[{"tier":0,"samples":0}]}]}`), false},
		"no agent":                 {parse(`{"api":2,"nodes":[]}`), false},
		"a second tier out of flight": {parse(`{"agents":[{"db_size":[{"tier":0,"to":1791438323},{"tier":1,` +
			`"to":1791438320}]}]}`), true},
	} {
		if err := v2TiersInFlight(c.v, asked); (err != nil) != c.bad {
			t.Errorf("v2TiersInFlight, %s: %v", name, err)
		}
	}
	if err := v2NowInFlight(parse(`{"api":2,"nodes":[]}`), asked); err != nil {
		t.Errorf("v2NowInFlight without agents: %v", err)
	}
	// a family's `now` members follow the same clock: with a window, what is collected ends at the second before
	windowed := `{"contexts":{"q.ctx":{"first_entry":1791438100,"last_entry":1791438322,"live":true}}}`
	if got := string(contextsFamily.normalise(0, before, asked, []byte(windowed))); !strings.Contains(got,
		`"last_entry":"NOW"`) {
		t.Errorf("a window's last entry at the walk's clock: %s", got)
	}
	if got := string(contextsFamily.normalise(0, asked, asked, []byte(windowed))); got != windowed {
		t.Errorf("a last entry a second before the agent's clock: %s", got)
	}

	// the rows that hid `now` whole: an agent's clock outside a v2 family (the dyncfg tree, a data wrapper)
	tree := `{"version":1,"tree":{},"agent":{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent",` +
		`"now":1791438314}}`
	for name, c := range map[string]struct {
		in, want string
		flight   [2]int64
	}{
		"in flight":             {tree, strings.Replace(tree, `"now":1791438314`, `"now":"NOW"`, 1), [2]int64{1791438314, 1791438314}},
		"before the flight":     {tree, tree, [2]int64{1791438315, 1791438316}},
		"after the flight":      {tree, tree, [2]int64{1791438312, 1791438313}},
		"a clock of 0":          {strings.Replace(tree, "1791438314", "0", 1), "", [2]int64{1791438314, 1791438314}},
		"in milliseconds":       {strings.Replace(tree, "1791438314", "1791438314000", 1), "", [2]int64{1791438314, 1791438314}},
		"a date in flight":      {strings.Replace(tree, "1791438314", `"2026-10-08T05:45:14Z"`, 1), strings.Replace(tree, `"now":1791438314`, `"now":"NOW"`, 1), [2]int64{1791438314, 1791438314}},
		"a date out of it":      {strings.Replace(tree, "1791438314", `"2026-10-08T05:45:19Z"`, 1), "", [2]int64{1791438314, 1791438314}},
		"pretty":                {"\"now\": 1791438314,\n", "\"now\": \"NOW\",\n", [2]int64{1791438314, 1791438314}},
		"another member":        {`{"nowhere":1791438314,"known":1791438314}`, "", [2]int64{1791438314, 1791438314}},
		"already a word":        {`{"now":"NOW"}`, "", [2]int64{1791438314, 1791438314}},
		"two seconds of flight": {tree, strings.Replace(tree, `"now":1791438314`, `"now":"NOW"`, 1), [2]int64{1791438313, 1791438314}},
	} {
		want := c.want
		if want == "" {
			want = c.in
		}
		if got := string(exactNow([]byte(c.in), c.flight)); got != want {
			t.Errorf("exactNow, %s: %s, want %s", name, got, want)
		}
	}
	// a data answer's masks: the contexts' version and the timings whole, and the agent's clock where it is a second
	// of the flight; one that is not is left, under a key that says so
	data := "HTTP/1.1 200 OK\r\nDate: Thu, 08 Oct 2026 05:45:14 GMT\r\nExpires: Thu, 08 Oct 2026 05:45:14 GMT\r\n" +
		"Content-Length: 123\r\nX-Transaction-ID: 5f0c6b1c7a3e4d5f9a8b7c6d5e4f3a2b\r\n\r\n" +
		`{"versions":{"contexts_hard_hash":13},"agents":[{"now":1791438314,"timings":{"prep_ms":0.1,"total_ms":0.2}}]}`
	dataWant := "HTTP/1.1 200 OK\r\nDate: now\r\nExpires: Date+0\r\nContent-Length: <masked>\r\n" +
		"X-Transaction-ID: <masked>\r\n\r\n" +
		`{"versions":{"contexts_hard_hash":"<masked>"},"agents":[{"now":"<masked>","timings":{"prep_ms":"<masked>","total_ms":"<masked>"}}]}`
	if got := string(dataMask([]byte(data), 1791438314, 1791438314)); got != dataWant {
		t.Errorf("dataMask:\n got %q\nwant %q", got, dataWant)
	}
	if got := string(dataMask([]byte(data), 1791438315, 1791438315)); strings.Contains(got, `"now":`) ||
		strings.Contains(got, "Date: now") ||
		!strings.Contains(got, `[{"now, not a second of the request's flight":1791438314,"timings"`) {
		t.Errorf("dataMask, a second after the answer's clock: %q", got)
	}
	for name, in := range map[string]string{
		"a clock of 0":       `{"agents":[{"now":0}]}`,
		"in milliseconds":    `{"agents":[{"now":1791438314000}]}`,
		"a date out of it":   `{"agents":[{"now":"2026-10-08T05:45:19Z"}]}`,
		"a word":             `{"agents":[{"now":"now"}]}`,
		"a second too early": `{"agents":[{"now":1791438313}]}`,
	} {
		if got := string(dataMask([]byte(in), 1791438314, 1791438314)); strings.Contains(got, `"now":`) ||
			!strings.Contains(got, "not a second of the request's flight") {
			t.Errorf("dataMask, %s: %s", name, got)
		}
	}
	for name, in := range map[string]string{
		"the second":       `{"agents":[{"now":1791438314}]}`,
		"a date in flight": `{"agents":[{"now":"2026-10-08T05:45:14Z"}]}`,
		"no clock":         `{"labels":["time"],"known":1}`,
	} {
		if got := string(dataMask([]byte(in), 1791438314, 1791438314)); strings.Contains(got, "flight") {
			t.Errorf("dataMask, %s: %s", name, got)
		}
	}

	// D5: a head's Date and expiry, read against the seconds the request was in flight. C's heads of H35's probe P1:
	// an answer that is not cacheable, an nRPC answer to a PUT, an absolute data window's, a static file's.
	const (
		date    = "Thu, 08 Oct 2026 05:40:39 GMT" // second 1791438039
		tx      = "X-Transaction-ID: e1d3fb973ef24a57b1769e6d3359b009"
		opening = "HTTP/1.1 200 OK\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\n" +
			"Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\n"
	)
	head := func(date, expires, tx string) string {
		h := opening + "Date: " + date + "\r\nContent-Type: application/json; charset=utf-8\r\n"
		if expires != "" {
			h += "Expires: " + expires + "\r\n"
		}
		return h + "Content-Length: 18195\r\n" + tx
	}
	at := [2]int64{1791438039, 1791438039}
	for name, c := range map[string]struct {
		in     string
		flight [2]int64
		want   string
	}{
		"not cacheable":           {head(date, date, tx), at, head("now", "Date+0", "X-Transaction-ID: <masked>")},
		"an nRPC answer":          {head(date, "Thu, 08 Oct 2026 05:40:40 GMT", tx), at, head("now", "Date+1", "X-Transaction-ID: <masked>")},
		"cacheable":               {head(date, "Fri, 09 Oct 2026 05:40:39 GMT", tx), at, head("now", "Date+86400", "X-Transaction-ID: <masked>")},
		"an expiry before":        {head(date, "Thu, 08 Oct 2026 05:40:38 GMT", tx), at, head("now", "Date-1", "X-Transaction-ID: <masked>")},
		"no expiry":               {head(date, "", tx), at, head("now", "", "X-Transaction-ID: <masked>")},
		"a flight of two seconds": {head(date, date, tx), [2]int64{1791438038, 1791438039}, head("now", "Date+0", "X-Transaction-ID: <masked>")},
		// what is not the clock stays, for the comparison
		"a date before the flight":  {head(date, date, tx), [2]int64{1791438040, 1791438041}, head(date, date, "X-Transaction-ID: <masked>")},
		"a date after the flight":   {head(date, date, tx), [2]int64{1791438037, 1791438038}, head(date, date, "X-Transaction-ID: <masked>")},
		"a date in another format":  {head("2026-10-08T05:40:39Z", date, tx), at, head("2026-10-08T05:40:39Z", date, "X-Transaction-ID: <masked>")},
		"a date in seconds":         {head("1791438039", date, tx), at, head("1791438039", date, "X-Transaction-ID: <masked>")},
		"an expiry that is no date": {head(date, "0", tx), at, head("now", "0", "X-Transaction-ID: <masked>")},
		// a static file: its Date is the file's, its expiry a day after the request
		"a file's": {head("Wed, 23 Sep 2026 09:19:11 GMT", "Fri, 09 Oct 2026 05:40:39 GMT", tx), at,
			head("Wed, 23 Sep 2026 09:19:11 GMT", "now+86400", "X-Transaction-ID: <masked>")},
		"a file's, a day and a second": {head("Wed, 23 Sep 2026 09:19:11 GMT", "Fri, 09 Oct 2026 05:40:40 GMT", tx), at,
			head("Wed, 23 Sep 2026 09:19:11 GMT", "Fri, 09 Oct 2026 05:40:40 GMT", "X-Transaction-ID: <masked>")},
		// a transaction id is masked by its shape alone, C's random one's: one a request named in that shape too
		"an id the request named": {head(date, date, "X-Transaction-ID: 5a1e0000000040008000000000003100"), at,
			head("now", "Date+0", "X-Transaction-ID: <masked>")},
		"a short id":  {head(date, date, "X-Transaction-ID: e1d3fb97"), at, head("now", "Date+0", "X-Transaction-ID: e1d3fb97")},
		"an empty id": {head(date, date, "X-Transaction-ID: "), at, head("now", "Date+0", "X-Transaction-ID: ")},
		"an id with dashes": {head(date, date, "X-Transaction-ID: e1d3fb97-3ef2-4a57-b176-9e6d3359b009"), at,
			head("now", "Date+0", "X-Transaction-ID: e1d3fb97-3ef2-4a57-b176-9e6d3359b009")},
		"an id in upper case": {head(date, date, "X-Transaction-ID: E1D3FB973EF24A57B1769E6D3359B009"), at,
			head("now", "Date+0", "X-Transaction-ID: E1D3FB973EF24A57B1769E6D3359B009")},
		"no head": {"You need to be authorized to access this resource", at,
			"You need to be authorized to access this resource"},
		"nothing": {"", at, ""},
	} {
		if got := string(maskHead([]byte(c.in), c.flight)); got != c.want {
			t.Errorf("maskHead, %s:\n got %q\nwant %q", name, got, c.want)
		}
	}
	// the body is not the head's: a line of it that reads as a header stays
	answer := head(date, date, tx) + "\r\n\r\nDate: " + date + "\r\nExpires: " + date + "\r\n" + tx
	if got, want := string(maskAnswer([]byte(answer), at)), head("now", "Date+0", "X-Transaction-ID: <masked>")+
		"\r\n\r\nDate: "+date+"\r\nExpires: "+date+"\r\n"+tx; got != want {
		t.Errorf("maskAnswer:\n got %q\nwant %q", got, want)
	}
	if got := string(maskAnswer([]byte("no answer"), at)); got != "no answer" {
		t.Errorf("maskAnswer without a body: %q", got)
	}

	// D7: each side's Content-Length is its own body's length
	for name, c := range map[string]struct {
		raw string
		bad bool
	}{
		"the body's length": {"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{\"a\"}", false},
		"a byte short":      {"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n{\"a\"}", true},
		"a byte long":       {"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\n{\"a\"}", true},
		"no length":         {"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\"}\r\n0\r\n\r\n", false},
		"an empty body":     {"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", false},
		"a body's own line": {"HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\nContent-Length: 999", false},
		"no answer":         {"", false},
	} {
		if err := rawLength([]byte(c.raw)); (err != nil) != c.bad {
			t.Errorf("rawLength, %s: %v", name, err)
		}
	}
	// D7: the strings' escapes as written. C's cloud status and MCP text: a slash as it is, a newline as `\n`, a
	// quote as `\"`; the decoded strings of each planted body are the oracle's (the comparison sees no difference)
	plainBody := `{"api":2,"agents":[{"application":{"cflags":"-O2 \"x\""},"cloud":{"url":"https://app.netdata.cloud",` +
		`"reason":"Agent is not claimed yet"}}],"info":"Next:\n - 'a' \"b\""}`
	fam := infoV2Family()
	for name, c := range map[string]struct {
		candidate string
		fam       v2Family
		bad       bool
	}{
		"the same bytes":              {plainBody, fam, false},
		"a slash escaped":             {strings.Replace(plainBody, `https://app`, `https:\/\/app`, 1), fam, true},
		"a newline as its code":       {strings.Replace(plainBody, `Next:\n`, `Next:\u000a`, 1), fam, true},
		"a quote as its code":         {strings.Replace(plainBody, `\"b\"`, `\u0022b\"`, 1), fam, true},
		"a letter as its code":        {strings.Replace(plainBody, `Agent is`, `\u0041gent is`, 1), fam, true},
		"in a masked value":           {strings.Replace(plainBody, `-O2 \"x\"`, `-O2 \u0022x\u0022`, 1), fam, false},
		"in a masked value, unmasked": {strings.Replace(plainBody, `-O2 \"x\"`, `-O2 \u0022x\u0022`, 1), v2Family{}, true},
		"in a key":                    {strings.Replace(plainBody, `"reason":`, `"re\u0061son":`, 1), fam, true},
	} {
		body := [2][]byte{[]byte(plainBody), []byte(c.candidate)}
		doc := [2]Value{parse(plainBody), parse(c.candidate)}
		if d := Compare(ApplyMasks(doc[0], c.fam.masks), ApplyMasks(doc[1], c.fam.masks)); len(d) != 0 {
			t.Errorf("escapes, %s: the decoded answers differ: %v", name, d)
		}
		if got := v2Escapes(body, doc, c.fam); (got != "") != c.bad {
			t.Errorf("escapes, %s: %q", name, got)
		}
	}
	// what jsonEscapes lists: each literal's escapes, in the text's order
	if got, err := jsonEscapes([]byte(plainBody), parse(plainBody), nil); err != nil ||
		strings.Join(got, " ") != `\"\" \n\"\"` {
		t.Errorf("jsonEscapes: %q (%v)", got, err)
	}
	if got, err := jsonEscapes([]byte(plainBody), parse(plainBody), fam.masks); err != nil ||
		strings.Join(got, " ") != `\n\"\"` {
		t.Errorf("jsonEscapes with the family's masks: %q (%v)", got, err)
	}
	// where a family leaves some members' order to each side, the escapes are compared as sorted lists
	labelsO := `{"nodes":[{"labels":{"a":"x\ny","b":"p\"q"}}]}`
	labelsC := `{"nodes":[{"labels":{"b":"p\"q","a":"x\ny"}}]}`
	bodies, docs := [2][]byte{[]byte(labelsO), []byte(labelsC)}, [2]Value{parse(labelsO), parse(labelsC)}
	if got := v2Escapes(bodies, docs, nodesFamily); got != "" {
		t.Errorf("escapes of labels in another order: %q", got)
	}
	if got := v2Escapes(bodies, docs, v2Family{}); got == "" {
		t.Errorf("escapes of labels in another order, compared in order: no difference")
	}
	if got := v2Escapes([2][]byte{[]byte(labelsO), []byte(strings.Replace(labelsC, `x\ny`, `x\u000ay`, 1))},
		[2]Value{docs[0], parse(strings.Replace(labelsC, `x\ny`, `x\u000ay`, 1))}, nodesFamily); got == "" {
		t.Errorf("another escape in labels in another order: no difference")
	}

	// the layout of a family whose unordered paths are flat maps is compared whole: two answers with the same count
	// of every byte and another shape are told apart (a count alone does not); the labels' order changes no layout
	shapeO := `{"nodes":[{"labels":{"a":"1","b":"2"},"hw":{"x":["1"],"y":"2"}}]}`
	shapeC := `{"nodes":[{"labels":{"b":"2","a":"1"},"hw":{"x":"1","y":["2"]}}]}`
	if l := v2Layouts([2][]byte{[]byte(shapeO), []byte(shapeC)}, true); l != "" {
		t.Errorf("layouts by count: %s", l)
	}
	if l := v2Layouts([2][]byte{[]byte(shapeO), []byte(shapeC)}, nodesFamily.layoutByCount()); l == "" {
		t.Errorf("nodesFamily's layout check took another shape")
	}
	if !contextsLabelsFamily.layoutByCount() || contextsFamily.layoutByCount() || infoTailFamily.layoutByCount() {
		t.Errorf("layoutByCount: the contexts lists' family %v, the contexts family %v, the info tail's %v, want true, "+
			"false, false", contextsLabelsFamily.layoutByCount(), contextsFamily.layoutByCount(),
			infoTailFamily.layoutByCount())
	}
	if l := v2Layouts([2][]byte{[]byte(shapeO), []byte(strings.Replace(shapeC, `{"x":"1","y":["2"]}`,
		`{"x":["1"],"y":"2"}`, 1))}, false); l != "" {
		t.Errorf("the labels in another order change the layout: %s", l)
	}
	if !nodesFamily.flat || !infoTailFamily.flat || contextsLabelsFamily.flat {
		t.Errorf("flat: nodes %v, info tail %v, contexts lists %v, want true, true, false (a label list is no flat map)",
			nodesFamily.flat, infoTailFamily.flat, contextsLabelsFamily.flat)
	}

	// D4: the streaming table, each side against what C fixes between its cells: C's two tables of H35's probe P1
	// (twelve of the 85 columns, renumbered), then one planted cell at a time in the candidate's
	const tableDoc = `{"columns":{"Node":{"index":0},"InStatus":{"index":1},"InReason":{"index":2},` +
		`"dbFrom":{"index":3,"max":{dbFrom.max}},"dbTo":{"index":4,"max":{dbTo.max}},` +
		`"dbDuration":{"index":5,"max":{dbDuration.max}},"InSince":{"index":6,"max":{InSince.max}},` +
		`"InAge":{"index":7,"max":{InAge.max}},"OutSince":{"index":8,"max":{OutSince.max}},` +
		`"OutAge":{"index":9,"max":{OutAge.max}},"InLocalPort":{"index":10,"max":{InLocalPort.max}},` +
		`"InRemotePort":{"index":11,"max":{InRemotePort.max}}},"data":[` +
		`["parity-parent","initializing","LOCALHOST",0,{l.dbTo},{l.dbDuration},{l.InSince},{l.InAge},{OutSince},{OutAge},{l.local},{l.remote}],` +
		`["parity-rchild","online","{reason}",{dbFrom},{dbTo},{dbDuration},{InSince},{InAge},{OutSince},{OutAge},{local},{remote}],` +
		`["rchild-v","online","CONNECTED",1791438074000,{l.dbTo},13,{v.InSince},{v.InAge},{OutSince},{OutAge},{v.local},{v.remote}]]}`
	table := func(defaults []string, cells ...string) Value {
		return parse(strings.NewReplacer(append(cells, defaults...)...).Replace(tableDoc))
	}
	oracle := []string{"{dbFrom.max}", "1791438074000", "{dbTo.max}", "1791438087000", "{dbDuration.max}", "26",
		"{InSince.max}", "1791438083000", "{InAge.max}", "35", "{OutSince.max}", "1791438052000", "{OutAge.max}", "35",
		"{InLocalPort.max}", "38231", "{InRemotePort.max}", "36916", "{l.dbTo}", "1791438087000",
		"{l.InSince}", "1791438052000", "{l.InAge}", "35", "{OutSince}", "1791438052000", "{OutAge}", "35",
		"{l.local}", "0", "{l.remote}", "0", "{l.dbDuration}", "null", "{reason}", "CONNECTED", "{dbFrom}", "1791438061000",
		"{dbTo}", "1791438087000", "{dbDuration}", "26", "{InSince}", "1791438068000", "{InAge}", "19",
		"{local}", "38231", "{remote}", "36916", "{v.InSince}", "1791438083000", "{v.InAge}", "4", "{v.local}", "38231",
		"{v.remote}", "34090"}
	candidate := []string{"{dbFrom.max}", "1791438074000", "{dbTo.max}", "1791438087000", "{dbDuration.max}", "22",
		"{InSince.max}", "1791438082000", "{InAge.max}", "32", "{OutSince.max}", "1791438055000", "{OutAge.max}", "32",
		"{InLocalPort.max}", "40861", "{InRemotePort.max}", "43806", "{l.dbTo}", "1791438087000",
		"{l.InSince}", "1791438055000", "{l.InAge}", "32", "{OutSince}", "1791438055000", "{OutAge}", "32",
		"{l.local}", "0", "{l.remote}", "0", "{l.dbDuration}", "null", "{reason}", "CONNECTED", "{dbFrom}", "1791438065000",
		"{dbTo}", "1791438087000", "{dbDuration}", "22", "{InSince}", "1791438072000", "{InAge}", "15",
		"{local}", "40861", "{remote}", "33994", "{v.InSince}", "1791438082000", "{v.InAge}", "5", "{v.local}", "40861",
		"{v.remote}", "43806"}
	sent := [2]int64{1791438087, 1791438087}
	if err := fnStreamingFacts(table(oracle), "38231", sent); err != nil {
		t.Errorf("fnStreamingFacts on the oracle's table: %v", err)
	}
	two := [2]int64{1791438086, 1791438087}
	for name, c := range map[string]struct {
		cells  []string
		port   string
		flight [2]int64
		want   string // what the refusal says; none for a table that holds
	}{
		"the candidate's table":   {nil, "40861", sent, ""},
		"a flight of two seconds": {nil, "40861", two, ""},
		"asked a second later":    {nil, "40861", [2]int64{1791438088, 1791438089}, "row 0: InSince 1791438055000 and InAge 32: not"},
		"asked a second earlier":  {nil, "40861", [2]int64{1791438085, 1791438086}, "row 0: InSince 1791438055000 and InAge 32: not"},
		"the oracle's port":       {nil, "38231", sent, "row 1 (CONNECTED): InLocalPort 40861 and InRemotePort 33994"},
		"a start in seconds":      {[]string{"{InSince}", "1791438072"}, "40861", sent, "row 1: InSince 1791438072 and InAge 15: not"},
		"a start that is no whole second": {[]string{"{InSince}", "1791438072500"}, "40861", sent,
			"row 1: InSince 1791438072500 and InAge 15: not"},
		"an age from another clock": {[]string{"{InAge}", "16"}, "40861", sent, "row 1: InSince 1791438072000 and InAge 16: not"},
		"an age in milliseconds":    {[]string{"{InAge}", "15000"}, "40861", sent, "row 1: InSince 1791438072000 and InAge 15000: not"},
		"a negative age":            {[]string{"{InAge}", "-15"}, "40861", sent, "row 1: InAge is -15, want a whole number or null"},
		// every row reads one clock: a row a second behind is refused though its second is one of the flight's
		"an age a second off, in a flight of two": {[]string{"{InAge}", "14"}, "40861", two,
			"row 1: InSince 1791438072000 and InAge 14: not"},
		"a length where there is no start": {[]string{"{l.dbDuration}", "87", "{dbDuration.max}", "87"}, "40861", sent,
			"row 0: dbDuration is not the seconds from dbFrom 0 to"},
		"a start without its age":    {[]string{"{InAge}", "null"}, "40861", sent, "row 1: InSince and InAge are not both null"},
		"an age without its start":   {[]string{"{InSince}", "null"}, "40861", sent, "row 1: InSince and InAge are not both null"},
		"an outbound age off by one": {[]string{"{OutAge}", "31", "{OutAge.max}", "31"}, "40861", sent, "row 0: OutSince 1791438055000 and OutAge 31: not"},
		"the retention's start in seconds": {[]string{"{dbFrom}", "1791438065"}, "40861", sent,
			"row 1: dbFrom 1791438065 and dbTo 1791438087000: not"},
		// the length is the whole seconds between the two, so only the start's own rule refuses half a second
		"the retention's start no whole second": {[]string{"{dbFrom}", "1791438065500", "{dbDuration}", "21",
			"{dbDuration.max}", "21"}, "40861", sent, "row 1: dbFrom 1791438065500 and dbTo 1791438087000: not"},
		"the retention's end in seconds": {[]string{"{dbTo}", "1791438087"}, "40861", sent,
			"row 1: dbFrom 1791438065000 and dbTo 1791438087: not"},
		// a second of the flight, with the length that fits it: only the table's one clock refuses it
		"the retention's end a second ago": {[]string{"{dbTo}", "1791438086000", "{dbDuration}", "21",
			"{dbDuration.max}", "21"}, "40861", two, "row 1: dbFrom 1791438065000 and dbTo 1791438086000: not"},
		"a length off by one": {[]string{"{dbDuration}", "23", "{dbDuration.max}", "23"}, "40861", sent,
			"row 1: dbDuration is not the seconds from dbFrom 1791438065000 to dbTo 1791438087000"},
		"a length in milliseconds": {[]string{"{dbDuration}", "22000", "{dbDuration.max}", "22000"}, "40861", sent,
			"row 1: dbDuration is not"},
		"no length where there is a start": {[]string{"{dbDuration}", "null", "{dbDuration.max}", "13"}, "40861", sent,
			"row 1: dbDuration is not"},
		"the two ports swapped": {[]string{"{local}", "33994", "{remote}", "40861"}, "40861", sent,
			"row 1 (CONNECTED): InLocalPort 33994 and InRemotePort 40861"},
		"a child's remote port of 0":         {[]string{"{remote}", "0"}, "40861", sent, "row 1 (CONNECTED): InLocalPort 40861 and InRemotePort 0,"},
		"a child's remote port the parent's": {[]string{"{remote}", "40861"}, "40861", sent, "row 1 (CONNECTED): InLocalPort 40861 and InRemotePort 40861,"},
		"a child's remote port that is none": {[]string{"{remote}", "65536", "{InRemotePort.max}", "65536"}, "40861", sent,
			"row 1 (CONNECTED): InLocalPort 40861 and InRemotePort 65536,"},
		"a child's local port of 0":      {[]string{"{local}", "0"}, "40861", sent, "row 1 (CONNECTED): InLocalPort 0 and"},
		"a port on localhost":            {[]string{"{l.local}", "40861"}, "40861", sent, "row 0 (LOCALHOST): InLocalPort 40861 and InRemotePort 0,"},
		"a remote port on localhost":     {[]string{"{l.remote}", "5"}, "40861", sent, "row 0 (LOCALHOST): InLocalPort 0 and InRemotePort 5,"},
		"a port that is null":            {[]string{"{remote}", "null"}, "40861", sent, "row 1: a port is null"},
		"a port that is null, localhost": {[]string{"{l.local}", "null"}, "40861", sent, "row 0: a port is null"},
		"a port as a string":             {[]string{"{remote}", `"33994"`}, "40861", sent, `row 1: InRemotePort is "33994", want a whole number or null`},
		"a negative port":                {[]string{"{remote}", "-1"}, "40861", sent, "row 1: InRemotePort is -1, want a whole number or null"},
		"a port with a point":            {[]string{"{remote}", "33994.5"}, "40861", sent, "row 1: InRemotePort is 33994.5, want a whole number or null"},
		"a maximum that is not the largest": {[]string{"{InRemotePort.max}", "33994"}, "40861", sent,
			"columns.InRemotePort.max is 33994, the largest cell is 43806"},
		"a maximum above the largest": {[]string{"{InAge.max}", "33"}, "40861", sent, "columns.InAge.max is 33, the largest cell is 32"},
		"a length's maximum, the smaller": {[]string{"{dbDuration.max}", "13"}, "40861", sent,
			"columns.dbDuration.max is 13, the largest cell is 22"},
		"a start's maximum in seconds": {[]string{"{InSince.max}", "1791438082"}, "40861", sent,
			"columns.InSince.max is 1791438082, the largest cell is 1791438082000"},
		"a maximum as a string": {[]string{"{InAge.max}", `"32"`}, "40861", sent, `columns.InAge.max is "32", the largest cell is 32`},
		"a host that is not received": {[]string{"{reason}", "VIRTUAL NODE"}, "40861", sent,
			"row 1 (VIRTUAL NODE): InLocalPort 40861 and InRemotePort 33994,"},
		"a host that is not received, without ports": {[]string{"{reason}", "VIRTUAL NODE", "{local}", "0", "{remote}", "0",
			"{InLocalPort.max}", "40861", "{InRemotePort.max}", "43806"}, "40861", sent, ""},
	} {
		if got := problem(fnStreamingFacts(table(candidate, c.cells...), c.port, c.flight)); (c.want == "") != (got == "") ||
			!strings.Contains(got, c.want) {
			t.Errorf("fnStreamingFacts, %s: %q, want %q", name, got, c.want)
		}
	}
	// a table without rows has no clock to read the others by: its columns, each maximum 0, are not what refuses it
	var none []string
	for _, name := range fnStreamingVolatile {
		none = append(none, `"`+name+`":{"index":0,"max":0}`)
	}
	if got := problem(fnStreamingFacts(parse(`{"columns":{`+strings.Join(none, ",")+`},"data":[]}`), "40861", sent)); got !=
		"no row has a time" {
		t.Errorf("fnStreamingFacts on a table without rows: %q", got)
	}
	// and the masks still hide what each side fixed for itself: the two recorded tables show no difference
	o, c := table(oracle), table(candidate)
	if d := Compare(ApplyMasks(o, fnStreamingMasks(o, fnStreamingVolatile)),
		ApplyMasks(c, fnStreamingMasks(c, fnStreamingVolatile))); len(d) != 0 {
		t.Errorf("the recorded tables differ: %v", d)
	}

	// D9: runR's tier 0 sample counts, beside D228's mask: whole numbers above 0, within 5% of each other (C's pair
	// of H31's probe: 106565 apart, 2.1%)
	tiers := func(samples string) Value {
		return parse(`{"agents":[{"db_size":[{"tier":0,"metrics":116,"samples":` + samples + `},{"tier":1,` +
			`"samples":66860}]}]}`)
	}
	for name, c := range map[string]struct {
		o, c string
		bad  bool
	}{
		"C's pair":             {"5294079", "5187514", false},
		"the same count":       {"5187514", "5187514", false},
		"five percent":         {"5187514", "5446889", false},
		"above five percent":   {"5187514", "5446890", true},
		"the lower one first":  {"5446890", "5187514", true},
		"a count of 0":         {"5187514", "0", true},
		"both counts 0":        {"0", "0", true},
		"the oracle's of 0":    {"0", "5187514", true},
		"a negative count":     {"5187514", "-5187514", true},
		"a count with a point": {"5187514", "5187514.5", true},
		"a count as a string":  {"5187514", `"5187514"`, true},
		"a count that is null": {"5187514", "null", true},
		"an exponent":          {"5187514", "5.187514e6", true},
	} {
		if _, err := infoV2RunRJudge(tiers(c.o), tiers(c.c)); (err != nil) != c.bad {
			t.Errorf("infoV2RunRJudge, %s: %v", name, err)
		}
	}
	if _, err := infoV2RunRJudge(tiers("5187514"), parse(`{"agents":[{"db_size":[]}]}`)); err == nil {
		t.Errorf("infoV2RunRJudge took an answer without a tier")
	}
	if n, err := infoV2RunRJudge(tiers("5294079"), tiers("5187514")); err != nil || n != [2]int64{5294079, 5187514} {
		t.Errorf("infoV2RunRJudge's counts: %v (%v)", n, err)
	}

	// the weights' `two-hosts` row: its contexts' version is twice the one host's serial walk's (C: 10 and 20, the
	// answers' other members trimmed to what the guards read)
	weights := func(hash string) Value {
		return parse(`{"api":2,"versions":{"contexts_hard_hash":` + hash + `},"dictionaries":{"dimensions":[{"id":"anom",` +
			`"di":0}]},"result":` + weightsAnomaly + `,"correlated_dimensions":1}`)
	}
	row := func(rows []v2Req, name string) v2Req {
		t.Helper()
		for _, r := range rows {
			if r.name == name {
				return r
			}
		}
		t.Fatalf("no row %s", name)
		return v2Req{}
	}
	rows := weightsRequests()
	if err := row(rows, "two-hosts").guard(weights("20")); err == nil || !strings.Contains(err.Error(), "harness:") {
		t.Errorf("two-hosts before anomaly: %v", err)
	}
	for name, c := range map[string]struct {
		anomaly, two string
		first, bad   bool
	}{
		"C's answers":           {"10", "20", false, false},
		"the serial walk":       {"10", "10", false, true},
		"three times":           {"10", "30", false, true},
		"no context":            {"0", "0", true, true},
		"a version as a string": {`"10"`, "20", true, true},
	} {
		rows := weightsRequests()
		err := row(rows, "anomaly").guard(weights(c.anomaly))
		if (err != nil) != c.first {
			t.Errorf("the anomaly guard, %s: %v", name, err)
		}
		if err == nil {
			if err := row(rows, "two-hosts").guard(weights(c.two)); (err != nil) != c.bad {
				t.Errorf("the two-hosts guard, %s: %v", name, err)
			}
		}
	}
	// its row beside the v1 guards: the deprecated route's dimensions, exactly (weightsV1Weigh)
	mc := row(weightsV1Requests(), "mc").guard
	for name, c := range map[string]struct {
		dims string
		bad  bool
	}{
		"C's answer":       {`{"flat2":0,"jump":1}`, false},
		"a weight off":     {`{"flat2":0,"jump":0.999}`, true},
		"a dimension more": {`{"flat2":0,"jump":1,"anom":0}`, true},
		"a dimension less": {`{"jump":1}`, true},
		"a weight as text": {`{"flat2":0,"jump":"1"}`, true},
	} {
		v := parse(`{"correlated_charts":{"` + fixture.WeightsKS2Context + `":{"dimensions":` + c.dims + `}}}`)
		if err := mc(v); (err != nil) != c.bad {
			t.Errorf("the mc guard, %s: %v", name, err)
		}
	}

	// two guards that had no unit (R91's sub-reviewer sa1): the search's facts and the agent's info of a node-instance
	// answer, on C's answers (H35's probe P1; the agent's application, capabilities and tiers emptied, its cloud status
	// as niIngestRender leaves it)
	search := `{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,` +
		`"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},` +
		`"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,` +
		`"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{` +
		`"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791438573,"ai":0,` +
		`"timings":{"prep_ms":0,"query_ms":0.094,"output_ms":0.078,"total_ms":0.172,"cloud_ms":0.172}}],` +
		`"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.172}}`
	for name, c := range map[string]struct {
		in  string
		bad bool
	}{
		"C's answer":            {search, false},
		"another count":         {strings.Replace(search, `"strings":15,"char":0,"total":15`, `"strings":14,"char":0,"total":14`, 1), true},
		"matched by its id too": {strings.Replace(search, `"matched":["dimensions"]`, `"matched":["id","dimensions"]`, 1), true},
		"the dimension's id":    {strings.Replace(search, `"dimensions":["alpha"]`, `"dimensions":["a"]`, 1), true},
		"no context":            {strings.Replace(search, `{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}}`, `{}`, 1), true},
		"no searches":           {strings.Replace(search, `"searches":{"strings":15,"char":0,"total":15},`, ``, 1), true},
	} {
		if err := dashGuard(dashWalkNodes(false), searchFacts)(parse(c.in)); (err != nil) != c.bad {
			t.Errorf("searchFacts, %s: %v", name, err)
		}
	}
	agentInfo := `{"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791438039,` +
		`"ai":0,"application":{},"cloud":{"id":0,"status":"available","since":"START","age":"NOW-SINCE"},` +
		`"nodes":{"total":2,"receiving":1,"sending":0,"archived":0},` +
		`"metrics":{"collected":7,"available":7},"instances":{"collected":2,"available":2},` +
		`"contexts":{"collected":1,"available":1,"unique":1},"capabilities":[],` +
		`"api":{"version":7,"bearer_protection":false},"db_size":[],"timings":{}}]}`
	for name, c := range map[string]struct {
		in  string
		bad bool
	}{
		"C's answer":           {agentInfo, false},
		"without the child":    {strings.Replace(agentInfo, `"total":2,"receiving":1`, `"total":1,"receiving":0`, 1), true},
		"the child archived":   {strings.Replace(agentInfo, `"receiving":1,"sending":0,"archived":0`, `"receiving":0,"sending":0,"archived":1`, 1), true},
		"another agent":        {strings.Replace(agentInfo, `0000000000aa`, `0000000000bb`, 1), true},
		"no tiers":             {strings.Replace(agentInfo, `"db_size":[],`, ``, 1), true},
		"the counts after api": {strings.Replace(strings.Replace(agentInfo, `"capabilities":[],`, ``, 1), `"db_size":[],`, `"capabilities":[],"db_size":[],`, 1), true},
		// the cloud status begins when the agent starts: a second the render did not name START is refused
		"a cloud status begun later": {strings.Replace(agentInfo, `"since":"START"`, `"since":1791438039`, 1), true},
		"a cloud status' age off":    {strings.Replace(agentInfo, `"age":"NOW-SINCE"`, `"age":0`, 1), true},
		"a cloud that is online":     {strings.Replace(agentInfo, `"status":"available"`, `"status":"online"`, 1), true},
	} {
		if err := dashGuard(niAgentFacts)(parse(c.in)); (err != nil) != c.bad {
			t.Errorf("niAgentFacts, %s: %v", name, err)
		}
	}

	// the capability lists by name (capabilityProblems; `health.api`'s node list reads it without the fatal end):
	// C's list of the probe and a candidate's that differs as a table of the pin's own lists it (the checks' table,
	// capabilityDiffs, loses an entry with each milestone that ports its subsystem)
	listed := map[string]capabilityDiff{
		"proto":      {"1/true", "1/false", "M11 Cloud"},
		"ml":         {"1/false", "0/false", "M14 ML"},
		"mc":         {"1/true", "1/false", "M10 weights"},
		"req_cancel": {"1/true", "1/false", "M11 Cloud"},
	}
	caps := func(values ...string) Value {
		var items []string
		for i := 0; i+1 < len(values); i += 2 {
			version, enabled, _ := strings.Cut(values[i+1], "/")
			items = append(items, `{"name":"`+values[i]+`","version":`+version+`,"enabled":`+enabled+`}`)
		}
		return parse("[" + strings.Join(items, ",") + "]")
	}
	cCaps := []string{"proto", "1/true", "ml", "1/false", "mc", "1/true", "ctx", "1/true", "health", "2/true",
		"req_cancel", "1/true"}
	rustCaps := []string{"proto", "1/false", "ml", "0/false", "mc", "1/false", "ctx", "1/true", "health", "2/true",
		"req_cancel", "1/false"}
	with := func(list []string, name, value string) Value {
		list = append([]string(nil), list...)
		for i := 0; i+1 < len(list); i += 2 {
			if list[i] == name {
				list[i+1] = value
			}
		}
		return caps(list...)
	}
	for name, c := range map[string]struct {
		o, c  Value
		same  bool
		names bool
		want  []string // a text of each problem, in order
	}{
		"C against C":              {caps(cCaps...), caps(cCaps...), true, false, nil},
		"C against the Rust agent": {caps(cCaps...), caps(rustCaps...), false, false, nil},
		"health off on the candidate": {caps(cCaps...), with(rustCaps, "health", "2/false"), false, false,
			[]string{"capability health: oracle 2/true, candidate 2/false"}},
		"health off, C against C": {caps(cCaps...), with(cCaps, "health", "2/false"), true, false,
			[]string{"capability health: oracle 2/true, candidate 2/false"}},
		"a listed one as C's, C against C": {caps(cCaps...), with(cCaps, "ml", "0/false"), true, false,
			[]string{"capability ml: oracle 1/false, candidate 0/false"}},
		"a listed one now as C's": {caps(cCaps...), with(rustCaps, "mc", "1/true"), false, false,
			[]string{"capability mc (M10 weights): oracle 1/true, candidate 1/true; listed 1/true and 1/false",
				"capability mc: the candidate now says 1/true as C does: remove it from capabilityDiffs"}},
		"a listed one at another value": {caps(cCaps...), with(rustCaps, "ml", "2/false"), false, false,
			[]string{"capability ml (M14 ML): oracle 1/false, candidate 2/false; listed 1/false and 0/false"}},
		"the oracle's listed one off": {with(cCaps, "proto", "1/false"), caps(rustCaps...), false, false,
			[]string{"capability proto (M11 Cloud): oracle 1/false, candidate 1/false; listed 1/true and 1/false",
				"capability proto: the candidate now says 1/false as C does"}},
		"a listed one that neither has": {caps(cCaps[2:]...), caps(rustCaps[2:]...), false, false,
			[]string{"3 of the 4 listed capabilities exist"}},
		"neither has it, C against C": {caps(cCaps[2:]...), caps(cCaps[2:]...), true, false, nil},
		"another order":               {caps(cCaps...), caps(append(append([]string(nil), rustCaps[2:]...), rustCaps[:2]...)...), false, true, []string{"capabilities differ"}},
		"a name less":                 {caps(cCaps...), caps(rustCaps[:10]...), false, true, []string{"capabilities differ"}},
		"no list on the candidate":    {caps(cCaps...), Value{}, false, true, []string{"capabilities differ"}},
		"no list on either, the Rust": {Value{}, Value{}, false, false, []string{"0 of the 4 listed capabilities exist"}},
		"no list on either, C's":      {Value{}, Value{}, true, false, nil},
	} {
		problems, names := capabilityProblems("x", c.o, c.c, c.same, listed)
		ok := names == c.names && len(problems) == len(c.want)
		for i := 0; ok && i < len(problems); i++ {
			ok = strings.HasPrefix(problems[i], "x: ") && strings.Contains(problems[i], c.want[i])
		}
		if !ok {
			t.Errorf("capabilityProblems, %s: names %v, problems %q, want %q", name, names, problems, c.want)
		}
	}

	// the node families keep their check: with the capabilities' values masked (D6), it alone compares them
	if nodesFamily.check == nil || nodeInstancesFamily([2]niSide{}).check == nil ||
		fnStreamNodesFamily([2]string{}).check == nil {
		t.Errorf("a node family has no check: nothing compares its capabilities' versions and flags")
	}

	// runR's family is the info family with runR's masks and the samples' judge for its check: the mask alone would
	// take any count (D9)
	if fam := infoV2RunRFamily(); fam.check == nil || len(fam.masks) != len(infoV2Volatile)+len(infoV2RunR) ||
		infoV2Family().check != nil {
		t.Errorf("infoV2RunRFamily: check %v, %d masks, want the samples' judge and %d", fam.check != nil, len(fam.masks),
			len(infoV2Volatile)+len(infoV2RunR))
	}

	// `/api/v1/contexts`' own mask (maskNow): a collected object's `last_time_t`, where it is a second of the flight
	for name, c := range map[string]struct{ in, want string }{
		"a second of the flight": {`{"last_time_t":1791438314,"first_time_t":1791438314}`, `{"last_time_t":"NOW","first_time_t":1791438314}`},
		"the flight's end":       {`{"last_time_t":1791438315}`, `{"last_time_t":"NOW"}`},
		"a second before":        {`{"last_time_t":1791438313}`, `{"last_time_t":1791438313}`},
		"a second after":         {`{"last_time_t":1791438316}`, `{"last_time_t":1791438316}`},
		"another key":            {`{"last_entry":1791438314,"le":1791438314}`, `{"last_entry":1791438314,"le":1791438314}`},
	} {
		if got := string(maskNow([]byte(c.in), 1791438314, 1791438315)); got != c.want {
			t.Errorf("maskNow, %s: %s, want %s", name, got, c.want)
		}
	}

	// the harness's own rails
	if facts := dashMembers([]string{"nodes"}, "state", `"reachable"`, "health"); len(facts) != 2 ||
		problem(facts[0](parse(`{"nodes":{"state":"reachable"}}`))) !=
			`harness: dashMembers of nodes got "health" without a want` {
		t.Errorf("dashMembers with a name and no want: %d facts", len(facts))
	}
	if facts := dashMembers([]string{"nodes"}, "state", `"reachable"`); len(facts) != 1 ||
		facts[0](parse(`{"nodes":{"state":"reachable"}}`)) != nil {
		t.Errorf("dashMembers of one pair")
	}
	confs := []accessConf{accessACL, accessBearer}
	if got := accessStray(confs, accessRoutes("/api/v3/nodes")); got != "" {
		t.Errorf("accessStray on a route's rows: %q", got)
	}
	if got := accessStray(confs, append(accessRoutes("/api/v3/nodes"), accessRow{conf: "acll", name: "typo"})); got !=
		"acll/typo" {
		t.Errorf("accessStray on a row of no configuration: %q", got)
	}
	replay := dashboardRequests(dashNormBase)
	for _, r := range replay {
		if !r.judged() {
			t.Errorf("the replay's row %s names no answer of the oracle's", r.name)
		}
	}
	for name, r := range map[string]dashReq{
		"a v2 row without a guard":     {kind: dashV2, status: "200"},
		"a v2 row without a status":    {kind: dashV2, guard: dashHolds("x")},
		"an exact row without its end": {kind: dashExact, want: [2]string{dashOK, ""}},
		"a masked row without a start": {kind: dashMasked, want: [2]string{"", "}"}},
		"a same row without a want":    {kind: dashSame},
		"a file row without its file":  {kind: dashStatic, target: "/"},
	} {
		if r.judged() {
			t.Errorf("judged: %s counts as judged", name)
		}
	}
	if len(replay) != 29 {
		t.Errorf("%d replay rows, pinned 29", len(replay))
	}
	if err := dashText("query timeout")(Value{Kind: KindString, Text: "query timeout"}); err != nil {
		t.Errorf("dashText on its text: %v", err)
	}
	if err := dashText("query timeout")(Value{Kind: KindString, Text: "query timeout\n"}); err == nil {
		t.Errorf("dashText took another text")
	}
}
