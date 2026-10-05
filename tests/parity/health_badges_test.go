// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"cmp"
	"fmt"
	"net/http"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The badges' checks (check `health.badges`, M9 commit 10, D217; plan evidence/2026-10-05-plan-m9-commit10.md §5.3).
// `/api/v1/badge.svg` and `/api/v3/badge.svg` are one C function (web/api/v1/api_v1_badge/web_buffer_svg.c:874-1174)
// with two paths: a chart's value (one query) and an alert's status and value (no query). Both are asked with raw
// requests and compared whole: the status line, every header and the body's bytes.
//   - TestBadgeCorpus: the chart path, on a fixture child with health off (badgeFixture);
//   - TestHealthBadges: the alert path, on the health runner with live alerts;
//   - the route's access list is `web.acl`'s (acl_test.go: `features-denied/badge`, `badges-only`).
//
// The body holds no clock, no id and no path: it is compared as it is. Of the headers two are the clock's
// (badgeView): `Date` prints `T`, and `Expires` its distance from `Date` in seconds, which is the behaviour (0 for an
// answer without a refresh, the refresh's seconds for one that carries it); the transaction id is random and masked.
// On the chart path C reads the clock for `Expires` in the handler and for `Date` when it builds the header, a few
// microseconds later (web_buffer_svg.c:1148, web_client.c:939-945): across a second's edge the distance is one less
// (seen on C: 7 of 22,558 answers asked within 15 ms of a whole second said `T+4` for a refresh of 5). Two answers
// that differ in nothing but that second are asked again (badgeBoth), and must then be equal.
//
// Every row checks the oracle first (badgeAsk.guard): a badge's row wants `200 OK`, `image/svg+xml` and a whole
// `<svg>`, and the parts the row names (the value's text, a fill, a header), so two agents that answer the same
// wrong thing (a 404, an empty badge of a stale fixture) never pass.

// badgeAsk is one row of a badge check: a request, and what the oracle's answer to it must hold.
type badgeAsk struct {
	// name is the row's name (its subtest's)
	name string
	// target is the request's target as it goes on the wire: the path and the query
	target string
	// status, when set, is the status line the oracle must answer with in place of a badge's (the row's answer is no
	// badge: C's 400, a route that does not exist)
	status string
	// label and value, when set, are the two texts the oracle's badge must show, as the body holds them (escaped), and
	// fill the color of the value's box
	label, value, fill string
	// want are parts the oracle's view must hold (badgeView: the headers and the body)
	want []string
	// lacks are parts it must not hold
	lacks []string
}

// What C answers a request that reaches the handler without a chart (web_buffer_svg.c:964-968), and a path below the
// command (web_api.c:73-78).
var (
	badgeNoChart   = []string{"\r\nContent-Type: text/plain; charset=utf-8\r\n", badgeNoCache, "\r\nExpires: T+0\r\n", "\r\n\r\nNo chart id is given at the request."}
	badgeNoSubpath = []string{"\r\n\r\nAPI command 'badge.svg' does not support subpaths."}
)

const (
	badgeBad = "HTTP/1.1 400 Bad Request"
	// badgeOK and badgeType start a badge's answer; a badge is 200 whatever it shows (web_buffer_svg.c:976-993,
	// :1140-1145)
	badgeOK   = "HTTP/1.1 200 OK\r\n"
	badgeType = "\r\nContent-Type: image/svg+xml\r\n"
	// the cache headers C writes for an answer that is not to be cached, and for one that may be
	// (web_client.c:1035-1039)
	badgeNoCache = "\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\n"
	badgePublic  = "\r\nCache-Control: public\r\n"
)

var (
	badgeDateRe        = regexp.MustCompile(`(?m)^Date: ([^\r\n]*)`)
	badgeExpiresRe     = regexp.MustCompile(`(?m)^Expires: ([^\r\n]*)`)
	badgeTransactionRe = regexp.MustCompile(`(?m)^X-Transaction-ID: [0-9a-f]*`)
	badgeDistanceRe    = regexp.MustCompile(`(?m)^Expires: T([+-][0-9]+)\r?$`)
)

// badgeColors are the parts of a badge of the texts' own widths that hold the label box's fill and the fills of the
// label's and the value's text (web_buffer_svg.c:799, :829, :831).
func badgeColors(box, labelText, valueText string) []string {
	return []string{`fill="#` + box + `"/><rect class="bdge-rect-val" `,
		`y="14" fill="#` + labelText + `" clip-path="url(#lbl-rect)">`, `y="14" fill="#` + valueText + `" clip-path="url(#val-rect)">`}
}

// badgeRefresh are the headers of an answer that carries a refresh of n seconds: `Expires` n seconds after `Date`,
// the `Refresh` line after it, and the cache word (C's two lines for an answer that is not to be cached, `public` for
// one that may be). badgeNoRefresh are those of an answer without one.
func badgeRefresh(n int, public bool) []string {
	cache := badgeNoCache
	if public {
		cache = badgePublic
	}
	return []string{cache + fmt.Sprintf("Expires: T+%d\r\nRefresh: %d\r\n", n, n)}
}

var badgeNoRefresh = []string{badgeNoCache + "Expires: T+0\r\nContent-Length: "}

// badgeView is a raw answer as the badge checks compare it: whole, but in its head `Date` printed `T`, `Expires` its
// distance from `Date` in seconds (`T+0`, `T+5`, `T+86400`) and the transaction id masked. A value that is no HTTP
// date stays as it is, and so does an `Expires` beside a `Date` that is none.
func badgeView(raw []byte) []byte {
	head, body, found := bytes.Cut(raw, []byte("\r\n\r\n"))
	head = badgeTransactionRe.ReplaceAll(head, []byte("X-Transaction-ID: <masked>"))
	if d := badgeDateRe.FindSubmatch(head); d != nil {
		if date, err := http.ParseTime(string(d[1])); err == nil {
			head = badgeExpiresRe.ReplaceAllFunc(head, func(m []byte) []byte {
				expires, err := http.ParseTime(string(badgeExpiresRe.FindSubmatch(m)[1]))
				if err != nil {
					return m
				}
				return fmt.Appendf(nil, "Expires: T%+d", int64(expires.Sub(date)/time.Second))
			})
			head = badgeDateRe.ReplaceAll(head, []byte("Date: T"))
		}
	}
	if !found {
		return head
	}
	return append(append(head, "\r\n\r\n"...), body...)
}

// badgeEdge tells whether two views differ in nothing but one second of `Expires` (the clock read twice across a
// second's edge on one side).
func badgeEdge(a, b []byte) bool {
	da, db := badgeDistanceRe.FindSubmatch(a), badgeDistanceRe.FindSubmatch(b)
	if da == nil || db == nil || bytes.Equal(da[1], db[1]) {
		return false
	}
	var na, nb int64
	if _, err := fmt.Sscanf(string(da[1]), "%d", &na); err != nil {
		return false
	}
	if _, err := fmt.Sscanf(string(db[1]), "%d", &nb); err != nil {
		return false
	}
	if na-nb != 1 && nb-na != 1 {
		return false
	}
	same := []byte("Expires: T<edge>")
	return bytes.Equal(badgeDistanceRe.ReplaceAll(a, same), badgeDistanceRe.ReplaceAll(b, same))
}

// badgeEdgeReads is how often a row whose two answers are a second of `Expires` apart is asked again.
const badgeEdgeReads = 3

// badgeBoth sends the request to both sides and returns their views. While the two differ by one second of `Expires`
// alone both are asked again, badgeEdgeReads times at most; `edges` counts those reads.
func badgeBoth(p *Pair, from, target string) (views [2][]byte, edges int, err error) {
	for {
		for i, side := range p.Each() {
			raw, err := rawExchangeFrom(from, side.Daemon.Addr, []byte("GET "+target+" HTTP/1.1\r\n\r\n"), 2*time.Second)
			if err != nil {
				return views, edges, fmt.Errorf("%s: %v", side.Role, err)
			}
			views[i] = badgeView(raw)
		}
		if edges == badgeEdgeReads || !badgeEdge(views[0], views[1]) {
			return views, edges, nil
		}
		edges++
	}
}

// guard is the row's check of the oracle's view.
func (a badgeAsk) guard(view []byte) error {
	v := string(view)
	if a.status != "" {
		if !strings.HasPrefix(v, a.status+"\r\n") {
			return fmt.Errorf("answered %q, want %q", strings.SplitN(v, "\r\n", 2)[0], a.status)
		}
	} else {
		_, body, _ := strings.Cut(v, "\r\n\r\n")
		switch {
		case !strings.HasPrefix(v, badgeOK):
			return fmt.Errorf("answered %q, want a badge (200)", strings.SplitN(v, "\r\n", 2)[0])
		case !strings.Contains(v, badgeType):
			return fmt.Errorf("the answer is no image/svg+xml")
		case !strings.HasPrefix(body, "<svg ") || !strings.HasSuffix(body, "</svg>"):
			return fmt.Errorf("the body is no whole <svg>")
		}
	}
	for _, shown := range []struct {
		what, want string
		re         *regexp.Regexp
		group      int
	}{{"label", a.label, badgeLabelTextRe, 1}, {"value", a.value, badgeValueTextRe, 1}, {"value's fill", a.fill, badgeValueRectRe, 3}} {
		if shown.want == "" {
			continue
		}
		// both texts are written twice (the shadow and the text): each must be the one wanted
		all := shown.re.FindAllSubmatch(view, -1)
		if len(all) == 0 {
			return fmt.Errorf("the answer shows no %s, want %q", shown.what, shown.want)
		}
		for _, m := range all {
			if got := string(m[shown.group]); got != shown.want {
				return fmt.Errorf("the %s is %q, want %q", shown.what, got, shown.want)
			}
		}
	}
	for _, part := range a.want {
		if !strings.Contains(v, part) {
			return fmt.Errorf("the answer does not hold %q", part)
		}
	}
	for _, part := range a.lacks {
		if strings.Contains(v, part) {
			return fmt.Errorf("the answer holds %q", part)
		}
	}
	return nil
}

// The parts of a badge a passing row logs (badgeBrief).
var (
	badgeLabelTextRe = regexp.MustCompile(`<text class="bdge-lbl-lbl"[^>]*>([^<]*)</text>`)
	badgeValueTextRe = regexp.MustCompile(`<text class="bdge-lbl-val"[^>]*>([^<]*)</text>`)
	badgeLabelRectRe = regexp.MustCompile(`<rect class="bdge-rect-lbl" width="([^"]*)" height="[^"]*" fill="#([^"]*)"/>`)
	badgeValueRectRe = regexp.MustCompile(`<rect class="bdge-rect-val" x="[^"]*" width="([^"]*)" height="([^"]*)" fill="#([^"]*)"/>`)
	badgeTextFillRe  = regexp.MustCompile(`<text class="bdge-lbl-(?:lbl|val)"[^>]*y="[0-9-]*" fill="#([^"]*)" clip-path`)
	badgeCacheRe     = regexp.MustCompile(`(?m)^Cache-Control: ([^\r\n]*)`)
	badgeHeaderRe    = regexp.MustCompile(`(?m)^(Expires|Refresh): ([^\r\n]*)`)
)

