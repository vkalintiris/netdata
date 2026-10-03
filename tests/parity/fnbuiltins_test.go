// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The built-in Functions (M8 commit 9, D176; plan evidence/2026-10-03-plan-m8-commit9.md §5): C registers five methods
// on localhost from the main thread, right after DynCfg's `config` (database/rrd.c:185-190, web/api/functions/
// functions.c:5-67). `fn.builtins` calls them over `/api/v1|v3/function` on a standalone pair (runFnHTTPCases);
// `fn.child`'s `bearer` case calls bearer_get_token through a scripted parent; the lists and re-lists that carry them
// are compared by `fn.http-catalog`, `fn.stream` and `stream.sender-capture`.

// fnBuiltin is one of C's built-ins as nrpc_method_register_builtin registers it (nrpc-builtin.c:29-48): sync, from the
// daemon, a timeout of 10 s, version 0.
type fnBuiltin struct {
	name, help, tags, access string
	priority                 int
}

// fnBuiltins are C's five in registration order (functions.c:8-66; the help texts are their headers' macros).
// bearer_get_token's `hidden` tag makes it restricted (nrpc-registry.c:486-488): re-listed to a parent, never listed
// to a user, refused on the HTTP API.
var fnBuiltins = []fnBuiltin{
	{"netdata-streaming", "Shows real-time streaming connections and replication status between parent and child nodes, " +
		"including connection health, data flow metrics, and ML status.", "top", "0x13", 101},
	{"topology:streaming", "Shows streaming topology relations across Netdata agents, including directional parent/child " +
		"links and transport metadata.", "top", "0x13", 101},
	{"netdata-api-calls", "Monitors active and recent Netdata API requests with transaction details, duration, and " +
		"response sizes.", "top", "0x13", 101},
	{"bearer_get_token", "Get a bearer token for authenticated direct access to the agent", "hidden", "0x13", 103},
	{"netdata-metrics-cardinality", "Displays metrics cardinality statistics showing distribution of instances and " +
		"time-series across contexts and nodes. To change grouping, append parameter to function name: " +
		"'netdata-metrics-cardinality' (default, group by context) or 'netdata-metrics-cardinality group:by-node' " +
		"(group by node).", "top", "0x8", 101},
}

// fnBuiltinNames are the four a user's listing shows, in C's order.
var fnBuiltinNames = []string{"netdata-streaming", "topology:streaming", "netdata-api-calls", "netdata-metrics-cardinality"}

// fnBuiltinRelist are the five's lines in a re-list (nrpc-catalog.c:97-110, `FUNCTION GLOBAL "%s" %d "%s" "%s" 0x%x %d
// %u`), in registration order.
func fnBuiltinRelist() []string {
	var out []string
	for _, b := range fnBuiltins {
		out = append(out, fmt.Sprintf(`FUNCTION GLOBAL "%s" 10 "%s" "%s" %s %d 0`, b.name, b.help, b.tags, b.access, b.priority))
	}
	return out
}

// fnBuiltinsLead tells whether a re-list (capture.relists' joined lines) holds C's five right after its deletes.
func fnBuiltinsLead(relist string) bool {
	lines := strings.Split(relist, "\n")
	i := 0
	for i < len(lines) && strings.HasPrefix(lines[i], "FUNCTION_DEL GLOBAL ") {
		i++
	}
	want := fnBuiltinRelist()
	return len(lines)-i >= len(want) && slices.Equal(lines[i:i+len(want)], want)
}

// fnBuiltinsTx is a transaction an fn.builtins case sends: fnTx's scheme, so fnMaskIDs keeps it.
func fnBuiltinsTx(n int) string { return fnTx(0x7000 + n) }

// fnBuiltinsExpiresRe is a reply's expiry (`"expires":<seconds>`, the tables' last member).
var fnBuiltinsExpiresRe = regexp.MustCompile(`"expires":(\d{10})\b`)

// fnBuiltinsExpires renders each expiry in body as its distance from date: the handlers write now + 1 (the api-calls
// and cardinality tables) or now + 10 (topology:streaming) and the head's Date is taken after them, so a second
// boundary on the way makes the distance one less: NOW+1 for 0 or 1, NOW+10 for 9 or 10, any other as it is.
func fnBuiltinsExpires(body []byte, date time.Time) []byte {
	if date.IsZero() {
		return body
	}
	return fnBuiltinsExpiresRe.ReplaceAllFunc(body, func(m []byte) []byte {
		e, err := strconv.ParseInt(string(fnBuiltinsExpiresRe.FindSubmatch(m)[1]), 10, 64)
		if err != nil {
			return m
		}
		switch d := e - date.Unix(); d {
		case 0, 1:
			return []byte(`"expires":NOW+1`)
		case 9, 10:
			return []byte(`"expires":NOW+10`)
		default:
			return []byte(fmt.Sprintf(`"expires":NOW%+d`, d))
		}
	})
}

// fnHTTPDate is a response's Date header (zero when it has none).
func fnHTTPDate(resp []byte) time.Time {
	head, _, _ := bytes.Cut(resp, []byte("\r\n\r\n"))
	for _, l := range strings.Split(string(head), "\r\n") {
		if v, ok := strings.CutPrefix(l, "Date: "); ok {
			d, _ := time.Parse(http.TimeFormat, v)
			return d
		}
	}
	return time.Time{}
}

// fnBuiltinMask is fnHTTPMask with the body's expiry rendered by fnBuiltinsExpires (its length does not change).
func fnBuiltinMask(resp []byte) string {
	head, body, ok := bytes.Cut(resp, []byte("\r\n\r\n"))
	if !ok {
		return fnHTTPMask(resp)
	}
	return fnHTTPMask(slices.Concat(head, []byte("\r\n\r\n"), fnBuiltinsExpires(body, fnHTTPDate(resp))))
}

// fnBuiltinsCall is one request of a case: its label, target, transaction and header lines.
type fnBuiltinsCall struct {
	label, target, tx string
	headers           []string
}

