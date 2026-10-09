// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"net"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/fixture"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The weights rows of D235 F8, of the two reviews' run-only lists (R106 section 10, R107's) and of the planted bugs
// that survived the units (m12, m13, m14, m19, m41, m42), each answer judged against C's:
//   - TestWeightsAPI: the prelude's arithmetic (a baseline longer than the highlight, its power of two and its
//     rewritten start, the one-sided relative test), its refusals, the two paths of the value method on one child,
//     the echo's other forms;
//   - TestWeightsV1API: version 1's echo, a limit over two charts of one context, the 404 of metric_correlations and
//     the subpath's 400 (volume on negative values and on anomaly rates are the corpus's `W/volume-absolute` and
//     `W/volume-anomaly-share`: one agent, one request, an oracle's value);
//   - TestWeightsHosts: two children holding contexts, the second with the same charts as the first and two of its
//     own (a chart whose name is not its id, a hidden dimension, a dimension that stops before the highlighted
//     window): the version hashes on two CPUs and on one, the same chart on two hosts, the grouped keys, the hidden
//     and the stopped dimension on both paths, a context no longer collected;
//   - TestWeightsTiers: a child kept by the dbengine in two tiers, each tier asked on both paths;
//   - TestWeightsBig: a child of 10,000 metrics, whose walk outlasts a wrapped timeout (504) and a client that goes
//     away (499).

const (
	// weightsNamedContext is a chart whose name is not its id (`nm`, which the parent makes `fixture.nm`:
	// pluginsd_parser.c:470-485, rrdset-index-name.c:16), with dimensions whose names are not their ids.
	weightsNamedContext = "fixture.weightsnm"
	// weightsGapContext is a chart with a hidden dimension and one that stops before the highlighted window.
	weightsGapContext = "fixture.weightsgap"
)

// weightsPoints is a dimension of the weights rows: id, then value(i) at T0+i for i in 1..last.
func weightsPoints(id string, last int, value func(i int) int) fixture.Dimension {
	d := fixture.Dimension{ID: id}
	for i := 1; i <= last; i++ {
		d.Points = append(d.Points, fixture.Point{T: fixture.T0 + int64(i), Collected: strconv.Itoa(value(i)),
			Flags: stream.FlagNotAnomalous})
	}
	return d
}

// weightsNamed is the chart of names: `n1` (named `one`) constant 5, `n2` (named `two`) rising by 1 a second.
func weightsNamed() fixture.Chart {
	return fixture.Chart{ID: weightsNamedContext, Title: "weights named", Units: "units", Family: "fixture",
		Context: weightsNamedContext, UpdateEvery: 1, Dimensions: []fixture.Dimension{
			weightsPoints("n1", fixture.WeightsRows, func(int) int { return 5 }),
			weightsPoints("n2", fixture.WeightsRows, func(i int) int { return i }),
		}}
}

// weightsGap is the chart of the hidden and the stopped dimension: `full` constant 7, `hid` (hidden) constant 9,
// `gone` constant 3 until the second before the highlighted window and never after (nothing is stored for it then:
// pluginsd_parser.c:1195, :1214-1290).
func weightsGap() fixture.Chart {
	return fixture.Chart{ID: weightsGapContext, Title: "weights gap", Units: "units", Family: "fixture",
		Context: weightsGapContext, UpdateEvery: 1, Dimensions: []fixture.Dimension{
			weightsPoints("full", fixture.WeightsRows, func(int) int { return 7 }),
			weightsPoints("hid", fixture.WeightsRows, func(int) int { return 9 }),
			weightsPoints("gone", fixture.WeightsSplit-1, func(int) int { return 3 }),
		}}
}

// weightsDefine sends ch's metadata: the chart of names with its name `nm` and its dimensions' names, the gap chart
// with `hid` hidden (the DIMENSION line's option word, pluginsd_parser.c:647-652), any other as fixture.Chart.Define.
func weightsDefine(conn *stream.Conn, ch fixture.Chart) {
	switch ch.ID {
	case weightsNamedContext:
		conn.Linef("CHART '%s' 'nm' '%s' '%s' '%s' '%s' line 1000 1 '' fixture-pusher corpus", ch.ID, ch.Title,
			ch.Units, ch.Family, ch.Context)
		conn.DimensionNamed("n1", "one", "", 1, 1)
		conn.DimensionNamed("n2", "two", "", 1, 1)
	case weightsGapContext:
		conn.DefineChart(stream.Chart{ID: ch.ID, Title: ch.Title, Units: ch.Units, Family: ch.Family,
			Context: ch.Context, UpdateEvery: ch.UpdateEvery})
		conn.Dimension("full", "", 1, 1)
		conn.DimensionWith("", "hid", "", "", 1, 1, "hidden")
		conn.Dimension("gone", "", 1, 1)
	default:
		ch.Define(conn)
	}
}

// weightsPush connects host to each side, sends it charts live over that one connection, waits on each side for the
// whole retention and the context's stamp of each chart in waits (as weightsChild), and hands back the two
// connections.
func weightsPush(t *testing.T, p *Pair, host stream.HostInfo, charts, waits []fixture.Chart) [2]*stream.Conn {
	t.Helper()
	conns := dashConnectAs(t, p, host)
	for i, conn := range conns {
		for _, ch := range charts {
			weightsDefine(conn, ch)
		}
		for _, ch := range charts {
			ch.PushLive(conn)
		}
		if err := conn.Flush(); err != nil {
			t.Fatalf("%s: %v", p.Each()[i].Role, err)
		}
	}
	for _, side := range p.Each() {
		for _, ch := range waits {
			if _, err := side.Daemon.WaitRetention(host.Hostname, ch.Context, ch.FirstT(), ch.LastT(),
				60*time.Second); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			if err := side.Daemon.WaitContextStamp(host.Hostname, ch.Context, 60*time.Second); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
		}
	}
	return conns
}