// badgeBrief is a view as a passing row logs it: the status line, the cache word, `Expires` and `Refresh`, and of a
// badge its label and value texts with their boxes' widths and fills, the two text fills, the height and the
// layout (`fixed` with clip paths, `auto` with the script). An answer that is no badge logs its body's start.
func badgeBrief(view []byte) string {
	head, body, _ := bytes.Cut(view, []byte("\r\n\r\n"))
	status, _, _ := bytes.Cut(head, []byte("\r\n"))
	out := []string{string(status)}
	if m := badgeCacheRe.FindSubmatch(head); m != nil {
		out = append(out, string(m[1]))
	}
	for _, m := range badgeHeaderRe.FindAllSubmatch(head, -1) {
		out = append(out, string(m[1])+" "+string(m[2]))
	}
	label, value := badgeLabelTextRe.FindSubmatch(body), badgeValueTextRe.FindSubmatch(body)
	lr, vr := badgeLabelRectRe.FindSubmatch(body), badgeValueRectRe.FindSubmatch(body)
	if label == nil || value == nil || lr == nil || vr == nil {
		return strings.Join(append(out, fmt.Sprintf("body %q", truncateBytes(body[:min(len(body), 120)]))), " | ")
	}
	layout := "auto"
	if bytes.Contains(body, []byte("<clipPath ")) {
		layout = "fixed"
	}
	if bytes.Contains(body, []byte("<script ")) == (layout == "fixed") {
		layout += " (the script and the clip paths do not agree)"
	}
	var fills []string
	for _, m := range badgeTextFillRe.FindAllSubmatch(body, -1) {
		fills = append(fills, string(m[1]))
	}
	return strings.Join(append(out,
		fmt.Sprintf("label %q %s #%s", label[1], lr[1], lr[2]),
		fmt.Sprintf("value %q %s #%s", value[1], vr[1], vr[3]),
		fmt.Sprintf("text #%s", strings.Join(fills, " #")),
		fmt.Sprintf("height %s %s, %d bytes", vr[2], layout, len(body))), " | ")
}

// compareBadge is one row: both sides' views (badgeBoth, from the local address `from`; empty: any), the oracle's
// held to the row's guard, then the two compared byte for byte. A passing row logs the oracle's brief.
func compareBadge(t *testing.T, p *Pair, from string, a badgeAsk) {
	t.Helper()
	views, edges, err := badgeBoth(p, from, a.target)
	if err != nil {
		t.Fatal(err)
	}
	if edges > 0 {
		t.Logf("asked %d more time(s): the two answers were one second of Expires apart", edges)
	}
	if err := a.guard(views[0]); err != nil {
		t.Fatalf("oracle: GET %s: %v\n%s", a.target, err, truncateBytes(views[0]))
	}
	if !bytes.Equal(views[0], views[1]) {
		t.Fatalf("GET %s: responses differ\n%s", a.target, firstDifference(views[0], views[1]))
	}
	t.Logf("GET %s\n%s", a.target, badgeBrief(views[0]))
}

// The fixture child's charts (badgeFixture).
const (
	badgeChart     = "b.val"
	badgeChartName = "b.b_val_name"
	// badgeEvery is the fixture charts' update every: C shows no value for a chart whose last point is older than
	// three of them (web_buffer_svg.c:1130: `[db] gap when lost iterations above` plus 2,
	// daemon/config/netdata-conf-db.c:475-480), so a chart of a static fixture that is collected every second would
	// only ever show `-`. With 300 s the fixture is fresh for 900 s after its last point.
	badgeEvery = 300
	// badgeFresh is how long after the fixture's last point the check may still run (b.half's last point is one
	// update every older, and stale after two more): a row that would be asked later is the harness's failure, not a
	// verdict
	badgeFresh = 2*badgeEvery - 10
)

// badgeFixture sends the badge check's child: charts of one context whose last point is at `base`, a multiple of
// badgeEvery not after now.
//   - b.val, named b_val_name, units `things`, update every 300 s, three points (base-600, base-300, base): `a`
//     (named alpha) 1000.5, 1100.25, 1234.5; `neg` -40.25, -41.5, -42.25; `zero` 0; `gap` 5, 7 and no value at the
//     last point; `h`, hidden, 1, 2, 3. The last point's visible values sum to 1192.25;
//   - b.pct (units `%`, 55.5), b.sec (units `seconds`, 90061), b.none (no units, 0.125), b.amp (units `a&b<c>d`, 3):
//     one point each at base, for the defaults of the units;
//   - b.half: 10, 20, 40 at base-900, base-600, base-300: its last point is one update every before b.val's, so
//     of the two charts' last points one is a multiple of 600 s, whatever the clock;
//   - b.old: update every 1 s, three points ending at base-600: stale;
//   - b.never: defined, never collected.
func badgeFixture(t *testing.T, conn *stream.Conn, base int64) {
	t.Helper()
	chart := func(id, name, units string, every int) {
		conn.Linef("CHART '%s' '%s' 'title' '%s' 'fam' 'b.ctx' line 1000 %d '' fixture-pusher corpus", id, name, units, every)
	}
	chart(badgeChart, "b_val_name", "things", badgeEvery)
	conn.Linef("DIMENSION 'a' 'alpha' absolute 1 1 ''")
	conn.Linef("DIMENSION 'neg' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'zero' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'gap' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'h' '' absolute 1 1 'hidden'")
	for _, c := range []struct {
		id, units string
		every     int
	}{{"b.pct", "%", badgeEvery}, {"b.sec", "seconds", badgeEvery}, {"b.none", "", badgeEvery}, {"b.amp", "a&b<c>d", badgeEvery},
		{"b.half", "things", badgeEvery}, {"b.old", "things", 1}, {"b.never", "things", badgeEvery}} {
		chart(c.id, "", c.units, c.every)
		conn.Linef("DIMENSION 'v' '' absolute 1 1 ''")
	}
	for k, row := range []struct{ a, neg, gap string }{{"1000.5", "-40.25", "5"}, {"1100.25", "-41.5", "7"}, {"1234.5", "-42.25", ""}} {
		conn.Linef("BEGIN2 '%s' %d %d #", badgeChart, badgeEvery, base-int64(2-k)*badgeEvery)
		conn.Linef("SET2 'a' %d %s A", 1000+k, row.a)
		conn.Linef("SET2 'neg' %d %s A", -40-k, row.neg)
		conn.Linef("SET2 'zero' 0 0 A")
		if row.gap == "" {
			conn.Linef("SET2 'gap' 0 0 E")
		} else {
			conn.Linef("SET2 'gap' %s %s A", row.gap, row.gap)
		}
		conn.Linef("SET2 'h' %d %d A", k+1, k+1)
		conn.Linef("END2")
	}
	for _, c := range []struct{ id, collected, value string }{{"b.pct", "55", "55.5"}, {"b.sec", "90061", "90061"},
		{"b.none", "0", "0.125"}, {"b.amp", "3", "3"}} {
		conn.Linef("BEGIN2 '%s' %d %d #", c.id, badgeEvery, base)
		conn.Linef("SET2 'v' %s %s A", c.collected, c.value)
		conn.Linef("END2")
	}
	for k, v := range []int{10, 20, 40} {
		conn.Linef("BEGIN2 'b.half' %d %d #", badgeEvery, base-int64(3-k)*badgeEvery)
		conn.Linef("SET2 'v' %d %d A", v, v)
		conn.Linef("END2")
	}
	for k := int64(2); k >= 0; k-- {
		conn.Linef("BEGIN2 'b.old' 1 %d #", base-2*badgeEvery-k)
		conn.Linef("SET2 'v' 9 9 A")
		conn.Linef("END2")
	}
	if err := conn.Flush(); err != nil {
		t.Fatal(err)
	}
}

// badgeEntries reads a chart's update every and its first and last entry from `/api/v1/chart` of the child.
func badgeEntries(d *daemon.Daemon, chart string) string {
	r, err := Get(d, "/host/"+childHost.Hostname+"/api/v1/chart?chart="+chart, nil)
	if err != nil {
		return err.Error()
	}
	doc, err := ParseJSON(r.Body)
	if err != nil || r.Status != http.StatusOK {
		return fmt.Sprintf("HTTP %d %s", r.Status, truncateBytes(r.Body))
	}
	out := []string{}
	for _, key := range []string{"name", "units", "update_every", "first_entry", "last_entry"} {
		for _, m := range doc.Members {
			if m.Key == key {
				out = append(out, key+"="+m.Value.String())
			}
		}
	}
	return strings.Join(out, " ")
}