func (c fnBuiltinsCall) request() []byte { return fnHTTPGet(c.target, c.tx, c.headers...) }

// fnBuiltinsUsers are the callers: anonymous (0x8), the member token (0x9: signed in, anonymous data) and the admin
// token (every bit), fnWriteTokens' two.
var fnBuiltinsUsers = []struct{ label, header string }{
	{"anonymous", ""},
	{"member", "Authorization: Bearer " + fnMemberToken.token},
	{"admin", "Authorization: Bearer " + fnAdminToken.token},
}

// fnBuiltinsSSO, fnBuiltinsSpace and fnBuiltinsRestricted are nRPC's refusals (nrpc-calls.c:540-561).
const (
	fnBuiltinsSSO = "You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. " +
		"Sign-in on this dashboard, or access your Netdata via https://app.netdata.cloud."
	fnBuiltinsSpace      = "You need to login to the Netdata Cloud space this agent is claimed to, to access this feature."
	fnBuiltinsRestricted = "This feature is not available via this API."
)

// fnBuiltins501Re is the answer of a streaming built-in until M10 (D176.3: nRPC's error shape with 501 and the text
// `daemon/src/builtins/mod.rs` NOT_IMPLEMENTED).
var fnBuiltins501Re = regexp.MustCompile(`^\{"status":501,"errorMessage":"This feature is not implemented yet on this agent\."\}$`)

// fnBuiltinsPending is an admin's call of one of the two streaming built-ins, whose handlers read the host status of
// every host (function-netdata-streaming.c:77, function-topology-streaming.c:810): M10. A DEVIATION guard, not a
// comparison: C answers 200 with its table (the oracle must); until M10 the candidate answers D176.3's 501. Either renders
// `answered`, anything else the response itself.
func fnBuiltinsPending(t *testing.T, x *fnHTTPSide, label string, req []byte) string {
	t.Helper()
	b, err := rawExchange(x.d.Addr, req, fnWait)
	if err != nil {
		t.Errorf("%s: %s: %v", x.role, label, err)
		return label + ": " + err.Error()
	}
	status, body := fnStatusLine(b), httpBody(b)
	c200 := status == "HTTP/1.1 200 OK" && bytes.HasPrefix(body, []byte("{\n")) && json.Valid(body)
	if x.role == Oracle && !c200 {
		t.Errorf("oracle: %s: %s", label, truncateBytes(b))
	}
	if c200 || x.role != Oracle && strings.HasPrefix(status, "HTTP/1.1 501 ") && fnBuiltins501Re.Match(body) {
		return label + ": answered (C's 200, or D176.3's 501 until M10)"
	}
	return label + ": " + strconv.Quote(fnHTTPMask(b))
}

// fnBuiltinsMaskRecord masks an access record's level, code and sizes when it is one of txs' (answers the case guards
// but does not compare).
func fnBuiltinsMaskRecord(l string, txs map[string]string) string {
	for tx, what := range txs {
		if !strings.Contains(l, " transaction=sent:"+tx+" ") {
			continue
		}
		l = responseBytesRe.ReplaceAllString(l, "${1}N")
		if what == "pending" {
			l = fnBuiltinsLevelRe.ReplaceAllString(l, " level=L ")
			l = fnBuiltinsCodeRe.ReplaceAllString(l, " code=C ")
		}
	}
	return l
}

var (
	fnBuiltinsLevelRe = regexp.MustCompile(` level=\w+ `)
	fnBuiltinsCodeRe  = regexp.MustCompile(` code=\d+ `)
)

// TestFnBuiltins (check `fn.builtins`, M8 commit 9, D176): C's five built-ins over `/api/v1|v3/function` on a
// standalone pair running the fake plugin (runFnHTTPCases: PULSE and health off, fn.http's bearer tokens). Compared per
// case: each response's status line, headers (fnHTTPMask) and body (fnBuiltinMask: the body's expiry as its distance
// from Date; netdata-api-calls' table through fnAPICallsView), every plugin start's stdin, the access records, the web
// workers' call records and the plugin threads' records.
func TestFnBuiltins(t *testing.T) {
	runFnHTTPCases(t, map[string]fnHTTPCase{
		"access":        fnBuiltinsAccess(),
		"cardinality":   fnBuiltinsCardinality(),
		"api-calls":     fnBuiltinsAPICalls(),
		"vnode":         fnBuiltinsVnode(),
		"topology-info": fnBuiltinsTopologyInfo(),
	})
}

// fnBuiltinsAccess: each of the five on v1 and v3, by each caller (nrpc-calls.c:540-580, http-access.h:81-85): the 0x13
// three refuse anonymous with the SSO 412 and the member with the space 403; bearer_get_token is restricted, refused
// before the access check (412 anonymous, 403 signed in); cardinality (0x8) answers all three. The admin's api-calls
// table compares its status line only (`api-calls` owns the table); the admin's calls of the streaming two are
// fnBuiltinsPending's.
func fnBuiltinsAccess() fnHTTPCase {
	var calls []fnBuiltinsCall
	masked := map[string]string{}
	pending := map[string]bool{}
	n := 0
	for _, v := range []string{"v1", "v3"} {
		for _, f := range fnBuiltins {
			for _, u := range fnBuiltinsUsers {
				n++
				c := fnBuiltinsCall{label: v + " " + u.label + " " + f.name, target: "/api/" + v + "/function?function=" + f.name,
					tx: fnBuiltinsTx(n)}
				if u.header != "" {
					c.headers = []string{u.header}
				}
				if u.label == "admin" {
					switch f.name {
					case "netdata-streaming", "topology:streaming":
						masked[c.tx], pending[c.tx] = "pending", true
					case "netdata-api-calls":
						// the table's length follows the probes' rows and the durations' digits
						masked[c.tx] = "sizes"
					}
				}
				calls = append(calls, c)
			}
		}
	}
	return fnHTTPCase{
		prepare: fnWriteTokens,
		sc:      fnScenario(plugin.Step{Emit: fnOpenRegister}),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			var out []string
			for _, c := range calls {
				switch {
				case pending[c.tx]:
					out = append(out, fnBuiltinsPending(t, x, c.label, c.request()))
				case masked[c.tx] == "sizes":
					out = append(out, x.doMasked(t, c.label, c.request(), func(b []byte) string { return fnStatusLine(b) }))
				default:
					out = append(out, x.doMasked(t, c.label, c.request(), fnBuiltinMask))
				}
			}
			return out
		},
		records: func(l string) string { return fnBuiltinsMaskRecord(l, masked) },
		want: []string{
			fnHTTPError(412, fnBuiltinsSSO), fnHTTPError(403, fnBuiltinsSpace),
			fnHTTPError(412, fnBuiltinsRestricted), fnHTTPError(403, fnBuiltinsRestricted),
			"v1 admin netdata-api-calls: \"HTTP/1.1 200 OK\"", "v3 admin netdata-api-calls: \"HTTP/1.1 200 OK\"",
			fnQ(`"type":"table",` + "\n" + `    "update_every":10,`),
		},
	}
}

