// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"math"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/fixture"
)

// The scoring engine (milestone 10 commits 9 and 10): `/api/v1/weights`, `/api/v1/metric_correlations`,
// `/api/v2/weights` and `/api/v3/weights`, one handler (api_v2_weights.c:18-187) and one engine
// (weights.c:2636-2915). Each check has its own pair (`dashPair`: the parent's own database stays dbengine, never
// the ram or alloc shape the weights walk crashes on) with the fixture child carrying the corpus's two weights
// charts at fixture.T0; every window is absolute but those of the rows `relative` and `one-sided` and of the routes'
// refusals that read none (weights_rows_test.go holds the rows added by H38).

// weightsT is a fixture second as a query value.
func weightsT(s int64) string { return strconv.FormatInt(fixture.T0+s, 10) }

var (
	// weightsHighlight is the highlight window [T0+120, T0+240] (after-inclusive: 121 points).
	weightsHighlight = "after=" + weightsT(fixture.WeightsSplit) + "&before=" + weightsT(fixture.WeightsRows)
	// weightsBaseline is the baseline window [T0, T0+120] of ks2 and volume.
	weightsBaseline = "baseline_after=" + weightsT(0) + "&baseline_before=" + weightsT(fixture.WeightsSplit)
)

// weightsCharts are the corpus's weights fixtures (fixture/weights.go).
func weightsCharts() []fixture.Chart { return []fixture.Chart{fixture.Weights(), fixture.WeightsKS2()} }

// weightsChild connects the fixture child (`childHost`) to each side and pushes both weights charts live over that one
// connection (fixture.Chart.Replicate serves one chart per dialogue, so replication would reconnect the host for the
// second), then waits on each side for each chart's whole retention and its context's retention stamp
// (Daemon.WaitContextStamp: the per-metric walk's retention gate reads it, weights.c:2490-2506): weightsPush.
func weightsChild(t *testing.T, p *Pair) {
	t.Helper()
	weightsPush(t, p, childHost, weightsCharts(), weightsCharts())
}

// weightsNumbers are an object's members as numbers.
func weightsNumbers(v Value) (map[string]float64, error) {
	if v.Kind != KindObject {
		return nil, fmt.Errorf("not an object: %s", v)
	}
	out := map[string]float64{}
	for _, m := range v.Members {
		f, err := strconv.ParseFloat(m.Value.Text, 64)
		if m.Value.Kind != KindNumber || err != nil {
			return nil, fmt.Errorf("%s is not a number: %s", m.Key, m.Value)
		}
		if _, dup := out[m.Key]; dup {
			return nil, fmt.Errorf("%s twice", m.Key)
		}
		out[m.Key] = f
	}
	return out, nil
}

// weightsEqual tells whether got holds exactly want's keys, each value within tol of want's.
func weightsEqual(got, want map[string]float64, tol float64) error {
	if len(got) != len(want) {
		return fmt.Errorf("weights %v, want %v", got, want)
	}
	for k, w := range want {
		if g, ok := got[k]; !ok || math.Abs(g-w) > tol {
			return fmt.Errorf("weights %v, want %v", got, want)
		}
	}
	return nil
}

// weightsV1Family compares the v1 answers: every member but the query's duration (weights.c:326, the walk's
// monotonic span, weights.c:2826-2844).
var weightsV1Family = v2Family{masks: []Mask{{"statistics.query_time_ms", "the query's duration"}}}

// weightsStatusMsRe is a node's status duration in a v2 dictionary (jsonwrap-v2.c:14-15: the summed durations of its
// metrics' queries, absent when 0), with the member before it.
var weightsStatusMsRe = regexp.MustCompile(`("msg":\s*"")\s*,\s*"ms":\s*-?[0-9][0-9.eE+-]*`)

// weightsRender drops a node's status duration on both sides: a mask replaces only a member that exists, and C
// leaves this one out when it is 0. The trade-off, accepted: whether the member is there is not compared, so an agent
// that never writes it passes; its value is a duration either way (0.007 against 0.006 C against C).
func weightsRender(_ int, _ [2]int64, body []byte) []byte {
	return weightsStatusMsRe.ReplaceAll(body, []byte("$1"))
}

