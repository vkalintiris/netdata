// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"
)

// infoTailLongRound is one recorded row of the long pair (dash_norm_info_long_data_test.go): its target, and each
// side's raw answer with the seconds it was in flight.
type infoTailLongRound struct {
	target string
	dashNormInfoTailExchange
}

// infoTailMethodsOf is the text C stores for the methods a configuration of lines sets to YES, as the script and C
// make it: the names in bytewise order (`${!SEND_@}` under LC_ALL=C), one a line, each line read in pieces of at most
// 199 bytes with its newline counted (fgets(line, 200): a 199-byte line gives an empty last piece), the pieces cut at
// their newline and joined with `|`, quoted (analytics.c:413-437).
func infoTailMethodsOf(lines []string) string {
	var names []string
	for _, l := range lines {
		if name, ok := strings.CutSuffix(l, `="YES"`); ok {
			names = append(names, name)
		}
	}
	slices.Sort(names)
	var pieces []string
	for _, name := range names {
		line := name + "\n"
		for len(line) > 0 {
			n := min(len(line), 199)
			pieces = append(pieces, strings.TrimSuffix(line[:n], "\n"))
			line = line[n:]
		}
	}
	return `"` + strings.Join(pieces, "|") + `"`
}

// infoTailLongJudge is what a round of row reports for the recorded answers x (dashNormInfoTailRound, then the
// family's buildinfo rule as the two sides were one binary).
func infoTailLongJudge(row infoTailLongRow, x dashNormInfoTailExchange) []string {
	a := dashNormInfoTailRound(x, row.req, row.fam)
	if len(a.problems) == 0 {
		a.problems = append(a.problems, infoBuildinfoJudge(a.doc[0], a.doc[1], true)...)
	}
	return a.problems
}