// fnVnode2GUID is a second vnode the cardinality case defines under the first one's hostname (P8: hosts sharing a
// hostname are one row by node, function-metrics-cardinality.c:105).
const fnVnode2GUID = "5a1e0000-0000-4000-8000-0000000000d2"

// fnBuiltinsCardinality: netdata-metrics-cardinality (anonymous: 0x8) over localhost's two charts (one and three
// dimensions) and two vnodes sharing a hostname, one with localhost's first chart (two dimensions), the other with a
// chart of its own, every chart collected twice. Its arguments (function-metrics-cardinality.c:69-88): `info` answers
// the header alone wherever it stands, `group:by-node`/`group:by-context` the last one wins, other words are ignored,
// quotes are the splitter's. v3 and the admin answer the same.
func fnBuiltinsCardinality() fnHTTPCase {
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	collect := func(chart string, dims ...string) plugin.Step {
		return plugin.Step{Collect: &plugin.Collect{Chart: chart, Dims: dims, N: 2}}
	}
	queries := []struct{ label, fn string }{
		{"plain", ""},
		{"info", "%20info"},
		{"by-node", "%20group:by-node"},
		{"by-context", "%20group:by-context"},
		{"by-node info", "%20group:by-node%20info"},
		{"info by-node", "%20info%20group:by-node"},
		{"last group wins", "%20group:by-node%20group:by-context"},
		{"unknown word", "%20bogus%20group:by-node"},
		{"unknown group", "%20group:by-host"},
		{"quoted", "%20%27group:by-node%27"},
	}
	return fnHTTPCase{
		prepare: fnWriteTokens,
		sc: fnScenario(emit(fnOpenRegister), collect("difftest.c1", "a"), collect("difftest.c2", "a", "b", "c"),
			emit(vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...)), collect("difftest.c1", "a", "b"),
			emit(vnodeDefine(fnVnode2GUID, vnodeName)), collect("difftest.cv", "x"), emit("HOST localhost\n")),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			if !x.step(t, x.l, "the eight blocks were not collected", fnCollected(8)) || !fnContextsSettled(t, x) {
				return nil
			}
			var out []string
			for i, q := range queries {
				out = append(out, x.doMasked(t, q.label, fnHTTPGet("/api/v1/function?function=netdata-metrics-cardinality"+q.fn,
					fnBuiltinsTx(0x100+i)), fnBuiltinMask))
			}
			return append(out,
				x.doMasked(t, "v3", fnHTTPGet("/api/v3/function?function=netdata-metrics-cardinality", fnBuiltinsTx(0x120)), fnBuiltinMask),
				x.doMasked(t, "admin by-node", fnHTTPGet("/api/v1/function?function=netdata-metrics-cardinality%20group:by-node",
					fnBuiltinsTx(0x121), fnBuiltinsUsers[2].header), fnBuiltinMask))
		},
		want: []string{fnQ(`"accepted_params":["group"],`), fnQ(`"default_sort_column":"Old Instances",`),
			fnQ(`"expires":NOW+1` + "\n}\n"), fnQ(`"id":"by-node",` + "\n" + `                    "name":"Group by Node"`),
			fnQ(`"Hostname":{`), fnQ(`"Context":{`),
			// the two vnodes sharing a hostname are one row by node (P8), localhost's and the vnode's c1 one by context
			fnQ(`["parity-vnode",2,2,0,3,3,0,0,0,50,50,0,42.8571429,42.8571429,0]`),
			fnQ(`["difftest.c1",2,2,0,2,2,0,3,3,0,0,0,50,50,0,42.8571429,42.8571429,0]`)},
	}
}

// fnBuiltinsVnode: the five through the vnode's route (`/host/<vnode>/`, the routed host's registry): C registers them
// on localhost only (functions.c:9), so each is unknown there (404, nrpc-registry.c:910), restricted or not, for any
// caller.
func fnBuiltinsVnode() fnHTTPCase {
	var calls []fnBuiltinsCall
	for i, f := range fnBuiltins {
		for j, u := range []int{0, 2} {
			user := fnBuiltinsUsers[u]
			c := fnBuiltinsCall{label: user.label + " " + f.name, target: "/host/" + vnodeName + "/api/v1/function?function=" + f.name,
				tx: fnBuiltinsTx(0x200 + 2*i + j)}
			if user.header != "" {
				c.headers = []string{user.header}
			}
			calls = append(calls, c)
		}
	}
	calls = append(calls, fnBuiltinsCall{label: "v3 anonymous netdata-metrics-cardinality",
		target: "/host/" + vnodeName + "/api/v3/function?function=netdata-metrics-cardinality", tx: fnBuiltinsTx(0x220)})
	return fnHTTPCase{
		prepare: fnWriteTokens,
		listed:  []fnListed{{"", "difftest-open"}, {"/host/" + vnodeName, "difftest-vfn"}},
		sc: fnScenario(plugin.Step{Emit: fnOpenRegister + vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...) +
			`FUNCTION GLOBAL "difftest-vfn" 10 "vnode fn" "top" "0x8" 100 1` + "\n"}),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			var out []string
			for _, c := range calls {
				out = append(out, x.do(t, c.label, c.request()))
			}
			return out
		},
		want: []string{fnHTTPError(404, "This feature is not available on this host at this time.")},
		wantNot: []string{fnHTTPError(412, fnBuiltinsRestricted), fnHTTPError(403, fnBuiltinsRestricted),
			fnQ("HTTP/1.1 200 OK")},
	}
}

