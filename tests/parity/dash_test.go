// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net/http"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The dashboard APIs (milestone 10): the shared pieces of their checks. Each check asks raw requests of both agents
// (`v2Req`), guards the oracle's answer with a fact of its fixture where the row has one (`api.v2-info`'s rows and the
// access rows of a route's own answers have none: the first compare the agent's own info, the others pin the answer
// whole), and compares the answers as the request's family says (`v2Family`): the envelope of `/api/v2/info` and its
// kin, with the family's masks, unordered paths and settle time.

// v2Req is one request of a dashboard check.
type v2Req struct {
	name    string
	method  string // empty: GET
	target  string
	body    []byte // nil: no body and no Content-Length
	headers []string
	status  string // the status code the oracle must answer
	// guard, when set, judges the oracle's answer before anything is compared: its JSON document for a 200, its body
	// as a string value otherwise. An error fails the row on the oracle.
	guard func(Value) error
	from  string // the client's address (empty: any)
	// targets, when set, are each side's own target (0 the oracle, 1 the candidate), for a request that names a value
	// of each side's own (a transition id); target then only names the row. Empty: target on both.
	targets [2]string
}

// targetOf is the target side i is asked.
func (r v2Req) targetOf(i int) string {
	if r.targets[i] != "" {
		return r.targets[i]
	}
	return r.target
}

// v2Family is how a family of answers is compared.
type v2Family struct {
	masks     []Mask
	unordered []string
	// flat says that every unordered path names a map or a list of scalars (a label map, a set of names): its layout
	// does not change with its members' order, so the two bodies' layouts are compared whole, as an ordered family's
	// (v2Layouts).
	flat bool
	// settle, when non-zero, asks both agents again each second until their answers agree or the time passes (the
	// guard is judged on each of the oracle's answers).
	settle time.Duration
	// now, when set, names the members written "NOW" where they hold a second of the answering agent's clock on that
	// side (v2Clock; maskNowKeys: C prints its walk's clock there for what is collected).
	now []string
	// wall says that the answering agent's clock is the wall clock whatever the request asks: the weights engine
	// passes no clock of its own (weights.c:1352, :1507; api_v2_contexts_agents.c:12-13), where the v2 walk, asked
	// with a window, takes the second before (v2Clock).
	wall bool
	// render, when set, normalises each side's body (0 the oracle, 1 the candidate) after `now`, before it is parsed;
	// flight are the seconds the request was in flight on that side.
	render func(i int, flight [2]int64, body []byte) []byte
	// check, when set, compares the parsed answers further once they agree (e.g. a list compared by name).
	check func(t *testing.T, name string, o, c Value)
}

// v2Request is req's bytes: its method (GET when empty), its target, `Host: localhost`, `Connection: close` and its
// header lines, then its body.
func v2Request(req v2Req) []byte {
	method := req.method
	if method == "" {
		method = "GET"
	}
	headers := append([]string{"Host: localhost", "Connection: close"}, req.headers...)
	return rawRequest(method, req.target, headers, req.body)
}

// v2Exchange sends one request and returns the raw answer.
func v2Exchange(addr string, req v2Req) ([]byte, error) {
	return rawExchangeFrom(req.from, addr, v2Request(req), 30*time.Second)
}

// v2Answer is one round of a request: both raw answers, the seconds each was in flight ([from, to]) and, for a 200,
// both parsed bodies.
type v2Answer struct {
	raw, body [2][]byte
	flight    [2][2]int64
	doc       [2]Value
	problems  []string
}

// v2Windowed tells whether target asks C's v2 walk for a window: an `after` or a `before` whose number is not 0, as the
// route reads them (web/api/v2/api_v2_contexts.c:13-18, :35-38: the last one of a name wins, a parameter without a
// value is skipped; str2l, libnetdata/inlined.h:159-171: leading blanks, a sign, then digits). It reads the target as
// it is written: C splits the query after decoding it (web_client.c:2109-2110), so a row must spell these two
// parameters without percent-escapes (every row does).
func v2Windowed(target string) bool {
	_, query, _ := strings.Cut(target, "?")
	window := map[string]bool{}
	for _, pair := range strings.Split(query, "&") {
		name, value, _ := strings.Cut(pair, "=")
		if (name != "after" && name != "before") || value == "" {
			continue
		}
		digits := strings.TrimLeft(value, " \t\n\v\f\r")
		if digits != "" && (digits[0] == '-' || digits[0] == '+') {
			digits = digits[1:]
		}
		n := strings.TrimLeft(digits, "0")
		window[name] = n != "" && n[0] >= '1' && n[0] <= '9'
	}
	return window["after"] || window["before"]
}