// weightsLimitCharts are the corpus's `W/limit-hierarchy` charts (weights_limit_test.go weightsLimitChart): two
// charts of `fixture.limith` and one of `fixture.limith-other`, two constant dimensions each.
func weightsLimitCharts() []fixture.Chart {
	chart := func(id, context string, values ...int) fixture.Chart {
		ch := fixture.Chart{ID: id, Context: context, Title: "weights limit", Units: "units", Family: "fixture",
			UpdateEvery: 1}
		for d, v := range values {
			ch.Dimensions = append(ch.Dimensions, weightsPoints("d000"+strconv.Itoa(d), fixture.WeightsRows,
				func(int) int { return v }))
		}
		return ch
	}
	return []fixture.Chart{
		chart("fixture.limith-a", "fixture.limith", 1, 100),
		chart("fixture.limith-b", "fixture.limith", 2, 90),
		chart("fixture.limith-c", "fixture.limith-other", 3, 80),
	}
}

// weightsSecondCharts are the second child's charts: the fixture child's two, the chart of names and the gap chart.
func weightsSecondCharts() []fixture.Chart {
	return append(weightsCharts(), weightsNamed(), weightsGap())
}

// weightsCPUs is a pair's `[global] cpu cores`, which the walk reads (netdata_conf_cpus(), weights.c:2529-2560):
// with 2, C walks two or more hosts to work on in threads, with 1 in one, on any box.
func weightsCPUs(n int) daemon.Options {
	return daemon.Options{GlobalExtra: "    cpu cores = " + strconv.Itoa(n) + "\n"}
}

// weightsRow is a row of a weights check with the family that compares it.
type weightsRow struct {
	req v2Req
	fam v2Family
}

// weightsRowsOf are reqs, each compared by fam.
func weightsRowsOf(fam v2Family, reqs []v2Req) []weightsRow {
	out := make([]weightsRow, 0, len(reqs))
	for _, r := range reqs {
		out = append(out, weightsRow{r, fam})
	}
	return out
}

// weightsAsk asks rows as subtests of t on p, in order.
func weightsAsk(t *testing.T, p *Pair, rows []weightsRow) {
	t.Helper()
	for _, r := range rows {
		t.Run(r.req.name, func(t *testing.T) {
			compareV2(t, p, r.req, r.fam)
		})
	}
}

// weightsWindowEndRe is a window end of the weights echo as a number: request.window's and view.window's `after` and
// `before`, request.baseline's `baseline_after` and `baseline_before`, view.baseline's `after` and `before`
// (weights.c:767-827).
var weightsWindowEndRe = regexp.MustCompile(`"(after|before|baseline_after|baseline_before)":(-?[0-9]+)`)

// weightsRelativeRender is weightsRender, then every window end written as its distance from the highlighted
// window's end, request.window's `before` (the answer's first `before`), as `"END-999"`; that end is the engine's
// clock less a second (rrdr_relative_window_to_absolute_query, libnetdata.c:549-551), so the ends are rewritten only
// when it is a second before one of the side's seconds of flight, and left as numbers otherwise, which the comparison
// reports.
func weightsRelativeRender(i int, flight [2]int64, body []byte) []byte {
	body = weightsRender(i, flight, body)
	var end int64
	found := false
	for _, m := range weightsWindowEndRe.FindAllSubmatch(body, -1) {
		if string(m[1]) == "before" {
			end, _ = strconv.ParseInt(string(m[2]), 10, 64)
			found = true
			break
		}
	}
	if !found || end+1 < flight[0] || end+1 > flight[1] {
		return body
	}
	return weightsWindowEndRe.ReplaceAllFunc(body, func(b []byte) []byte {
		m := weightsWindowEndRe.FindSubmatch(b)
		n, _ := strconv.ParseInt(string(m[2]), 10, 64)
		if n == end {
			return []byte(`"` + string(m[1]) + `":"END"`)
		}
		return []byte(`"` + string(m[1]) + `":"END` + strconv.FormatInt(n-end, 10) + `"`)
	})
}

// weightsRelativeFamily compares the answers of a request with a relative window: weightsFamily with the window ends
// read against the highlighted window's end (weightsRelativeRender).
var weightsRelativeFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, wall: true,
	render: weightsRelativeRender}

// weightsNodeDims are a multinode answer's dimension rows as `<dimension id>@<node index>` → weight (weightsDimsBy).
func weightsNodeDims(v Value) (map[string]float64, error) {
	return weightsDimsBy(v, func(id, ni string) string { return id + "@" + ni })
}

// weightsBetween is a fact: the dimension id's weight lies strictly between 0 and 1 (a ks2 probability that is
// none of KSfbar's exact ends).
func weightsBetween(id string) dashFact {
	return func(v Value) error {
		dims, err := weightsV2Dims(v)
		if err != nil {
			return err
		}
		if w, ok := dims[id]; !ok || w <= 0 || w >= 1 {
			return fmt.Errorf("%s weighs %v (present %v), want a weight strictly between 0 and 1", id, w, ok)
		}
		return nil
	}
}

// weightsSame is a pair of facts over two rows asked in turn: the first records its answer's dimension weights, the
// second holds that its own are the same (the value method's one query and its walk: weights.c:2773-2782).
func weightsSame() (first, second dashFact) {
	var seen map[string]float64
	first = func(v Value) error {
		d, err := weightsV2Dims(v)
		if err != nil {
			return err
		}
		if len(d) == 0 {
			return fmt.Errorf("no dimension row")
		}
		seen = d
		return nil
	}
	second = func(v Value) error {
		if seen == nil {
			return fmt.Errorf("harness: the row that records the weights did not run before this one")
		}
		d, err := weightsV2Dims(v)
		if err != nil {
			return err
		}
		return weightsEqual(d, seen, 0)
	}
	return first, second
}