// fnBuiltinsTopologyInfo: topology:streaming's `info` (function-topology-streaming.c:231-242, :2949-2958, :260-273): any
// word from the second on, quoted or not, answers the header at once (no host status; no `hostname`; expires now + 10);
// the access check comes first (412 anonymous, 403 the member).
func fnBuiltinsTopologyInfo() fnHTTPCase {
	admin, member := fnBuiltinsUsers[2].header, fnBuiltinsUsers[1].header
	calls := []fnBuiltinsCall{
		{"admin info", "/api/v1/function?function=topology:streaming%20info", fnBuiltinsTx(0x301), []string{admin}},
		{"admin info v3", "/api/v3/function?function=topology:streaming%20info", fnBuiltinsTx(0x302), []string{admin}},
		{"admin x info", "/api/v1/function?function=topology:streaming%20x%20info", fnBuiltinsTx(0x303), []string{admin}},
		{"admin info x", "/api/v1/function?function=topology:streaming%20info%20x", fnBuiltinsTx(0x304), []string{admin}},
		{"admin quoted", "/api/v1/function?function=topology:streaming%20%27info%27", fnBuiltinsTx(0x305), []string{admin}},
		{"anonymous info", "/api/v1/function?function=topology:streaming%20info", fnBuiltinsTx(0x306), nil},
		{"member info", "/api/v1/function?function=topology:streaming%20info", fnBuiltinsTx(0x307), []string{member}},
	}
	return fnHTTPCase{
		prepare: fnWriteTokens,
		sc:      fnScenario(plugin.Step{Emit: fnOpenRegister}),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			var out []string
			for _, c := range calls {
				out = append(out, x.doMasked(t, c.label, c.request(), fnBuiltinMask))
			}
			return out
		},
		want: []string{fnQ(`{` + "\n" + `    "status":200,` + "\n" + `    "type":"topology",`), fnQ(`"accepted_params":["info"],` + "\n" +
			`    "required_params":[],` + "\n" + `    "expires":NOW+10` + "\n}\n"), fnHTTPError(412, fnBuiltinsSSO),
			fnHTTPError(403, fnBuiltinsSpace)},
		wantNot: []string{fnQ(`"hostname":`)},
	}
}

// ---------------------------------------------------------------------------------------------------------------
// netdata-api-calls

// fnAPICallsBig is the body of the case's largest answer: the table's `max` of Size and Sent is its length, whatever
// the probes' rows (each a smaller /api/v1/info).
var fnAPICallsBig = strings.Repeat(strings.Repeat("x", 63)+"\n", 1024)

