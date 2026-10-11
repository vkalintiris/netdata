// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"maps"
	"net/url"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
)

// The pins of the closer rows of plugins.spawn (`env-claim`), api.settings (pair `errno`), api.v2-contexts and
// api.v2-q (the closer drafter SA-R's), on C's answers recorded C against C (dash_norm_closers_r_data_test.go), with
// no agent started.

// testDashNormClosersRSpawn pins `env-claim`'s guard (spawnRunsProblems over the variant's runs) on the
// NETDATA_REGISTRY_* lines of each child's environment as the two C agents gave them (dashNormClosersRSpawnRuns): the
// guard takes both C sides and refuses the named wrong agents it should see. For each named wrong agent it also holds
// that the planted runs differ from the recorded ones line for line, which is what runSpawnEnv's live comparison of
// the two sides' views (diffLines) reports; that comparison itself runs only against agents.
func testDashNormClosersRSpawn(t *testing.T) {
	v := spawnEnvClaim()
	start, reload := spawnClaimURL(spawnClaimStart), spawnClaimURL(spawnClaimReload)
	// the variant: its URLs differ, name reserved hosts (nothing resolves them, nothing is claimed) and are what
	// each run must hold; it reloads before the labels reload
	for _, u := range []string{spawnClaimStart, spawnClaimReload} {
		parsed, err := url.Parse(u)
		if err != nil || parsed.Scheme != "https" || !strings.HasSuffix(parsed.Hostname(), ".invalid") {
			t.Errorf("env-claim's cloud URL %q must be https on a .invalid host", u)
		}
	}
	if spawnClaimStart == spawnClaimReload || v.reload == nil || v.adjust == nil {
		t.Errorf("env-claim must start with one URL and reload another")
	}
	want := map[string][][]string{
		"system-info.sh":           {{start}, {start}},
		"get-kubernetes-labels.sh": {{start}, {reload}},
	}
	if !maps.EqualFunc(v.runs, want, func(a, b [][]string) bool {
		return slices.EqualFunc(a, b, func(x, y []string) bool { return slices.Equal(x, y) })
	}) {
		t.Errorf("env-claim's runs: %q, want %q", v.runs, want)
	}
	if got := slices.Sorted(maps.Keys(dashNormClosersRSpawnRuns)); !slices.Equal(got, slices.Sorted(maps.Keys(want))) {
		t.Fatalf("recorded scripts %v", got)
	}
	// the recorded C pair: alike, and each side holds the guard
	for script, sides := range dashNormClosersRSpawnRuns {
		if !slices.EqualFunc(sides[0], sides[1], func(a, b []string) bool { return slices.Equal(a, b) }) {
			t.Errorf("%s: the two C sides differ: %q, %q", script, sides[0], sides[1])
		}
		for i, runs := range sides {
			if p := spawnRunsProblems(script, v.runs[script], runs); len(p) > 0 {
				t.Errorf("%s: the guard on C's side %d: %q", script, i, p)
			}
		}
	}
	// the named wrong agents, each made of C's runs by an edit of one script's: whether the guard sees it (it reads
	// each run's lines) and that the comparison does
	edit := func(script string, change func(runs [][]string) [][]string) map[string][][]string {
		out := map[string][][]string{}
		for s, sides := range dashNormClosersRSpawnRuns {
			runs := make([][]string, len(sides[0]))
			for j, r := range sides[0] {
				runs[j] = slices.Clone(r)
			}
			if s == script {
				runs = change(runs)
			}
			out[s] = runs
		}
		return out
	}
	swap := func(j int, from, to string) func(runs [][]string) [][]string {
		return func(runs [][]string) [][]string {
			runs[j] = slices.Clone(runs[j])
			for k, l := range runs[j] {
				if l == from {
					runs[j][k] = to
				}
			}
			return runs
		}
	}
	for _, c := range []struct {
		what, script string
		change       func(runs [][]string) [][]string
		guarded      bool
	}{
		// planted bug r8 (the registry fixes' list): the reload copies the URL and does not export it
		{"r8: the URL copied at the reload, not exported", "get-kubernetes-labels.sh", swap(1, reload, start), true},
		{"the reload's URL exported from the start", "system-info.sh", swap(0, start, reload), true},
		{"the reload's URL in the first labels run", "get-kubernetes-labels.sh", swap(0, start, reload), true},
		{"no child after the reload", "get-kubernetes-labels.sh",
			func(runs [][]string) [][]string { return runs[:1] }, true},
		// nd_setenv overwrites in place (setenv(.., 1)): a port that appended the variable again would give the
		// child both lines
		{"the URL appended, not replaced", "get-kubernetes-labels.sh", func(runs [][]string) [][]string {
			runs[1] = slices.Sorted(slices.Values(append(slices.Clone(runs[1]), start)))
			return runs
		}, false},
	} {
		planted := edit(c.script, c.change)
		var problems []string
		for s, runs := range planted {
			problems = append(problems, spawnRunsProblems(s, v.runs[s], runs)...)
		}
		if (len(problems) > 0) != c.guarded {
			t.Errorf("%s: the guard says %q (it should see it: %v)", c.what, problems, c.guarded)
		}
		recorded := dashNormClosersRSpawnRuns[c.script][0]
		if slices.EqualFunc(recorded, planted[c.script], func(a, b []string) bool { return slices.Equal(a, b) }) {
			t.Errorf("%s: the comparison sees no difference", c.what)
		}
	}
}

