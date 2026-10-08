// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"compress/gzip"
	"fmt"
	"io"
	"net/http"
	"net/http/httputil"
	"os"
	"path"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"unicode"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// The shipped dashboard against both agents (milestone 10): every file of the web directory (`web.dashboard-files`)
// and the requests the local dashboard sends, in its order (`web.dashboard-replay`).

// walkTypes are C's content types by the served file's last extension (http_defs.c:180-204, the header texts
// content_type.c:13-43, 77-99); any other extension, or none, is `application/octet-stream` (http_defs.c:225-258).
var walkTypes = map[string]string{
	"html": "text/html; charset=utf-8", "js": "application/javascript; charset=utf-8", "css": "text/css; charset=utf-8",
	"xml": "text/xml; charset=utf-8", "xsl": "text/xsl; charset=utf-8", "txt": "text/plain; charset=utf-8",
	"svg": "image/svg+xml", "ttf": "application/x-font-truetype", "otf": "application/x-font-opentype",
	"woff2": "application/font-woff2", "woff": "application/font-woff", "eot": "application/vnd.ms-fontobject",
	"png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg", "gif": "image/gif", "bmp": "image/bmp",
	"ico": "image/x-icon", "icns": "image/icns", "wasm": "application/wasm",
}

// walkType is C's content type for a served file.
func walkType(served string) string {
	if i := strings.LastIndexByte(served, '.'); i >= 0 {
		if ct, ok := walkTypes[served[i+1:]]; ok {
			return ct
		}
	}
	return "application/octet-stream"
}

// walkServed is the file C serves for the web path rel (find_filename_to_serve, web_client.c:399-474): under a
// version prefix a name without an extension is looked up without the prefix, then falls back to the version's
// index (fallback 2, :415-421, :442-447), so `v3/apple-app-site-association` serves `v3/index.html`.
func walkServed(webDir, rel string) string {
	version, rest, ok := strings.Cut(rel, "/")
	if !ok || version != "v3" || strings.Contains(path.Base(rest), ".") {
		return rel
	}
	if st, err := os.Stat(filepath.Join(webDir, rest)); err == nil && st.Mode().IsRegular() {
		return rest
	}
	return "v3/index.html"
}

// walkFiles are the web paths of every file under webDir, in order.
func walkFiles(t *testing.T, webDir string) []string {
	t.Helper()
	var out []string
	err := filepath.WalkDir(webDir, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		rel, err := filepath.Rel(webDir, p)
		out = append(out, filepath.ToSlash(rel))
		return err
	})
	if err != nil || len(out) == 0 {
		t.Fatalf("parity: the files of %s: %v", webDir, err)
	}
	return out
}

// walkHeader is a header line's value in a raw answer's head (empty when it has none).
func walkHeader(head []byte, name string) string {
	for _, l := range strings.Split(string(head), "\r\n")[1:] {
		if v, ok := strings.CutPrefix(l, name+": "); ok {
			return v
		}
	}
	return ""
}

// walkGuard holds the oracle's answer for a static file to the file: a 200, C's content type for it, its
// modification time as Date, and its bytes (inflated from the chunks when gzip was asked).
func walkGuard(raw []byte, served string, want []byte, mtime time.Time, gz bool) error {
	head, body, ok := bytes.Cut(raw, []byte("\r\n\r\n"))
	if !ok || !bytes.HasPrefix(head, []byte("HTTP/1.1 200 OK\r\n")) {
		return fmt.Errorf("not a 200: %q", truncateBytes(head))
	}
	if ct := walkHeader(head, "Content-Type"); ct != walkType(served) {
		return fmt.Errorf("content type %q, want %q", ct, walkType(served))
	}
	if d := walkHeader(head, "Date"); d != mtime.UTC().Format(http.TimeFormat) {
		return fmt.Errorf("date %q, want the modification time of %s", d, served)
	}
	if gz {
		if e := walkHeader(head, "Content-Encoding"); e != "gzip" {
			return fmt.Errorf("no gzip encoding: %q", truncateBytes(head))
		}
		zr, err := gzip.NewReader(httputil.NewChunkedReader(bytes.NewReader(body)))
		if err != nil {
			return fmt.Errorf("gzip: %v", err)
		}
		if body, err = io.ReadAll(zr); err != nil {
			return fmt.Errorf("gzip: %v", err)
		}
	}
	if !bytes.Equal(body, want) {
		return fmt.Errorf("the body is not %s (%d bytes, want %d)\n%s", served, len(body), len(want),
			walkPrintable(firstDifference(append([]byte("\r\n\r\n"), body...), append([]byte("\r\n\r\n"), want...))))
	}
	return nil
}