// fnBuiltinsAPICalls: netdata-api-calls' table (progress.c:397-612) after requests with the case's own transactions:
// an unknown command (404); a call of the plugin's method (200); a POST with a payload; two on one keep-alive connection
// (the second has no client address: `unknown`, web_client.c:220); one with X-Forwarded-For (its client); plugin answers
// with codes 304, 409, 302, 503 and 201 (each severity); a 64 KiB answer (the maxima of Size and Sent); an OPTIONS; a
// call the client closes while the plugin works (499); then the admin's table, its own row running. Thirty requests
// without a transaction (random ids) come first: their rows share the table's hash slots with the case's
// (simple_hashtable.h:320-376), so the case's rows come in an order that varies run to run (each side logs it).
func fnBuiltinsAPICalls() fnHTTPCase {
	tx := fnBuiltinsTx
	emit := func(s string) plugin.Step { return plugin.Step{Emit: s} }
	expect := plugin.ExpectFunction
	answer := func(name, code, body string) plugin.Step {
		return emit(plugin.Result("{{"+name+"}}", code, "text/plain", "0", body))
	}
	ok := func(name string) plugin.Step { return answer(name, "200", name+"\n") }
	codes := []string{"304", "409", "302", "503", "201"}
	steps := []plugin.Step{emit(fnOpenRegister), expect("a"), ok("a"), plugin.ExpectPayload("p"), ok("p"),
		expect("k1"), ok("k1"), expect("k2"), ok("k2"), expect("f"), ok("f")}
	for _, c := range codes {
		steps = append(steps, expect("r"+c), answer("r"+c, c, "r"+c+"\n"))
	}
	steps = append(steps, expect("big"), answer("big", "200", fnAPICallsBig),
		expect("c"), plugin.ExpectCancel("x"), ok("c"))
	table := tx(0x420)
	call := func(cmd string) string { return "/api/v1/function?function=difftest-open%20" + cmd }
	return fnHTTPCase{
		prepare: fnWriteTokens,
		sc:      fnScenario(steps...),
		play: func(t *testing.T, x *fnHTTPSide) []string {
			for i := range 30 {
				if _, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v1/nope", ""), fnWait); err != nil {
					t.Errorf("%s: filler %d: %v", x.role, i, err)
				}
			}
			out := []string{
				x.do(t, "unknown", fnHTTPGet("/api/v1/nope", tx(0x401))),
				x.do(t, "call", fnHTTPGet(call("a"), tx(0x402))),
				x.do(t, "post", rawRequest("POST", call("p"), []string{"X-Transaction-Id: " + tx(0x403),
					"Content-Type: application/json"}, []byte("{\"a\":1}"))),
			}
			rs, err := rawExchanges(x.d.Addr, [][]byte{
				fnHTTPGet(call("k1"), tx(0x404), "Connection: keep-alive"),
				fnHTTPGet(call("k2"), tx(0x405), "Connection: keep-alive"),
			}, fnWait)
			if err != nil {
				t.Errorf("%s: keep-alive: %v", x.role, err)
			}
			for i, r := range rs {
				out = append(out, fmt.Sprintf("keep-alive %d: %s", i+1, strconv.Quote(fnHTTPMask(r))))
			}
			out = append(out, x.do(t, "forwarded", fnHTTPGet(call("f"), tx(0x406), "X-Forwarded-For: 10.20.30.40")))
			for i, c := range codes {
				out = append(out, x.do(t, "code "+c, fnHTTPGet(call("r"+c), tx(0x407+i))))
			}
			out = append(out, x.doMasked(t, "big", fnHTTPGet(call("big"), tx(0x410)), func(b []byte) string {
				head, body, _ := bytes.Cut(b, []byte("\r\n\r\n"))
				return fmt.Sprintf("%s, a body of %d bytes, as sent: %t", fnHTTPMask(head), len(body), string(body) == fnAPICallsBig)
			}))
			out = append(out, x.do(t, "options", rawRequest("OPTIONS", "/api/v1/data", []string{"X-Transaction-Id: " + tx(0x411)}, nil)))
			got, err := rawHoldAndClose(x.d.Addr, fnHTTPGet(call("c"), tx(0x412)), func() {
				x.step(t, x.l, "the plugin did not get the call", fnMatched(1, "c"))
			}, false, fnWait)
			if err != nil {
				t.Errorf("%s: cancel: %v", x.role, err)
			}
			out = append(out, fmt.Sprintf("cancelled: %q, the plugin got the cancel: %t", got,
				x.step(t, x.l, "no cancel", fnMatched(1, "x"))))
			// the cancelled call's access record and row finish once the 499 went out
			time.Sleep(500 * time.Millisecond)
			b, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v1/function?function=netdata-api-calls", table,
				fnBuiltinsUsers[2].header), fnWait)
			if err != nil {
				t.Errorf("%s: the table: %v", x.role, err)
				return append(out, "table: "+err.Error())
			}
			view, order := fnAPICallsView(b)
			// the evidence of the view's sort and of the record's size mask: both vary C against C
			t.Logf("%s: the case's rows in the table's order: %s; the table's %s", x.role, strings.Join(order, " "),
				contentLengthRe.Find(b))
			return append(out, "table: "+strconv.Quote(view))
		},
		records: func(l string) string { return fnBuiltinsMaskRecord(l, map[string]string{table: "sizes"}) },
		want: []string{
			// the table's own row (running: no code, sizes or severity of its own, progress the done count) and the
			// others' severities (progress.c:457-505)
			fnQ(`"` + table + `",T15,"GET","/api/v1/function?function=netdata-api-calls","localhost","in-progress","0",D,null,null,null,{` +
				"\n" + `                "severity":"notice"`),
			fnQ(`"` + tx(0x401) + `",T1,"GET","/api/v1/nope","localhost","finished","100.00 %%",D,404,`),
			fnQ(`"/api/v1/function?function=difftest-open%20k2","unknown","finished"`),
			fnQ(`"/api/v1/function?function=difftest-open%20f","10.20.30.40","finished"`),
			fnQ(`"` + tx(0x411) + `",T13,"OPTIONS","/api/v1/data",`),
			fnQ(`,499,49,49,{` + "\n" + `                "severity":"debug"`),
			fnQ(`,304,5,5,{` + "\n" + `                "severity":"debug"`),
			fnQ(`,409,5,5,{` + "\n" + `                "severity":"debug"`),
			fnQ(`,302,5,5,{` + "\n" + `                "severity":"notice"`),
			fnQ(`,503,5,5,{` + "\n" + `                "severity":"error"`),
			fnQ(`,201,5,5,{` + "\n" + `                "severity":"normal"`),
			fnQ(`"max":65536,`),
			fnQ(`"default_sort_column":"Started",` + "\n" + `    "expires":NOW+1` + "\n}\n"),
		},
	}
}

// jsonSpan is one item of a JSON array or one member of an object, as byte offsets in the document: from is where the
// previous item ended (or the opening delimiter), start and end the value's own bytes.
type jsonSpan struct {
	key              string
	from, start, end int
}

// jsonItems are the items (or members) of the JSON array (or object) b, with their bytes' spans.
func jsonItems(b []byte) ([]jsonSpan, error) {
	dec := json.NewDecoder(bytes.NewReader(b))
	tok, err := dec.Token()
	if err != nil {
		return nil, err
	}
	d, ok := tok.(json.Delim)
	if !ok || d != '[' && d != '{' {
		return nil, fmt.Errorf("parity: not a JSON array or object: %.40q", b)
	}
	// More() skips the whitespace before a value, so each item's `from` is where the previous one ended
	from := int(dec.InputOffset())
	var out []jsonSpan
	for dec.More() {
		s := jsonSpan{from: from}
		if d == '{' {
			k, err := dec.Token()
			if err != nil {
				return nil, err
			}
			s.key, _ = k.(string)
		}
		var raw json.RawMessage
		if err := dec.Decode(&raw); err != nil {
			return nil, err
		}
		s.end = int(dec.InputOffset())
		s.start = s.end - len(raw)
		from = s.end
		out = append(out, s)
	}
	return out, nil
}