// v2Clock are the seconds the answering agent's clock may hold in an answer to target that was in flight for the
// seconds flight: those seconds themselves, the agent reading its clock while it answers (api_v2_contexts_agents.c:
// 12-13; database/contexts/api_v2_contexts.c:1374). C's v2 walk asked for a window takes the second before its clock
// instead (:1367-1370; rrdr_relative_window_to_absolute_query, libnetdata/libnetdata.c:549-555) and prints that as the
// agent's `now` and as what is collected now (:1542, :1213): each end a second earlier, unless the family's clock is
// the wall's whatever is asked (v2Family.wall).
func (fam v2Family) v2Clock(target string, flight [2]int64) [2]int64 {
	if !fam.wall && v2Windowed(target) {
		return [2]int64{flight[0] - 1, flight[1] - 1}
	}
	return flight
}

// normalise is what fam does to side i's body before it is parsed: `now`'s members written NOW for the seconds
// clock of the agent's clock (v2Clock), then the render, with the seconds flight the request was in flight.
func (fam v2Family) normalise(i int, clock, flight [2]int64, body []byte) []byte {
	if len(fam.now) > 0 {
		body = maskNowKeys(body, clock[0], clock[1], fam.now...)
	}
	if fam.render != nil {
		body = fam.render(i, flight, body)
	}
	return body
}

// v2Second is the second a clock member of a v2 answer holds: a number of seconds or, with `options=rfc3339`, a date
// (buffer_json_member_add_time_t_formatted: libnetdata/buffer/buffer.h).
func v2Second(v Value) (int64, error) {
	switch v.Kind {
	case KindNumber:
		return strconv.ParseInt(v.Text, 10, 64)
	case KindString:
		t, err := time.Parse(time.RFC3339Nano, v.Text)
		return t.Unix(), err
	}
	return 0, fmt.Errorf("neither a number nor a date")
}

// v2NowInFlight holds when the first agent's `now`, where the answer has agents, is a second in [clock[0], clock[1]]
// (v2Clock): a number of seconds or an RFC 3339 date, nothing else.
func v2NowInFlight(v Value, clock [2]int64) error {
	now, ok := agentMember(v, "now")
	if !ok {
		return nil
	}
	if n, err := v2Second(now); err != nil || n < clock[0] || n > clock[1] {
		return fmt.Errorf("agents[0].now is %s, not a second of the agent's clock while the request was in flight "+
			"[%d, %d]", now, clock[0], clock[1])
	}
	return nil
}

// v2TiersInFlight holds when each tier's `to` of the first agent's `db_size`, where the answer has one, is a second
// the request was in flight: C ends every tier's retention at the wall clock it reads while it answers
// (database/rrd-retention.c:30, :84), whatever the request's window.
func v2TiersInFlight(v Value, flight [2]int64) error {
	db, ok := agentMember(v, "db_size")
	if !ok {
		return nil
	}
	for i, tier := range db.Items {
		to, err := dashMember(tier, "to")
		if err != nil {
			continue
		}
		if n, err := v2Second(to); err != nil || n < flight[0] || n > flight[1] {
			return fmt.Errorf("agents[0].db_size[%d].to is %s, not a second of the request's flight [%d, %d]", i, to,
				flight[0], flight[1])
		}
	}
	return nil
}