// weightsPreludeRows are TestWeightsAPI's rows of the prelude, its refusals, the two paths and the echo, on the
// fixture child.
func weightsPreludeRows() []weightsRow {
	child := "&scope_nodes=" + childHost.Hostname
	ks2 := "/api/v3/weights?method=ks2&options=raw&"
	one, walk := weightsSame()
	out := weightsRowsOf(weightsFamily, []v2Req{
		// a baseline of 120 s against a highlight of 40 s (D235 F8): the multiplier 3 is made the power of two 4
		// (weights.c:2712-2730), so 2 shifts: the baseline asks 500 << 2 points and starts 160 s before its end
		// (:2757), at T0+40; with `raw` the probabilities are compared, and one of them (`level`'s) is none of
		// KSfbar's exact ends: the statistic's scaled cursor (weights.c:1594, :1608, :1627) is judged
		{name: "ks2-shift", target: ks2 + "after=" + weightsT(200) + "&before=" + weightsT(fixture.WeightsRows) +
			"&baseline_after=" + weightsT(80) + "&baseline_before=" + weightsT(200) + child, status: "200",
			guard: dashGuard(
				dashMembers([]string{"request", "baseline"}, "baseline_after", weightsT(40), "baseline_before",
					weightsT(200)),
				dashMembers([]string{"view", "baseline"}, "duration", "160", "points", "2000"),
				[]dashFact{dashIs("500", "view", "window", "points"), weightsBetween("level"),
					weightsV2Weigh(map[string]float64{"flat": 0, "level": 0.9756785, "split": 1, "anom": 0,
						"flat2": 0, "jump": 1}, 1e-6)},
			)},
		// the same windows as `ks2` with `raw`: no shift (the baseline asks 500 points), and every probability one of
		// the exact ends; `level` weighs 1 here, where `ks2` (no option) ranks it apart from `split` and `jump`, so
		// `raw` reaches the queries
		{name: "ks2-raw", target: ks2 + weightsHighlight + "&" + weightsBaseline + child, status: "200",
			guard: dashGuard([]dashFact{dashIs("500", "view", "baseline", "points"), weightsV2Weigh(map[string]float64{
				"flat": 0, "level": 1, "split": 1, "anom": 0, "flat2": 0, "jump": 1}, 0)})},
		// 250 s against 100 s: the multiplier 2.5 rounds half away from zero to 3 (round(), weights.c:2715), then 4:
		// the baseline starts 400 s before its end, at T0-260, before the data
		{name: "shift-round", target: ks2 + "after=" + weightsT(140) + "&before=" + weightsT(240) +
			"&baseline_after=" + weightsT(-110) + "&baseline_before=" + weightsT(140) + child, status: "200",
			guard: dashGuard(
				dashMembers([]string{"request", "baseline"}, "baseline_after", weightsT(-260), "baseline_before",
					weightsT(140)),
				dashMembers([]string{"view", "baseline"}, "duration", "400", "points", "2000"),
				[]dashFact{weightsBetween("level")},
			)},
		// a negative `baseline_before` beyond the relative range is still relative (the one-sided test,
		// weights.c:2701): it counts from the highlight's start, T0+120-94608001; `baseline_after` counts from that
		// end, a second later (a relative start is, libnetdata.c:514-521): 2399 s against 120 s rounds to 20, then
		// 32, five shifts, trimmed to four by the 10000 points (weights.c:9-14, :2735-2745). Its baseline lies before
		// every retention, so no context is walked (weights.c:2496-2506). The row holds until 2030-11: its baseline's
		// end then falls under the ten years before now that a window is clamped to (libnetdata.c:561-573)
		{name: "one-sided", target: "/api/v3/weights?method=ks2&" + weightsHighlight +
			"&baseline_after=-2400&baseline_before=-94608001" + child, status: "200",
			guard: dashGuard(
				dashMembers([]string{"request", "baseline"}, "baseline_after", weightsT(120-94608001-1920),
					"baseline_before", weightsT(120-94608001)),
				dashMembers([]string{"view", "baseline"}, "duration", "1920", "points", "8000"),
				[]dashFact{dashIs("0", "total_dimensions_count")},
			)},
		// equal baseline ends (weights.c:2704-2710); only ks2 and volume read the baseline
		{name: "bad-baseline", target: "/api/v3/weights?method=ks2&" + weightsHighlight + "&baseline_after=" +
			weightsT(fixture.WeightsSplit) + "&baseline_before=" + weightsT(fixture.WeightsSplit), status: "400",
			guard: dashText(`{"error": "Invalid baseline time-range." }`)},
		// fewer than 15 points (weights.c:2750-2754), ks2 and volume alike
		{name: "few-points", target: "/api/v3/weights?method=ks2&points=14&" + weightsHighlight + "&" +
			weightsBaseline, status: "400",
			guard: dashText(`{"error": "Too few points available, at least 15 are needed." }`)},
		{name: "few-points-volume", target: "/api/v3/weights?method=volume&points=14&" + weightsHighlight + "&" +
			weightsBaseline, status: "400",
			guard: dashText(`{"error": "Too few points available, at least 15 are needed." }`)},
		// both refusals apply: the baseline's is tested first
		{name: "few-points-baseline", target: "/api/v3/weights?method=ks2&points=14&" + weightsHighlight +
			"&baseline_after=" + weightsT(fixture.WeightsSplit) + "&baseline_before=" + weightsT(fixture.WeightsSplit),
			status: "400", guard: dashText(`{"error": "Invalid baseline time-range." }`)},
		// the value method with a contexts pattern takes the one query (weights.c:2776-2778): no per-tier points
		// (weights.c:1996-2001); with `contexts=*`, no pattern, the walk (weights.c:2779-2780), whose queries count
		// theirs: the same weights (R106 item 6)
		{name: "one-query", target: "/api/v2/weights?method=value&contexts=fixture.*&" + weightsHighlight + child,
			status: "200", guard: dashGuard([]dashFact{dashIs("[0]", "db", "db_points_per_tier"), one})},
		{name: "walk", target: "/api/v2/weights?method=value&contexts=*&" + weightsHighlight + child,
			status: "200", guard: dashGuard([]dashFact{dashIs("[726]", "db", "db_points_per_tier"), walk})},
		// the echo with rfc3339 (buffer_json_member_add_time_t_formatted) and the options' table order
		{name: "echo-rfc3339", target: "/api/v3/weights?method=value&options=rfc3339|minify&scope_contexts=" +
			fixture.WeightsContext + "&" + weightsHighlight, status: "200", guard: dashGuard(
			dashMembers([]string{"request", "window"}, "after", `"2023-11-14T22:15:20Z"`, "before",
				`"2023-11-14T22:17:20Z"`),
			[]dashFact{dashIs(`["null2zero","unaligned","minify","rfc3339"]`, "request", "options")},
		)},
		// tier=0 selects the tier (api_v2_weights.c:121-124), which is an option: no `nonzero` default
		{name: "echo-tier", target: "/api/v3/weights?method=value&tier=0&scope_contexts=" + fixture.WeightsContext +
			"&" + weightsHighlight, status: "200", guard: dashGuard([]dashFact{dashIs("0", "request", "window", "tier"),
			dashIs(`["null2zero","unaligned","selected-tier"]`, "request", "options")})},
		// the trimmed mean is printed by the table's first name of its value (query-group-over-time.c:707-715)
		{name: "echo-trimmed", target: "/api/v3/weights?method=value&time_group=trimmed-mean&scope_contexts=" +
			fixture.WeightsContext + "&" + weightsHighlight, status: "200", guard: dashGuard([]dashFact{
			dashIs(`"trimmed-mean5"`, "view", "time_group"),
			dashIs(`"trimmed-mean5"`, "request", "aggregations", "time", "time_group"),
			weightsV2Weigh(map[string]float64{"flat": 50, "level": 30, "split": 276.9972477, "anom": 20}, 1e-6)})},
		// version 2 has no metric_correlations (web_api_v2.c:20-24): the dispatcher's 404 (web_api.c:103-106)
		{name: "mc-v2", target: "/api/v2/metric_correlations?" + weightsHighlight, status: "404",
			guard: dashText(`Unsupported API command: metric_correlations`)},
	})
	// a relative window, the baseline's end the highlight's start (weights.c:2701-2702) and its start 100 s before;
	// a relative start is moved a second later (libnetdata.c:514-521), so 99 s against 999 s rounds to 0,
	// which leaves no shift and stretches the baseline to the highlight's length (:2757); the window lies after the
	// data, so nothing is found, but the contexts are collected and pass the retention gate (rrdcontext.c:559-568):
	// each metric is examined. Its cache headers are compared as every row's are: no weights answer is cacheable,
	// whatever its window (buffer_json_initialize(), buffer.c:351; web_client.c:936-937), so no row can tell a
	// relative window's headers from an absolute one's (R107 item 1)
	return append(out, weightsRow{v2Req{name: "relative", target: "/api/v3/weights?method=ks2&after=-1000" +
		"&baseline_after=-100" + child, status: "200", guard: dashGuard(
		dashMembers([]string{"request", "window"}, "after", `"END-999"`, "before", `"END"`),
		dashMembers([]string{"request", "baseline"}, "baseline_after", `"END-1998"`, "baseline_before", `"END-999"`),
		dashMembers([]string{"view", "baseline"}, "duration", "999", "points", "500"),
		[]dashFact{dashIs("6", "total_dimensions_count"), dashIs("0", "correlated_dimensions")},
	)}, weightsRelativeFamily})
}