// dashNormClosersRSetting is one recorded outcome of the `errno` pair's rows (both C sides alike).
type dashNormClosersRSetting struct {
	answer  string
	state   settingsState
	records []string
}

// testDashNormClosersRSettings pins the `errno` pair of api.settings: its rows as a table (settingsRowsProblem, their
// names and order, the pair's routing), and each row's guard and judge on C's recorded outcome
// (dashNormClosersRSettings): the C pair shows nothing; each of dashNormSettingsPlants' wrong outcomes is one
// difference and fails the guard where it reads it; and the named wrong agents below.
func testDashNormClosersRSettings(t *testing.T) {
	rows := settingsErrnoRows()
	if p := settingsRowsProblem(rows); p != "" {
		t.Errorf("errno: %s", p)
	}
	var names []string
	for _, r := range rows {
		names = append(names, r.name)
	}
	if got := strings.Join(names, " "); got != "control-get bearer-get bearer-put string-get string-put" {
		t.Errorf("the errno pair's rows: %s", got)
	}
	for _, pair := range settingsCloserPairs {
		if slices.Contains(settingsPairs, pair) {
			t.Errorf("pair %s is the check's own too", pair)
		}
	}
	if !slices.Equal(settingsCloserPairs, []string{"errno"}) || len(settingsCheckRows("errno")) != len(rows) ||
		len(settingsCheckRows("rows")) != len(settingsRowsOf("rows")) ||
		len(settingsCheckRows("first")) != len(settingsRowsOf("first")) {
		t.Errorf("the pairs' rows are routed elsewhere")
	}
	if got := slices.Sorted(maps.Keys(dashNormClosersRSettings)); !slices.Equal(got, slices.Sorted(slices.Values(names))) {
		t.Fatalf("recorded rows %v", got)
	}
	// what each row asks, as the recorded run asked it: the client of each (anonymous, an unknown token, the admin)
	for _, r := range rows {
		want := map[string]string{"control-get": "GET default", "bearer-get": "GET default " + setUnknownBearer,
			"bearer-put": "PUT default " + setUnknownBearer, "string-get": "GET st-strx " + dcAdmin,
			"string-put": "PUT st-strx " + dcAdmin}[r.name]
		method := r.method
		if method == "" {
			method = "GET"
		}
		if got := strings.TrimSpace(method + " " + strings.TrimPrefix(r.target, setPath+"?file=") + " " +
			strings.Join(r.headers, " ")); got != want || r.target != setFile(r.file) {
			t.Errorf("%s asks %q (file %q), want %q", r.name, got, r.file, want)
		}
	}
	errnoRe := regexp.MustCompile(` errno="[^"]*"`)
	for _, r := range rows {
		d := dashNormClosersRSettings[r.name]
		o := settingsOutcome{answer: []byte(d.answer), state: d.state, records: slices.Clone(d.records)}
		if oracle, diffs := settingsJudge(r, [2]settingsOutcome{o, o}); oracle != "" || len(diffs) > 0 {
			t.Errorf("%s: the recorded C pair: oracle %q, differences %q", r.name, oracle, diffs)
		}
		plants := dashNormSettingsPlants(r, o)
		// named: the port's record (no errno where json-c's reading sets none, daemon/src/settings.rs:66), and one
		// with another errno
		for k, l := range o.records {
			if m := errnoRe.FindString(l); m != "" {
				changed := slices.Clone(o.records)
				changed[k] = strings.Replace(l, m, ` errno="13, Permission denied"`, 1)
				plants = append(plants, dashNormSettingsPlant{"a record with another errno", true,
					settingsOutcome{answer: o.answer, state: o.state, records: changed}})
			}
			// the anonymous client's record taken for a token's: an account and a user
			if strings.Contains(l, " role=none ") {
				changed := slices.Clone(o.records)
				changed[k] = strings.Replace(l, " role=none ", " account=00000000000000000000000000000001 user=x role=none ", 1)
				plants = append(plants, dashNormSettingsPlant{"an account on the anonymous client's record", true,
					settingsOutcome{answer: o.answer, state: o.state, records: changed}})
			}
		}
		if len(plants) < 4 {
			t.Errorf("%s: %d wrong outcomes only", r.name, len(plants))
		}
		for _, plant := range plants {
			oracle, diffs := settingsJudge(r, [2]settingsOutcome{o, plant.outcome})
			if oracle != "" || len(diffs) != 1 {
				t.Errorf("%s: a candidate with %s: oracle %q, differences %q, want one difference", r.name,
					plant.what, oracle, diffs)
			}
			oracle, diffs = settingsJudge(r, [2]settingsOutcome{plant.outcome, o})
			if (oracle != "") != plant.guarded || (oracle == "" && len(diffs) != 1) {
				t.Errorf("%s: an oracle with %s: oracle %q, differences %q (the guard reads it: %v)", r.name,
					plant.what, oracle, diffs, plant.guarded)
			}
		}
	}
	// the facts the rows stand on, in C's records: no errno on the control, ENOENT after the unknown token, EINVAL
	// after the string version
	for name, errno := range map[string]string{"control-get": "", "bearer-get": "2, No such file or directory",
		"bearer-put": "2, No such file or directory", "string-get": "22, Invalid argument",
		"string-put": "22, Invalid argument"} {
		recs := dashNormClosersRSettings[name].records
		got := ""
		if len(recs) == 1 {
			if m := regexp.MustCompile(` errno="([^"]*)"`).FindStringSubmatch(recs[0]); m != nil {
				got = m[1]
			}
		}
		if len(recs) != 1 || got != errno {
			t.Errorf("%s: C's records %q, want one with errno %q", name, recs, errno)
		}
	}
}