// v2Round asks both agents once and lists what differs. A problem of the oracle's answer starts with "oracle:".
func v2Round(p *Pair, req v2Req, fam v2Family) v2Answer {
	var a v2Answer
	for i, side := range p.Each() {
		r := req
		r.target = req.targetOf(i)
		from := time.Now().Unix()
		b, err := v2Exchange(side.Daemon.Addr, r)
		if err != nil {
			a.problems = append(a.problems, fmt.Sprintf("%s: %v", side.Role, err))
			return a
		}
		a.raw[i], a.flight[i] = b, [2]int64{from, time.Now().Unix()}
	}
	if !bytes.HasPrefix(a.raw[0], []byte("HTTP/1.1 "+req.status+" ")) {
		a.problems = append(a.problems, fmt.Sprintf("oracle: answered %q, expected %s", truncateBytes(a.raw[0]),
			req.status))
		return a
	}
	// the heads but for the length, which each side must have written for its own body
	var head [2][]byte
	for i, side := range p.Each() {
		h, _, _ := bytes.Cut(a.raw[i], []byte("\r\n\r\n"))
		if err := rawLength(a.raw[i]); err != nil {
			a.problems = append(a.problems, fmt.Sprintf("%s: %v", side.Role, err))
		}
		head[i] = contentLengthRe.ReplaceAll(maskHead(h, a.flight[i]), []byte("Content-Length: <masked>"))
	}
	if !bytes.Equal(head[0], head[1]) {
		a.problems = append(a.problems, fmt.Sprintf("headers differ\noracle:    %q\ncandidate: %q", head[0], head[1]))
	}
	// each side's run directory, in the build info's directories and the rules' sources
	var clock [2][2]int64
	for i, side := range p.Each() {
		clock[i] = fam.v2Clock(req.targetOf(i), a.flight[i])
		a.body[i] = fam.normalise(i, clock[i], a.flight[i],
			bytes.ReplaceAll(httpBody(a.raw[i]), []byte(side.Daemon.Opts.RunDir), []byte("<run>")))
	}
	if req.status != "200" {
		if req.guard != nil {
			if err := req.guard(Value{Kind: KindString, Text: string(a.body[0])}); err != nil {
				a.problems = append(a.problems, fmt.Sprintf("oracle: %v: %q", err, truncateBytes(a.body[0])))
				return a
			}
		}
		if !bytes.Equal(a.body[0], a.body[1]) {
			a.problems = append(a.problems, fmt.Sprintf("bodies differ\noracle:    %q\ncandidate: %q",
				truncateBytes(a.body[0]), truncateBytes(a.body[1])))
		}
		return a
	}
	var err error
	if a.doc[0], err = ParseJSON(a.body[0]); err != nil {
		a.problems = append(a.problems, fmt.Sprintf("oracle: %v: %s", err, truncateBytes(a.body[0])))
		return a
	}
	if req.guard != nil {
		if err := req.guard(a.doc[0]); err != nil {
			a.problems = append(a.problems, fmt.Sprintf("oracle: %v: %s", err, truncateBytes(a.body[0])))
			return a
		}
	}
	if a.doc[1], err = ParseJSON(a.body[1]); err != nil {
		a.problems = append(a.problems, fmt.Sprintf("candidate: %v: %s", err, truncateBytes(a.body[1])))
		return a
	}
	// the clocks the masks hide: each side's own, read while it answered
	for i, side := range p.Each() {
		for _, err := range []error{v2NowInFlight(a.doc[i], clock[i]), v2TiersInFlight(a.doc[i], a.flight[i])} {
			if err != nil {
				a.problems = append(a.problems, fmt.Sprintf("%s: %v", side.Role, err))
			}
		}
	}
	if o, c := clockShapes(a.doc[0]), clockShapes(a.doc[1]); strings.Join(o, " ") != strings.Join(c, " ") {
		a.problems = append(a.problems, fmt.Sprintf("clocks: oracle %v, candidate %v", o, c))
	}
	for _, d := range Compare(ApplyMasks(a.doc[0], fam.masks), ApplyMasks(a.doc[1], fam.masks), fam.unordered...) {
		a.problems = append(a.problems, d.String())
	}
	if l := v2Layouts(a.body, fam.layoutByCount()); l != "" {
		a.problems = append(a.problems, l)
	}
	if e := v2Escapes(a.body, a.doc, fam); e != "" {
		a.problems = append(a.problems, e)
	}
	return a
}

// jsonStringRe finds every string literal of a JSON text, keys and values, in the text's order.
var jsonStringRe = regexp.MustCompile(`"(?:[^"\\]|\\.)*"`)

// jsonEscapeRe finds the escapes of a string literal as written: `\uXXXX`, or a backslash and the byte after it.
var jsonEscapeRe = regexp.MustCompile(`\\u[0-9a-fA-F]{4}|\\.`)

// jsonEscapes are the escapes of each string literal of body that has one, keys and values in the text's order, as the
// agent wrote them, but for the literals of the values that masks hide (v is body parsed). The comparison of two
// answers reads decoded strings, to which `\/` is `/` and `\u000a` is `\n`: C writes a quote and a backslash
// escaped, a control byte as `\n`, `\r`, `\t`, `\b`, `\f` or `\u00XX`, and every other byte as it is
// (buffer_json_strcat, libnetdata/buffer/buffer.h:301-374).
func jsonEscapes(body []byte, v Value, masks []Mask) ([]string, error) {
	raws := jsonStringRe.FindAll(body, -1)
	compiled := make([][]string, len(masks))
	for i, m := range masks {
		compiled[i] = strings.Split(m.Pattern, ".")
	}
	var out []string
	next := 0
	take := func(keep bool) error {
		if next >= len(raws) {
			return fmt.Errorf("harness: the answer parses to more strings than its %d literals", len(raws))
		}
		if e := jsonEscapeRe.FindAll(raws[next], -1); keep && len(e) > 0 {
			out = append(out, string(bytes.Join(e, nil)))
		}
		next++
		return nil
	}
	var walk func(v Value, path []string, keep bool) error
	walk = func(v Value, path []string, keep bool) error {
		for _, pattern := range compiled {
			if keep && matchPath(pattern, path) {
				keep = false
			}
		}
		switch v.Kind {
		case KindString:
			return take(keep)
		case KindObject:
			for _, m := range v.Members {
				if err := take(keep); err != nil {
					return err
				}
				if err := walk(m.Value, append(slices.Clone(path), m.Key), keep); err != nil {
					return err
				}
			}
		case KindArray:
			for i, item := range v.Items {
				if err := walk(item, append(slices.Clone(path), "["+strconv.Itoa(i)+"]"), keep); err != nil {
					return err
				}
			}
		}
		return nil
	}
	if err := walk(v, nil, true); err != nil {
		return nil, err
	}
	if next != len(raws) {
		return nil, fmt.Errorf("harness: the answer parses to %d strings, it has %d literals", next, len(raws))
	}
	return out, nil
}