// weightsAPIRows are TestWeightsAPI's rows: the committed ones, then the prelude's.
func weightsAPIRows() []weightsRow {
	return append(weightsRowsOf(weightsFamily, weightsRequests()), weightsPreludeRows()...)
}

// weightsV1MoreRequests are TestWeightsV1API's further rows.
func weightsV1MoreRequests() []v2Req {
	dims := func(context string) []string { return []string{"contexts", context, "charts", context, "dimensions"} }
	return []v2Req{
		// metric_correlations with nothing printed: version 1's 404 (weights.c:2883-2886)
		{name: "mc-none", target: "/api/v1/metric_correlations?context=no.such.ctx&" + weightsHighlight + "&" +
			weightsBaseline, status: "404", guard: dashText(`{"error": "no results produced." }`)},
		// no command of the weights takes a subpath (web_api.c:59-77; R107 item 10)
		{name: "subpath", target: "/api/v1/weights/x?after=-60", status: "400",
			guard: dashText(`API command 'weights' does not support subpaths.`)},
		// version 1's echo (R107 item 8): `group` by the table's first name, the window's ends in rfc3339
		{name: "v1-echo", target: "/api/v1/weights?method=value&context=" + fixture.WeightsContext +
			"&group=trimmed-mean&options=rfc3339|raw&" + weightsHighlight, status: "200", guard: dashGuard([]dashFact{
			dashIs(`"trimmed-mean5"`, "group"), dashIs(`"2023-11-14T22:15:20Z"`, "after"),
			dashIs(`"2023-11-14T22:17:20Z"`, "before"),
			dashFact(weightsV1Weigh(map[string]float64{"flat": 50, "level": 30, "split": 276.9972477, "anom": 20},
				1e-6, dims(fixture.WeightsContext)...))})},
		// two charts of one context under a limit of 1 (R107 item 7): the strongest dimension (d0001 of limith-a,
		// 100) alone, its chart weighed by its two dimensions (50.5), the context by its four (48.25), the other
		// chart not printed (weights.c:401-497)
		{name: "limit-hierarchy", target: "/api/v1/weights?context=fixture.limith&method=value&options=raw&limit=1&" +
			weightsHighlight, status: "200", guard: dashGuard([]dashFact{dashIs(`{"fixture.limith":{"charts":`+
			`{"fixture.limith-a":{"dimensions":{"d0001":100},"weight":50.5}},"weight":48.25}}`, "contexts")})},
	}
}

// weightsV1Rows are TestWeightsV1API's rows.
func weightsV1Rows() []weightsRow {
	return weightsRowsOf(weightsV1Family, append(weightsV1Requests(), weightsV1MoreRequests()...))
}

// weightsHash is the hash rule's facts over the rows of one pair (weights.c:2521-2617; query_scope.c:64-81): the
// rows `hash-c1` and `hash-c2` record each child's contexts version (each alone in scope), and the rows asked after
// them hold their sums.
type weightsHash struct{ v [2]int64 }

// record is the fact of a child's row: its hash, above 0, is that child's.
func (h *weightsHash) record(i int) dashFact {
	return func(v Value) error {
		n, err := weightsContextsHash(v)
		if err != nil || n <= 0 {
			return fmt.Errorf("versions.contexts_hard_hash is %d (%v), want a number above 0", n, err)
		}
		h.v[i] = n
		return nil
	}
}

// sum is the fact that the hash is a times the first child's plus b times the second's.
func (h *weightsHash) sum(a, b int64) dashFact {
	return func(v Value) error {
		n, err := weightsContextsHash(v)
		if err != nil {
			return err
		}
		if h.v[0] == 0 || h.v[1] == 0 {
			return fmt.Errorf("harness: the rows that record the children's versions did not run before this one")
		}
		if want := a*h.v[0] + b*h.v[1]; n != want {
			return fmt.Errorf("versions.contexts_hard_hash is %d, want %d*%d + %d*%d = %d", n, a, h.v[0], b, h.v[1],
				want)
		}
		return nil
	}
}