// walkPrintable keeps a difference report readable when the answers are binary.
func walkPrintable(s string) string {
	return strings.Map(func(r rune) rune {
		if r == '\n' || unicode.IsPrint(r) {
			return r
		}
		return '.'
	}, strings.ToValidUTF8(s, "."))
}

// walkFile asks both agents for the web path target, plain or asking for gzip, one connection each, and judges the
// answers: the oracle's must be the file served (walkGuard), then the two must agree after maskAnswer: the
// transaction id masked, and an expiry of now + 86400 for a second the request was in flight written `now+86400`
// (web_client.c:628; another expiry is kept); the Date of a static answer is the file's modification time
// (web_client.c:623-627), which no flight holds: it is compared. It returns the oracle's problem or the difference
// (both empty when the answers agree).
func walkFile(t *testing.T, p *Pair, webDir, target, served string, gz bool) (oracle, differ string) {
	t.Helper()
	want, err := os.ReadFile(filepath.Join(webDir, served))
	if err != nil {
		t.Fatal(err)
	}
	st, err := os.Stat(filepath.Join(webDir, served))
	if err != nil {
		t.Fatal(err)
	}
	var headers []string
	if gz {
		headers = []string{"Accept-Encoding: gzip"}
	}
	request := rawRequest("GET", target, headers, nil)
	var raw, got [2][]byte
	for i, side := range p.Each() {
		from := time.Now().Unix()
		if raw[i], err = rawExchange(side.Daemon.Addr, request, 10*time.Second); err != nil {
			t.Fatalf("%s: %s: %v", target, side.Role, err)
		}
		got[i] = maskAnswer(raw[i], [2]int64{from, time.Now().Unix()})
	}
	if err := walkGuard(raw[0], served, want, st.ModTime(), gz); err != nil {
		return err.Error(), ""
	}
	if !bytes.Equal(got[0], got[1]) {
		return "", walkPrintable(firstDifference(got[0], got[1]))
	}
	return "", ""
}

// TestDashboardFiles (check `web.dashboard-files`) asks both agents, which serve the oracle's web directory, for
// every file in it, plain and with gzip, one connection each, and compares the whole answers byte for byte after
// maskAnswer (Date compared: the file's modification time). The oracle is held to the file first (walkGuard). A gzip
// answer is compared as sent: both agents deflate through the system zlib at C's settings.
func TestDashboardFiles(t *testing.T) {
	webDir := oracleWebDir(t)
	files := walkFiles(t, webDir)
	p := StartPair(t, daemon.Options{WebDir: webDir, PulseOff: true}, parentIdentity)
	var differ, wrong []string
	report := func(list *[]string, name, text string) {
		if *list = append(*list, name); len(*list) <= 10 {
			t.Errorf("%s: %s", name, text)
		}
	}
	for _, rel := range files {
		served := walkServed(webDir, rel)
		for _, gz := range []bool{false, true} {
			name := rel
			if gz {
				name += " (gzip)"
			}
			switch oracle, diff := walkFile(t, p, webDir, "/"+rel, served, gz); {
			case oracle != "":
				report(&wrong, name, "oracle: "+oracle)
			case diff != "":
				report(&differ, name, "answers differ\n"+diff)
			}
		}
	}
	for _, l := range []struct {
		what string
		list []string
	}{{"the oracle's answers are not their files", wrong}, {"the answers differ", differ}} {
		if len(l.list) > 10 {
			t.Errorf("%s for %d requests in all; past the first 10: %s", l.what, len(l.list),
				strings.Join(l.list[10:], ", "))
		}
	}
	t.Logf("%d files, %d requests per side", len(files), 2*len(files))
}

// dashKind is how the replay compares a row's answers.
type dashKind int

const (
	// dashStatic: a file of the web directory (walkFile); the oracle serves `file`.
	dashStatic dashKind = iota
	// dashExact: compareExact (maskAnswer, then the row's mask), the oracle judged by want, holds and guard.
	dashExact
	// dashMasked: as dashExact after the data masks (dataMask: the head's clock and expiry, the timings, the last
	// entries of now), the labels' order no contract (dataJudge).
	dashMasked
	// dashSame: a POST whose payload C does not read: the oracle answers it as its GET of the target; then judged
	// as a dashMasked row (dataSame).
	dashSame
	// dashV2: compareV2 with the row's family, status and guard.
	dashV2
)