// dashNormClosersRPlant is a named wrong answer of a closer row: C's recorded answer with one thing another agent
// would print (old replaced by new, once; an old written `re:<expression>` is a regular expression that must match
// once, for a text that holds the run's seconds).
type dashNormClosersRPlant struct{ what, old, new string }

// dashNormClosersRCases are the closer rows as their checks ask them (family and request), by
// `<check>/<case>/<row>`.
func dashNormClosersRCases(t *testing.T) map[string]contextsRow {
	t.Helper()
	// the bases are the recorded run's, read back from a window's target and from the gone child's first entry
	base := func(key, re string, offset int64) int64 {
		var text string
		if strings.HasPrefix(re, "after") {
			text = dashNormClosersRRows[key].target
		} else {
			text = dashNormClosersRRows[key].bodies[0]
		}
		m := regexp.MustCompile(re).FindStringSubmatch(text)
		if m == nil {
			t.Fatalf("the recorded %s holds no %s", key, re)
		}
		n, _ := strconv.ParseInt(m[1], 10, 64)
		return n - offset
	}
	obsolete := base("search/obsolete/obsolete-chart", `after=(\d+)&`, 61)
	gone := base("search/three/first-gone-child", `"first_entry":(\d+)`, 0)
	rows := map[string]contextsRow{}
	for prefix, list := range map[string][]contextsRow{
		"contexts/data/":   contextsCloserRows(),
		"contexts/three/":  contextsThreeRows(),
		"search/data/":     searchDataCloserRows(),
		"search/three/":    slices.Concat(searchThreeRows(), searchFirstGoneRows(gone)),
		"search/keep/":     searchKeepRows(),
		"search/obsolete/": searchObsoleteRows(obsolete),
	} {
		for _, r := range list {
			if _, twice := rows[prefix+r.req.name]; twice {
				t.Errorf("two rows named %s%s", prefix, r.req.name)
			}
			rows[prefix+r.req.name] = r
		}
	}
	return rows
}