// weightsFamily compares the v2 and v3 answers but for the envelope's clocks and durations (infoV2Volatile) and each
// node's status duration (weightsRender), asked again while they differ (dashSettle, as the other v2 families). The
// `versions` hashes are compared: they held C against C, the two hosts' walk included. The agent's clock is the wall
// clock though every row asks a window (`wall`: weights.c:1352, :1507 pass the agents' writer no clock).
var weightsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, wall: true, render: weightsRender}

// weightsV2Dims are a multinode answer's dimension rows (`[0, ni, ci, ii, di, weight, …]`, weights.c:890-948) as
// dimension id → weight, the ids from `dictionaries.dimensions` (weights.c:1279-1350).
func weightsV2Dims(v Value) (map[string]float64, error) {
	return weightsDimsBy(v, func(id, _ string) string { return id })
}

// weightsDimsBy are a multinode answer's dimension rows as key(dimension id, node index) → weight; a key twice, a
// row whose dimension is not in the dictionary or whose weight is no number is an error.
func weightsDimsBy(v Value, key func(id, ni string) string) (map[string]float64, error) {
	dict, err := dashMember(v, "dictionaries", "dimensions")
	if err != nil {
		return nil, err
	}
	ids := map[string]string{}
	for _, d := range dict.Items {
		id, err1 := dashMember(d, "id")
		di, err2 := dashMember(d, "di")
		if err1 != nil || err2 != nil {
			return nil, fmt.Errorf("dimension entry %s", d)
		}
		ids[di.Text] = id.Text
	}
	rows, err := dashMember(v, "result")
	if err != nil {
		return nil, err
	}
	out := map[string]float64{}
	for _, r := range rows.Items {
		if len(r.Items) < 6 || r.Items[0].Text != "0" {
			continue
		}
		id, ok := ids[r.Items[4].Text]
		if !ok {
			return nil, fmt.Errorf("row %s: no dimension %s", r, r.Items[4])
		}
		k := key(id, r.Items[1].Text)
		if _, dup := out[k]; dup {
			return nil, fmt.Errorf("dimension %s twice", k)
		}
		f, err := strconv.ParseFloat(r.Items[5].Text, 64)
		if err != nil {
			return nil, fmt.Errorf("row %s: weight %s", r, r.Items[5])
		}
		out[k] = f
	}
	return out, nil
}

// weightsV2Weigh is a guard: the answer's dimensions weigh want, each within tol (weightsV2Dims, weightsEqual).
func weightsV2Weigh(want map[string]float64, tol float64) func(Value) error {
	return func(v Value) error {
		got, err := weightsV2Dims(v)
		if err != nil {
			return err
		}
		return weightsEqual(got, want, tol)
	}
}

// weightsV2Holds is a guard: the answer's dimensions hold every id of in and none of out, and `correlated_dimensions`
// is n (-1: not checked).
func weightsV2Holds(in, out []string, n int) func(Value) error {
	return func(v Value) error {
		dims, err := weightsV2Dims(v)
		if err != nil {
			return err
		}
		for _, id := range in {
			if _, ok := dims[id]; !ok {
				return fmt.Errorf("no %s in %v", id, dims)
			}
		}
		for _, id := range out {
			if _, ok := dims[id]; ok {
				return fmt.Errorf("%s in %v", id, dims)
			}
		}
		if n >= 0 {
			return dashIs(strconv.Itoa(n), "correlated_dimensions")(v)
		}
		return nil
	}
}

// weightsV1Weigh is a guard of a v1 answer: the dimensions object at path weighs want, each within tol.
func weightsV1Weigh(want map[string]float64, tol float64, path ...string) func(Value) error {
	return func(v Value) error {
		dims, err := dashMember(v, path...)
		if err != nil {
			return err
		}
		got, err := weightsNumbers(dims)
		if err != nil {
			return err
		}
		return weightsEqual(got, want, tol)
	}
}