// dashReq is one request of the dashboard replay: what the shipped dashboard sends, where it sends it from, the port
// commit that routes it in the Rust agent, and how the answers are compared and the oracle's judged.
type dashReq struct {
	name    string
	source  string // where the dashboard sends it: a line of WEB/index.html, or a bundle of WEB/v3 @ its byte offset
	commit  int    // the port commit that routes it in the Rust agent (0: routed now)
	kind    dashKind
	method  string // empty: GET
	target  string
	body    []byte
	headers []string
	file    string    // dashStatic: the file the oracle serves
	want    [2]string // dashExact, dashMasked, dashSame: the start and the end of the oracle's answer
	holds   []string  // dashExact, dashMasked, dashSame: parts the oracle's answer holds
	status  string    // dashV2: the oracle's status
	guard   func(Value) error
	fam     v2Family // dashV2
	// dashExact: what is hidden after maskAnswer, given the seconds the request was in flight (exactReq.mask)
	mask func(answer []byte, flight [2]int64) []byte
}

// judged tells whether the row names what the oracle's answer must be: its file, the start and the end of its
// answer, or its guard. A row without it would compare the two agents' answers whatever they are.
func (r dashReq) judged() bool {
	switch r.kind {
	case dashStatic:
		return r.file != ""
	case dashV2:
		return r.status != "" && r.guard != nil
	}
	return r.want[0] != "" && r.want[1] != ""
}

// v2 is the row's request for compareV2 and v2Exchange.
func (r dashReq) v2() v2Req {
	return v2Req{name: r.name, method: r.method, target: r.target, body: r.body, headers: r.headers, status: r.status,
		guard: r.guard}
}

// dashHolds is a guard: the answer, rendered compactly, holds each part.
func dashHolds(parts ...string) func(Value) error {
	return func(v Value) error {
		s := v.String()
		for _, part := range parts {
			if !strings.Contains(s, part) {
				return fmt.Errorf("no %s", part)
			}
		}
		return nil
	}
}

// dashJSON is the header the dashboard's request wrapper sends on every call, GET included (WEB/v3/app.*.js module
// 91130, @21386).
const dashJSON = "Content-Type: application/json"

// dashOK is the start of a 200 answer.
const dashOK = "HTTP/1.1 200 OK\r\n"