// v2Escapes compares the escapes the two bodies' string literals were written with (jsonEscapes), in the text's order;
// as sorted lists where the family leaves some members' order to each side. Empty when they agree.
func v2Escapes(body [2][]byte, doc [2]Value, fam v2Family) string {
	var got [2][]string
	for i := range body {
		e, err := jsonEscapes(body[i], doc[i], fam.masks)
		if err != nil {
			return err.Error()
		}
		if len(fam.unordered) > 0 {
			slices.Sort(e)
		}
		got[i] = e
	}
	if !slices.Equal(got[0], got[1]) {
		return fmt.Sprintf("the strings' escapes differ: oracle %q, candidate %q", got[0], got[1])
	}
	return ""
}

// rawLength holds when a raw answer's Content-Length, where its head has one, is the length of its body.
func rawLength(raw []byte) error {
	head, body, _ := bytes.Cut(raw, []byte("\r\n\r\n"))
	if n := contentLengthOf(head); n >= 0 && n != len(body) {
		return fmt.Errorf("Content-Length is %d, the body has %d bytes", n, len(body))
	}
	return nil
}

// headDate is a head line's HTTP date (RFC 7231), as C writes Date and Expires (web_client.c:953-955).
func headDate(value string) (int64, bool) {
	t, err := time.Parse(http.TimeFormat, value)
	return t.Unix(), err == nil
}

// maskHead hides what differs between two runs in one answer's head, given the seconds [flight[0], flight[1]] its
// request was in flight, and nothing wider:
//   - `Date` reads `now` when it is a second of the flight (web_client.c:939-940; another date stays: a file's
//     modification time, :623-627);
//   - `Expires` then reads its distance from that Date (`Date+0` for an answer that is not cacheable, `Date+86400`
//     for one that is, web_client.c:943-945, buffer.h:78; `Date+1` for an nRPC answer, json-c-parser-inline.c:49-50);
//     beside a Date that stays it reads `now+86400` when it is a day after a second of the flight (a static file's,
//     web_client.c:628);
//   - `X-Transaction-ID` is masked when it has the shape of C's random id of a request that names none, a UUID's 32
//     hex digits in lower case (web_client.c:1470-1471). The shape is all that is read: an id a request named in that
//     shape would be masked as well, and no request of this function's users names one (the function checks judge
//     the echo, fnhttp_test.go).
//
// Anything else is left as the agent wrote it, for the comparison: a date in another format, an expiry at another
// distance than the oracle's, a date outside the flight.
func maskHead(head []byte, flight [2]int64) []byte {
	lines := strings.Split(string(head), "\r\n")
	date, dated := int64(0), false
	for i, l := range lines {
		if v, ok := strings.CutPrefix(l, "Date: "); ok {
			if d, ok := headDate(v); ok && d >= flight[0] && d <= flight[1] {
				date, dated = d, true
				lines[i] = "Date: now"
			}
		}
	}
	for i, l := range lines {
		if v, ok := strings.CutPrefix(l, "Expires: "); ok {
			e, ok := headDate(v)
			switch {
			case ok && dated:
				lines[i] = fmt.Sprintf("Expires: Date%+d", e-date)
			case ok && e >= flight[0]+86400 && e <= flight[1]+86400:
				lines[i] = "Expires: now+86400"
			}
		} else if v, ok := strings.CutPrefix(l, "X-Transaction-ID: "); ok && rawTransactionIDRe.MatchString(v) {
			lines[i] = "X-Transaction-ID: <masked>"
		}
	}
	return []byte(strings.Join(lines, "\r\n"))
}

// rawTransactionIDRe is a transaction id C made: a random UUID, compact and in lower case.
var rawTransactionIDRe = regexp.MustCompile(`^[0-9a-f]{32}$`)

// maskAnswer is a raw answer with its head masked for the seconds its request was in flight (maskHead); the body is
// left as it is.
func maskAnswer(raw []byte, flight [2]int64) []byte {
	head, body, whole := bytes.Cut(raw, []byte("\r\n\r\n"))
	out := maskHead(head, flight)
	if whole {
		out = append(append(out, "\r\n\r\n"...), body...)
	}
	return out
}

// layoutByCount tells whether fam's answers can only be held to the same count of each layout byte (v2Layouts): some
// members' order is each side's, and what is unordered is more than flat maps.
func (fam v2Family) layoutByCount() bool { return len(fam.unordered) > 0 && !fam.flat }

// v2Layouts compares the two bodies' layouts (minified or pretty): the same bytes once every key and scalar is
// masked; where some objects' member order is no contract, the same count of each remaining byte. Empty when they
// agree.
func v2Layouts(body [2][]byte, unordered bool) string {
	layout := func(b []byte) string { return jsonScalarRe.ReplaceAllString(string(b), "V") }
	o, c := layout(body[0]), layout(body[1])
	if unordered {
		count := func(s string) string {
			n := map[rune]int{}
			for _, r := range s {
				n[r]++
			}
			return fmt.Sprint(n)
		}
		if count(o) != count(c) {
			return fmt.Sprintf("layouts differ: oracle %s, candidate %s", count(o), count(c))
		}
		return ""
	}
	if o != c {
		return "layouts differ\n" + firstDifference([]byte(o), []byte(c))
	}
	return ""
}