// fnAPICallsView renders a netdata-api-calls answer: the head (fnHTTPMask, the length masked) and the body's bytes with
//   - only the rows of the case's transactions (fnTxPrefix): the launcher's probes, the listing polls and the fillers
//     carry random ids and come in a number that is each side's timing;
//   - those rows sorted by transaction: rows sit in the hash table's slot order (progress.c:417), and the random ids'
//     rows displace the case's run to run;
//   - each row's Started as T<k>, its rank among the kept rows (the clock; order and equality stay compared), its
//     Duration as D (the clock);
//   - the Duration column's `max` as M (the largest duration of every row, the table's own running one included);
//   - the expiry as its distance from Date (fnBuiltinsExpires).
//
// Every other byte is compared, the Size and Sent maxima included (fnAPICallsBig is the largest answer). order is the
// kept rows' transactions in the table's own order (logged).
func fnAPICallsView(resp []byte) (view string, order []string) {
	head, body, ok := bytes.Cut(resp, []byte("\r\n\r\n"))
	members, err := jsonItems(body)
	if !ok || err != nil {
		return fnHTTPMask(resp), nil
	}
	var out bytes.Buffer
	last := 0
	for _, m := range members {
		var value []byte
		switch m.key {
		case "data":
			value, order = fnAPICallsRows(body[m.start:m.end])
		case "columns":
			value = fnAPICallsColumns(body[m.start:m.end])
		default:
			continue
		}
		out.Write(body[last:m.start])
		out.Write(value)
		last = m.end
	}
	out.Write(body[last:])
	h := contentLengthRe.ReplaceAllString(fnHTTPMask(slices.Concat(head, []byte("\r\n\r\n"))), "Content-Length: N")
	return h + string(fnBuiltinsExpires(out.Bytes(), fnHTTPDate(resp))), order
}

// fnAPICallsRows is the `data` array kept, sorted and masked as fnAPICallsView says, each row with the bytes before it.
func fnAPICallsRows(arr []byte) ([]byte, []string) {
	items, err := jsonItems(arr)
	if err != nil || len(items) == 0 {
		return arr, nil
	}
	type row struct {
		tx, before string
		text       []byte
		cells      []jsonSpan
	}
	var rows []row
	var order []string
	for _, it := range items {
		text := arr[it.start:it.end]
		cells, err := jsonItems(text)
		if err != nil || len(cells) < 8 {
			continue
		}
		var tx string
		if json.Unmarshal(text[cells[0].start:cells[0].end], &tx) != nil || !strings.HasPrefix(tx, fnTxPrefix) {
			continue
		}
		order = append(order, tx)
		rows = append(rows, row{tx: tx, before: strings.TrimPrefix(string(arr[it.from:it.start]), ","), text: text, cells: cells})
	}
	slices.SortFunc(rows, func(a, b row) int { return strings.Compare(a.tx, b.tx) })
	var started []int64
	for _, r := range rows {
		v, _ := strconv.ParseInt(string(r.text[r.cells[1].start:r.cells[1].end]), 10, 64)
		started = append(started, v)
	}
	ranks := slices.Clone(started)
	slices.Sort(ranks)
	ranks = slices.Compact(ranks)
	var out bytes.Buffer
	out.WriteByte('[')
	for i, r := range rows {
		if i > 0 {
			out.WriteByte(',')
		}
		k, _ := slices.BinarySearch(ranks, started[i])
		out.WriteString(r.before)
		out.Write(r.text[:r.cells[1].start])
		fmt.Fprintf(&out, "T%d", k+1)
		out.Write(r.text[r.cells[1].end:r.cells[7].start])
		out.WriteString("D")
		out.Write(r.text[r.cells[7].end:])
	}
	out.Write(arr[items[len(items)-1].end:])
	return out.Bytes(), order
}

// fnAPICallsColumns is the `columns` object with the Duration column's `max` as M.
func fnAPICallsColumns(obj []byte) []byte {
	cols, err := jsonItems(obj)
	if err != nil {
		return obj
	}
	for _, c := range cols {
		if c.key != "Duration" {
			continue
		}
		fields, err := jsonItems(obj[c.start:c.end])
		if err != nil {
			return obj
		}
		for _, f := range fields {
			if f.key == "max" {
				return slices.Concat(obj[:c.start+f.start], []byte("M"), obj[c.start+f.end:])
			}
		}
	}
	return obj
}

// ---------------------------------------------------------------------------------------------------------------
// bearer_get_token through a parent (fn.child's `bearer` case)

// The bearer case's ids: the parent's claim id and the node id its NODE_ID gives the child, the Cloud account; the
// child's machine guid is senderChild's.
const (
	fnBearerClaim   = "5a1e0000-0000-4000-8000-0000000000e2"
	fnBearerNode    = "5a1e0000-0000-4000-8000-0000000000e1"
	fnBearerAccount = "5a1e0000-0000-4000-8000-0000000000e3"
	fnBearerGUID    = "5a1e0000-0000-4000-8000-0000000000cc"
	// fnBearerSource is a Cloud caller's source as a claimed parent writes it down: "via NC" is its prefix
	// (user-auth.c:19-21)
	fnBearerSource = "method=NC,role=admin,permissions=0x7ff,user=parity-user,account=5a1e000000004000800000000000e3"
)

// fnBearerPayload is a parent's bearer_get_token call with a payload (stream-sender-execute.c:279-312: the child adds a
// newline after each line).
func fnBearerPayload(tx, body string) []string {
	return []string{`FUNCTION_PAYLOAD ` + tx + ` 10 "bearer_get_token" "0x13" "` + fnBearerSource + `" "application/json"`, body,
		`FUNCTION_PAYLOAD_END`}
}

// fnBearerAccess are the access names of fnBearerRequest's tokens: what netdata-api-calls needs (0x13) and what the
// HTTP endpoint needs (anonymous data, web_api.c:82-89).
const fnBearerAccess = `["signed-in","same-space","anonymous-data","sensitive-data"]`

// fnBearerRequest is a whole payload (function-bearer_get_token.c:16-27) for the child under claim and guid, the role
// role, with the access names access (a JSON array).
func fnBearerRequest(claim, guid, role, access string) string {
	return fmt.Sprintf(`{"claim_id":"%s","machine_guid":"%s","node_id":"%s","user_role":"%s","access":%s,`+
		`"cloud_account_id":"%s","client_name":"parity-cloud"}`, claim, guid, fnBearerNode, role, access, fnBearerAccount)
}