// testDashNormInfoLong pins the long pair of `api.v1-info-tail` (info_tail_long_test.go) on one C-against-C run of its
// rows (infoTailLongRecorded): the notifier configuration's rail, the recorded rows against the rows the check asks,
// each row's recorded pair, each guard against its neighbours' answers, and named wrong candidates.
func testDashNormInfoLong(t *testing.T) {
	t.Run("conf", func(t *testing.T) {
		conf, err := infoTailNotifyConf(infoTailNotifyLines)
		if err != nil || !strings.Contains(conf, "\nSEND_PARITY_A=\"YES\"\n") || strings.Count(conf, "\n") != 10 {
			t.Fatalf("the configuration: %v: %q", err, conf)
		}
		for _, bad := range []string{`SEND_PARITY_A="$(id)"`, "SEND_PARITY_A=\"`id`\"", `SEND_PARITY_A="YES"; id`,
			`SEND_PARITY_A=YES`, `SEND_PARITY_A="yes"`, ` SEND_PARITY_A="YES"`, `SEND_PARITY_A="YES" `,
			"SEND_PARITY_A=\"YES\"\nid", `use_fqdn="YES"`, `DEFAULT_RECIPIENT_EMAIL="root"`, `. /etc/profile`,
			`source /etc/profile`, `send_parity_a="YES"`, `SEND_="YES"`, `SEND_PARITY-A="YES"`, `# a comment`, ``,
			`SEND_EMAIL="YES"`, `SEND_CUSTOM="YES"`, `SEND_email="NO"`, `SEND_PARITY_="YES"`} {
			if _, err := infoTailNotifyConf(append(append([]string{}, infoTailNotifyLines...), bad)); err == nil {
				t.Errorf("the rail took %q", bad)
			}
		}
		for env, want := range map[string]string{"HOME=/x PATH=/y": "", "XSEND_A=YES": "", "SEND_X=YES": "SEND_X",
			"A=1 SEND_PARITY_A=": "SEND_PARITY_A"} {
			if got := infoTailNotifyEnv(strings.Fields(env)); got != want {
				t.Errorf("infoTailNotifyEnv(%s) = %q, want %q", env, got, want)
			}
		}
	})

	// the recorded rows are the check's, in its order, each asking what was recorded
	t.Run("rows", func(t *testing.T) {
		if len(infoTailLongRecorded) != len(infoTailLongRows) {
			t.Fatalf("%d rows recorded, the check asks %d", len(infoTailLongRecorded), len(infoTailLongRows))
		}
		for _, row := range infoTailLongRows {
			r, ok := infoTailLongRecorded[row.req.name]
			if !ok || r.target != row.req.target {
				t.Errorf("%s: recorded %v %q, the row asks %q", row.req.name, ok, r.target, row.req.target)
			}
		}
		for _, row := range infoTailLongRows {
			want := time.Duration(0)
			if strings.HasPrefix(row.req.name, "old") {
				want = infoTailOldSettle
			}
			check := reflect.ValueOf(row.fam.check).Pointer() == reflect.ValueOf(infoBuildinfoCheck).Pointer()
			if row.fam.settle != want || !row.fam.flat || len(row.fam.unordered) != 1 || len(row.fam.masks) != 1 ||
				!check {
				t.Errorf("%s: settle %v (want %v), flat %v, unordered %v, masks %v, the buildinfo check %v",
					row.req.name, row.fam.settle, want, row.fam.flat, row.fam.unordered, row.fam.masks, check)
			}
		}
		// `old` is asked past the 121st tick's latest second after the later side's readiness with a margin,
		// `before` well before the oracle's first, `charts` past the 10th tick's (infoTailOldAge's note)
		if infoTailOldAge < 122*time.Second || infoTailBeforeAge > 110*time.Second ||
			infoTailBeforeAge < 90*time.Second || infoTailAge < 11*time.Second {
			t.Errorf("ages: old %v, before %v, charts %v", infoTailOldAge, infoTailBeforeAge, infoTailAge)
		}
	})

	// each recorded C pair shows no difference, and its guard takes the oracle's answer
	t.Run("recorded", func(t *testing.T) {
		for _, row := range infoTailLongRows {
			r := infoTailLongRecorded[row.req.name]
			if got := infoTailLongJudge(row, r.dashNormInfoTailExchange); len(got) > 0 {
				t.Errorf("%s: the recorded C pair: %q", row.req.name, got)
			}
		}
		// the methods' text the guard holds is the recorded one: the names in bytewise order, cut at 199 bytes
		m, err := dashAt(infoTailLongDoc(t, "old", 0), "notification-methods")
		if err != nil || m.String() != infoTailOldMethods {
			t.Errorf("recorded methods %s (%v), infoTailOldMethods %s", m, err, infoTailOldMethods)
		}
		var pieces []int
		for _, piece := range strings.Split(strings.Trim(infoTailOldMethods, `"`), "|") {
			pieces = append(pieces, len(piece))
		}
		if fmt.Sprint(pieces) != "[13 13 198 199 0 199 51]" {
			t.Errorf("the methods' pieces: %v", pieces)
		}
		// and it is what the configuration's YES names make
		if got := infoTailMethodsOf(infoTailNotifyLines); got != m.String() {
			t.Errorf("the configuration makes %s, C answered %s", got, m)
		}
	})

	// each guard refuses the oracle's answer of every other row: a row asked in another phase or of another host
	t.Run("guards", func(t *testing.T) {
		for _, row := range infoTailLongRows {
			for _, other := range infoTailLongRows {
				if other.req.name == row.req.name {
					continue
				}
				if err := row.req.guard(infoTailLongDoc(t, other.req.name, 0)); err == nil {
					t.Errorf("%s's guard took %s's answer", row.req.name, other.req.name)
				}
			}
		}
		// each guard holds the members that never change (infoTailFixed) on its own row's answer
		for _, row := range infoTailLongRows {
			raw := infoTailLongRecorded[row.req.name].raw[0]
			for _, edit := range [][2]string{{`"agent-claimed":false`, `"agent-claimed":true`},
				{`"cloud-available":true`, `"cloud-available":false`}, {`"dashboard-used":0`, `"dashboard-used":1`}} {
				if err := row.req.guard(infoTailLongParse(t, strings.Replace(raw, edit[0], edit[1], 1))); err == nil {
					t.Errorf("%s's guard took %s", row.req.name, edit[1])
				}
			}
		}
		// the committed row's phase (no chart) is no `charts` phase
		empty := strings.Replace(infoTailLongRecorded["charts"].raw[0], `"collectors":[{`, `"collectors":[],"x":[{`, 1)
		if err := infoTailChartsGuard(infoTailLongParse(t, empty)); err == nil {
			t.Errorf("the charts guard took no collector")
		}
	})

	// a candidate's answer with one thing a port could get wrong is reported at that member
	t.Run("candidates", func(t *testing.T) {
		long, cut := infoTailLongName, infoTailOldMethods
		// item is a collector as C writes it into the recorded answers (buffer_json_add_array_item_object)
		item := func(plugin, module string) string {
			return "{\n            \"plugin\":\"" + plugin + "\",\n            \"module\":\"" + module + "\"\n        }"
		}
		for _, c := range []struct {
			row, why, old, new, path string
		}{
			{"charts", "deduplicated by the plugin alone", item("parity", "m2") + ",", ``, "$.collectors"},
			{"charts", "the pairs in another order", item("parity", "m1") + "," + item("parity", "m2"),
				item("parity", "m2") + "," + item("parity", "m1"), "$.collectors[1].module"},
			{"charts", "the hidden chart listed", item("a:b", "c"), item("hid", "h") + "," + item("a:b", "c"),
				"$.collectors"},
			{"charts", "the colliding pair listed", item("a:b", "c"), item("a:b", "c") + "," + item("a", "b:c"),
				"$.collectors"},
			{"charts", "the plugin named without its suffix", item("difftest.plugin", ""), item("difftest", ""),
				"$.collectors[0].plugin"},
			{"charts", "the charts counted at the request", `"charts-count":0`, `"charts-count":9`, "$.charts-count"},
			{"child", "localhost's pairs for the child", item("fixture-pusher", "corpus"), item("difftest.plugin", ""),
				"$.collectors[0]"},
			{"child", "localhost's memory mode", `"memory-mode":"ram"`, `"memory-mode":"dbengine"`, "$.memory-mode"},
			{"child", "the child's compression", `"stream-compression":false`, `"stream-compression":true`,
				"$.stream-compression"},
			{"before", "the first gather made early", `"notification-methods":null`,
				`"notification-methods":` + cut, "$.notification-methods"},
			{"before", "the charts counted at the request", `"charts-count":0`, `"charts-count":8`, "$.charts-count"},
			{"before", "the obsolete chart's pair at its first creation", item("parity", "m2") + "," +
				item("parity", "m1"), item("parity", "m1") + "," + item("parity", "m2"), "$.collectors[1].module"},
			{"old", "the methods joined with a comma", cut, strings.ReplaceAll(cut, "|", ","),
				"$.notification-methods"},
			{"old", "no cut at 199 bytes", cut, `"` + strings.Join([]string{"SEND_PARITY_A", "SEND_PARITY_B",
				long("SEND_PARITY_L", 198), long("SEND_PARITY_M", 199), long("SEND_PARITY_N", 250)}, "|") + `"`,
				"$.notification-methods"},
			{"old", "a 199-byte line without its empty piece", long("SEND_PARITY_M", 199) + "||",
				long("SEND_PARITY_M", 199) + "|", "$.notification-methods"},
			{"old", "the methods not gathered", cut, `null`, "$.notification-methods"},
			{"old", "the script's null printed bare", cut, `"null"`, "$.notification-methods"},
			{"old", "the obsolete chart's pair at its first creation", item("parity", "m2") + "," +
				item("parity", "m1"), item("parity", "m1") + "," + item("parity", "m2"), "$.collectors[1].module"},
			{"old", "the obsolete chart still counted", "\"charts-count\":8,\n    \"metrics-count\":9",
				"\"charts-count\":9,\n    \"metrics-count\":11", "$.charts-count"},
			{"old", "every host's charts counted", `"charts-count":8`, `"charts-count":10`, "$.charts-count"},
			{"old", "the hidden and obsolete dimensions counted", `"metrics-count":9`, `"metrics-count":11`,
				"$.metrics-count"},
			{"old", "the counts swapped", "\"charts-count\":8,\n    \"metrics-count\":9",
				"\"charts-count\":9,\n    \"metrics-count\":8", "$.charts-count"},
			{"old-child", "the routed child's own counts", `"charts-count":8`, `"charts-count":2`, "$.charts-count"},
			{"old-child", "the routed child's own methods", cut, `null`, "$.notification-methods"},
		} {
			t.Run(c.row+"/"+c.why, func(t *testing.T) {
				var row infoTailLongRow
				for _, r := range infoTailLongRows {
					if r.req.name == c.row {
						row = r
					}
				}
				r := infoTailLongRecorded[c.row]
				if strings.Count(r.raw[1], c.old) != 1 {
					t.Fatalf("harness: %q is not in the recorded candidate's answer once", c.old)
				}
				x := r.dashNormInfoTailExchange
				x.raw[1] = dashNormInfoTailWith(x.raw[1], strings.Replace(string(httpBody([]byte(x.raw[1]))), c.old,
					c.new, 1))
				got := strings.Join(infoTailLongJudge(row, x), "\n")
				if !strings.Contains(got, c.path) || strings.Contains(got, "Content-Length") {
					t.Errorf("not reported at %s (or reported at the length): %q", c.path, got)
				}
			})
		}
	})
}

// infoTailLongDoc is side i's recorded body of row name, parsed.
func infoTailLongDoc(t *testing.T, name string, i int) Value {
	t.Helper()
	return infoTailLongParse(t, infoTailLongRecorded[name].raw[i])
}

// infoTailLongParse is a raw answer's body, parsed.
func infoTailLongParse(t *testing.T, raw string) Value {
	t.Helper()
	v, err := ParseJSON(httpBody([]byte(raw)))
	if err != nil {
		t.Fatalf("%v: %q", err, truncateBytes([]byte(raw)))
	}
	return v
}