// dashboardRequests are the requests the shipped dashboard (7.128.1, the oracle's WEB) sends to a local agent, in
// the order a visit sends them, for the fixture of dashChild at base (health off: D224 F5). Each row's guard is a fact
// of C's answer seen in helper s6's probe pass and read in C's code (cited at the row); a guard that names the child or
// the fixture's data fails on an oracle without them. Not in the table:
//   - `GET /api/v3/claim`: the main layout calls it on mount (WEB/v3/4183.*.chunk.js), but D220 fork 2 A keeps the
//     route out of the Rust agent until the Cloud milestone, and C's answer writes a random session file
//     (api_v2_claim.c:17, :145, :181);
//   - `/api/v2|v3/function` data calls of a plugin's Function: the pair runs no plugin;
//   - the chunk groups the app loads and every other static file: TestDashboardFiles asks for each file;
//   - the Cloud's and third parties' URLs (app.netdata.cloud, the CDN, analytics).
func dashboardRequests(base int64) []dashReq {
	parent, child := parentIdentity.MachineGUID, childHost.MachineGUID
	win := fmt.Sprintf("after=%d&before=%d", base, base+60)
	weights := "/api/v3/weights?format=json&options=%s&contexts=*&scope_contexts=*&scope_nodes=*&nodes=*&instances=*" +
		"&dimensions=*&labels=*&group_by=context&aggregation=avg&method=%s&time_group=average&time_group_options=" +
		"&time_resampling=0&%s&baseline_after=%d&baseline_before=%d&timeout=180000"
	app := []string{dashJSON}
	// the child in a v2 answer's node list (api_v2_contexts.c:1472-1480, the nodes in index order)
	childNode := `{"mg":"` + child + `","nm":"` + childHost.Hostname + `","ni":1`
	// the fixture's context (api_v2_contexts.c:1199-1215)
	context := `"q.ctx":{"family":"fam","units":"units","priority":1000,`
	return []dashReq{
		{name: "page-root", source: "the page", kind: dashStatic, target: "/", file: "index.html"},
		{name: "page-v3", source: "the page", kind: dashStatic, target: "/v3/", file: "v3/index.html"},
		// the registry disabled still says hello and lists the agent's hosts (registry.c:178-226)
		{name: "registry-hello", source: "index.html:1999", commit: 1, kind: dashExact,
			target: "/api/v1/registry?action=hello", want: [2]string{dashOK, "}\n"},
			guard: dashHolds(`"action":"hello"`, `{"machine_guid":"`+child+`","hostname":"`+childHost.Hostname+`"}`)},
		// the node counts: the child is received (api_v2_contexts_agents.c:40-46)
		{name: "info", source: "index.html:1809", kind: dashV2, target: "/api/v3/info", status: "200",
			fam: dashInfoFamily, guard: dashHolds(`"mg":"`+parent+`"`,
				`"nodes":{"total":2,"receiving":1,"sending":0,"archived":0}`)},
		// an unknown token is no user (api_v3_me.c:10-35)
		{name: "me", source: "index.html:2111-2115", kind: dashExact, target: "/api/v3/me",
			headers: []string{"X-Netdata-Auth: Bearer null"}, want: [2]string{dashOK, `{"auth":"none",` +
				`"cloud_account_id":null,"client_name":"","access":["anonymous-data"],"user_role":"any"}`}},
		{name: "manifest", source: "index.html:1895", kind: dashStatic, target: "/v3/bundlesManifest.7.json",
			file: "v3/bundlesManifest.7.json"},
		{name: "route-fallback", source: "the router's basename, app @1952400", kind: dashStatic,
			target: "/v3/spaces/parity-parent/rooms/local/overview", file: "v3/index.html"},
		// no favicon at the top (web_client.c:405-407, :518-522)
		{name: "favicon", source: "the browser's convention", kind: dashExact, target: "/favicon.ico",
			want: [2]string{"HTTP/1.1 404 Not Found\r\n",
				"\r\n\r\nFile does not exist, or is not accessible: favicon.ico"}},
		// a file never saved is at version 1 (api_v3_settings.c:75-79, :108)
		{name: "settings-get", source: "app @1630935", commit: 7, kind: dashExact,
			target: setPath + "?file=default", headers: app, want: [2]string{dashOK, "\r\n\r\n" + setFresh}},
		{name: "nodes", source: "app @1840461", commit: 2, kind: dashV2, target: "/api/v3/nodes", headers: app,
			status: "200", fam: nodesFamily,
			guard: dashHolds(`{"mg":"`+parent+`","nm":"parity-parent","ni":0`, childNode)},
		{name: "contexts-all", source: "app @1841716", commit: 3, kind: dashV2,
			target: "/api/v2/contexts?scope_nodes=*", headers: app, status: "200", fam: contextsFamily,
			guard: dashHolds(childNode, context)},
		// the node ids of the room's nodes (`nd || mg`; unclaimed: the machine GUIDs), joined by a raw `|`
		{name: "contexts-ids", source: "app @1841716", commit: 3, kind: dashV2,
			target: "/api/v2/contexts?scope_nodes=" + parent + "|" + child, headers: app, status: "200",
			fam: contextsFamily, guard: dashHolds(childNode, context)},
		// the fixture's dimensions: `a` named alpha, `z` dropped by nonzero, `h` hidden (streamDataFixture)
		{name: "data-chart", source: "netdata.charts.*.js @~420587", kind: dashMasked,
			target: "/api/v3/data?points=60&format=json2&time_group=average&time_resampling=0&" + win +
				"&options=jsonwrap%7Cnonzero%7Cflip%7Cms%7Cjw-anomaly-rates%7Cminify&contexts=q.ctx" +
				"&scope_contexts=q.ctx&scope_nodes=*&nodes=*&instances=*&dimensions=*&labels=*" +
				"&group_by%5B0%5D=dimension&group_by_label%5B0%5D=&aggregation%5B0%5D=sum",
			want: [2]string{dashOK, "}}"}, holds: []string{`"result":{"labels":["time","alpha","b","inc","a"],`}},
		// the GET's defaults, not the payload's: grouped by dimension, not by node (api_v2_data.c:20-340)
		{name: "data-post", source: "app @191540", kind: dashSame, method: "POST", target: "/api/v3/data",
			headers: []string{dataDashboardType}, body: dataDashboardBody("q.ctx", time.Now().Unix()),
			want: [2]string{dashOK, "\n}\n"}, holds: []string{`"id":"q.ctx"`, `"grouped_by":["dimension"]`}},
		// health off: no alert, the hosts listed (api_v2_contexts_alerts.c:628; api_v2_contexts.c:1472-1480)
		{name: "alerts-raised", source: "app @140438", commit: 4, kind: dashV2,
			target: "/api/v3/alerts?options=summary,values,instances,minify&status=raised", headers: app,
			status: "200", fam: alertsV2Family([2]*healthNorm{}, nil), guard: dashHolds(childNode, `"alerts":[]`)},
		{name: "alerts-configs", source: "app @142618", commit: 4, kind: dashV2,
			target: "/api/v3/alerts?options=minify,summary", headers: app, status: "200",
			fam: alertsV2Family([2]*healthNorm{}, nil), guard: dashHolds(childNode, `"alerts":[]`)},
		{name: "alerts-named", source: "app @143067", commit: 4, kind: dashV2,
			target: "/api/v3/alerts?options=summary,values,instances,minify&alert=nope", headers: app, status: "200",
			fam: alertsV2Family([2]*healthNorm{}, nil), guard: dashHolds(childNode, `"alerts":[]`)},
		// health off: no transition; the request's `last` (api_v2_contexts_alert_transitions.c:393, :507)
		{name: "events-feed", source: "app @~1600400", commit: 5, kind: dashV2,
			target:  "/api/v2/alert_transitions?" + win + "&last=200&anchor_gi=&options=minify&scope_nodes=*",
			headers: app, status: "200", fam: alertsV2Family([2]*healthNorm{}, nil),
			guard: dashHolds(`"transitions":[]`, `"max_to_return":200,`)},
		// one transition asked, none found (as events-feed; `last` is 1 without the parameter,
		// web/api/v2/api_v2_contexts.c:70-71)
		{name: "transition", source: "app @141327", commit: 5, kind: dashV2,
			target:  "/api/v3/alert_transitions?options=minify&transition=00000000-0000-4000-8000-000000000001",
			headers: app, status: "200", fam: alertsV2Family([2]*healthNorm{}, nil),
			guard: dashHolds(`"transitions":[]`, `"max_to_return":1,`)},
		// api_v2_contexts_alert_config.c:133
		{name: "alert-config", source: "app @144364", kind: dashExact,
			target: "/api/v3/alert_config?options=minify&config=00000000-0000-4000-8000-000000000001", headers: app,
			want: [2]string{"HTTP/1.1 404 Not Found\r\n", "\r\n\r\nConfig is not found."}},
		// the agent's own Functions, on every host's list (api_v2_contexts.c:1482-1500)
		{name: "functions", source: "app @1594324", kind: dashV2, target: "/api/v3/functions?scope_nodes=*",
			headers: app, status: "200", fam: dashFunctionsFamily,
			guard: dashHolds(childNode, `{"name":"netdata-streaming",`)},
		// an anonymous client to a signed-in Function (nrpc-calls.c:547-552)
		{name: "function-info", source: "app @1597324", kind: dashExact,
			target: fmt.Sprintf("/host/%s/api/v3/function?function=netdata-streaming%%20info%%20after:%d%%20before:%d",
				parentIdentity.Hostname, base, base+60), headers: app,
			want: [2]string{"HTTP/1.1 412 Precondition Failed\r\n", "\r\n\r\n{\"status\":412,\"errorMessage\":" +
				"\"You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. " +
				"Sign-in on this dashboard, or access your Netdata via https://app.netdata.cloud.\"}"}},
		// progress.c:358-364
		{name: "progress", source: "app @1631795", kind: dashExact,
			target: "/api/v3/progress?transaction=00000000-0000-4000-8000-000000000002", headers: app,
			want: [2]string{"HTTP/1.1 404 Not Found\r\n",
				"\r\n\r\n{\"status\":404,\"message\":\"Transaction not found\"}"}},
		// no plugin: an empty tree; the agent's clock (dyncfg-tree.c:112-162; api_v2_contexts_agents.c:12-13, :25),
		// written NOW only where it is a second the request was in flight (exactNow)
		{name: "config-tree", source: "3738.*.chunk.js @279651", kind: dashExact,
			target: "/api/v1/config?timeout=120&action=tree&path=%2Fcollectors", headers: app, mask: exactNow,
			want:  [2]string{dashOK, `"nm":"parity-parent","now":"NOW"}}`},
			holds: []string{"\r\n\r\n{\"version\":1,\"tree\":{},"}},
		// the fixture's context, its 7 dimensions examined (weights.c:1484-1509)
		{name: "weights-anomalies", source: "app @190306, @1629832", commit: 10, kind: dashV2,
			target:  fmt.Sprintf(weights, "raw%7Cnull2zero%7Cminify%7Cnonzero%7Cunaligned", "anomaly-rate", win, 0, 0),
			headers: app, status: "200", fam: weightsFamily,
			guard: dashHolds(`"result":[{"id":"q.ctx",`, `"total_dimensions_count":7`)},
		{name: "weights-mc", source: "app @1629832", commit: 10, kind: dashV2,
			target: fmt.Sprintf(weights, "minify%7Cnonzero%7Cunaligned", "ks2",
				fmt.Sprintf("after=%d&before=%d", base+30, base+60), base, base+30),
			headers: app, status: "200", fam: weightsFamily,
			guard: dashHolds(`"result":[{"id":"q.ctx",`, `"total_dimensions_count":7`)},
		// the fixture's dimension name (api_v2_contexts.c:1064-1080)
		{name: "search", source: "app @1841007", commit: 6, kind: dashV2,
			target: "/api/v3/q?q=*alpha*&scope_nodes=*&nodes=*&scope_contexts=*&contexts=*", headers: app,
			status: "200", fam: searchFamily,
			guard: dashHolds(`"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}`)},
		// settings' PUT changes what the next GET answers: it comes last. The stored version matches, C saves version 2
		// (api_v3_settings.c:223-235, :285).
		{name: "settings-put", source: "app @1631343", commit: 7, kind: dashExact, method: "PUT",
			target: setPath + "?file=default", headers: app, body: setPutV1,
			want: [2]string{dashOK, "\r\n\r\n" + setOK}},
		{name: "settings-reget", source: "app @1630935", commit: 7, kind: dashExact,
			target: setPath + "?file=default", headers: app, want: [2]string{dashOK, "\r\n\r\n" + setV2}},
	}
}