// testDashNormClosersRSearch pins the closer rows of api.v2-contexts and api.v2-q: every row asks what was recorded,
// the two C sides show no difference under its family, its guard takes both, a row that answers no label is compared
// in order and one that answers labels as a set; each guard refuses the other rows' answers of its case; and the
// named wrong answers below, each refused by the guard and reported by the family.
func testDashNormClosersRSearch(t *testing.T) {
	rows := dashNormClosersRCases(t)
	keys := slices.Sorted(maps.Keys(rows))
	if recorded := slices.Sorted(maps.Keys(dashNormClosersRRows)); !slices.Equal(keys, recorded) {
		t.Fatalf("the rows are %v, the recorded ones %v", keys, recorded)
	}
	// read is a body as v2Round hands it to r's guard: normalised by r's family with the clock of the agent that
	// answered it (v2Clock over the seconds that answer was in flight: key's side), and parsed
	read := func(r contextsRow, key string, side int, body string) (Value, bool) {
		if !strings.HasPrefix(body, "{") {
			return Value{}, false
		}
		flight := dashNormClosersRRows[key].flight[side]
		v, err := ParseJSON(r.fam.normalise(0, r.fam.v2Clock(dashNormClosersRRows[key].target, flight), flight,
			[]byte(body)))
		if err != nil {
			t.Fatalf("%v: %s", err, body)
		}
		return v, true
	}
	differs := func(key, o, c string) string {
		r, rec := rows[key], dashNormClosersRRows[key]
		return dashNormDiffs(t, r.fam, r.fam.masks, rec.flight[0], o, c)
	}
	for _, key := range keys {
		r, rec := rows[key], dashNormClosersRRows[key]
		if r.req.target != rec.target || r.req.status != "200" || r.req.guard == nil {
			t.Errorf("%s asks %q (%s), recorded %q", key, r.req.target, r.req.status, rec.target)
			continue
		}
		if got := differs(key, rec.bodies[0], rec.bodies[1]); got != "" {
			t.Errorf("%s, the recorded pair: differences at %q", key, got)
		}
		for side, body := range rec.bodies {
			v, _ := read(r, key, side, body)
			if err := r.req.guard(v); err != nil {
				t.Errorf("the %s guard on C's answer (side %d): %v", key, side, err)
			}
		}
		// as TestDashNorm/search's rule: labels with an order to be each agent's are compared as a set, else in order
		labelled, onePair := searchOnePairLabels(t, rec.bodies[0])
		if (!labelled || onePair) && len(r.fam.unordered) > 0 {
			t.Errorf("%s answers no label set to order and its family leaves %v unordered", key, r.fam.unordered)
		} else if labelled && !onePair && !slices.Equal(r.fam.unordered, searchLabelsFamily.unordered) {
			t.Errorf("%s answers labels and its family leaves %v unordered", key, r.fam.unordered)
		}
	}
	// each guard refuses the recorded answers of the other rows of its check and case
	for _, key := range keys {
		for _, other := range keys {
			if key == other || key[:strings.LastIndex(key, "/")] != other[:strings.LastIndex(other, "/")] {
				continue
			}
			if v, ok := read(rows[key], other, 0, dashNormClosersRRows[other].bodies[0]); ok && rows[key].req.guard(v) == nil {
				t.Errorf("the %s guard took the answer of %s", key, other)
			}
		}
	}
	// the named wrong answers (dashNormClosersRPlants)
	for key, plants := range dashNormClosersRPlants {
		r, ok := rows[key]
		if !ok {
			t.Errorf("a wrong answer of %s, which is no row", key)
			continue
		}
		c := dashNormClosersRRows[key].bodies[0]
		for _, p := range plants {
			planted := strings.Replace(c, p.old, p.new, 1)
			if expr, ok := strings.CutPrefix(p.old, "re:"); ok {
				re := regexp.MustCompile(expr)
				if n := len(re.FindAllStringIndex(c, -1)); n != 1 {
					t.Errorf("%s, %s: %s matches C's answer %d times", key, p.what, expr, n)
					continue
				}
				planted = re.ReplaceAllLiteralString(c, p.new)
			}
			if planted == c {
				t.Errorf("%s, %s: C's answer does not hold %s", key, p.what, p.old)
				continue
			}
			if v, ok := read(r, key, 0, planted); ok && r.req.guard(v) == nil {
				t.Errorf("the %s guard took %s", key, p.what)
			}
			if got := differs(key, c, planted); got == "" {
				t.Errorf("%s: its family reports no difference for %s", key, p.what)
			}
		}
	}
	for _, key := range keys {
		if len(dashNormClosersRPlants[key]) == 0 {
			t.Errorf("%s has no named wrong answer", key)
		}
	}
}