// weightsHashRequests are the rows of the hash rule (D235 F8, R106 item 4), ks2 over the two children (localhost
// holds no context): each child alone; every host (S = Q = all three: with two CPUs S + Q, the children's sum twice);
// `nodes` naming one child (one host to work on: one thread, S once); `nodes` naming that child and localhost (two to
// work on: with two CPUs S + Q, where Q holds the first child's contexts only). With one CPU every row is S once.
// The prefix names the pair's rows.
func weightsHashRequests(prefix string, cpus int) []v2Req {
	ks2 := "/api/v3/weights?method=ks2&" + weightsHighlight + "&" + weightsBaseline
	h := &weightsHash{}
	twice := int64(1)
	if cpus >= 2 {
		twice = 2
	}
	return []v2Req{
		{name: prefix + "hash-c1", target: ks2 + "&scope_nodes=" + childHost.Hostname, status: "200",
			guard: dashGuard([]dashFact{h.record(0)})},
		{name: prefix + "hash-c2", target: ks2 + "&scope_nodes=" + child2Host.Hostname, status: "200",
			guard: dashGuard([]dashFact{h.record(1)})},
		{name: prefix + "hash-all", target: ks2, status: "200", guard: dashGuard([]dashFact{h.sum(twice, twice)})},
		{name: prefix + "hash-one", target: ks2 + "&nodes=" + childHost.Hostname, status: "200",
			guard: dashGuard([]dashFact{h.sum(1, 1)})},
		{name: prefix + "hash-two", target: ks2 + "&nodes=" + childHost.Hostname + "|" + parentIdentity.Hostname,
			status: "200", guard: dashGuard([]dashFact{h.sum(twice, 1)})},
	}
}

// weightsGroups is a fact: the grouped answer's groups are these, in this order, each `id` with its `nm` (`-` when
// none is printed, the name being the id: weights.c:1493-1494).
func weightsGroups(pairs ...string) dashFact {
	return func(v Value) error {
		res, err := dashMember(v, "result")
		if err != nil {
			return err
		}
		var got []string
		for _, g := range res.Items {
			id, _ := dashMember(g, "id")
			name := "-"
			if nm, err := dashMember(g, "nm"); err == nil {
				name = nm.Text
			}
			got = append(got, id.Text, name)
		}
		if strings.Join(got, " ") != strings.Join(pairs, " ") {
			return fmt.Errorf("groups %q, want %q", got, pairs)
		}
		return nil
	}
}

