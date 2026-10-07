// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
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
// (`v2Req`), guards the oracle's answer with a fact of its fixture, and compares the answers as the request's family
// says (`v2Family`): the envelope of `/api/v2/info` and its kin, with the family's masks, unordered paths and settle
// time.

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
	// settle, when non-zero, asks both agents again each second until their answers agree or the time passes (the
	// guard is judged on each of the oracle's answers).
	settle time.Duration
	// now, when set, names the members written "NOW" where they hold a second the request was in flight on that side
	// (maskNowKeys: C prints its walk's clock there for what is collected); the first agent's `now` must then be
	// such a second too. Only for requests without `after` or `before`: with a window, C's walk takes the second
	// before its clock (libnetdata.c:549-555, database/contexts/api_v2_contexts.c:1368-1374).
	now []string
	// render, when set, normalises each side's body (0 the oracle, 1 the candidate) after `now`, before it is parsed.
	render func(i int, body []byte) []byte
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

// normalise is what fam does to side i's body before it is parsed: `now`'s members written NOW for the seconds
// [from, to] the request was in flight, then the render.
func (fam v2Family) normalise(i int, flight [2]int64, body []byte) []byte {
	if len(fam.now) > 0 {
		body = maskNowKeys(body, flight[0], flight[1], fam.now...)
	}
	if fam.render != nil {
		body = fam.render(i, body)
	}
	return body
}

// v2NowInFlight holds when the first agent's `now`, where the answer has one as a number, is a second in [from, to].
func v2NowInFlight(v Value, flight [2]int64) error {
	now, ok := agentMember(v, "now")
	if !ok || now.Kind != KindNumber {
		return nil
	}
	if n, err := strconv.ParseInt(now.Text, 10, 64); err != nil || n < flight[0] || n > flight[1] {
		return fmt.Errorf("agents[0].now is %s, not a second of the request's flight [%d, %d]", now.Text, flight[0],
			flight[1])
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
	var head [2][]byte
	for i := range a.raw {
		h, _, _ := bytes.Cut(a.raw[i], []byte("\r\n\r\n"))
		head[i] = contentLengthRe.ReplaceAll(maskRaw(h), []byte("Content-Length: <masked>"))
	}
	if !bytes.Equal(head[0], head[1]) {
		a.problems = append(a.problems, fmt.Sprintf("headers differ\noracle:    %q\ncandidate: %q", head[0], head[1]))
	}
	// each side's run directory, in the build info's directories and the rules' sources
	for i, side := range p.Each() {
		a.body[i] = fam.normalise(i, a.flight[i],
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
	if len(fam.now) > 0 {
		for i, side := range p.Each() {
			if err := v2NowInFlight(a.doc[i], a.flight[i]); err != nil {
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
	if l := v2Layouts(a.body, len(fam.unordered) > 0); l != "" {
		a.problems = append(a.problems, l)
	}
	return a
}

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

// compareV2 asks both agents req and compares the answers as fam says: the oracle's status and guard first, the
// heads but for the clock and the length, a JSON body as ordered values after the masks (its build info slot by slot
// and its capabilities by name, its clocks' shapes, its layout), any other body byte for byte. It reports each
// difference and tells whether the answers agreed (the build info's and the capabilities' checks, and the family's
// own, report theirs without changing the result).
func compareV2(t *testing.T, p *Pair, req v2Req, fam v2Family) bool {
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
		return len(a.problems) == 0
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
	return true
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

// dashConnect connects the fixture child (`childHost`) live to each side in turn, sending nothing yet, and hands back
// the two connections (0 the oracle's); the test's end closes them.
func dashConnect(t *testing.T, p *Pair) [2]*stream.Conn {
	t.Helper()
	var conns [2]*stream.Conn
	for i, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		conns[i] = conn
	}
	return conns
}

// dashChild connects the fixture child to each side (dashConnect) and sends it the data fixture ending at base+60
// (`streamDataFixture`), the same base on both sides, then waits for both to take it in. It hands back the two
// connections (0 the oracle's), for a check that disconnects the child itself.
func dashChild(t *testing.T, p *Pair, base int64) [2]*stream.Conn {
	t.Helper()
	conns := dashConnect(t, p)
	for _, conn := range conns {
		streamDataFixture(t, conn, base)
	}
	time.Sleep(2500 * time.Millisecond)
	return conns
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

// exactReq is one request whose answers are compared byte for byte after maskRaw (the clock and the transaction id),
// with what the oracle's answer must be first: its start and its end (want), the parts it must hold besides (holds:
// header lines or body text), and an optional judge of its body parsed as JSON (guard).
type exactReq struct {
	name    string
	method  string // empty: GET
	target  string
	body    []byte // nil: no body and no Content-Length
	headers []string
	from    string // the client's address (empty: any)
	want    [2]string
	holds   []string
	guard   func(Value) error
	// mask, when set, hides more of each answer after maskRaw (a dyncfg tree's agent clock)
	mask func([]byte) []byte
}

// compareExact sends r to both agents (Host: localhost, Connection: close, then r.headers), then judges and compares
// the answers (exactJudge).
func compareExact(t *testing.T, p *Pair, r exactReq) {
	t.Helper()
	if r.want[0] == "" || r.want[1] == "" {
		t.Fatalf("harness: %s has no guard", r.name)
	}
	var got [2][]byte
	for i, side := range p.Each() {
		b, err := v2Exchange(side.Daemon.Addr, v2Req{method: r.method, target: r.target, body: r.body,
			headers: r.headers, from: r.from})
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		got[i] = maskRaw(b)
		if r.mask != nil {
			got[i] = r.mask(got[i])
		}
	}
	exactJudge(t, got, r.want, r.holds, r.guard)
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

// dashMembers are the facts that each member of the object at path named in pairs (name, want, name, want, ...)
// renders as its want.
func dashMembers(path []string, pairs ...string) []dashFact {
	var facts []dashFact
	for i := 0; i+1 < len(pairs); i += 2 {
		facts = append(facts, dashIs(pairs[i+1], append(slices.Clone(path), pairs[i])...))
	}
	return facts
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