// badgeAsks are TestBadgeCorpus' rows. `base` is the fixture's last second.
func badgeAsks(base int64) []badgeAsk {
	host := "/host/" + childHost.Hostname
	v1 := host + "/api/v1/badge.svg"
	q := func(query string) string { return v1 + "?" + query }
	c := func(query string) string { return v1 + "?chart=" + badgeChart + "&" + query }
	abs := fmt.Sprintf("after=%d&before=%d", base-2*badgeEvery, base)
	// the fixture's three points in one window, whatever the clock's phase
	const whole = "after=-900&options=unaligned"
	long := func(n int, s string) string { return strings.Repeat(s, n) }
	// a badge of fixed widths, with more parts it must hold; and one of the texts' own widths, at the default size
	fixed := func(a badgeAsk, parts ...string) badgeAsk {
		a.want = append(append(a.want, `<clipPath id="lbl-rect">`, `<clipPath id="val-rect">`), parts...)
		a.lacks = append(a.lacks, "<script ")
		return a
	}
	auto := func(a badgeAsk) badgeAsk {
		a.want, a.lacks = append(a.want, `width="166.55" height="20.00">`, "<script "), append(a.lacks, "<clipPath")
		return a
	}
	rows := []badgeAsk{
		// the chart, and what is no badge (web_buffer_svg.c:908-981, web_api.c:49-98)
		{name: "by-id", target: q("chart=" + badgeChart), label: badgeChartName, value: "1192 things", fill: "4c1",
			want: append(badgeColors("555", "fff", "fff"), badgeNoRefresh...), lacks: []string{"<clipPath", "\r\nRefresh: "}},
		{name: "by-name", target: q("chart=" + badgeChartName), label: badgeChartName, value: "1192 things"},
		{name: "by-short-name", target: q("chart=b_val_name"), label: "chart not found", value: "-", fill: "999"},
		{name: "unknown-chart", target: q("chart=nope"), label: "chart not found", value: "-", fill: "999",
			want: append([]string{`width="111.15" height="20.00"`}, badgeNoRefresh...)},
		// of a request whose chart is unknown only the scale counts (:976-981)
		{name: "unknown-chart-all-given", target: q("chart=nope&scale=200&label=x&units=u&label_color=blue&value_color=red&text_color_lbl=red" +
			"&text_color_val=red&fixed_width_lbl=50&fixed_width_val=60&precision=3&refresh=5&options=display-absolute"),
			label: "chart not found", value: "-", fill: "999", lacks: []string{"<clipPath", "\r\nRefresh: "},
			want: append([]string{`width="222.29" height="40.00"`, `fill="#555"/><rect class="bdge-rect-val"`}, badgeNoRefresh...)},
		{name: "no-chart", target: q("label=x&refresh=5"), status: badgeBad, want: badgeNoChart},
		{name: "no-query", target: v1, status: badgeBad, want: badgeNoChart},
		{name: "empty-query", target: v1 + "?", status: badgeBad, want: badgeNoChart},
		{name: "chart-empty", target: q("chart=&label=x"), status: badgeBad, want: badgeNoChart},
		{name: "chart-twice", target: q("chart=nope&chart=" + badgeChart), value: "1192 things"},
		{name: "chart-twice-unknown-last", target: q("chart=" + badgeChart + "&chart=nope"), label: "chart not found", value: "-", fill: "999"},
		{name: "chart-upper-case", target: q("Chart=" + badgeChart), status: badgeBad, want: badgeNoChart},
		{name: "empty-pairs", target: q("&&chart=" + badgeChart + "&&=x&label&units=&&"), value: "1192 things"},
		{name: "equals-twice", target: q("chart==" + badgeChart), label: "chart not found", value: "-", fill: "999"},
		{name: "equals-first", target: q("==chart=" + badgeChart + "&=label=x"), label: "x", value: "1192 things"},
		{name: "equals-in-value", target: c("label=a=b==c"), label: "a=b==c", value: "1192 things"},
		{name: "unknown-names", target: c("tier=0&gtime=60&timeout=1&format=json&_=123&points_=5&LABEL=x"), value: "1192 things"},
		{name: "encoded-separators", target: q("chart=" + badgeChart + "%26label%3Dx%26units%3Dy"), label: "x", value: "1192 y"},
		{name: "plus-is-a-space", target: c("label=a+b&units=c+d"), label: "a b", value: "1192 c d"},
		{name: "localhost", target: "/api/v1/badge.svg?chart=" + badgeChart, label: "chart not found", value: "-", fill: "999"},
		{name: "host-by-guid", target: "/host/" + childHost.MachineGUID + "/api/v1/badge.svg?chart=" + badgeChart, value: "1192 things"},
		{name: "v2", target: host + "/api/v2/badge.svg?chart=" + badgeChart, status: "HTTP/1.1 404 Not Found",
			want: []string{"\r\n\r\nUnsupported API command: badge.svg"}},
		{name: "subpath", target: v1 + "/x?chart=" + badgeChart, status: badgeBad, want: badgeNoSubpath},
		{name: "unknown-command", target: v1 + "x?chart=" + badgeChart, status: "HTTP/1.1 404 Not Found",
			want: []string{"\r\n\r\nUnsupported API command: badge.svgx"}},

		// the other charts' defaults: the units' text beside the value (:253-312, :331-362)
		{name: "units-percent-sign", target: q("chart=b.pct"), label: "b.pct", value: "55.5%"},
		{name: "units-seconds", target: q("chart=b.sec"), label: "b.sec", value: "1 day 01:01:01"},
		{name: "units-none", target: q("chart=b.none"), label: "b.none", value: "0.12"},
		{name: "units-escaped", target: q("chart=b.amp"), label: "b.amp", value: "3 a&amp;b&lt;c&gt;d"},
		// the chart whose points are half a window of 600 s off b.val's: of the two, one ends a window of 600 s
		{name: "half-default", target: q("chart=b.half"), label: "b.half", value: "40 things"},
		{name: "half-after-600", target: q("chart=b.half&after=-600"), label: "b.half"},
		{name: "half-absolute", target: q(fmt.Sprintf("chart=b.half&after=%d&before=%d", base-3*badgeEvery, base-badgeEvery)), label: "b.half"},

		// the dimensions: four names of one parameter, each value added behind a `|` (:922-928); the label is the text
		// without its first `|` (:1032-1035)
		{name: "dim-a", target: c("dimension=a"), label: "a", value: "1234 things"},
		{name: "dim-neg", target: c("dim=neg"), label: "neg", value: "-42.2 things"},
		{name: "dim-zero", target: c("dimensions=zero"), label: "zero", value: "0 things"},
		{name: "dim-gap", target: c("dims=gap"), label: "gap", value: "-", fill: "999"},
		{name: "dim-hidden", target: c("dim=h"), label: "h", value: "3 things"},
		{name: "dim-two", target: c("dim=a&dims=neg"), label: "a|neg", value: "1192 things"},
		{name: "dim-four-names", target: c("dimension=a&dim=neg&dimensions=zero&dims=gap"), label: "a|neg|zero|gap", value: "1192 things"},
		{name: "dim-list", target: c("dimensions=a|zero"), label: "a|zero", value: "1234 things"},
		{name: "dim-list-encoded", target: c("dimensions=a%7Czero,neg"), label: "a|zero,neg", value: "1192 things"},
		{name: "dim-none-matches", target: c("dim=nope"), label: "nope", value: "-", fill: "999"},
		{name: "dim-negative-pattern", target: c("dims=!a|*"), label: "!a|*", value: "-39.2 things"},
		{name: "dim-by-name", target: c("dim=alpha"), label: "alpha", value: "1234 things"},
		{name: "dim-match-names", target: c("dim=alpha&options=match-names"), label: "alpha", value: "1234 things"},
		{name: "dim-match-ids", target: c("dim=alpha&options=match-ids"), label: "alpha", value: "-", fill: "999"},
		{name: "dim-with-label", target: c("dim=a&label=mine"), label: "mine", value: "1234 things"},

		// the window, the points and the grouping (:929-935, :996-1001). A window of more than one update every is
		// aligned to its own length unless the request says `unaligned`: what such a row shows follows the phase of
		// the fixture's last second, so those rows name no value (the two sides are compared all the same)
		{name: "window-absolute", target: c(abs), label: badgeChartName},
		{name: "window-absolute-unaligned", target: c(abs + "&options=unaligned"), value: "1076 things"},
		{name: "window-after-600-unaligned", target: c("after=-600&options=unaligned"), value: "1132 things"},
		{name: "window-whole-unaligned", target: c(whole), value: "1076 things"},
		{name: "window-after-alone", target: c("after=-600"), label: badgeChartName},
		{name: "window-after-900", target: c("after=-900"), label: badgeChartName},
		{name: "window-before-relative", target: c("after=-300&before=-300"), value: "1066 things"},
		{name: "window-first-point", target: c(fmt.Sprintf("after=%d&before=%d", base-3*badgeEvery, base-2*badgeEvery)), value: "965.2 things"},
		{name: "window-past-the-data", target: c(fmt.Sprintf("after=%d&before=%d", base+10*badgeEvery, base+11*badgeEvery)), value: "-", fill: "999"},
		{name: "window-after-past-before", target: c(fmt.Sprintf("after=%d&before=%d", base, base-2*badgeEvery)), label: badgeChartName},
		{name: "window-text", target: c("after=abc&before=xyz"), label: badgeChartName},
		{name: "points-0", target: c("after=-900&points=0"), value: "1192 things"},
		{name: "points-3", target: c("after=-900&points=3"), value: "1192 things"},
		{name: "points-3-reversed", target: c("after=-900&points=3&options=reversed"), value: "965.2 things"},
		{name: "points-negative", target: c("after=-900&points=-1"), value: "1192 things"},
		{name: "points-text", target: c("after=-900&points=abc"), value: "1192 things"},
		{name: "group-max", target: c(whole + "&group=max"), value: "1199 things"},
		{name: "group-min", target: c(whole + "&group=min"), value: "965.2 things"},
		{name: "group-sum", target: c(whole + "&group=sum"), value: "3223 things"},
		{name: "group-median", target: c(whole + "&group=median"), value: "1065 things"},
		{name: "group-unknown", target: c(whole + "&group=nope"), value: "1076 things"},
		{name: "group-twice", target: c(whole + "&group=max&group=min"), value: "965.2 things"},
		{name: "group-unknown-last", target: c(whole + "&group=max&group=nope"), value: "1076 things"},
		{name: "group-countif", target: c(whole + "&dim=a&group=countif&group_options=>1050"), label: "a", value: "66.7 things"},
		{name: "group-countif-encoded", target: c(whole + "&dim=a&group=countif&group_options=%3E1200"), label: "a", value: "33.3 things"},
		{name: "group-percentile", target: c(whole + "&dim=a&group=percentile&group_options=50"), label: "a", value: "1025 things"},
		{name: "group-options-alone", target: c(whole + "&dim=a&group_options=>1050"), label: "a", value: "1112 things"},
		// no grouping of this C has the name: the average, whatever the options say
		{name: "group-percentage-of-time", target: c(whole + "&group=percentage-of-time&group_options=not-a-condition"), value: "1076 things"},
		{name: "group-max-aligned", target: c("after=-900&group=max"), label: badgeChartName},

		// the options: the query's, and `display-absolute`, which is the badge's own (:762-763, :936-938, :1047-1048)
		{name: "options-percentage", target: c("options=percentage"), value: "99.7%"},
		{name: "options-percentage-dim", target: c("dim=a&options=percentage"), label: "a", value: "103.3%"},
		{name: "options-percentage-units", target: c("options=percentage&units=share"), value: "99.7 share"},
		{name: "options-abs", target: c("options=abs"), value: "1277 things"},
		{name: "options-absolute-dim", target: c("dim=neg&options=absolute"), label: "neg", value: "42.2 things"},
		{name: "options-display-absolute", target: c("dim=neg&options=display-absolute&value_color=red<0|green"),
			label: "neg", value: "42.2 things", fill: "e05d44"},
		{name: "options-display_absolute", target: c("dim=neg&options=display_absolute"), label: "neg", value: "42.2 things"},
		{name: "options-unaligned", target: c("after=-450&options=unaligned"), value: "1132 things"},
		{name: "options-null2zero", target: c("dim=gap&options=null2zero"), label: "gap", value: "-", fill: "999"},
		{name: "options-min2max", target: c("options=min2max"), value: "1277 things"},
		{name: "options-average", target: c("options=average"), value: "397.4 things"},
		{name: "options-min", target: c("options=min"), value: "-42.2 things"},
		{name: "options-max", target: c("options=max"), value: "1234 things"},
		{name: "options-unknown", target: c("options=nope"), value: "1192 things"},
		{name: "options-repeated", target: c("options=abs&options=max"), value: "1234 things"},
		{name: "options-list", target: c("options=abs,min|nope max"), value: "0 things"},
		{name: "options-text-formats", target: c("options=jsonwrap,ms,objectrows,raw,debug,minify,rfc3339"), value: "1192 things"},

		// the label and the units (:939-940, :1023-1051), the XML escape and its limits (:178-251, :692-698), the
		// widths of the texts as given (:147-176, :765-768)
		{name: "label-spaces", target: c("label=my%20own%20label"), label: "my own label", value: "1192 things"},
		{name: "label-escapes", target: c("label=%3C%3E%22%27%5C%26x"), label: "&lt;&gt;&quot;&apos;/", value: "1192 things"},
		// a character of several bytes counts 11 whatever it is: three of them and the padding are 41.80 wide (:156-162)
		{name: "label-utf8", target: c("label=%CE%B1%CE%B2%CE%B3%20%E2%82%AC%20%F0%9F%98%80"), label: "\u03b1\u03b2\u03b3 \u20ac \U0001f600", value: "1192 things"},
		{name: "label-utf8-raw", target: c("label=\xce\xb1\xce\xb2\xce\xb3"), label: "\u03b1\u03b2\u03b3", value: "1192 things",
			want: []string{`<rect class="bdge-rect-lbl" width="41.80" `}},
		// the label's box is as wide as the whole label, the text is cut at 200 bytes: before an entity that does not
		// fit with a byte to spare, and inside a character (:178-251, :765-771)
		{name: "label-250", target: c("label=" + long(250, "W")), label: long(200, "W"), value: "1192 things",
			want: []string{`<rect class="bdge-rect-lbl" width="2777.92" `}},
		{name: "label-entity-fits", target: c("label=" + long(195, "a") + "%3C%3C"), label: long(195, "a") + "&lt;", value: "1192 things"},
		{name: "label-entity-would-fill", target: c("label=" + long(196, "a") + "%3Cx"), label: long(196, "a"), value: "1192 things"},
		{name: "label-quote-does-not-fit", target: c("label=" + long(194, "a") + "%22x"), label: long(194, "a"), value: "1192 things"},
		{name: "label-cut-in-a-character", target: c("label=" + long(199, "a") + "%CE%B1b"), label: long(199, "a") + "\xce", value: "1192 things"},
		// `%26` is a separator before it is a byte of the label (web_api.c:92-97)
		{name: "label-encoded-ampersand", target: c("label=a%26b%26units%3Dx"), label: "a", value: "1192 x"},
		{name: "label-control-encoded", target: c("label=a%01b"), label: "a", value: "1192 things"},
		{name: "label-control-raw", target: c("label=a\x01b\x7fc"), label: "a\x01b\x7fc", value: "1192 things",
			want: []string{`<rect class="bdge-rect-lbl" width="28.59" `}},
		{name: "label-not-utf8", target: c("label=a\xffb"), label: "a\xffb", value: "1192 things"},
		{name: "label-encoded-nul", target: c("label=a%00b"), label: "a", value: "1192 things"},
		{name: "label-empty", target: c("label=&units="), value: "1192 things"},
		{name: "units-given", target: c("units=KiB/s"), value: "1192 KiB/s"},
		{name: "units-symbol-first", target: c("units=%2Fs"), value: "1192/s"},
		{name: "units-digit-first", target: c("units=5x"), value: "1192 5x"},
		{name: "units-backslash", target: c("units=%5C"), value: "1192/"},
		{name: "units-percent", target: c("units=%25"), value: "1192%"},
		{name: "units-empty-word", target: c("units=empty"), value: "1192"},
		{name: "units-null-word", target: c("units=null"), value: "1192"},
		{name: "units-percentage-word", target: c("units=percentage"), value: "1192%"},
		{name: "units-percent-word", target: c("dim=neg&units=percent"), label: "neg", value: "-42.2%"},
		{name: "units-pcent-word", target: c("dim=zero&units=pcent"), label: "zero", value: "0%"},
		// the value's text is cut at 99 bytes, then escaped into 100 (:304-309, :692-693, :772)
		{name: "units-120-bytes", target: c("units=" + long(120, "u")), value: "1192 " + long(94, "u")},
		{name: "units-escaped-at-the-cut", target: c("units=" + long(90, "u") + "%3C%3C%3C"), value: "1192 " + long(90, "u") + "&lt;"},
		{name: "units-seconds-of-a", target: c("dim=a&units=seconds"), label: "a", value: "00:20:34"},
		{name: "units-seconds-days", target: c("dim=a&units=seconds&multiply=100"), label: "a", value: "1 day 10:17:30"},
		{name: "units-seconds-ago", target: c("dim=a&units=seconds+ago"), label: "a", value: "00:20:34 ago"},
		{name: "units-seconds-zero", target: c("dim=zero&units=seconds"), label: "zero", value: "now"},
		{name: "units-seconds-negative", target: c("dim=neg&units=seconds+ago"), label: "neg", value: "undefined"},
		{name: "units-minutes", target: c("dim=a&units=minutes"), label: "a", value: "20h 34m"},
		{name: "units-minutes-days", target: c("dim=a&units=minutes+ago&multiply=10"), label: "a", value: "8d 13h 45m ago"},
		{name: "units-minutes-zero", target: c("dim=zero&units=minutes"), label: "zero", value: "now"},
		{name: "units-hours", target: c("dim=a&units=hours&divide=100"), label: "a", value: "12h"},
		{name: "units-hours-days", target: c("dim=a&units=hours+ago"), label: "a", value: "51d 10h ago"},
		{name: "units-hours-negative", target: c("dim=neg&units=hours"), label: "neg", value: "undefined"},
		{name: "units-on-off", target: c("dim=a&units=on/off"), label: "a", value: "on"},
		{name: "units-on-off-dash", target: c("dim=zero&units=on-off"), label: "zero", value: "off"},
		{name: "units-onoff", target: c("dim=neg&units=onoff"), label: "neg", value: "on"},
		{name: "units-up-down", target: c("dim=zero&units=up/down"), label: "zero", value: "down"},
		{name: "units-up-down-dash", target: c("dim=a&units=up-down"), label: "a", value: "up"},
		{name: "units-updown", target: c("dim=zero&units=updown"), label: "zero", value: "down"},
		{name: "units-ok-error", target: c("dim=a&units=ok/error"), label: "a", value: "ok"},
		{name: "units-ok-error-dash", target: c("dim=zero&units=ok-error"), label: "zero", value: "error"},
		{name: "units-okerror", target: c("dim=zero&units=okerror"), label: "zero", value: "error"},
		{name: "units-ok-failed", target: c("dim=zero&units=ok/failed"), label: "zero", value: "failed"},
		{name: "units-ok-failed-dash", target: c("dim=a&units=ok-failed"), label: "a", value: "ok"},
		{name: "units-okfailed", target: c("dim=zero&units=okfailed"), label: "zero", value: "failed"},
		{name: "units-other-case", target: c("dim=a&units=Seconds"), label: "a", value: "1234 Seconds"},

		// multiply and divide: C's str2l, and 0 is 1 (:996-1004, :1155)
		{name: "multiply-divide", target: c("multiply=1024&divide=3"), value: "406955 things"},
		{name: "multiply-0", target: c("multiply=0"), value: "1192 things"},
		{name: "divide-0", target: c("divide=0"), value: "1192 things"},
		{name: "multiply-negative", target: c("multiply=-2"), value: "-2384 things"},
		{name: "multiply-text", target: c("multiply=abc&divide=xyz"), value: "1192 things"},
		{name: "multiply-fraction", target: c("multiply=1.5&divide=2.9"), value: "596.1 things"},
		{name: "multiply-19-digits", target: c("multiply=1000000000000000000"), value: "1192249999999999934464 things"},
		{name: "divide-19-digits", target: c("divide=1000000000000000000"), value: "0 things"},
		{name: "multiply-spaces-plus", target: c("multiply=%20%2B3&divide=%20-2"), value: "-1788 things"},

		// the precision (:262-309, :1001)
		{name: "precision-0", target: c("dim=a&precision=0"), label: "a", value: "1234 things"},
		{name: "precision-1", target: c("dim=a&precision=1"), label: "a", value: "1234.5 things"},
		{name: "precision-2", target: c("dim=neg&precision=2"), label: "neg", value: "-42.25 things"},
		{name: "precision-7", target: c("dim=a&precision=7"), label: "a", value: "1234.5000000 things"},
		{name: "precision-15", target: c("dim=a&precision=15"), label: "a", value: "1234.500000000000000 things"},
		{name: "precision-50", target: c("dim=a&divide=7&precision=50"),
			label: "a", value: "176.35714285714286120310134720057249069213867187500000 things"},
		{name: "precision-51", target: c("dim=a&divide=7&precision=51"),
			label: "a", value: "176.35714285714286120310134720057249069213867187500000 things"},
		{name: "precision-100", target: c("dim=a&divide=7&precision=100&units=x"),
			label: "a", value: "176.35714285714286120310134720057249069213867187500000 x"},
		{name: "precision-minus-1", target: c("dim=a&divide=7&precision=-1"), label: "a", value: "176.4 things"},
		{name: "precision-minus-5", target: c("dim=a&divide=7&precision=-5"), label: "a", value: "176.4 things"},
		{name: "precision-text", target: c("dim=a&divide=7&precision=abc"), label: "a", value: "176 things"},
		{name: "precision-plus", target: c("dim=a&divide=7&precision=%2B3"), label: "a", value: "176.357 things"},
		{name: "precision-space", target: c("dim=a&divide=7&precision=%203"), label: "a", value: "176.357 things"},
		{name: "precision-of-a-null", target: c("dim=gap&precision=3"), label: "gap", value: "-", fill: "999"},
		{name: "precision-of-a-time", target: c("dim=a&units=seconds&precision=3"), label: "a", value: "00:20:34"},
		{name: "precision-small", target: c("dim=a&divide=100000000&precision=-1"), label: "a", value: "0.0000123 things"},

		// the value's color (:567-689, :759-762): one expression over a positive, a negative, a zero and a null value
		{name: "color-entries-positive", target: c("dim=a&value_color=red<0|yellow=0|green>0|blue:null"),
			label: "a", value: "1234 things", fill: "97CA00"},
		{name: "color-entries-negative", target: c("dim=neg&value_color=red<0|yellow=0|green>0|blue:null"),
			label: "neg", value: "-42.2 things", fill: "e05d44"},
		{name: "color-entries-zero", target: c("dim=zero&value_color=red<0|yellow=0|green>0|blue:null"),
			label: "zero", value: "0 things", fill: "dfb317"},
		{name: "color-entries-null", target: c("dim=gap&value_color=red<0|yellow=0|green>0|blue:null"), label: "gap", value: "-", fill: "007ec6"},
		// each operator and its other spellings
		{name: "color-le", target: c("dim=zero&value_color=red<=0|green"), label: "zero", value: "0 things", fill: "e05d44"},
		{name: "color-ge", target: c("dim=zero&value_color=red>=0|green"), label: "zero", value: "0 things", fill: "e05d44"},
		{name: "color-gt-not", target: c("dim=zero&value_color=red>0|green"), label: "zero", value: "0 things", fill: "97CA00"},
		{name: "color-lt-not", target: c("dim=zero&value_color=red<0|green"), label: "zero", value: "0 things", fill: "97CA00"},
		{name: "color-colon", target: c("dim=zero&value_color=red:0|green"), label: "zero", value: "0 things", fill: "e05d44"},
		{name: "color-equals", target: c("dim=zero&value_color=red=0|green"), label: "zero", value: "0 things", fill: "e05d44"},
		{name: "color-ne", target: c("dim=a&value_color=red!=0|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-ne-not", target: c("dim=zero&value_color=red!=0|green"), label: "zero", value: "0 things", fill: "97CA00"},
		{name: "color-bang", target: c("dim=a&value_color=red!0|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-ltgt", target: c("dim=a&value_color=red<>0|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-ltgt-not", target: c("dim=zero&value_color=red<>0|green"), label: "zero", value: "0 things", fill: "97CA00"},
		{name: "color-brace-open", target: c("dim=neg&value_color=red{0|green"), label: "neg", value: "-42.2 things", fill: "e05d44"},
		{name: "color-paren-open", target: c("dim=neg&value_color=red(0|green"), label: "neg", value: "-42.2 things", fill: "e05d44"},
		{name: "color-brace-close", target: c("dim=a&value_color=red}0|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-paren-close", target: c("dim=a&value_color=red)0|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-lt-paren", target: c("dim=a&value_color=red<)0|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-lt-brace", target: c("dim=a&value_color=red<}0|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-brace-equals", target: c("dim=zero&value_color=red{=0|green"), label: "zero", value: "0 things", fill: "e05d44"},
		{name: "color-paren-equals", target: c("dim=zero&value_color=red)=0|green"), label: "zero", value: "0 things", fill: "e05d44"},
		// a null value, a threshold that is none, no entry chosen, an empty color, the thresholds' numbers
		{name: "color-null-of-a-null", target: c("dim=gap&value_color=grey:null|green"), label: "gap", value: "-", fill: "555"},
		{name: "color-null-of-a-number", target: c("dim=a&value_color=grey:null|green"), label: "a", value: "1234 things", fill: "97CA00"},
		{name: "color-null-any-operator", target: c("dim=gap&value_color=red!=null|green"), label: "gap", value: "-", fill: "e05d44"},
		{name: "color-null-empty-threshold", target: c("dim=gap&value_color=red>|green"), label: "gap", value: "-", fill: "e05d44"},
		{name: "color-null-number-threshold", target: c("dim=gap&value_color=red>5|blue<5"), label: "gap", value: "-", fill: "007ec6"},
		{name: "color-number-null-threshold", target: c("dim=a&value_color=red>null|green"), label: "a", value: "1234 things", fill: "97CA00"},
		{name: "color-no-entry-chosen", target: c("dim=a&value_color=red>100000"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-no-entry-chosen-two", target: c("dim=a&value_color=red>100000|blue<0"), label: "a", value: "1234 things", fill: "007ec6"},
		{name: "color-empty-color", target: c("dim=a&value_color=>5"), label: "a", value: "1234 things", fill: "555"},
		{name: "color-empty-first-entry", target: c("dim=a&value_color=|red"), label: "a", value: "1234 things", fill: "555"},
		{name: "color-starts-with-equals", target: c("dim=zero&value_color==0|green"), label: "zero", value: "0 things", fill: "555"},
		{name: "color-threshold-fraction", target: c("dim=a&divide=117&value_color=red>10.9|green"),
			label: "a", value: "10.6 things", fill: "e05d44"},
		{name: "color-threshold-text", target: c("dim=a&value_color=red>abc|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-threshold-text-negative", target: c("dim=neg&value_color=red>abc|green"), label: "neg", value: "-42.2 things", fill: "97CA00"},
		{name: "color-threshold-negative", target: c("dim=neg&value_color=red<-5|green"), label: "neg", value: "-42.2 things", fill: "e05d44"},
		{name: "color-two-operators", target: c("dim=a&value_color=red<5>1000|green"), label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-300-bytes", target: c("dim=a&value_color=" + long(300, "a")), label: "a", value: "1234 things", fill: "555"},
		{name: "color-threshold-300-digits", target: c("dim=a&value_color=red>" + long(300, "0") + "|green"),
			label: "a", value: "1234 things", fill: "e05d44"},
		{name: "color-multiplied", target: c("dim=a&multiply=-1&value_color=red<0|green"), label: "a", value: "-1234 things", fill: "e05d44"},
		// a plain color: a name, hex of 3 and of 6 digits, anything else is the default (:700-742)
		{name: "color-name", target: c("value_color=blue"), value: "1192 things", fill: "007ec6"},
		{name: "color-hex-6", target: c("value_color=FFaa00"), value: "1192 things", fill: "FFaa00"},
		{name: "color-hex-3", target: c("value_color=abc"), value: "1192 things", fill: "abc"},
		{name: "color-hex-4", target: c("value_color=abcd"), value: "1192 things", fill: "555"},
		{name: "color-first-entry-plain", target: c("value_color=red|green"), value: "1192 things", fill: "e05d44"},
		{name: "color-empty", target: c("value_color=&dim=gap"), label: "gap", value: "-", fill: "999"},
	}
	// the label's color and the two text colors, the same argument in all three: a name of the table, exactly 3 or 6
	// hex digits as given, and anything else the defaults (`555` for the label's box, `fff` for the texts)
	for _, arg := range []struct{ name, value, color string }{{"brightgreen", "brightgreen", "4c1"}, {"green", "green", "97CA00"},
		{"yellow", "yellow", "dfb317"}, {"yellowgreen", "yellowgreen", "a4a61d"}, {"orange", "orange", "fe7d37"}, {"red", "red", "e05d44"},
		{"blue", "blue", "007ec6"}, {"grey", "grey", "555"}, {"gray", "gray", "555"}, {"lightgrey", "lightgrey", "9f9f9f"},
		{"lightgray", "lightgray", "9f9f9f"}, {"hex-3", "fa0", "fa0"}, {"hex-3-upper", "FA0", "FA0"}, {"hex-6-mixed", "a1B2c3", "a1B2c3"},
		{"hex-4", "ffff", ""}, {"hex-7", "1234567", ""}, {"hex-not", "gggggg", ""}, {"1-byte", "f", ""}, {"2-bytes", "ff", ""},
		{"19-bytes", long(19, "a"), ""}, {"20-bytes", long(20, "a"), ""}, {"hash", "%23fff", ""}, {"other-case", "Red", ""},
		{"unknown", "purple", ""}} {
		box, text := cmp.Or(arg.color, "555"), cmp.Or(arg.color, "fff")
		rows = append(rows, badgeAsk{name: "colors-" + arg.name,
			target: c("label_color=" + arg.value + "&text_color_lbl=" + arg.value + "&text_color_val=" + arg.value),
			value:  "1192 things", want: badgeColors(box, text, text)})
	}
	rows = append(rows, []badgeAsk{
		{name: "colors-each-its-own", target: c("label_color=blue&text_color_lbl=red&text_color_val=000&value_color=yellow"),
			value: "1192 things", fill: "dfb317", want: badgeColors("007ec6", "e05d44", "000")},

		// the scale and the fixed widths (:757, :765-768, :779-785, :955-962)
		{name: "scale-99", target: c("scale=99"), value: "1192 things", want: []string{`width="166.55" height="20.00"`}},
		{name: "scale-100", target: c("scale=100"), value: "1192 things", want: []string{`width="166.55" height="20.00"`}},
		{name: "scale-150", target: c("scale=150"), value: "1192 things", want: []string{`width="249.82" height="30.00"`, `font-size="16.50"`}},
		{name: "scale-1000", target: c("scale=1000"), value: "1192 things", want: []string{`height="200.00"`, `rx="30.00"`}},
		{name: "scale-negative", target: c("scale=-5"), value: "1192 things", want: []string{`width="166.55" height="20.00"`}},
		{name: "scale-text", target: c("scale=abc"), value: "1192 things", want: []string{`width="166.55" height="20.00"`}},
		{name: "scale-int-max", target: c("scale=2147483647"), value: "1192 things", want: []string{`height="429496729.40"`}},
		// fixed widths only when both are given and both are above 0: then the two clip paths and no script (:804-822,
		// :840); else the texts' own widths and the script
		fixed(badgeAsk{name: "fixed-both", target: c("fixed_width_lbl=80&fixed_width_val=120"), value: "1192 things"},
			`<rect class="bdge-rect-lbl" width="80.00" `, `x="80.00" width="120.00" `, `width="200.00" height="20.00">`),
		auto(badgeAsk{name: "fixed-label-alone", target: c("fixed_width_lbl=80"), value: "1192 things"}),
		auto(badgeAsk{name: "fixed-value-alone", target: c("fixed_width_val=120"), value: "1192 things"}),
		auto(badgeAsk{name: "fixed-label-zero", target: c("fixed_width_lbl=0&fixed_width_val=5"), value: "1192 things"}),
		auto(badgeAsk{name: "fixed-value-zero", target: c("fixed_width_lbl=5&fixed_width_val=0"), value: "1192 things"}),
		auto(badgeAsk{name: "fixed-negative", target: c("fixed_width_lbl=-3&fixed_width_val=10"), value: "1192 things"}),
		auto(badgeAsk{name: "fixed-text", target: c("fixed_width_lbl=80&fixed_width_val=abc"), value: "1192 things"}),
		fixed(badgeAsk{name: "fixed-one", target: c("fixed_width_lbl=1&fixed_width_val=1"), value: "1192 things"}, `width="2.00" height="20.00">`),
		fixed(badgeAsk{name: "fixed-scaled", target: c("fixed_width_lbl=80&fixed_width_val=120&scale=250"), value: "1192 things"},
			`<rect class="bdge-rect-lbl" width="200.00" `, `x="200.00" width="300.00" `, `width="500.00" height="50.00">`),
		fixed(badgeAsk{name: "fixed-long-label", target: c("fixed_width_lbl=30&fixed_width_val=30&label=" + long(60, "W")),
			label: long(60, "W"), value: "1192 things"}, `width="60.00" height="20.00">`),

		// the refresh (:1006-1021, :1140-1150; the cache headers: formatters/rrd2json.c:84-89, web_client.c:935-945)
		// A number, or `auto`: the chart's update every with `unaligned`, else the request's own before minus after; a
		// negative number is made positive; a text is 0. With a refresh the answer expires then, and may be cached only
		// when the query's window was absolute. Without a value read (a stale chart) no refresh is written.
		{name: "refresh-5", target: c("refresh=5"), value: "1192 things", want: badgeRefresh(5, false)},
		{name: "refresh-5-absolute", target: c("refresh=5&" + abs), label: badgeChartName, want: badgeRefresh(5, true)},
		{name: "refresh-5-absolute-unaligned", target: c("refresh=5&" + abs + "&options=unaligned"), value: "1076 things", want: badgeRefresh(5, true)},
		{name: "refresh-auto", target: c("refresh=auto"), value: "1192 things", want: badgeRefresh(badgeEvery, false)},
		{name: "refresh-auto-after-900", target: c("refresh=auto&after=-900"), label: badgeChartName, want: badgeRefresh(900, false)},
		{name: "refresh-auto-unaligned", target: c("refresh=auto&after=-900&options=unaligned"), value: "1076 things", want: badgeRefresh(badgeEvery, false)},
		{name: "refresh-auto-absolute", target: c("refresh=auto&" + abs), label: badgeChartName, want: badgeRefresh(2*badgeEvery, true)},
		{name: "refresh-auto-absolute-unaligned", target: c("refresh=auto&" + abs + "&options=unaligned"), value: "1076 things",
			want: badgeRefresh(badgeEvery, true)},
		{name: "refresh-auto-after-past-before", target: c("refresh=auto&after=-300&before=-900"), label: badgeChartName, want: badgeRefresh(600, false)},
		{name: "refresh-negative", target: c("refresh=-7"), value: "1192 things", want: badgeRefresh(7, false)},
		{name: "refresh-0", target: c("refresh=0"), value: "1192 things", want: badgeNoRefresh},
		{name: "refresh-0-absolute", target: c("refresh=0&" + abs), label: badgeChartName, want: badgeNoRefresh},
		{name: "refresh-none-absolute", target: c(abs + "&options=unaligned"), value: "1076 things", want: badgeNoRefresh},
		{name: "refresh-text", target: c("refresh=abc"), value: "1192 things", want: badgeNoRefresh},
		{name: "refresh-auto-upper-case", target: c("refresh=AUTO"), value: "1192 things", want: badgeNoRefresh},
		{name: "refresh-86400", target: c("refresh=86400"), value: "1192 things", want: badgeRefresh(86400, false)},
		{name: "refresh-stale", target: q("chart=b.old&refresh=5"), label: "b.old", value: "-", fill: "999", want: badgeNoRefresh},
		{name: "refresh-auto-stale", target: q("chart=b.never&refresh=auto"), label: "b.never", value: "-", fill: "999", want: badgeNoRefresh},
		// a query that ran and found nothing to show is a value read: the refresh stays
		{name: "refresh-no-dimension", target: c("refresh=5&dim=nope"), label: "nope", value: "-", fill: "999", want: badgeRefresh(5, false)},
		{name: "refresh-null-value", target: c("refresh=5&dim=gap"), label: "gap", value: "-", fill: "999", want: badgeRefresh(5, false)},
		{name: "refresh-past-the-data", target: c(fmt.Sprintf("refresh=5&after=%d&before=%d", base+10*badgeEvery, base+11*badgeEvery)),
			value: "-", fill: "999", want: badgeRefresh(5, true)},
		{name: "refresh-auto-past-the-data", target: c(fmt.Sprintf("refresh=auto&after=%d&before=%d", base+10*badgeEvery, base+11*badgeEvery)),
			value: "-", fill: "999", want: badgeRefresh(badgeEvery, true)},
		{name: "refresh-points-2-absolute", target: c("refresh=5&" + abs + "&points=2"), label: badgeChartName, want: badgeRefresh(5, true)},

		// a chart with no fresh point, and a value that is none (:1130, :1140-1145, :1155)
		{name: "stale", target: q("chart=b.old"), label: "b.old", value: "-", fill: "999", want: badgeNoRefresh},
		{name: "stale-absolute-window", target: q(fmt.Sprintf("chart=b.old&after=%d&before=%d", base-2*badgeEvery-3, base-2*badgeEvery)),
			label: "b.old", value: "-", fill: "999"},
		{name: "never-collected", target: q("chart=b.never"), label: "b.never", value: "-", fill: "999"},
		{name: "stale-color-null", target: q("chart=b.old&value_color=grey:null|green"), label: "b.old", value: "-", fill: "555"},
		{name: "stale-color-plain", target: q("chart=b.old&value_color=green&label=old"), label: "old", value: "-", fill: "97CA00"},
		{name: "stale-units-seconds", target: q("chart=b.old&units=seconds"), label: "b.old", value: "undefined", fill: "999"},
		{name: "stale-units-on-off", target: q("chart=b.old&units=on/off"), label: "b.old", value: "on", fill: "999"},
		{name: "stale-units-given", target: q("chart=b.never&units=KiB&precision=2&multiply=5"), label: "b.never", value: "-", fill: "999"},
		{name: "null-color-null", target: c("dim=gap&value_color=grey:null|green"), label: "gap", value: "-", fill: "555"},
		{name: "null-units-minutes", target: c("dim=gap&units=minutes+ago"), label: "gap", value: "undefined", fill: "999"},
		{name: "null-units-ok-failed", target: c("dim=gap&units=ok/failed"), label: "gap", value: "ok", fill: "999"},
		{name: "null-units-percent", target: c("dim=gap&units=percent"), label: "gap", value: "-", fill: "999"},

		// an alarm of a host whose health is off (:984-994)
		{name: "alarm-unknown", target: c("alarm=nope"), label: "alarm not found", value: "-", fill: "999",
			want: append([]string{`width="114.45" height="20.00">`}, badgeNoRefresh...)},
		// of a request whose alarm is unknown only the scale counts
		{name: "alarm-unknown-all-given", target: c("alarm=no_such_alarm&scale=150&label=x&value_color=red&fixed_width_lbl=50&fixed_width_val=60&refresh=5"),
			label: "alarm not found", value: "-", fill: "999", lacks: []string{"<clipPath", "\r\nRefresh: "},
			want: append([]string{`height="30.00">`}, badgeNoRefresh...)},
		{name: "alarm-of-an-unknown-chart", target: q("chart=nope&alarm=x"), label: "chart not found", value: "-", fill: "999"},
		{name: "alarm-empty", target: c("alarm="), value: "1192 things"},
	}...)
	// v3: the same function behind the other table's row (web_api_v3.c:19-26): rows of the above again, with their
	// guards, on `/api/v3/badge.svg`
	for _, name := range []string{"by-id", "by-name", "unknown-chart-all-given", "no-chart", "subpath", "localhost", "units-seconds",
		"dim-neg", "group-max", "window-absolute", "options-display-absolute", "label-escapes", "precision-7", "color-entries-null",
		"colors-each-its-own", "fixed-scaled", "refresh-5", "refresh-5-absolute", "refresh-auto-absolute", "refresh-stale", "stale",
		"alarm-unknown"} {
		i := slices.IndexFunc(rows, func(r badgeAsk) bool { return r.name == name })
		r := rows[i]
		r.name, r.target = "v3-"+name, strings.Replace(r.target, "/api/v1/badge.svg", "/api/v3/badge.svg", 1)
		rows = append(rows, r)
	}
	return rows
}

// TestBadgeCorpus (check `health.badges`, the chart path) streams the badge fixture into both daemons (ram, one tier,
// health off) and asks `/api/v1/badge.svg` and `/api/v3/badge.svg` of the child with raw requests: every parameter
// with its edge values, the value's text by its units, the colors and the color expressions, the widths, the scale,
// the refresh with its cache headers, and the answers that are no value (an unknown chart, an unknown alarm, a stale
// chart, a query that found nothing to show). Each row: the oracle's view under the row's guard, then both views byte
// for byte.
func TestBadgeCorpus(t *testing.T) {
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
	base := time.Now().Unix() / badgeEvery * badgeEvery
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		badgeFixture(t, conn, base)
	}
	// the fixture as each side holds it: the last chart is defined, and the collected chart's last entry is `base`
	// (a ram ring's first entry is the start of its oldest stored interval: database/ram/rrddim_mem.c
	// rrddim_query_oldest_time_s)
	want := fmt.Sprintf("name=%q units=\"things\" update_every=%d first_entry=%d last_entry=%d", badgeChartName, badgeEvery,
		base-3*badgeEvery, base)
	var held [2]string
	for end := time.Now().Add(15 * time.Second); ; time.Sleep(250 * time.Millisecond) {
		ready := true
		for i, side := range p.Each() {
			held[i] = badgeEntries(side.Daemon, badgeChart)
			ready = ready && held[i] == want && strings.HasPrefix(badgeEntries(side.Daemon, "b.never"), "name=")
		}
		if ready || time.Now().After(end) {
			break
		}
	}
	if held[0] != want {
		t.Fatalf("oracle: the fixture's chart is %s, want %s", held[0], want)
	}
	if held[1] != want {
		t.Errorf("the fixture's chart is %s on the candidate, %s on the oracle", held[1], held[0])
	}
	// a query over more than one update every is aligned to its own length (the rows without `unaligned`): what such
	// a row shows follows the phase of the fixture's last second, which the log names
	t.Logf("the fixture: %s; asked %d s after its last point; the last point's second is %d past a multiple of 600 and %d past one of 900",
		held[0], time.Now().Unix()-base, base%600, base%900)

	for _, a := range badgeAsks(base) {
		if late := time.Now().Unix() - base; late > badgeFresh {
			t.Fatalf("harness: row %s would be asked %d s after the fixture's last point: past %d s a chart of the fixture is stale", a.name, late, badgeFresh)
		}
		t.Run(a.name, func(t *testing.T) { compareBadge(t, p, "", a) })
	}
}

// The alert path's chart and rules (TestHealthBadges). hbdg.values is collected (`a`: 70, then 10; the fake plugin's
// charts have the units `units`); hbdg.idle, named hbdg.pretty, is defined before it and never collected.
const (
	healthBadgeChart = "hbdg.values"
	healthBadgeEmit  = `CHART hbdg.idle 'pretty' 'title' 'units' 'family' 'hbdg.ictx' line 1000 1 '' '' ''
DIMENSION a '' absolute 1 1
`
	// healthBadgeConf: at 70 one alert of each status a badge colors (web_buffer_svg.c:1078-1104), the calc alerts
	// first and the lookup last.
	//   - hb_warn WARNING, hb_crit CRITICAL, hb_clear CLEAR: the rule's units;
	//   - hb_novalue: its calculation names a variable nothing has, so it has no value; without a threshold it is
	//     UNDEFINED. hb_nolimits: a value and no threshold: UNDEFINED with a value;
	//   - hb_nounits: no `units` line: an alert takes its chart's units when it is linked (health/rrdcalc.c:416-417);
	//   - hb_secs: `units: seconds`, the value printed as a duration; hb_neg: a negative value;
	//   - hb_five: evaluated every 5 s: what `refresh=auto` gives for an alert (:1009);
	//   - hb_idle: on the chart that is never collected: linked and never run, UNINITIALIZED;
	//   - hb_uninit: its lookup reads a day (healthLinkRule): never run on a chart of the run, UNINITIALIZED.
	healthBadgeConf = `# the badge check's alerts
 alarm: hb_warn
    on: hbdg.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: WARNING at 70

 alarm: hb_crit
    on: hbdg.values
  calc: $a
 every: 1s
  crit: $this > 50
 units: things
  info: CRITICAL at 70

 alarm: hb_clear
    on: hbdg.values
  calc: $a
 every: 1s
  warn: $this > 500
 units: things
  info: CLEAR

 alarm: hb_novalue
    on: hbdg.values
  calc: $hb_nothing_has_this_name
 every: 1s
 units: things
  info: a calculation that fails and no threshold

 alarm: hb_nolimits
    on: hbdg.values
  calc: $a
 every: 1s
 units: things
  info: a value and no threshold

 alarm: hb_nounits
    on: hbdg.values
  calc: $a
 every: 1s
  warn: $this > 500
  info: no units of its own

 alarm: hb_secs
    on: hbdg.values
  calc: $a * 1000
 every: 1s
  warn: $this > 5000000
 units: seconds
  info: a duration

 alarm: hb_neg
    on: hbdg.values
  calc: 0 - $a
 every: 1s
  warn: $this > 500
 units: things
  info: a negative value

 alarm: hb_five
    on: hbdg.values
  calc: $a
 every: 5s
  warn: $this > 500
 units: things
  info: evaluated every five seconds

 alarm: hb_idle
    on: hbdg.idle
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: on a chart that is never collected

 alarm: hb_uninit
    on: hbdg.values
lookup: average -1d of a
 every: 1s
 units: things
  info: its lookup reads a day
`
)

// healthBadgeWant are the statuses `/api/v1/alarms?all` lists while `a` is 70 (raised) and once it is 10. hb_idle's
// chart was never collected: C lists no alert of such a chart.
func healthBadgeWant(raised bool) map[string]string {
	want := map[string]string{"hb_warn": "CLEAR", "hb_crit": "CLEAR", "hb_clear": "CLEAR", "hb_novalue": "UNDEFINED",
		"hb_nolimits": "UNDEFINED", "hb_nounits": "CLEAR", "hb_secs": "CLEAR", "hb_neg": "CLEAR", "hb_five": "CLEAR",
		"hb_uninit": "UNINITIALIZED"}
	if raised {
		want["hb_warn"], want["hb_crit"] = "WARNING", "CRITICAL"
	}
	return want
}

// healthBadgeAsks are TestHealthBadges' rows while `a` is 70.
func healthBadgeAsks() []badgeAsk {
	v1 := "/api/v1/badge.svg?chart=" + healthBadgeChart
	a := func(alarm, more string) string { return v1 + "&alarm=" + alarm + more }
	return []badgeAsk{
		// each status's badge: the label is the alarm's name with spaces, the units the rule's, the fill the status's
		// (red, orange, brightgreen, lightgrey; UNINITIALIZED's `#000` is no color argument, so the default: `555`)
		{name: "warning", target: a("hb_warn", ""), label: "hb warn", value: "70 things", fill: "fe7d37",
			want: append(badgeColors("555", "fff", "fff"), badgeNoRefresh...), lacks: []string{"<clipPath", "\r\nRefresh: "}},
		{name: "critical", target: a("hb_crit", ""), label: "hb crit", value: "70 things", fill: "e05d44"},
		{name: "clear", target: a("hb_clear", ""), label: "hb clear", value: "70 things", fill: "4c1"},
		{name: "undefined-no-value", target: a("hb_novalue", ""), label: "hb novalue", value: "-", fill: "9f9f9f"},
		{name: "undefined-with-a-value", target: a("hb_nolimits", ""), label: "hb nolimits", value: "70 things", fill: "9f9f9f"},
		{name: "uninitialized", target: a("hb_uninit", ""), label: "hb uninit", value: "-", fill: "555"},
		{name: "uninitialized-idle-chart", target: "/api/v1/badge.svg?chart=hbdg.idle&alarm=hb_idle", label: "hb idle", value: "-", fill: "555"},
		{name: "chart-by-name", target: "/api/v1/badge.svg?chart=hbdg.pretty&alarm=hb_idle", label: "hb idle", value: "-", fill: "555"},
		{name: "units-of-the-chart", target: a("hb_nounits", ""), label: "hb nounits", value: "70 units", fill: "4c1"},
		{name: "units-seconds", target: a("hb_secs", ""), label: "hb secs", value: "19:26:40"},
		{name: "negative", target: a("hb_neg", ""), label: "hb neg", value: "-70 things"},
		{name: "every-five", target: a("hb_five", ""), label: "hb five", value: "70 things"},
		// the request's own label, units, numbers and colors
		{name: "label", target: a("hb_warn", "&label=my+alert"), label: "my alert", value: "70 things", fill: "fe7d37"},
		{name: "label-empty", target: a("hb_warn", "&label="), label: "hb warn", value: "70 things", fill: "fe7d37"},
		{name: "label-with-underscores", target: a("hb_warn", "&label=x_y"), label: "x_y", value: "70 things", fill: "fe7d37"},
		{name: "units", target: a("hb_warn", "&units=%25"), label: "hb warn", value: "70%", fill: "fe7d37"},
		{name: "units-as-a-time", target: a("hb_warn", "&units=minutes"), label: "hb warn", value: "1h 10m", fill: "fe7d37"},
		{name: "units-empty-word", target: a("hb_clear", "&units=empty"), label: "hb clear", value: "70"},
		{name: "multiply-divide-precision", target: a("hb_warn", "&multiply=2&divide=4&precision=3"),
			label: "hb warn", value: "35.000 things", fill: "fe7d37"},
		{name: "multiply-0", target: a("hb_warn", "&multiply=0&divide=0"), label: "hb warn", value: "70 things", fill: "fe7d37"},
		{name: "multiply-no-value", target: a("hb_novalue", "&multiply=2&precision=2"), label: "hb novalue", value: "-", fill: "9f9f9f"},
		{name: "color", target: a("hb_warn", "&value_color=blue"), label: "hb warn", value: "70 things", fill: "007ec6"},
		{name: "color-empty", target: a("hb_warn", "&value_color="), label: "hb warn", value: "70 things", fill: "fe7d37"},
		{name: "color-by-value", target: a("hb_warn", "&value_color=red>60|green"), label: "hb warn", value: "70 things", fill: "e05d44"},
		{name: "color-by-value-not", target: a("hb_warn", "&value_color=red>600|green"), label: "hb warn", value: "70 things", fill: "97CA00"},
		{name: "color-of-no-value", target: a("hb_novalue", "&value_color=blue:null|red"), label: "hb novalue", value: "-", fill: "007ec6"},
		{name: "colors", target: a("hb_crit", "&label_color=blue&text_color_lbl=000&text_color_val=yellow"),
			label: "hb crit", value: "70 things", fill: "e05d44", want: badgeColors("007ec6", "000", "dfb317")},
		{name: "display-absolute", target: a("hb_neg", "&options=display-absolute&value_color=red<0|green"),
			label: "hb neg", value: "70 things", fill: "e05d44"},
		{name: "scale", target: a("hb_warn", "&scale=200"), label: "hb warn", value: "70 things", fill: "fe7d37",
			want: []string{`height="40.00">`}},
		{name: "fixed-widths", target: a("hb_warn", "&fixed_width_lbl=80&fixed_width_val=120"), label: "hb warn", value: "70 things", fill: "fe7d37",
			want: []string{`<clipPath id="lbl-rect">`, `<clipPath id="val-rect">`, `width="200.00" height="20.00">`}, lacks: []string{"<script "}},
		// the refresh: an alert's answer with one may be cached, and expires exactly then (:1066-1073); `auto` is the
		// rule's `every`
		{name: "refresh-auto", target: a("hb_warn", "&refresh=auto"), label: "hb warn", value: "70 things", fill: "fe7d37",
			want: badgeRefresh(1, true)},
		{name: "refresh-auto-every-five", target: a("hb_five", "&refresh=auto"), label: "hb five", value: "70 things", want: badgeRefresh(5, true)},
		{name: "refresh-auto-never-run", target: a("hb_uninit", "&refresh=auto"), label: "hb uninit", value: "-", fill: "555",
			want: badgeRefresh(1, true)},
		{name: "refresh-30", target: a("hb_warn", "&refresh=30"), label: "hb warn", value: "70 things", fill: "fe7d37", want: badgeRefresh(30, true)},
		{name: "refresh-0", target: a("hb_warn", "&refresh=0"), label: "hb warn", value: "70 things", fill: "fe7d37", want: badgeNoRefresh},
		{name: "refresh-negative", target: a("hb_warn", "&refresh=-3"), label: "hb warn", value: "70 things", fill: "fe7d37",
			want: badgeRefresh(3, true)},
		{name: "refresh-text", target: a("hb_warn", "&refresh=abc"), label: "hb warn", value: "70 things", fill: "fe7d37", want: badgeNoRefresh},
		// what the alert path does not read: the dimensions, the window, the points, the grouping (:1065-1122)
		{name: "query-parameters", target: a("hb_warn", "&dimensions=nope&after=-999999&before=-999&points=7&group=max&options=percentage"),
			label: "hb warn", value: "70 things", fill: "fe7d37"},
		// an alarm the chart does not have; the name is looked up as given (:985, :1025-1029)
		{name: "unknown-alarm", target: a("nope", ""), label: "alarm not found", value: "-", fill: "999"},
		{name: "unknown-alarm-all-given", target: a("no_such", "&scale=150&label=x&value_color=red&fixed_width_lbl=50&fixed_width_val=60&refresh=5"),
			label: "alarm not found", value: "-", fill: "999", lacks: []string{"<clipPath", "\r\nRefresh: "},
			want: append([]string{`height="30.00">`}, badgeNoRefresh...)},
		{name: "alarm-with-a-space", target: a("hb%20warn", ""), label: "alarm not found", value: "-", fill: "999"},
		{name: "alarm-twice", target: a("nope", "&alarm=hb_warn"), label: "hb warn", value: "70 things", fill: "fe7d37"},
		{name: "alarm-of-another-chart", target: "/api/v1/badge.svg?chart=hbdg.idle&alarm=hb_warn",
			label: "alarm not found", value: "-", fill: "999"},
		{name: "alarm-unknown-chart", target: "/api/v1/badge.svg?chart=nope&alarm=hb_warn", label: "chart not found", value: "-", fill: "999"},
		{name: "v3-warning", target: "/api/v3/badge.svg?chart=" + healthBadgeChart + "&alarm=hb_warn",
			label: "hb warn", value: "70 things", fill: "fe7d37"},
		{name: "v3-critical-refresh", target: "/api/v3/badge.svg?chart=" + healthBadgeChart + "&alarm=hb_crit&refresh=auto",
			label: "hb crit", value: "70 things", fill: "e05d44", want: badgeRefresh(1, true)},
		{name: "v3-unknown-alarm", target: "/api/v3/badge.svg?chart=" + healthBadgeChart + "&alarm=nope",
			label: "alarm not found", value: "-", fill: "999"},
		// the chart that was never collected has no value and no refresh, where its alert has a badge all the same
		{name: "chart-value-idle", target: "/api/v1/badge.svg?chart=hbdg.idle", label: "hbdg.pretty", value: "-", fill: "999"},
		{name: "chart-value-idle-by-name", target: "/api/v1/badge.svg?chart=hbdg.pretty&refresh=5", label: "hbdg.pretty", value: "-", fill: "999",
			want: badgeNoRefresh},
	}
}

// healthBadgeLater are the rows asked again once `a` is 10: the two raised alerts are CLEAR.
func healthBadgeLater() []badgeAsk {
	v1 := "/api/v1/badge.svg?chart=" + healthBadgeChart
	return []badgeAsk{
		{name: "cleared-warning", target: v1 + "&alarm=hb_warn", label: "hb warn", value: "10 things", fill: "4c1"},
		{name: "cleared-critical", target: v1 + "&alarm=hb_crit", label: "hb crit", value: "10 things", fill: "4c1"},
		{name: "cleared-seconds", target: v1 + "&alarm=hb_secs", label: "hb secs", value: "02:46:40"},
		{name: "cleared-negative", target: v1 + "&alarm=hb_neg&options=display-absolute&value_color=red<0|green",
			label: "hb neg", value: "10 things", fill: "e05d44"},
		{name: "cleared-v3", target: "/api/v3/badge.svg?chart=" + healthBadgeChart + "&alarm=hb_warn&refresh=auto",
			label: "hb warn", value: "10 things", fill: "4c1", want: badgeRefresh(1, true)},
	}
}

// healthBadgeLive are the rows of the chart's own value, a live chart collected every second: while `a` is 70 (its
// answer with a refresh is not to be cached and expires then: the query's window is relative), and once it is 10.
// They are not asked once but compared as the runner compares a view (compareNow): read until the oracle's badge
// shows the value, then until the two sides are equal. A badge's query reads the points the database stored, and
// for a moment in every second the newest one is not stored yet, on each side at its own moment: the badge then
// shows `-` (seen C against C, 1 run in 4: three rows asked within milliseconds all said `-`, with their refresh).
// And the stored points hold a new value a second after the alerts read it as collected (seen C against C: `70
// units` right after the alerts showed 10).
func healthBadgeLive(value string) []badgeAsk {
	v1 := "/api/v1/badge.svg?chart=" + healthBadgeChart
	if value != "70 units" {
		return []badgeAsk{{name: "cleared-chart-value", target: v1, label: healthBadgeChart, value: value, fill: "4c1", want: badgeNoRefresh}}
	}
	return []badgeAsk{
		{name: "chart-value", target: v1, label: healthBadgeChart, value: value, fill: "4c1", want: badgeNoRefresh},
		{name: "chart-value-refresh-auto", target: v1 + "&refresh=auto", label: healthBadgeChart, value: value, want: badgeRefresh(1, false)},
		{name: "chart-value-refresh-unaligned", target: v1 + "&refresh=auto&after=-3&options=unaligned", label: healthBadgeChart, value: value,
			want: badgeRefresh(1, false)},
	}
}

// badge is side i's view of a badge (badgeView), as a text for the runner's comparison.
func (h *healthPair) badge(i int, target string) string {
	raw, err := rawExchange(h.p.Each()[i].Daemon.Addr, []byte("GET "+target+" HTTP/1.1\r\n\r\n"), 2*time.Second)
	if err != nil {
		return err.Error()
	}
	return string(badgeView(raw))
}

// TestHealthBadges (check `health.badges`, the alert path) asks the badges of live alerts on the health runner: one
// case, `alarms`. The chart's value is held at 70; once `/api/v1/alarms?all` shows every alert's status on both sides
// (the barrier the rows stand on) each row is asked of both, the oracle's view held to the row's guard and the two
// compared byte for byte, and the chart's own value as a view that settles (healthBadgeLive); then the value is 10,
// the barrier again, the raised alerts' badges once more and the chart's own value. An alert's badge reads the alert's published status
// and value and makes no query (web_buffer_svg.c:1065-1122), so the rows hold HEALTH up no more than `/api/v1/alarms`
// does; the rows of the chart's own value make one query each, one after the other, and a badge's is no user data query
// (those are counted at formatters/rrd2json.c:165-167; HEALTH waits while more than one runs, stream-control.c:99-103).
func TestHealthBadges(t *testing.T) {
	runHealthCases(t, map[string]healthCase{
		"alarms": {
			conf: healthBadgeConf,
			sc: healthScenario(healthBadgeEmit, healthBadgeChart, "hbdg.ctx", []string{"a"},
				map[string]int64{"a": 70}, map[string]int64{"a": 10}),
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				alarms := func(i int) string { return h.get(i, "/api/v1/alarms?all") }
				h.compareNow(t, "a at 70: /api/v1/alarms?all", alarms, healthWant(healthBadgeWant(true)))
				for _, a := range healthBadgeAsks() {
					t.Run(a.name, func(t *testing.T) { compareBadge(t, h.p, "", a) })
				}
				live := func(value string) {
					for _, a := range healthBadgeLive(value) {
						t.Run(a.name, func(t *testing.T) {
							h.compareNow(t, "GET "+a.target, func(i int) string { return h.badge(i, a.target) },
								func(oracle string) error { return a.guard([]byte(oracle)) })
						})
					}
				}
				live("70 units")
				h.release(t, "p1", 1, healthCalcHold)
				h.compareNow(t, "a at 10: /api/v1/alarms?all", alarms, healthWant(healthBadgeWant(false)))
				for _, a := range healthBadgeLater() {
					t.Run(a.name, func(t *testing.T) { compareBadge(t, h.p, "", a) })
				}
				live("10 units")
			},
		},
	})
}

// TestHealthBadgesNorm holds the badge checks' own parts to what they are for: the view's two clock headers, the
// second of `Expires` that is asked again, the guard of a row and the brief a passing row logs.
func TestHealthBadgesNorm(t *testing.T) {
	answer := func(date, expires, more, body string) []byte {
		return []byte("HTTP/1.1 200 OK\r\nConnection: close\r\nDate: " + date + "\r\nContent-Type: image/svg+xml\r\n" +
			"Cache-Control: public\r\nExpires: " + expires + "\r\n" + more + "X-Transaction-ID: 0123456789abcdef0123456789abcdef\r\n" +
			"Content-Length: 4\r\n\r\n" + body)
	}
	const at = "Mon, 05 Oct 2026 14:10:59 GMT"
	view := func(expires, more, body string) string { return string(badgeView(answer(at, expires, more, body))) }
	head := func(expires, more string) string {
		return "HTTP/1.1 200 OK\r\nConnection: close\r\nDate: T\r\nContent-Type: image/svg+xml\r\nCache-Control: public\r\nExpires: " +
			expires + "\r\n" + more + "X-Transaction-ID: <masked>\r\nContent-Length: 4\r\n\r\n"
	}
	for name, c := range map[string]struct{ got, want string }{
		"the same second":   {view(at, "", "<svg"), head("T+0", "") + "<svg"},
		"five seconds":      {view("Mon, 05 Oct 2026 14:11:04 GMT", "Refresh: 5\r\n", "<svg"), head("T+5", "Refresh: 5\r\n") + "<svg"},
		"a day":             {view("Tue, 06 Oct 2026 14:10:59 GMT", "", "<svg"), head("T+86400", "") + "<svg"},
		"before the date":   {view("Mon, 05 Oct 2026 14:10:58 GMT", "", "<svg"), head("T-1", "") + "<svg"},
		"no date":           {view("soon", "", "<svg"), head("soon", "") + "<svg"},
		"the body is kept":  {view(at, "", "Date: x\r\nExpires: y\r\nX-Transaction-ID: abc"), head("T+0", "") + "Date: x\r\nExpires: y\r\nX-Transaction-ID: abc"},
		"an empty date":     {string(badgeView(answer("", at, "", "<svg"))), strings.Replace(head(at, ""), "Date: T", "Date: ", 1) + "<svg"},
		"no head's end":     {string(badgeView([]byte("HTTP/1.1 200 OK\r\nDate: " + at + "\r\nExpires: " + at))), "HTTP/1.1 200 OK\r\nDate: T\r\nExpires: T+0"},
		"nothing came back": {string(badgeView(nil)), ""},
	} {
		if c.got != c.want {
			t.Errorf("%s: the view is\n%q, want\n%q", name, c.got, c.want)
		}
	}

	for name, c := range map[string]struct {
		a, b string
		want bool
	}{
		"one second apart":       {head("T+5", "Refresh: 5\r\n") + "x", head("T+4", "Refresh: 5\r\n") + "x", true},
		"the other way":          {head("T+0", "") + "x", head("T+1", "") + "x", true},
		"equal":                  {head("T+5", "") + "x", head("T+5", "") + "x", false},
		"two seconds apart":      {head("T+5", "") + "x", head("T+3", "") + "x", false},
		"another body too":       {head("T+5", "") + "x", head("T+4", "") + "y", false},
		"another header too":     {head("T+5", "Refresh: 5\r\n") + "x", head("T+4", "Refresh: 4\r\n") + "x", false},
		"one side has no Expire": {head("T+5", "") + "x", "HTTP/1.1 404 Not Found\r\n\r\nx", false},
		"no distance":            {head("soon", "") + "x", head("T+4", "") + "x", false},
	} {
		if got := badgeEdge([]byte(c.a), []byte(c.b)); got != c.want {
			t.Errorf("%s: badgeEdge is %t, want %t", name, got, c.want)
		}
	}

	const svg = `<svg xmlns="x" width="100.00" height="20.00"><rect class="bdge-rect-lbl" width="40.00" height="20.00" fill="#555"/>` +
		`<rect class="bdge-rect-val" x="40.00" width="60.00" height="20.00" fill="#4c1"/>` +
		`<text class="bdge-lbl-lbl" x="20.00" y="15" fill="#010101" fill-opacity=".3" clip-path="url(#lbl-rect)">a &amp; b</text>` +
		`<text class="bdge-lbl-lbl" x="20.00" y="14" fill="#fff" clip-path="url(#lbl-rect)">a &amp; b</text>` +
		`<text class="bdge-lbl-val" x="69.00" y="15" fill="#010101" fill-opacity=".3" clip-path="url(#val-rect)">7 things</text>` +
		`<text class="bdge-lbl-val" x="69.00" y="14" fill="#000" clip-path="url(#val-rect)">7 things</text><script type="text/javascript">x</script></svg>`
	badge := head("T+5", "Refresh: 5\r\n") + svg
	if got, want := badgeBrief([]byte(badge)), `HTTP/1.1 200 OK | public | Expires T+5 | Refresh 5 | label "a &amp; b" 40.00 #555 | `+
		`value "7 things" 60.00 #4c1 | text #fff #000 | height 20.00 auto, `+fmt.Sprint(len(svg))+` bytes`; got != want {
		t.Errorf("the brief is\n%s, want\n%s", got, want)
	}
	if got, want := badgeBrief([]byte("HTTP/1.1 400 Bad Request\r\nCache-Control: no-cache\r\n\r\nNo chart id is given at the request.")),
		`HTTP/1.1 400 Bad Request | no-cache | body "No chart id is given at the request."`; got != want {
		t.Errorf("the brief of an answer that is no badge is\n%s, want\n%s", got, want)
	}

	for name, c := range map[string]struct {
		ask  badgeAsk
		view string
		ok   bool
	}{
		"a badge":                    {badgeAsk{}, badge, true},
		"a badge with its parts":     {badgeAsk{want: []string{">7 things<", `fill="#4c1"`, "\r\nRefresh: 5\r\n"}, lacks: []string{"<clipPath"}}, badge, true},
		"a part is missing":          {badgeAsk{want: []string{">8 things<"}}, badge, false},
		"its texts and its fill":     {badgeAsk{label: "a &amp; b", value: "7 things", fill: "4c1"}, badge, true},
		"another label":              {badgeAsk{label: "a & b"}, badge, false},
		"another value":              {badgeAsk{value: "7"}, badge, false},
		"another fill":               {badgeAsk{fill: "555"}, badge, false},
		"a text only in the shadow":  {badgeAsk{value: "7 things"}, strings.Replace(badge, `clip-path="url(#val-rect)">7 things</text><script`, `clip-path="url(#val-rect)">8 things</text><script`, 1), false},
		"a value where none shows":   {badgeAsk{status: "HTTP/1.1 400 Bad Request", value: "7"}, "HTTP/1.1 400 Bad Request\r\n\r\nNo chart id is given at the request.", false},
		"a part it must not hold":    {badgeAsk{lacks: []string{"<script "}}, badge, false},
		"a 404 is no badge":          {badgeAsk{}, "HTTP/1.1 404 Not Found\r\n\r\nUnsupported API command: badge.svg", false},
		"a text is no badge":         {badgeAsk{}, strings.Replace(badge, "image/svg+xml", "text/plain", 1), false},
		"half an svg is no badge":    {badgeAsk{}, strings.TrimSuffix(badge, "</svg>"), false},
		"a status of its own":        {badgeAsk{status: "HTTP/1.1 400 Bad Request", want: []string{"No chart id"}}, "HTTP/1.1 400 Bad Request\r\n\r\nNo chart id is given at the request.", true},
		"another status":             {badgeAsk{status: "HTTP/1.1 400 Bad Request"}, badge, false},
		"a status that only begins":  {badgeAsk{status: "HTTP/1.1 400"}, "HTTP/1.1 400 Bad Request\r\n\r\nx", false},
		"nothing came back at all":   {badgeAsk{}, "", false},
		"a refused connection's row": {badgeAsk{status: ""}, "<timeout>", false},
	} {
		if err := c.ask.guard([]byte(c.view)); (err == nil) != c.ok {
			t.Errorf("%s: the guard says %v, want pass=%t", name, err, c.ok)
		}
	}
}