// weightsHostsRequests are TestWeightsHosts' version-2 rows while both children are connected.
func weightsHostsRequests() []v2Req {
	c2 := "&scope_nodes=" + child2Host.Hostname
	named := "&scope_contexts=" + weightsNamedContext + "|" + fixture.WeightsContext
	bb, cc := childHost.MachineGUID, child2Host.MachineGUID
	value := "/api/v3/weights?method=value&"
	return append(weightsHashRequests("", 2), []v2Req{
		// the same charts on two hosts (R107 item 5): one context, instance and dimension entry each, the rows
		// closed per host (weights.c:1170-1255); `fixture.nm` the named instance's name
		{name: "dup-plain", target: value + "options=minify&" + weightsHighlight, status: "200",
			guard: dashGuard([]dashFact{func(v Value) error {
				d, err := weightsNodeDims(v)
				if err != nil {
					return err
				}
				if len(d) != 15 || d["jump@0"] != 300 || d["jump@1"] != 300 || d["n2@1"] != 180 {
					return fmt.Errorf("dimension rows %v, want 15, with jump 300 on both nodes and n2 180", d)
				}
				return nil
			}, dashIs(`[{"id":"fixture.weights","ii":0},{"id":"fixture.weightsks2","ii":1},`+
				`{"id":"fixture.weightsnm","nm":"fixture.nm","ii":2},{"id":"fixture.weightsgap","ii":3}]`,
				"dictionaries", "instances")})},
		// a limit of 1 on the value method keeps the largest, `jump` 300 of the first child by its node's bytes
		// (weights.c:223-236), and every node with a result stays listed (weights.c:1279-1300)
		{name: "dup-limit", target: value + "options=minify&limit=1&" + weightsHighlight, status: "200",
			guard: dashGuard([]dashFact{func(v Value) error {
				d, err := weightsNodeDims(v)
				if err != nil {
					return err
				}
				return weightsEqual(d, map[string]float64{"jump@0": 300}, 0)
			}, dashIs(`{"limit":1,"total":15,"returned":1,"unit":"dimensions","truncated":true,"summary_scope":"all"}`,
				"result_limit"), func(v Value) error {
				nodes, err := dashMember(v, "dictionaries", "nodes")
				if err != nil || len(nodes.Items) != 2 {
					return fmt.Errorf("dictionaries.nodes is %s (%v), want both children", nodes, err)
				}
				return nil
			}})},
		// grouped keys (weights.c:1405-1451; R107 item 6) over the chart of names on the second child and the weights
		// chart on both: an instance by its id and the node's id, named by its name and the hostname; with the node
		// too, joined by a comma; a dimension by its NAME, with the units; a node by its id, named by the hostname
		{name: "group-instance", target: value + "group_by=instance&" + weightsHighlight + named, status: "200",
			guard: dashGuard([]dashFact{weightsGroups("fixture.weights@"+bb, "fixture.weights@parity-child",
				"fixture.weights@"+cc, "fixture.weights@parity-child2", "fixture.weightsnm@"+cc,
				"fixture.nm@parity-child2")})},
		{name: "group-instance-node", target: value + "group_by=instance,node&" + weightsHighlight + named,
			status: "200", guard: dashGuard([]dashFact{weightsGroups("fixture.weights,"+bb, "fixture.weights,parity-child",
				"fixture.weights,"+cc, "fixture.weights,parity-child2", "fixture.weightsnm,"+cc,
				"fixture.nm,parity-child2")})},
		{name: "group-dim-units", target: value + "group_by=dimension,units&" + weightsHighlight + named,
			status: "200", guard: dashGuard([]dashFact{weightsGroups("flat,units", "-", "level,units", "-", "split,units", "-",
				"anom,units", "-", "one,units", "-", "two,units", "-")})},
		{name: "group-node", target: value + "group_by=node&" + weightsHighlight + named, status: "200",
			guard: dashGuard([]dashFact{weightsGroups(bb, "parity-child", cc, "parity-child2")})},
		// the hidden dimension (R106 item 6): on the one query a column only with `percentage`
		// (query_target.c:490-514), examined, exposed only with `raw` (rrd2json.h:84-93), and a query counted only
		// when exposed (weights.c:2007-2037); without `raw`, `full`'s share of the row is 7/16
		{name: "hidden-one", target: value + "options=percentage&scope_contexts=" + weightsGapContext + "&" +
			weightsHighlight, status: "200", guard: dashGuard([]dashFact{dashIs("3", "total_dimensions_count"),
			dashIs("1", "db", "db_queries"), weightsV2Weigh(map[string]float64{"full": 43.75}, 1e-9)})},
		{name: "hidden-one-raw", target: value + "options=percentage,raw&scope_contexts=" + weightsGapContext + "&" +
			weightsHighlight, status: "200", guard: dashGuard([]dashFact{dashIs("3", "total_dimensions_count"),
			dashIs("2", "db", "db_queries"), weightsV2Weigh(map[string]float64{"full": 7, "hid": 9}, 1e-9)})},
		// on the walk every metric is examined and counted as a query (weights.c:2350, :1955); a query of one metric
		// with `percentage` is 100% of itself; the hidden one is queried only with `percentage` (its query target
		// admits it then: query_target.c:498-508) and exposed only with `raw` too; `gone` has no value
		{name: "hidden-walk", target: value + "options=percentage&" + weightsHighlight + c2, status: "200",
			guard: dashGuard([]dashFact{dashIs("11", "total_dimensions_count"), dashIs("11", "db", "db_queries"),
				weightsV2Holds([]string{"full", "n1"}, []string{"hid", "gone"}, 9)})},
		{name: "hidden-walk-raw", target: value + "options=percentage,raw&" + weightsHighlight + c2, status: "200",
			guard: dashGuard([]dashFact{dashIs("11", "total_dimensions_count"), dashIs("11", "db", "db_queries"),
				weightsV2Holds([]string{"full", "hid"}, []string{"gone"}, 10)})},
		// the dimension that stopped before the highlighted window (R106 item 7; planted bug m14) has no value on
		// either path: on the one query its column failed its plan and is not exposed (query-plan.c:375-379); on the
		// walk its every cell is empty, which is no value (value.c:43-50, :147-150), not the 0 null2zero would read
		{name: "hole-one", target: value + "options=raw&scope_contexts=" + weightsGapContext + "&" + weightsHighlight,
			status: "200", guard: dashGuard([]dashFact{dashIs("2", "total_dimensions_count"), dashIs("1", "db", "db_queries"),
				weightsV2Weigh(map[string]float64{"full": 7}, 1e-9)})},
		{name: "hole-walk", target: value + "options=raw&scope_contexts=*&" + weightsHighlight + c2, status: "200",
			guard: dashGuard([]dashFact{dashIs("11", "total_dimensions_count"), dashIs("11", "db", "db_queries"),
				weightsV2Holds([]string{"full", "n1", "jump"}, []string{"gone", "hid"}, 9)})},
		// a window after the data while the child is connected: its contexts are collected, so they pass the
		// retention gate (a collected context ends at the window's end, rrdcontext.c:559-568) and each metric is
		// examined and queried, with no value
		{name: "later", target: value + "options=raw&after=" + weightsT(600) + "&before=" + weightsT(660) + c2,
			status: "200", guard: dashGuard([]dashFact{dashIs("11", "total_dimensions_count"), dashIs("11", "db", "db_queries"),
				dashIs("0", "correlated_dimensions")})},
	}...)
}

// weightsHostsV1Requests are TestWeightsHosts' version-1 rows.
func weightsHostsV1Requests() []v2Req {
	return []v2Req{
		// version 1's charts format over both children (R107 item 5): a chart's object per instance in walk order, so
		// the same chart's id twice (weights.c:346-399)
		{name: "dup-mc", target: "/api/v1/metric_correlations?options=raw&" + weightsHighlight + "&" + weightsBaseline,
			status: "200", guard: func(v Value) error {
				charts, err := dashMember(v, "correlated_charts")
				if err != nil {
					return err
				}
				keys := strings.Join(memberKeys(charts), " ")
				want := "fixture.weights fixture.weightsks2 fixture.weights fixture.weightsks2 fixture.weightsnm " +
					"fixture.weightsgap"
				if keys != want {
					return fmt.Errorf("correlated_charts has %q, want %q", keys, want)
				}
				return nil
			}},
	}
}

// weightsHostsRows are TestWeightsHosts' rows while both children are connected.
func weightsHostsRows() []weightsRow {
	return append(weightsRowsOf(weightsFamily, weightsHostsRequests()),
		weightsRowsOf(weightsV1Family, weightsHostsV1Requests())...)
}

// weightsGoneRows are TestWeightsHosts' rows once the second child is gone: its contexts are no longer collected
// (rrdcontext-worker.c:1125-1128, :408-411, a second or so later: the family asks again until both sides agree and
// the guard holds), so the retention gate keeps the later window off them before any metric (planted bug m19).
func weightsGoneRows() []weightsRow {
	return weightsRowsOf(weightsFamily, []v2Req{
		{name: "later-gone", target: "/api/v3/weights?method=value&options=raw&after=" + weightsT(600) + "&before=" +
			weightsT(660) + "&scope_nodes=" + child2Host.Hostname, status: "200",
			guard: dashGuard([]dashFact{dashIs("0", "total_dimensions_count"), dashIs("0", "db", "db_queries")})},
	})
}

// weightsOneCPURows are the second pair's rows (one CPU).
func weightsOneCPURows() []weightsRow {
	return weightsRowsOf(weightsFamily, weightsHashRequests("1cpu-", 1))
}