var (
	// fnBearerRecordRe are the case's stream-thread records besides the calls': an unknown role's WARNING
	// (http-access.c:26-36) and the NODE_ID's (command-nodeid.c:96-161)
	fnBearerRecordRe = regexp.MustCompile(`msg="(HTTP user role |STREAM SND '[^']*' \[to [^]]*\] \[PCLAIMID\])`)
	fnBearerAnswerRe = regexp.MustCompile(`"token":"([0-9a-f-]{36})","expiration":(\d+)`)
	// a token and its expiry in fnUpstream's quoted bodies
	fnBearerTokenRe = regexp.MustCompile(`(\\"token\\":\\")([0-9a-f-]{36})(\\")`)
	fnBearerExpRe   = regexp.MustCompile(`(\\"expiration\\":)(\d+)`)
)

// fnBearerAnswer is the token and expiry of tx's 200 answer in a session's data (empty when it has none).
func fnBearerAnswer(data []byte, tx string) (string, int64) {
	i := bytes.Index(data, []byte("\nFUNCTION_RESULT_BEGIN \""+tx+"\" 200 "))
	if i < 0 {
		return "", 0
	}
	span := data[i:]
	if j := bytes.Index(span, []byte("\nFUNCTION_RESULT_END\n")); j >= 0 {
		span = span[:j]
	}
	m := fnBearerAnswerRe.FindSubmatch(span)
	if m == nil {
		return "", 0
	}
	e, _ := strconv.ParseInt(string(m[2]), 10, 64)
	return string(m[1]), e
}

// fnBearerMask renders the answers' tokens as TOKEN<k> and their expiries as E<k>, k the value's rank of first
// appearance: a token is random (uuid_generate_random, http_auth.c:224) and its expiry the creation's clock, while a
// reuse stays visible as the same name.
func fnBearerMask(lines []string) []string {
	tokens, exps := map[string]int{}, map[string]int{}
	rank := func(m map[string]int, v string) int {
		if _, ok := m[v]; !ok {
			m[v] = len(m) + 1
		}
		return m[v]
	}
	out := make([]string, len(lines))
	for i, l := range lines {
		l = fnBearerTokenRe.ReplaceAllStringFunc(l, func(s string) string {
			sm := fnBearerTokenRe.FindStringSubmatch(s)
			return fmt.Sprintf("%sTOKEN%d%s", sm[1], rank(tokens, sm[2]), sm[3])
		})
		out[i] = fnBearerExpRe.ReplaceAllStringFunc(l, func(s string) string {
			sm := fnBearerExpRe.FindStringSubmatch(s)
			return fmt.Sprintf("%sE%d", sm[1], rank(exps, sm[2]))
		})
	}
	return out
}

// fnBearerFiles renders the child's `<varlib>/bearer_tokens/` (http_auth.c:136-171): the directory's mode, then each
// file in tokens' order (TOKEN<k>, the answered tokens; any other file by its name after them) with its mode, whether it
// ends with a newline, and its members in order: created_s as NOW within [lo, hi], expires_s as `created_s+86400` when
// it is, the signature as SIG (XXH3 over the random token and the times, http_auth.c:85-111), the rest as written.
func fnBearerFiles(t *testing.T, runDir string, tokens []string, lo, hi int64) []string {
	t.Helper()
	dir := filepath.Join(runDir, "lib", "bearer_tokens")
	info, err := os.Stat(dir)
	if err != nil {
		return []string{"token files: " + err.Error()}
	}
	out := []string{fmt.Sprintf("token directory mode %04o", info.Mode().Perm())}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return append(out, "token files: "+err.Error())
	}
	rank := func(name string) int {
		if k := slices.Index(tokens, name); k >= 0 {
			return k
		}
		return len(tokens)
	}
	slices.SortStableFunc(entries, func(a, b os.DirEntry) int { return rank(a.Name()) - rank(b.Name()) })
	for _, e := range entries {
		label := "token file " + e.Name()
		k := slices.Index(tokens, e.Name())
		if k >= 0 {
			label = fmt.Sprintf("token file TOKEN%d", k+1)
		}
		fi, err := e.Info()
		if err != nil {
			out = append(out, label+": "+err.Error())
			continue
		}
		b, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			out = append(out, label+": "+err.Error())
			continue
		}
		out = append(out, fmt.Sprintf("%s mode %04o, ends with a newline: %t", label, fi.Mode().Perm(), bytes.HasSuffix(b, []byte("\n"))))
		v, err := ParseJSON(b)
		if err != nil {
			out = append(out, fmt.Sprintf("%s: %v: %q", label, err, b))
			continue
		}
		var created int64
		for _, m := range v.Members {
			val := m.Value.String()
			n, _ := strconv.ParseInt(m.Value.Text, 10, 64)
			switch m.Key {
			case "token":
				if k >= 0 && m.Value.Text == tokens[k] {
					val = fmt.Sprintf(`"TOKEN%d"`, k+1)
				}
			case "created_s":
				if created = n; n >= lo && n <= hi {
					val = "NOW"
				}
			case "expires_s":
				if n == created+86400 {
					val = "created_s+86400"
				}
			case "signature":
				val = "SIG"
			}
			out = append(out, label+" "+m.Key+"="+val)
		}
	}
	return out
}

