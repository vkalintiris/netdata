// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"slices"
	"strings"
	"testing"
)

// testDashNormRulesOrder pins the `rules-order` rows (D234 F5) on the answers and the alert logs of one C-against-C
// run (dash_norm_rules_order_data_test.go), without a daemon:
//   - each row asks what was asked when its answer was recorded, the `cut` row with each side's own end of the window
//     as alertsV2RulesOrderCutEnd reads it from that side's recorded log; the two recorded ends are one second, so the
//     rows are also asked for two ends that differ, and the reader is held on hand-made logs (the newest of two such
//     changes, whatever their order; no other alert's and no other change; 0 without one);
//   - each recorded C pair shows no difference under the alerts family, and the row's guard takes C's answer;
//   - the `rowid` guard refuses the Rust build's own recorded answer to that row (`configurations[]` in the order of
//     the rules' first appearance: the named wrong candidate), and the comparison of that answer with C's reports
//     `configurations[]` and nothing else; it refuses C's answer before the stop too;
//   - the `first-seen` guard refuses the rowid order (C's answer after the restart), and the `cut` guard the whole
//     window's answer.
func testDashNormRulesOrder(t *testing.T) {
	norm := func() *healthNorm {
		return &healthNorm{entries: map[int64]healthEntry{}, tids: map[string]int64{}, moved: map[string]int64{}}
	}
	// a recorded pair as compareV2 compares it: both bodies normalised by the alerts family over the recorded logs
	type pair struct {
		fam  v2Family
		body [2][]byte
		doc  [2]Value
	}
	pairOf := func(what string, bodies, logs [2]string, flight [2][2]int64) pair {
		t.Helper()
		log := func(i int) ([]byte, error) { return []byte(logs[i]), nil }
		p := pair{fam: alertsV2Family([2]*healthNorm{norm(), norm()}, log)}
		for i := range bodies {
			p.body[i] = p.fam.normalise(i, flight[i], flight[i], []byte(bodies[i]))
			v, err := ParseJSON(p.body[i])
			if err != nil {
				t.Fatalf("%s, side %d: %v: %s", what, i, err, p.body[i])
			}
			p.doc[i] = v
		}
		return p
	}
	diffs := func(p pair) []string {
		var out []string
		o, c := ApplyMasks(p.doc[0], p.fam.masks), ApplyMasks(p.doc[1], p.fam.masks)
		for _, d := range Compare(o, c, p.fam.unordered...) {
			out = append(out, d.Path)
		}
		if v2Layouts(p.body, p.fam.layoutByCount()) != "" {
			out = append(out, "layout")
		}
		if v2Escapes(p.body, p.doc, p.fam) != "" {
			out = append(out, "escapes")
		}
		return out
	}

	// the case's rows, the `cut` row's window ended as its recorded logs say
	cutLogs := dashNormRulesOrderRows["cut"].logs
	rows := map[string]v2Req{}
	ends := [2]int64{alertsV2RulesOrderCutEnd([]byte(cutLogs[0])), alertsV2RulesOrderCutEnd([]byte(cutLogs[1]))}
	for _, req := range alertsV2RulesOrderRows(ends) {
		if req.guard == nil || req.status != "200" {
			t.Errorf("%s: a row without a guard, or one that wants another status than 200", req.name)
		}
		rows[req.name] = req
	}
	got, want := slices.Sorted(maps.Keys(dashNormRulesOrderRows)), slices.Sorted(maps.Keys(rows))
	if !slices.Equal(got, want) || len(got) != 3 {
		t.Fatalf("the recorded rows are %q, the case's rows %q", got, want)
	}

	// each side is asked its own end of the cut window, and the other rows one request
	const cutAt = "/api/v2/alert_transitions?after=-600&before=%d&last=200&options=minify,config"
	for _, req := range alertsV2RulesOrderRows([2]int64{1700000001, 1700000004}) {
		want := [2]string{alertsV2RulesOrderWindow, alertsV2RulesOrderWindow}
		if req.name == "cut" {
			want = [2]string{fmt.Sprintf(cutAt, 1700000001), fmt.Sprintf(cutAt, 1700000004)}
		}
		if got := [2]string{req.targetOf(0), req.targetOf(1)}; got != want {
			t.Errorf("%s asks %q of two sides whose cut windows end at different seconds, want %q", req.name, got, want)
		}
	}
	// the end of the cut window, read from a log
	entry := func(name, old, status string, when int64) string {
		return fmt.Sprintf(`{"name":%q,"old_status":%q,"status":%q,"when":%d}`, name, old, status, when)
	}
	fall := func(name string, when int64) string { return entry(name, "WARNING", "CLEAR", when) }
	for _, c := range []struct {
		what, log string
		want      int64
	}{
		{"the newest of two, the older first", "[" + fall("hs_max", 100) + "," + fall("hs_max", 107) + "]", 107},
		{"the newest of two, the newer first", "[" + fall("hs_max", 107) + "," + fall("hs_max", 100) + "]", 107},
		{"hs_max's own return to CLEAR", "[" + fall("hs_calc", 300) + "," + fall("hs_max", 107) + "," +
			entry("hs_max", "CLEAR", "WARNING", 200) + "]", 107},
		{"no such change", "[" + fall("hs_calc", 300) + "," + entry("hs_max", "CLEAR", "WARNING", 200) + "]", 0},
		{"no log", "the alert log is not served", 0},
	} {
		if got := alertsV2RulesOrderCutEnd([]byte(c.log)); got != c.want {
			t.Errorf("the cut window's end, %s: %d, want %d", c.what, got, c.want)
		}
	}

	recorded := map[string]pair{}
	for _, key := range slices.Sorted(maps.Keys(dashNormRulesOrderRows)) {
		r := dashNormRulesOrderRows[key]
		for i := range r.targets {
			if got := rows[key].targetOf(i); got != r.targets[i] {
				t.Errorf("%s asks side %d %q, its answer was recorded for %q", key, i, got, r.targets[i])
			}
		}
		p := pairOf(key, r.bodies, r.logs, r.flight)
		recorded[key] = p
		if got := diffs(p); len(got) > 0 {
			t.Errorf("%s: the recorded C pair differs at %q", key, got)
		}
		if p.fam.check != nil {
			p.fam.check(t, key, p.doc[0], p.doc[1])
		}
		if err := rows[key].guard(p.doc[0]); err != nil {
			t.Errorf("%s: its guard refuses C's answer: %v", key, err)
		}
	}

	// the named wrong candidate: the Rust build's own answer to `rowid`, read against its own log
	rowid, w := dashNormRulesOrderRows["rowid"], dashNormRulesOrderWrong
	wrong := pairOf("the Rust build's rowid answer", [2]string{rowid.bodies[0], w.body},
		[2]string{rowid.logs[0], w.log}, [2][2]int64{rowid.flight[0], w.flight})
	if err := rows["rowid"].guard(wrong.doc[1]); err == nil || !strings.Contains(err.Error(), "configurations") {
		t.Errorf("the rowid guard on the Rust build's answer: %v, want it refused for its configurations", err)
	}
	apart := diffs(wrong)
	for _, path := range apart {
		if path != "layout" && !strings.HasPrefix(path, "$.configurations[") {
			t.Errorf("C's rowid answer and the Rust build's differ at %q, outside configurations[]", path)
		}
	}
	if !slices.Contains(apart, "$.configurations[0].name") {
		t.Errorf("C's rowid answer and the Rust build's differ at %q, not at the first rule's name", apart)
	}

	// what each guard refuses of C's own answers
	for _, refused := range []struct{ guard, answer string }{
		{"rowid", "first-seen"}, {"first-seen", "rowid"}, {"cut", "rowid"}, {"rowid", "cut"}, {"first-seen", "cut"},
	} {
		if err := rows[refused.guard].guard(recorded[refused.answer].doc[0]); err == nil {
			t.Errorf("the %s guard takes C's answer to %s", refused.guard, refused.answer)
		}
	}

	// and of each row's own answer with one member wrong: its transitions oldest first, a row counted as left out,
	// no rule listed
	planted := func(v Value, member string, change func(m *Value)) Value {
		out := v
		out.Members = slices.Clone(v.Members)
		for k := range out.Members {
			if out.Members[k].Key == member {
				m := out.Members[k].Value
				m.Items, m.Members = slices.Clone(m.Items), slices.Clone(m.Members)
				change(&m)
				out.Members[k].Value = m
			}
		}
		return out
	}
	for _, key := range slices.Sorted(maps.Keys(recorded)) {
		doc := recorded[key].doc[0]
		for what, wrong := range map[string]Value{
			"its transitions oldest first": planted(doc, "transitions", func(m *Value) { slices.Reverse(m.Items) }),
			"a row counted as left out": planted(doc, "items", func(m *Value) {
				for k := range m.Members {
					if m.Members[k].Key == "after" {
						m.Members[k].Value.Text = "1"
					}
				}
			}),
			"no rule listed": planted(doc, "configurations", func(m *Value) { m.Items = nil }),
		} {
			if err := rows[key].guard(wrong); err == nil {
				t.Errorf("the %s guard takes its answer with %s", key, what)
			}
		}
	}
}