// compareV2 asks both agents req and compares the answers as fam says: the oracle's status and guard first; the
// heads, each side's clock and expiry read against its own flight (maskHead) and its length against its own body; a
// JSON body as ordered values after the masks (its build info slot by slot and its capabilities by name; its clocks'
// shapes, and each side's clocks against its own flight; its layout; its strings' escapes), any other body byte for
// byte. It reports each difference.
func compareV2(t *testing.T, p *Pair, req v2Req, fam v2Family) {
	t.Helper()
	deadline := time.Now().Add(fam.settle)
	a := v2Round(p, req, fam)
	for len(a.problems) > 0 && time.Now().Before(deadline) {
		time.Sleep(time.Second)
		a = v2Round(p, req, fam)
	}
	for _, problem := range a.problems {
		t.Errorf("%s: %s", req.name, problem)
	}
	if len(a.problems) > 0 || req.status != "200" {
		return
	}
	if oa, ok := agentMember(a.doc[0], "application"); ok {
		ca, _ := agentMember(a.doc[1], "application")
		keys, oValues := buildinfoSlotsOf(oa)
		cKeys, cValues := buildinfoSlotsOf(ca)
		compareBuildinfoSlots(t, req.name+" application", keys, cKeys, oValues, cValues)
	}
	if oc, ok := agentMember(a.doc[0], "capabilities"); ok {
		cc, _ := agentMember(a.doc[1], "capabilities")
		compareCapabilities(t, req.name, oc, cc)
	}
	if fam.check != nil {
		fam.check(t, req.name, a.doc[0], a.doc[1])
	}
}

// sameBinary tells whether the oracle is its own candidate (a check's validation C against C): the tripwires that
// list the Rust agent's known differences (capabilityDiffs, buildinfoDiffs) then do not apply.
func sameBinary(t *testing.T) bool {
	t.Helper()
	bins := binaries(t)
	var real [2]string
	for i, b := range bins {
		r, err := filepath.EvalSymlinks(b)
		if err != nil {
			t.Fatalf("parity: %s: %v", b, err)
		}
		real[i] = r
	}
	return real[0] == real[1]
}

// dashOptions are the options of a dashboard check's pair before its dashboard list: extra's, with memory-mode
// children and one tier unless extra says otherwise, and no pulse charts (localhost adds no context). The parent's own
// database stays dbengine.
func dashOptions(extra daemon.Options) daemon.Options {
	opts := extra
	if opts.StreamMemoryMode == "" {
		opts.StreamMemoryMode = "ram"
	}
	if opts.StorageTiers == 0 {
		opts.StorageTiers = 1
	}
	opts.PulseOff = true
	return opts
}

// dashPairOptions are dashPair's options: dashOptions, and the dashboard list left to localhost, which the
// launcher's probe passes; extra's WebExtra lines follow the dashboard's.
func dashPairOptions(extra daemon.Options) daemon.Options {
	opts := dashOptions(extra)
	opts.WebExtra = "    allow dashboard from = localhost\n" + extra.WebExtra
	return opts
}

// dashPair starts the pair of a dashboard check (dashPairOptions).
func dashPair(t *testing.T, extra daemon.Options) *Pair {
	t.Helper()
	return StartPair(t, dashPairOptions(extra), parentIdentity)
}

// dashConnect connects the fixture child (`childHost`) live to each side (dashConnectAs).
func dashConnect(t *testing.T, p *Pair) [2]*stream.Conn {
	t.Helper()
	return dashConnectAs(t, p, childHost)
}

// dashConnectAs connects a child as `host` live to each side in turn, sending nothing yet, and hands back the two
// connections (0 the oracle's); the test's end closes them.
func dashConnectAs(t *testing.T, p *Pair, host stream.HostInfo) [2]*stream.Conn {
	t.Helper()
	conns, _ := dashLinkAs(t, p, host)
	return conns
}

// dashLinkAs is dashConnectAs, handing back with each connection the seconds its opening took: from before the child
// dialled to after the parent's answer was read. A parent attaches the child's receiver between the two (C answers
// once it has: stream-receiver-connection.c:224, :280-292).
func dashLinkAs(t *testing.T, p *Pair, host stream.HostInfo) (conns [2]*stream.Conn, opened [2][2]int64) {
	t.Helper()
	for i, side := range p.Each() {
		from := time.Now().Unix()
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, host, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		conns[i], opened[i] = conn, [2]int64{from, time.Now().Unix()}
	}
	return conns, opened
}

// dashChild connects the fixture child to each side and sends it the data fixture ending at base+60 (dashChildAs:
// `childHost` with qCharts, as `streamDataFixture`).
func dashChild(t *testing.T, p *Pair, base int64) [2]*stream.Conn {
	t.Helper()
	return dashChildAs(t, p, base, childHost, qCharts)
}