// dashInfoFamily compares `/api/v3/info` as `api.v2-info` does on a fresh dbengine (infoV2Fresh): the empty tier's
// retention starts with its engine (helper s6's probe, C against C: `from` 1791318706 and 1791318709, `retention` 5
// and 2, `expected_retention` 655360 and 262144).
var dashInfoFamily = infoV2Family(infoV2Fresh...)

// dashFunctionsFamily compares `/api/v3/functions`: the v2 envelope, asked again until the contexts' version settles
// (fnContextsSettled's rule).
var dashFunctionsFamily = v2Family{masks: infoV2Volatile, settle: 15 * time.Second}

// TestDashboardReplay (check `web.dashboard-replay`) sends both agents, which serve the oracle's web directory and
// hold the dashChild fixture (health off), the requests of the shipped dashboard in its order (dashboardRequests),
// one subtest per row; a row a port commit has not routed yet fails alone.
func TestDashboardReplay(t *testing.T) {
	webDir := oracleWebDir(t)
	p := dashPair(t, daemon.Options{WebDir: webDir})
	base := dashBase()
	dashChild(t, p, base)
	rows := dashboardRequests(base)
	for _, r := range rows {
		if !r.judged() {
			t.Fatalf("harness: %s has no guard", r.name)
		}
	}
	for _, r := range rows {
		t.Run(r.name, func(t *testing.T) {
			defer func() {
				if t.Failed() && r.commit > 0 {
					t.Logf("%s (%s) is routed in the Rust agent by milestone 10's commit %d", r.name, r.source,
						r.commit)
				}
			}()
			switch r.kind {
			case dashStatic:
				switch oracle, diff := walkFile(t, p, webDir, r.target, r.file, false); {
				case oracle != "":
					t.Errorf("oracle: %s", oracle)
				case diff != "":
					t.Errorf("answers differ\n%s", diff)
				}
			case dashExact:
				compareExact(t, p, exactReq{name: r.name, method: r.method, target: r.target, body: r.body,
					headers: r.headers, want: r.want, holds: r.holds, guard: r.guard, mask: r.mask})
			case dashMasked:
				var got [2][]byte
				for i, side := range p.Each() {
					got[i] = dataAsk(t, side.Role, side.Daemon.Addr, v2Request(r.v2()))
				}
				dataJudge(t, got, r.want, r.holds, r.guard)
			case dashSame:
				dataSame(t, p, v2Request(v2Req{target: r.target}), v2Request(r.v2()), r.want, r.holds, r.guard)
			case dashV2:
				compareV2(t, p, r.v2(), r.fam)
			}
		})
	}
}