// searchOnePairLabels reads a recorded search or contexts answer's body: whether a context of it has labels
// (`contexts.*.labels`), and whether each such context has one key of one value, which no agent can print in another
// order (the families: searchFamily compares those in order, searchLabelsFamily the others as sets).
func searchOnePairLabels(t *testing.T, body string) (labelled, onePair bool) {
	t.Helper()
	if !strings.HasPrefix(body, "{") {
		return false, false // a text (a 504's): no labels
	}
	v, err := ParseJSON([]byte(body))
	if err != nil {
		t.Fatalf("%v: %s", err, body)
	}
	onePair = true
	contexts, _ := dashMember(v, "contexts")
	for _, c := range contexts.Members {
		for _, m := range c.Value.Members {
			if m.Key != "labels" {
				continue
			}
			labelled = true
			if len(m.Value.Members) != 1 || len(m.Value.Members[0].Value.Items) != 1 {
				onePair = false
			}
		}
	}
	return labelled, labelled && onePair
}

// dashNormClosersRPlants are the named wrong answers of the closer v2 rows: what another agent would print, each made
// of C's recorded answer (dashNormClosersRRows) by one replacement.
var dashNormClosersRPlants = map[string][]dashNormClosersRPlant{
	"contexts/data/q-ignored": {
		{"q searched on the contexts route: nothing kept", `re:"contexts":\{"q\.ctx":\{[^}]*\}\}`, `"contexts":{}`},
		{"a search's matched list", `"live":true}}`, `"live":true,"matched":[]}}`},
	},
	"contexts/three/truncated2": {
		{"returned and remaining swapped", `"returned":2,"remaining":1`, `"returned":1,"remaining":2`},
		{"the total of those printed", `"total_contexts":3`, `"total_contexts":2`},
		{"no cut at two", `"__truncated__":{"total_contexts":3,"returned":2,"remaining":1}`,
			`"s.ctx":{"family":"sfam","units":"sunits","priority":1200,"first_entry":1791641040,` +
				`"last_entry":1791641280,"live":true}`},
	},
	"search/data/every-mcp": {
		{"matched printed under mcp", `"q.ctx":{"title":"title [x]",`, `"q.ctx":{"title":"title [x]","matched":["id"],`},
		{"the dimensions not cut under mcp", `"dimensions":["alpha","b","... 4 dimensions more"]`,
			`"dimensions":["alpha","b","z","inc","h","a"]`},
		{"an info text without a limit", `}},"searches":{`, `}},"info":"Cardinality limit reached.","searches":{`},
		{"the api printed", `{"nodes":[{"machine_guid"`, `{"api":2,"nodes":[{"machine_guid"`},
		{"the labels' matches not counted", `"strings":13,"char":12,"total":25`, `"strings":13,"char":0,"total":13`},
	},
	"search/three/truncated3": {
		{"returned and remaining swapped", `"returned":2,"remaining":1`, `"returned":1,"remaining":2`},
		{"the cut after one", `"r.ctx":{"family":"rfam","matched":["families"]},"__truncated__":{"total_contexts":3,` +
			`"returned":2,"remaining":1}`, `"__truncated__":{"total_contexts":3,"returned":1,"remaining":2}`},
		{"the third context's texts not tested", `"strings":60,"char":0,"total":60`, `"strings":45,"char":0,"total":45`},
	},
	"search/three/first-gone-child": {
		{"still collected", `"live":false`, `"live":true`},
		{"its retention lost", `re:"first_entry":[0-9]+`, `"first_entry":0`},
	},
	"search/three/first-gone": {
		{"keep new: the collected newcomer's title", `"title":"[x] [x]"`, `"title":"other [x]"`},
		{"keep old: the first host's title", `"title":"[x] [x]"`, `"title":"title [x]"`},
		{"the gone host's context not searched", `"strings":60,"char":0,"total":60`, `"strings":45,"char":0,"total":45`},
		{"the third context missed", `,"s.ctx":{"title":"s title [x]","matched":["title"]}`, ``},
	},
	"search/keep/second-gone": {
		{"always merged", `"title":"title [x]","matched":["title","instances"]`,
			`"title":"[x] [x]","matched":["title","instances"]`},
		{"the newcomer's match lost", `"matched":["title","instances"],"instances":["q.q_a_other"]`, `"matched":["title"]`},
	},
	"search/obsolete/obsolete-chart": {
		{"the obsolete chart taken as no longer collected", `"instances":["o.live","o.obs"],"dimensions":["d1","e1"]`,
			`"instances":["o.live"],"dimensions":["d1"]`},
		{"the obsolete dimension met", `"dimensions":["d1","e1"]`, `"dimensions":["d1","d2","e1"]`},
		{"the obsolete chart's labels left out", `re:"ol":\["(live","obs|obs","live)"\]`, `"ol":["live"]`},
		{"its texts not counted", `"strings":8,"char":12,"total":20`, `"strings":6,"char":6,"total":12`},
	},
}