// dashChildAs connects a child as `host` to each side (dashConnectAs) and sends it the data fixture of each of charts
// in turn, each ending at base+60 (streamChartsFixture), the same base on both sides, then sleeps 2.5 s (a fixed
// pause, not a wait on either agent: the rows' guards and settle time judge what arrived). It hands back the two
// connections (0 the oracle's), for a check that disconnects the child itself.
func dashChildAs(t *testing.T, p *Pair, base int64, host stream.HostInfo, charts ...dataCharts) [2]*stream.Conn {
	t.Helper()
	conns, _ := dashChildLinkAs(t, p, base, host, charts...)
	return conns
}

// dashChildLinkAs is dashChildAs, handing back dashLinkAs' seconds too.
func dashChildLinkAs(t *testing.T, p *Pair, base int64, host stream.HostInfo, charts ...dataCharts) (
	[2]*stream.Conn, [2][2]int64) {
	t.Helper()
	conns, opened := dashLinkAs(t, p, host)
	for _, conn := range conns {
		for _, cs := range charts {
			streamChartsFixture(t, conn, base, cs)
		}
	}
	time.Sleep(2500 * time.Millisecond)
	return conns, opened
}

// dashBase is a fixture base whose 60 s end a minute or more before now.
func dashBase() int64 { return time.Now().Unix()/60*60 - 120 }

// accessConf is one configuration of a family's `access` subtest: the lines its pair's netdata.conf gets after
// `[web]`'s own (WebExtra: a line may open another section, as regDenied's `[registry]`, which C merges with the
// launcher's), and the client's address.
type accessConf struct{ name, web, from string }

var (
	// accessACL leaves the client (127.0.0.2) out of the dashboard list.
	accessACL = accessConf{"acl", "    allow dashboard from = localhost\n", "127.0.0.2"}
	// accessBearer asks anonymous clients for a bearer token.
	accessBearer = accessConf{"bearer", "    bearer token protection = yes\n", ""}
)

// accessRow is one request of an `access` subtest under one configuration, with the start and the end the oracle's
// answer must have, and a part it must hold besides (empty: none).
type accessRow struct {
	conf, name string
	request    []byte
	want       [2]string
	holds      string
}

// accessGet is a GET of target with the header lines.
func accessGet(target string, headers ...string) []byte {
	return rawRequest("GET", target, append([]string{"Host: localhost"}, headers...), nil)
}

// accessRows starts one pair per configuration (each without a fixture) and compares each of its rows raw through
// aclRow: the routes' access lists, bearer protection and their answers before any data.
func accessRows(t *testing.T, confs []accessConf, rows []accessRow) {
	t.Helper()
	if stray := accessStray(confs, rows); stray != "" {
		t.Fatalf("harness: %s names no configuration of this check: the row would never be asked", stray)
	}
	for _, conf := range confs {
		t.Run(conf.name, func(t *testing.T) {
			p := StartPair(t, dashOptions(daemon.Options{WebExtra: conf.web}), parentIdentity)
			n := 0
			for _, row := range rows {
				if row.conf != conf.name {
					continue
				}
				n++
				if row.want[0] == "" || row.want[1] == "" {
					t.Fatalf("harness: %s/%s has no guard", conf.name, row.name)
				}
				t.Run(row.name, func(t *testing.T) { aclRow(t, p, conf.from, row.request, row.want, row.holds) })
			}
			if n == 0 {
				t.Fatalf("harness: no row for %s", conf.name)
			}
		})
	}
}

// accessStray is the first of rows (`<configuration>/<name>`) whose configuration is none of confs; empty when every
// row has one.
func accessStray(confs []accessConf, rows []accessRow) string {
	for _, row := range rows {
		if !slices.ContainsFunc(confs, func(c accessConf) bool { return c.name == row.conf }) {
			return row.conf + "/" + row.name
		}
	}
	return ""
}

// accessDenied is the answer of a route whose access list leaves the client out.
var accessDenied = [2]string{"HTTP/1.1 451 Unavailable For Legal Reasons\r\n",
	"\r\n\r\nYou need to be authorized to access this resource"}

// accessAnonymous is the answer of a route that wants anonymous data to an anonymous client under bearer protection:
// 412 with the text of a refusal (web_api.c:86-90; web_client.c:57-70; http-access.h:85).
var accessAnonymous = [2]string{"HTTP/1.1 412 Precondition Failed\r\n", accessDenied[1]}

// accessRoutes are the `acl` and `bearer` rows of GET routes whose ACL is one of the dashboard's and whose access is
// anonymous data: the `acl` client is refused for the ACL (451), the anonymous `bearer` client for the token it lacks
// (412). A row is named by the route's path after `/api/` (`v3-alerts`).
func accessRoutes(routes ...string) []accessRow {
	var rows []accessRow
	for _, route := range routes {
		path, _, _ := strings.Cut(strings.TrimPrefix(route, "/api/"), "?")
		name := strings.ReplaceAll(path, "/", "-")
		rows = append(rows,
			accessRow{conf: accessACL.name, name: name, request: accessGet(route), want: accessDenied},
			accessRow{conf: accessBearer.name, name: name, request: accessGet(route), want: accessAnonymous})
	}
	return rows
}