// weightsV1Dims is a guard of a v1 answer of the weights context: its dimensions weigh want (within 1e-6).
func weightsV1Dims(want map[string]float64) func(Value) error {
	return weightsV1Weigh(want, 1e-6, "contexts", fixture.WeightsContext, "charts", fixture.WeightsContext, "dimensions")
}

// weightsV1Requests are the v1 rows.
func weightsV1Requests() []v2Req {
	return []v2Req{
		// the one-query path (weights.c:2776-2778; v1's default method anomaly-rate, api_v1_weights.c:9-11): the window's
		// anomaly rates, raw: `anom` is anomalous at T0+121..T0+240, 120 of the 121 points
		{name: "weights", target: "/api/v1/weights?context=" + fixture.WeightsContext + "&" + weightsHighlight +
			"&options=raw", status: "200",
			guard: weightsV1Dims(map[string]float64{"flat": 0, "level": 0, "split": 0, "anom": 12000.0 / 121})},
		// the same without options: the defaults (unaligned, null2zero, nonzero: api_v2_weights.c:130-132) drop the
		// three dimensions at rate 0 (weights.c:193-194), and the default rank normalization gives the one left
		// 1 - 1/1 = 0 (spread_results_evenly, weights.c:2079-2138, run unless raw, :2803-2807)
		{name: "weights-default", target: "/api/v1/weights?context=" + fixture.WeightsContext + "&" + weightsHighlight,
			status: "200", guard: weightsV1Dims(map[string]float64{"anom": 0})},
		// ks2 (the deprecated route's default, api_v1_weights.c:5-7) through the per-metric walk, raw: identical diff
		// distributions weigh 0, fully one-sided ones 1 (KSfbar's exact ends)
		{name: "mc", target: "/api/v1/metric_correlations?context=" + fixture.WeightsKS2Context + "&" + weightsHighlight +
			"&" + weightsBaseline + "&options=raw", status: "200", guard: weightsV1Weigh(
			map[string]float64{"flat2": 0, "jump": 1}, 0, "correlated_charts", fixture.WeightsKS2Context, "dimensions")},
		// v1 with nothing emitted answers 404 (weights.c:2883-2886) with the engine's error shape (weights.c:2909-2912)
		{name: "no-context", target: "/api/v1/weights?context=no.such.ctx&" + weightsHighlight, status: "404",
			guard: dashText(`{"error": "no results produced." }`)},
	}
}

// weightsAnomaly is C's whole `result` for the default anomaly-rate request on the fixture child (R93): `anom`'s
// dimension row and its instance, context and node rollups (weights.c:890-948, :718-735), each weighing 0 (the
// default rank normalization of the one result, 1 - 1/1: spread_results_evenly, weights.c:2079-2138, run unless raw,
// :2803-2807) with its timeframe, the window's merged storage point (weights.c:927-934: minimum, average, maximum,
// sum, count and anomalous count; the value 20 at each of the 121 points, 120 of them anomalous), where the rate
// stays readable (120 x 100 / 121).
const weightsAnomaly = `[[0,0,0,0,0,0,[20,20,20,2420,121,120]],[1,0,0,0,null,0,[20,20,20,2420,121,120]],` +
	`[2,0,0,null,null,0,[20,20,20,2420,121,120]],[3,0,null,null,null,0,[20,20,20,2420,121,120]]]`

// weightsContextsHash is the contexts' version of a weights answer (`versions.contexts_hard_hash`), a whole number.
func weightsContextsHash(v Value) (int64, error) {
	hash, err := dashAt(v, "versions", "contexts_hard_hash")
	if err != nil {
		return 0, err
	}
	n, err := strconv.ParseInt(hash.Text, 10, 64)
	if hash.Kind != KindNumber || err != nil {
		return 0, fmt.Errorf("versions.contexts_hard_hash is %s", hash)
	}
	return n, nil
}