// weightsTiersRows are TestWeightsTiers' rows: per-tier counts only where a value query has rows (value.c:122-151,
// the copy at :141-142), none on the one query (weights.c:1996-2001).
func weightsTiersRows() []weightsRow {
	one := "/api/v3/weights?method=value&options=raw&scope_contexts=" + fixture.WeightsContext + "&" + weightsHighlight
	walk := "/api/v3/weights?method=value&options=raw&scope_nodes=" + childHost.Hostname + "&" + weightsHighlight
	tier0 := map[string]float64{"flat": 50, "level": 3611.0 / 121, "split": 33521.0 / 121, "anom": 20}
	tier1 := map[string]float64{"flat": 50, "level": 30, "split": 263.5, "anom": 20}
	both := func(m map[string]float64, jump float64) map[string]float64 {
		out := map[string]float64{"flat2": 50, "jump": jump}
		for k, v := range m {
			out[k] = v
		}
		return out
	}
	return weightsRowsOf(weightsFamily, []v2Req{
		{name: "tier0-one", target: one + "&tier=0", status: "200", guard: dashGuard([]dashFact{
			dashIs("0", "request", "window", "tier"), dashIs("[0,0]", "db", "db_points_per_tier"),
			weightsV2Weigh(tier0, 1e-6)})},
		// tier 1 alone (query-plan.c:363-366; planted bug m41): its points of 10 seconds give other averages
		{name: "tier1-one", target: one + "&tier=1", status: "200", guard: dashGuard([]dashFact{
			dashIs("1", "request", "window", "tier"), dashIs("[0,0]", "db", "db_points_per_tier"),
			weightsV2Weigh(tier1, 1e-6)})},
		{name: "tier0-walk", target: walk + "&tier=0", status: "200", guard: dashGuard([]dashFact{
			dashIs("[726,0]", "db", "db_points_per_tier"), weightsV2Weigh(both(tier0, 300), 1e-6)})},
		{name: "tier1-walk", target: walk + "&tier=1", status: "200", guard: dashGuard([]dashFact{
			dashIs("[0,96]", "db", "db_points_per_tier"), weightsV2Weigh(both(tier1, 277.5), 1e-6)})},
		// a tier the agent does not keep selects none (api_v2_weights.c:125-126): echoed null, and the queries
		// choose their tiers themselves
		{name: "tier-over", target: walk + "&tier=5", status: "200", guard: dashGuard([]dashFact{
			dashIs("null", "request", "window", "tier"), dashIs(`["null2zero","unaligned","raw"]`, "request",
				"options")})},
		{name: "tier1-ks2", target: "/api/v3/weights?method=ks2&options=raw&tier=1&scope_nodes=" +
			childHost.Hostname + "&" + weightsHighlight + "&" + weightsBaseline, status: "200", guard: dashGuard([]dashFact{
			dashIs("[0,168]", "db", "db_points_per_tier"), weightsBetween("split")})},
	})
}

// weightsBigCharts are a child of 1,000 charts of 10 dimensions over the weights fixture's 240 seconds, in 20
// contexts, of rising, constant and alternating shapes in turn: C walks them with ks2 in about 200 ms.
func weightsBigCharts() []fixture.Chart {
	var out []fixture.Chart
	for c := 0; c < 1000; c++ {
		ch := fixture.Chart{ID: fmt.Sprintf("big.c%04d", c), Context: fmt.Sprintf("big.ctx%02d", c%20),
			Title: "big", Units: "units", Family: "big", UpdateEvery: 1}
		for d := 0; d < 10; d++ {
			k := c*10 + d
			ch.Dimensions = append(ch.Dimensions, weightsPoints(fmt.Sprintf("d%02d", d), fixture.WeightsRows,
				func(i int) int {
					switch k % 3 {
					case 0:
						return i * (k%7 + 1)
					case 1:
						return k % 11
					}
					if i <= fixture.WeightsSplit {
						return i % 2
					}
					return 5 * (i % 3)
				}))
		}
		out = append(out, ch)
	}
	return out
}

// weightsBigRows are TestWeightsBig's rows compared by a family.
func weightsBigRows() []weightsRow {
	return weightsRowsOf(weightsFamily, []v2Req{
		// a timeout of 18446744073709552 ms wraps in microseconds to 384 µs (weights.c:2667: the product in 64
		// unsigned bits; R107 item 2), which a walk of 10,000 metrics outlasts: 504 (weights.c:2789-2793,
		// :2909-2912; planted bug m42)
		{name: "timeout-wrap", target: "/api/v3/weights?method=ks2&timeout=18446744073709552&" + weightsHighlight +
			"&" + weightsBaseline, status: "504", guard: dashText(`{"error": "timed out" }`)},
	})
}

// weightsHalfCloseTarget is the request of TestWeightsBig's `interrupted` row, marked by its attempt.
func weightsHalfCloseTarget(attempt int) string {
	return "/api/v2/weights?method=ks2&options=minify&" + weightsHighlight + "&" + weightsBaseline +
		"&harness=halfclose" + strconv.Itoa(attempt)
}

// weightsHalfClose sends target, closes the connection's writing side 50 ms later, while the walk runs, and reads
// what the agent sends until it closes.
func weightsHalfClose(addr, target string) ([]byte, error) {
	c, err := net.Dial("tcp", addr)
	if err != nil {
		return nil, err
	}
	defer c.Close()
	if _, err := c.Write(rawRequest("GET", target, []string{"Host: localhost", "Connection: close"}, nil)); err != nil {
		return nil, err
	}
	time.Sleep(50 * time.Millisecond)
	if err := c.(*net.TCPConn).CloseWrite(); err != nil {
		return nil, err
	}
	if err := c.SetReadDeadline(time.Now().Add(60 * time.Second)); err != nil {
		return nil, err
	}
	var out bytes.Buffer
	_, err = out.ReadFrom(c)
	return out.Bytes(), err
}

// weightsAccessFieldRe is a field of an access record that the `interrupted` row compares.
var weightsAccessFieldRe = regexp.MustCompile(`\b(level|req_method|code|sent_bytes|size_bytes|request)=("[^"]*"|\S+)`)

// weightsAccess is the access record of the request target in a side's access log, reduced to the fields
// weightsAccessFieldRe names, in their order; empty unless exactly one record names it.
func weightsAccess(lines []string, target string) string {
	var out []string
	for _, l := range lines {
		if !strings.Contains(l, `request="`+target+`"`) || !strings.Contains(l, " code=") {
			continue
		}
		var f []string
		for _, m := range weightsAccessFieldRe.FindAllStringSubmatch(l, -1) {
			f = append(f, m[1]+"="+m[2])
		}
		out = append(out, strings.Join(f, " "))
	}
	if len(out) != 1 {
		return ""
	}
	return out[0]
}