// exactReq is one request whose answers are compared byte for byte after maskAnswer (each side's clock and expiry
// read against its own flight, the transaction id), with what the oracle's answer must be first: its start and its
// end (want), the parts it must hold besides (holds: header lines or body text), and an optional judge of its body
// parsed as JSON (guard).
type exactReq struct {
	name    string
	method  string // empty: GET
	target  string
	body    []byte // nil: no body and no Content-Length
	headers []string
	want    [2]string
	holds   []string
	guard   func(Value) error
	// mask, when set, hides more of each answer after maskAnswer, given the seconds its request was in flight (a
	// dyncfg tree's agent clock, exactNow)
	mask func(answer []byte, flight [2]int64) []byte
}

// exactNow is an exactReq mask: the answering agent's clock written NOW where it is a second of the flight
// (maskAgentNow), in an answer no v2 family compares.
func exactNow(answer []byte, flight [2]int64) []byte {
	return maskAgentNow(answer, flight[0], flight[1])
}

// compareExact sends r to both agents (exactAnswers), then judges and compares the answers (exactJudge).
func compareExact(t *testing.T, p *Pair, r exactReq) {
	t.Helper()
	if r.want[0] == "" || r.want[1] == "" {
		t.Fatalf("harness: %s has no guard", r.name)
	}
	got, err := exactAnswers(p, r)
	if err != nil {
		t.Fatal(err)
	}
	exactJudge(t, got, r.want, r.holds, r.guard)
}

// exactAnswers sends r to both agents (Host: localhost, Connection: close, then r.headers) and hands back each side's
// answer as exactJudge compares it: after maskAnswer for the seconds its own request was in flight, then the row's
// mask.
func exactAnswers(p *Pair, r exactReq) ([2][]byte, error) {
	var got [2][]byte
	for i, side := range p.Each() {
		from := time.Now().Unix()
		b, err := v2Exchange(side.Daemon.Addr, v2Req{method: r.method, target: r.target, body: r.body,
			headers: r.headers})
		if err != nil {
			return got, fmt.Errorf("%s: %v", side.Role, err)
		}
		flight := [2]int64{from, time.Now().Unix()}
		got[i] = maskAnswer(b, flight)
		if r.mask != nil {
			got[i] = r.mask(got[i], flight)
		}
	}
	return got, nil
}

// compareExacts runs each request as its own subtest, in order.
func compareExacts(t *testing.T, p *Pair, reqs []exactReq) {
	t.Helper()
	for _, r := range reqs {
		t.Run(r.name, func(t *testing.T) { compareExact(t, p, r) })
	}
}

// exactJudge judges the oracle's masked answer first (it must start with want[0], end with want[1], hold each of
// holds and, with a guard, have a JSON body the guard accepts), then compares the two answers byte for byte.
func exactJudge(t *testing.T, got [2][]byte, want [2]string, holds []string, guard func(Value) error) {
	t.Helper()
	o := string(got[0])
	if !strings.HasPrefix(o, want[0]) || !strings.HasSuffix(o, want[1]) {
		t.Fatalf("oracle: answered %q, want %q at its start and %q at its end", truncateBytes(got[0]), want[0],
			want[1])
	}
	for _, part := range holds {
		if !strings.Contains(o, part) {
			t.Fatalf("oracle: answered %q, which does not hold %q", truncateBytes(got[0]), part)
		}
	}
	if guard != nil {
		v, err := ParseJSON(httpBody(got[0]))
		if err == nil {
			err = guard(v)
		}
		if err != nil {
			t.Fatalf("oracle: %v: %q", err, truncateBytes(got[0]))
		}
	}
	if !bytes.Equal(got[0], got[1]) {
		t.Errorf("answers differ\n%s\noracle:    %q\ncandidate: %q", firstDifference(got[0], got[1]),
			truncateBytes(got[0]), truncateBytes(got[1]))
	}
}

// dashMember is the member of v at the path of keys, one object key per step (a key may hold dots: a context's id).
func dashMember(v Value, keys ...string) (Value, error) {
	for i, k := range keys {
		found := false
		for _, m := range v.Members {
			if m.Key == k {
				v, found = m.Value, true
				break
			}
		}
		if !found {
			return Value{}, fmt.Errorf("no %s", strings.Join(keys[:i+1], "."))
		}
	}
	return v, nil
}

// dashSettle is how long a dashboard check asks both agents again while their answers differ: the contexts
// dictionaries count update events the contexts worker times (`contexts_hard_hash`, query_scope.c:64,
// dictionary-internals.h:257-262) and a host reports `initializing` or `replicating` until its data arrives
// (rrdhost-status.c:124-130, :179-182).
const dashSettle = 10 * time.Second

// dashFact is one fact a guard asks of the oracle's answer: nil when it holds.
type dashFact func(v Value) error

// dashIndex is the item index of a path step `[i]`.
func dashIndex(step string) (int, bool) {
	inner, ok := strings.CutPrefix(step, "[")
	if !ok {
		return 0, false
	}
	n, err := strconv.Atoi(strings.TrimSuffix(inner, "]"))
	return n, err == nil && strings.HasSuffix(inner, "]")
}