// weightsRequests are the v2 and v3 rows.
func weightsRequests() []v2Req {
	child := "&scope_nodes=" + childHost.Hostname
	anomaly := dashGuard([]dashFact{
		weightsV2Holds([]string{"anom"}, []string{"flat", "level", "split", "flat2", "jump"}, 1),
		dashIs(weightsAnomaly, "result"),
	})
	// the child's contexts' version as the one host's serial walk of `anomaly` counted it, once (weights.c:674-675):
	// what `two-hosts` reads its own against. Above 0: the child has contexts.
	serial := int64(0)
	return []v2Req{
		// the per-metric walk of one host (no context scope, weights.c:2779-2780); the default options drop zero weights
		// (api_v2_weights.c:130-133, weights.c:2760-2765), so `anom` alone remains, normalized (weightsAnomaly)
		{name: "anomaly", target: "/api/v3/weights?method=anomaly-rate&" + weightsHighlight + child, status: "200",
			guard: func(v Value) error {
				hash, err := weightsContextsHash(v)
				if err != nil || hash <= 0 {
					return fmt.Errorf("versions.contexts_hard_hash is %d (%v), want a number above 0", hash, err)
				}
				serial = hash
				return anomaly(v)
			}},
		// the same walk with `options=raw`: any option given adds only unaligned and null2zero
		// (api_v2_weights.c:133-135), so the zero weights stay, and raw skips the normalization (weights.c:2803-2807):
		// each dimension's anomaly rate, the average of its points' 0 or 100 (storage-point.h:120-121), `anom`
		// 120 x 100 / 121, the five others 0
		{name: "anomaly-raw", target: "/api/v3/weights?method=anomaly-rate&options=raw&" + weightsHighlight + child,
			status: "200", guard: dashGuard([]dashFact{weightsV2Holds(nil, nil, 6), weightsV2Weigh(map[string]float64{
				"flat": 0, "level": 0, "split": 0, "anom": 12000.0 / 121, "flat2": 0, "jump": 0}, 1e-6)})},
		// the dashboard's default scope: both hosts are queryable, so with the pair's two CPUs (`cpu cores = 2`) C
		// walks them in threads (weights.c:2543-2607; with one CPU it walks them as `anomaly` does, :2546-2561), and
		// its counting pass and its threads each add the hosts' context versions (weights.c:2543, :674-675,
		// :2600-2601): twice the child's, which is what tells that the threads ran (localhost has no context).
		// localhost has no data, so the answer is the child's (weightsAnomaly)
		{name: "two-hosts", target: "/api/v3/weights?method=anomaly-rate&" + weightsHighlight, status: "200",
			guard: func(v Value) error {
				hash, err := weightsContextsHash(v)
				switch {
				case err != nil:
					return err
				case serial == 0:
					return fmt.Errorf("harness: the `anomaly` row did not run before this one: no serial walk to " +
						"read the parallel walk's hash against")
				case hash != 2*serial:
					return fmt.Errorf("versions.contexts_hard_hash is %d, want %d, twice what the one host's walk of "+
						"`anomaly` read (%d): the oracle did not walk the hosts in threads (the pair sets two CPUs), "+
						"or the child's contexts changed between the two rows", hash, 2*serial, serial)
				}
				return anomaly(v)
			}},
		{name: "ks2", target: "/api/v3/weights?method=ks2&" + weightsHighlight + "&" + weightsBaseline + child,
			status: "200", guard: weightsV2Holds([]string{"jump"}, []string{"flat", "flat2", "anom"}, -1)},
		// volume skips a metric whose two averages are equal (weights.c:1890-1892)
		{name: "volume", target: "/api/v3/weights?method=volume&" + weightsHighlight + "&" + weightsBaseline + child,
			status: "200", guard: weightsV2Holds([]string{"level", "split"}, []string{"flat", "anom", "flat2"}, -1)},
		// the one-query path on v2's route: the highlight averages (the corpus's W/value)
		{name: "value", target: "/api/v2/weights?method=value&scope_contexts=" + fixture.WeightsContext + "&" +
			weightsHighlight, status: "200", guard: weightsV2Weigh(weightsValues(false), 1e-7)},
		// an inverted window is swapped, not refused (libnetdata.c:526-531): the highlight window, the route's default
		// method value (api_v2_weights.c:189-191) over both hosts' walk
		{name: "swapped", target: "/api/v3/weights?after=" + weightsT(fixture.WeightsRows) + "&before=" +
			weightsT(fixture.WeightsSplit), status: "200", guard: weightsSwapped},
		// an empty window is refused (weights.c:2692-2696) with the engine's error shape (weights.c:2909-2912)
		{name: "empty-window", target: "/api/v3/weights?after=" + weightsT(fixture.WeightsRows) + "&before=" +
			weightsT(fixture.WeightsRows), status: "400", guard: dashText(`{"error": "Invalid selected time-range." }`)},
		// a limit that is not a number is refused while the parameters are parsed (api_v2_weights.c:69-75)
		{name: "bad-limit", target: "/api/v3/weights?cardinality_limit=x", status: "400",
			guard: dashText(`{"error":"Weights limits must be nonnegative integers within the supported range."}`)},
	}
}