// weightsInterruptedJudge judges the `interrupted` row's two answers (0 the oracle), each read by a client that closed
// its writing side while the walk ran, with the seconds each was in flight, and each side's access record of it
// (weightsAccess). C finds the client gone before a metric or a host (is_socket_closed(), web_api.c:144-154;
// weights.c:2345-2348, :633-637) and answers 499 `interrupted` (weights.c:2795-2798): the status line and the head go
// out at the end of the processing (web_client.c:1093-1131), the body never (the next poll reports the hangup first),
// and the access record says 499 with the body's 25 bytes. Problems, none when the oracle's answer is that and the
// candidate's agrees.
func weightsInterruptedJudge(raw [2][]byte, flight [2][2]int64, access [2]string) []string {
	o := raw[0]
	switch {
	case !bytes.HasPrefix(o, []byte("HTTP/1.1 499 Client Closed Request\r\n")):
		return []string{fmt.Sprintf("oracle: answered %q, want a 499's head", truncateBytes(o))}
	case !bytes.Contains(o, []byte("\r\nContent-Length: 25\r\n")) ||
		!bytes.Contains(o, []byte("\r\nContent-Type: application/json; charset=utf-8\r\n")):
		return []string{fmt.Sprintf("oracle: a 499 without the JSON body's type and 25 bytes: %q", truncateBytes(o))}
	case !bytes.HasSuffix(o, []byte("\r\n\r\n")):
		return []string{fmt.Sprintf("oracle: a body came after the head: %q", truncateBytes(o))}
	case !strings.Contains(access[0], " code=499 ") || !strings.Contains(access[0], " sent_bytes=25 "):
		return []string{fmt.Sprintf("oracle: access record %q, want code=499 with 25 bytes", access[0])}
	}
	var problems []string
	if a, b := maskAnswer(raw[0], flight[0]), maskAnswer(raw[1], flight[1]); !bytes.Equal(a, b) {
		problems = append(problems, fmt.Sprintf("answers differ\noracle:    %q\ncandidate: %q", a, b))
	}
	if access[0] != access[1] {
		problems = append(problems, fmt.Sprintf("access records differ\noracle:    %q\ncandidate: %q", access[0],
			access[1]))
	}
	return problems
}

// weightsOracleProblem tells whether the `interrupted` row asks again: the judge's first problem is the oracle's (its
// walk may have ended before the client's half-close arrived, or its access record come late), not the candidate's,
// which ends the row at once.
func weightsOracleProblem(problems []string) bool {
	return len(problems) > 0 && strings.HasPrefix(problems[0], "oracle:")
}

// TestWeightsHosts (check `api.weights-hosts`): two children holding contexts, the second with the same charts as the
// first and two charts of its own; the hash rule on two CPUs and on one.
func TestWeightsHosts(t *testing.T) {
	p := dashPair(t, weightsCPUs(2))
	weightsPush(t, p, childHost, weightsCharts(), weightsCharts())
	second := weightsPush(t, p, child2Host, weightsSecondCharts(), weightsSecondCharts())
	weightsAsk(t, p, weightsHostsRows())
	t.Run("gone", func(t *testing.T) {
		dashGone(t, p, child2Host, second)
		weightsAsk(t, p, weightsGoneRows())
	})
	t.Run("one-cpu", func(t *testing.T) {
		p := dashPair(t, weightsCPUs(1))
		weightsPush(t, p, childHost, weightsCharts(), weightsCharts())
		weightsPush(t, p, child2Host, weightsSecondCharts(), weightsSecondCharts())
		weightsAsk(t, p, weightsOneCPURows())
	})
}

// TestWeightsTiers (check `api.weights-tiers`): the fixture child kept by the dbengine in two tiers (tier 1 every 10
// tier-0 points), each tier asked on both paths.
func TestWeightsTiers(t *testing.T) {
	opts := weightsCPUs(2)
	opts.StreamMemoryMode, opts.StorageTiers = "dbengine", 2
	opts.TierGrouping[1] = 10
	p := dashPair(t, opts)
	weightsPush(t, p, childHost, weightsCharts(), weightsCharts())
	weightsAsk(t, p, weightsTiersRows())
}

// TestWeightsBig (check `api.weights-big`): the fixture child and a child of 10,000 metrics, whose walk outlasts a
// wrapped timeout (504) and a client that goes away (499; R106 item 8).
func TestWeightsBig(t *testing.T) {
	p := dashPair(t, weightsCPUs(2))
	weightsChild(t, p)
	big := weightsBigCharts()
	weightsPush(t, p, child2Host, big, []fixture.Chart{big[0], big[len(big)-1]})
	for _, side := range p.Each() {
		for c := 0; c < 20; c++ {
			if err := side.Daemon.WaitContextStamp(child2Host.Hostname, fmt.Sprintf("big.ctx%02d", c),
				60*time.Second); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
		}
	}
	weightsAsk(t, p, weightsBigRows())
	// the client gone: asked again (a new request, its own access record) while the oracle's answer is not the 499
	// the judge wants (weightsOracleProblem), up to three times; a candidate's problem with an oracle's 499 ends the
	// row at once
	t.Run("interrupted", func(t *testing.T) {
		var problems []string
		for attempt := 0; attempt < 3; attempt++ {
			target := weightsHalfCloseTarget(attempt)
			var raw [2][]byte
			var flight [2][2]int64
			var access [2]string
			for i, side := range p.Each() {
				from := time.Now().Unix()
				b, err := weightsHalfClose(side.Daemon.Addr, target)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				raw[i], flight[i] = b, [2]int64{from, time.Now().Unix()}
			}
			time.Sleep(time.Second)
			for i, side := range p.Each() {
				access[i] = weightsAccess(logLines(t, side.Daemon.Opts.RunDir, "access.log"), target)
			}
			if problems = weightsInterruptedJudge(raw, flight, access); !weightsOracleProblem(problems) {
				break
			}
		}
		for _, pr := range problems {
			t.Error(pr)
		}
	})
}