// dashAt is the value at path: each step an object's key (dashMember: a key may hold dots) or `[i]`, an array's
// item i.
func dashAt(v Value, path ...string) (Value, error) {
	for i, step := range path {
		if n, ok := dashIndex(step); ok {
			if v.Kind != KindArray || n < 0 || n >= len(v.Items) {
				return Value{}, fmt.Errorf("no %s", strings.Join(path[:i+1], "."))
			}
			v = v.Items[n]
			continue
		}
		next, err := dashMember(v, step)
		if err != nil {
			return Value{}, fmt.Errorf("no %s", strings.Join(path[:i+1], "."))
		}
		v = next
	}
	return v, nil
}

// dashPath names a path in a guard's error.
func dashPath(path []string) string {
	if len(path) == 0 {
		return "the answer"
	}
	return strings.Join(path, ".")
}

// dashIs holds when the value at path renders compactly (Value.String) as want.
func dashIs(want string, path ...string) dashFact {
	return func(v Value) error {
		got, err := dashAt(v, path...)
		if err != nil {
			return fmt.Errorf("%v, want %s", err, want)
		}
		if got.String() != want {
			return fmt.Errorf("%s is %s, want %s", dashPath(path), got, want)
		}
		return nil
	}
}

// dashAbsent holds when path names nothing.
func dashAbsent(path ...string) dashFact {
	return func(v Value) error {
		if got, err := dashAt(v, path...); err == nil {
			return fmt.Errorf("%s is %s, want nothing there", dashPath(path), got)
		}
		return nil
	}
}

// dashAbove holds when the value at path is a whole number above n.
func dashAbove(n int64, path ...string) dashFact {
	return func(v Value) error {
		got, err := dashAt(v, path...)
		if err != nil {
			return fmt.Errorf("%v, want a number above %d", err, n)
		}
		if x, perr := strconv.ParseInt(got.Text, 10, 64); got.Kind != KindNumber || perr != nil || x <= n {
			return fmt.Errorf("%s is %s, want a number above %d", dashPath(path), got, n)
		}
		return nil
	}
}

// dashKeys holds when the object at path has exactly these members (space-separated), in this order.
func dashKeys(keys string, path ...string) dashFact {
	return func(v Value) error {
		got, err := dashAt(v, path...)
		if err != nil {
			return fmt.Errorf("%v, want members %s", err, keys)
		}
		if have := strings.Join(memberKeys(got), " "); got.Kind != KindObject || have != keys {
			return fmt.Errorf("%s has members [%s], want [%s]", dashPath(path), have, keys)
		}
		return nil
	}
}

// dashSet holds when the value at path is want's JSON but for the order of every object's members and every array's
// items in it: an aggregated label list, which C walks in the order of its strings' heap addresses (keys and values,
// database/rrdlabels-aggregated.c:130-173).
func dashSet(want string, path ...string) dashFact {
	w, err := ParseJSON([]byte(want))
	return func(v Value) error {
		if err != nil {
			return fmt.Errorf("harness: %s: %v", want, err)
		}
		got, err := dashAt(v, path...)
		if err != nil {
			return fmt.Errorf("%v, want %s as a set", err, want)
		}
		if d := Compare(w, got, "**", "**[]"); len(d) > 0 {
			return fmt.Errorf("%s is %s, want %s as a set (%s)", dashPath(path), got, want, d[0])
		}
		return nil
	}
}

// dashMembers are the facts that each member of the object at path named in pairs (name, want, name, want, ...)
// renders as its want. A name without a want is a mistake of the caller's: the fact it makes fails every answer.
func dashMembers(path []string, pairs ...string) []dashFact {
	var facts []dashFact
	if len(pairs)%2 != 0 {
		last := pairs[len(pairs)-1]
		facts = append(facts, func(Value) error {
			return fmt.Errorf("harness: dashMembers of %s got %q without a want", dashPath(path), last)
		})
	}
	for i := 0; i+1 < len(pairs); i += 2 {
		facts = append(facts, dashIs(pairs[i+1], append(slices.Clone(path), pairs[i])...))
	}
	return facts
}

// dashText is a guard of an answer that is not a 200: its body exactly.
func dashText(want string) func(Value) error {
	return func(v Value) error {
		if v.Text != want {
			return fmt.Errorf("body is not %q", want)
		}
		return nil
	}
}

// dashGuard is a v2Req guard of facts, judged in order: the first that does not hold fails the row.
func dashGuard(facts ...[]dashFact) func(Value) error {
	all := slices.Concat(facts...)
	return func(v Value) error {
		for _, f := range all {
			if err := f(v); err != nil {
				return err
			}
		}
		return nil
	}
}

// dashNowRe is the answering agent's clock in a v2 answer, the first `now` of the body (api_v2_contexts_agents.c:25):
// in these answers the agent's, printed after the nodes.
var dashNowRe = regexp.MustCompile(`"now":(\d+)`)