// fnBearerCase (fn.child's `bearer`, M8 commit 9, D176.4): a parent's calls of the child's bearer_get_token
// (function-bearer_get_token.c:29-57, api_v2_bearer.c:72-95, http_auth.c:208-235). nRPC's gates first (0x13; the method
// is restricted but a parent's call may run it, stream-sender-execute.c:89); then the NC source; the payload (json-c's
// 500, the fields' 400s in their order, an unknown access name's text glued to the next field's, an unknown role's
// WARNING, json-c-parser-inline.c:55-90 and .h); the claim id (an unclaimed child matches only its parent's, which the
// parent's NODE_ID sets with the child's node id, claim.c:130-149, command-nodeid.c:96-161); the host ids; then a token
// (now + 86400, saved under `<varlib>/bearer_tokens`), the same token for the same request (reused while it expires in
// more than 2 h), another for another access, and each token's access over the child's own HTTP API: the first reaches
// netdata-api-calls, the second lacks anonymous data and stops at the endpoint (403, web_api.c:82-89).
func fnBearerCase() fnCase {
	b := func(i int) string { return fnTx(0xc00 + i) }
	call := func(tx, access, source string) string { return fnCall(tx, 10, "bearer_get_token", access, source) }
	return fnCase{
		sc:      plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Emit: fnRegister}}}}},
		records: fnBearerRecordRe,
		mask:    fnBearerMask,
		play: func(t *testing.T, x *fnSide) []string {
			ask := func(tx string, lines ...string) {
				x.send(t, lines...)
				x.answered(t, tx, 1)
			}
			ask(b(1), call(b(1), "0x8", fnBearerSource))
			ask(b(2), call(b(2), "0x9", fnBearerSource))
			ask(b(3), call(b(3), "0xb", fnBearerSource))
			ask(b(4), call(b(4), "0x13", "method=api-bearer,role=admin"))
			ask(b(5), call(b(5), "0x13", fnBearerSource))
			ask(b(6), fnBearerPayload(b(6), "{")...)
			ask(b(7), fnBearerPayload(b(7), "x")...)
			ask(b(8), fnBearerPayload(b(8), `{"machine_guid":"`+fnBearerGUID+`"}`)...)
			ask(b(9), fnBearerPayload(b(9), `{"claim_id":"not-a-uuid"}`)...)
			ask(b(10), fnBearerPayload(b(10), `{"claim_id":"`+fnBearerClaim+`","machine_guid":"`+fnBearerGUID+`","node_id":"`+
				fnBearerNode+`","user_role":"admin","access":["signed-in","bogus"]}`)...)
			ask(b(11), fnBearerPayload(b(11), fnBearerRequest(fnBearerClaim, fnBearerGUID, "bogus", fnBearerAccess))...)
			// the parent's claim and node id (a claimed parent sends them at the child's connection); the child reads
			// its lines in order, so the next call sees them
			x.send(t, "NODE_ID '"+fnBearerClaim+"' '"+fnBearerNode+"' 'https://app.netdata.cloud'")
			ask(b(12), fnBearerPayload(b(12), fnBearerRequest(fnBearerClaim, "5a1e0000-0000-4000-8000-0000000000dd", "admin",
				fnBearerAccess))...)
			lo := time.Now().Unix()
			ask(b(13), fnBearerPayload(b(13), fnBearerRequest(fnBearerClaim, fnBearerGUID, "admin", fnBearerAccess))...)
			ask(b(14), fnBearerPayload(b(14), fnBearerRequest(fnBearerClaim, fnBearerGUID, "admin", fnBearerAccess))...)
			ask(b(15), fnBearerPayload(b(15), fnBearerRequest(fnBearerClaim, fnBearerGUID, "admin",
				`["signed-in","same-space","sensitive-data"]`))...)
			var out, tokens []string
			for _, tx := range []string{b(13), b(15)} {
				token, expires := fnBearerAnswer(x.s.Data(), tx)
				if token == "" {
					return append(out, "no token for "+tx)
				}
				tokens = append(tokens, token)
				k := len(tokens)
				out = append(out, fmt.Sprintf("TOKEN%d expires a day after its call: %t", k,
					expires >= lo+86400 && expires <= time.Now().Unix()+86400))
				resp, err := rawExchange(x.d.Addr, fnHTTPGet("/api/v1/function?function=netdata-api-calls", fnTx(0xc10+k),
					"Authorization: Bearer "+token), fnWait)
				if err != nil {
					t.Errorf("%s: TOKEN%d over HTTP: %v", x.role, k, err)
				}
				line := fnStatusLine(resp)
				if !strings.HasSuffix(line, " 200 OK") {
					line += " " + strconv.Quote(string(httpBody(resp)))
				}
				out = append(out, fmt.Sprintf("netdata-api-calls with TOKEN%d: %s", k, line))
			}
			return append(out, fnBearerFiles(t, x.d.Opts.RunDir, tokens, lo, time.Now().Unix())...)
		},
		want: []string{
			fnError(b(1), 412, fnBuiltinsSSO),
			fnError(b(2), 403, fnBuiltinsSpace),
			fnError(b(3), 403, "This feature requires additional permissions: sensitive-data."),
			fnError(b(4), 400, "Bearer tokens can only be provided via NC."),
			fnError(b(5), 400, "No payload given, but a payload is required for this feature."),
			fnError(b(6), 500, "JSON parser failed: continue"),
			fnError(b(7), 500, "JSON parser failed: unexpected character"),
			fnError(b(8), 400, "JSON parser failed: missing UUID '.claim_id'"),
			fnError(b(9), 400, "JSON parser failed: invalid UUID '.claim_id'"),
			fnError(b(10), 400, "JSON parser failed: unknown option 'bogus' in '.access' at index 1missing UUID '.cloud_account_id'"),
			`msg="HTTP user role 'bogus' is not valid"`,
			fnError(b(11), 400, "The request is for a different agent"),
			"[PCLAIMID] set parent's claim id to " + fnBearerClaim + " (was empty)",
			fnError(b(12), 400, "The request is missing or not matching local node UUIDs"),
			fnAnswer(b(13), "200", "application/json", "0", `{"status":200,"mg":"`+fnBearerGUID+
				`","bearer_protection":false,"token":"TOKEN1","expiration":E1}`),
			fnAnswer(b(14), "200", "application/json", "0", `{"status":200,"mg":"`+fnBearerGUID+
				`","bearer_protection":false,"token":"TOKEN1","expiration":E1}`),
			`"token\":\"TOKEN2\"`,
			"TOKEN1 expires a day after its call: true", "TOKEN2 expires a day after its call: true",
			"netdata-api-calls with TOKEN1: HTTP/1.1 200 OK", "netdata-api-calls with TOKEN2: HTTP/1.1 403 Forbidden",
			`token file TOKEN1 token="TOKEN1"`, `token file TOKEN1 expires_s=created_s+86400`,
			`token file TOKEN1 access=["signed-in","same-space","anonymous-data","sensitive-data"]`,
			`token file TOKEN2 access=["signed-in","same-space","sensitive-data"]`,
		},
	}
}