// weightsValues are the value method's weights over the highlight window: each dimension's average there, its absolute
// value (weights.c:190), of `fixture.weights` and, with ks2, of `fixture.weightsks2` too.
func weightsValues(ks2 bool) map[string]float64 {
	w := map[string]float64{"flat": 50, "level": 3611.0 / 121, "split": 33521.0 / 121, "anom": 20}
	if ks2 {
		w["flat2"], w["jump"] = 50, 300
	}
	return w
}

// weightsSwapped is the guard of the inverted window: the answer's window is the highlight window and its weights are
// both charts' highlight averages.
func weightsSwapped(v Value) error {
	window := dashGuard(dashMembers([]string{"request", "window"}, "after", weightsT(fixture.WeightsSplit), "before",
		weightsT(fixture.WeightsRows)))
	if err := window(v); err != nil {
		return err
	}
	return weightsV2Weigh(weightsValues(true), 1e-7)(v)
}

// weightsReady compares each route's first answer while startup runs: the handler answers 503 until the agent is ready
// (api_v2_weights.c:19-20).
func weightsReady(t *testing.T, routes ...string) {
	t.Helper()
	for _, r := range routes {
		t.Run("ready-"+strings.ReplaceAll(strings.TrimPrefix(r, "/api/"), "/", "-"), func(t *testing.T) {
			compareBeforeReady(t, r)
		})
	}
}

// TestWeightsV1API (check `api.weights-v1`, milestone 10 commit 9): the v1 routes on the weights fixtures.
func TestWeightsV1API(t *testing.T) {
	p := dashPair(t, daemon.Options{})
	weightsChild(t, p)
	weightsPush(t, p, child2Host, weightsLimitCharts(), weightsLimitCharts())
	weightsAsk(t, p, weightsV1Rows())
	t.Run("access", func(t *testing.T) {
		routes := []string{"/api/v1/weights", "/api/v1/metric_correlations"}
		accessRows(t, []accessConf{accessACL, accessBearer}, accessRoutes(routes...))
		weightsReady(t, routes...)
	})
}

// TestWeightsAPI (check `api.weights`, milestone 10 commit 10): the v2 and v3 routes on the weights fixtures.
func TestWeightsAPI(t *testing.T) {
	p := dashPair(t, weightsCPUs(2))
	weightsChild(t, p)
	weightsAsk(t, p, weightsAPIRows())
	t.Run("access", func(t *testing.T) {
		routes := []string{"/api/v2/weights", "/api/v3/weights"}
		accessRows(t, []accessConf{accessACL, accessBearer}, accessRoutes(routes...))
		weightsReady(t, routes...)
		t.Run("ready-v3-limit", func(t *testing.T) {
			compareBeforeReady(t, "/api/v3/weights?limit=x")
		})
	})
}
