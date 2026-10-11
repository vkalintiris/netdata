// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"maps"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
)

// dashNormSearchAlike are the groups of rows of one case that C answers alike, so that each one's guard takes the
// others' recorded answers. Every other guard refuses every other row's answer.
var dashNormSearchAlike = [][]string{
	// the word `alpha` in another spelling (between asterisks, beside an encoded separator, with a dropped
	// backslash, as the second `q`, before a byte that ends the request) or in another layout, every host listed
	{"data/v2-all", "data/comma", "data/backslash-plain", "data/ampersand", "data/control-cut", "data/minify"},
	// the child alone: by a host scope, and by a context selector that drops the parent
	{"data/v3-child", "data/contexts-beside"},
	// three ways to ask for no search
	{"data/all", "data/wordless", "data/star"},
	// words that match nothing
	{"data/nomatch", "data/negative", "data/backslash", "data/control"},
	// a limit that cuts nothing more
	{"data/cut", "data/cut2"},
	// no host: by a context scope, by a window
	{"data/nomatch-scoped", "data/window-negative"},
	// two dimensions without a stored name
	{"restart/nameless/null-name", "restart/nameless/empty-name"},
}

// dashNormSearchReversed is a search answer's body with the keys of every context's labels, and each key's values,
// in the opposite order (rendered compactly; a cut list's marker stays its last item, as C prints it); ok is false
// when no context has two keys or two values to turn.
func dashNormSearchReversed(t *testing.T, body string) (string, bool) {
	t.Helper()
	v, err := ParseJSON([]byte(body))
	if err != nil {
		t.Fatalf("%v: %s", err, body)
	}
	turned := false
	for i, top := range v.Members {
		if top.Key != "contexts" {
			continue
		}
		for j, context := range top.Value.Members {
			for k, m := range context.Value.Members {
				if m.Key != "labels" {
					continue
				}
				labels := slices.Clone(m.Value.Members)
				for n := range labels {
					values := slices.Clone(labels[n].Value.Items)
					last := len(values)
					if last > 0 && searchCutRe.MatchString(values[last-1].String()) {
						last--
					}
					turned = turned || last > 1
					slices.Reverse(values[:last])
					labels[n].Value.Items = values
				}
				turned = turned || len(labels) > 1
				slices.Reverse(labels)
				v.Members[i].Value.Members[j].Value.Members[k].Value.Members = labels
			}
		}
	}
	return v.String(), turned
}

// testDashNormSearch pins the families and the guards of `api.v2-q`'s rows (D234 F9 and review R104's requests:
// searchFamily, searchLabelsFamily, searchCutFamily, searchMCPFamily, searchFacts and every row's guard) on C's
// answers, recorded C against C (dashNormSearchRows).
func testDashNormSearch(t *testing.T) {
	// the rows as TestSearchAPI asks them, by `<case>/<row>`; the bases are the recorded run's, read back from a
	// window's target (every other target of the case is then held to its record)
	base := func(key string, offset int64) int64 {
		m := regexp.MustCompile(`after=(\d+)&`).FindStringSubmatch(dashNormSearchRows[key].target)
		if m == nil {
			t.Fatalf("the recorded %s names no window: %q", key, dashNormSearchRows[key].target)
		}
		after, _ := strconv.ParseInt(m[1], 10, 64)
		return after - offset
	}
	live, gone := searchWindowRows(base("window/collected", 61))
	dashboard := func() []contextsRow {
		var out []contextsRow
		for _, r := range searchRows() {
			out = append(out, contextsRow{r, searchFamily})
		}
		return out
	}
	rows := map[string]contextsRow{}
	for name, list := range map[string][]contextsRow{
		"data":             slices.Concat(dashboard(), searchDataRows(base("data/window-every", 10))),
		"merge":            searchMergeRows(),
		"labels":           searchLabelRows(),
		"window":           slices.Concat(live, gone),
		"restart":          searchStoredRows(),
		"restart/nameless": searchNamelessRows(),
	} {
		for _, r := range list {
			if _, twice := rows[name+"/"+r.req.name]; twice {
				t.Errorf("two rows named %s/%s", name, r.req.name)
			}
			rows[name+"/"+r.req.name] = r
		}
	}
	keys := slices.Sorted(maps.Keys(rows))
	if recorded := slices.Sorted(maps.Keys(dashNormSearchRows)); !slices.Equal(keys, recorded) {
		t.Fatalf("the rows are %v, the recorded ones %v", keys, recorded)
	}
	if len(keys) != 65 {
		t.Errorf("%d rows, pinned 65", len(keys))
	}
	// read is a body as v2Round hands it to r's guard: a 200's normalised by the family and parsed, any other
	// answer's as a text; ok is false for a body of the other kind (a text for a 200's guard, JSON for another's)
	read := func(r contextsRow, body string) (v Value, ok bool) {
		document := strings.HasPrefix(body, "{")
		if r.req.status != "200" {
			return Value{Kind: KindString, Text: body}, !document
		}
		if !document {
			return Value{}, false
		}
		v, err := ParseJSON(r.fam.normalise(0, [2]int64{}, [2]int64{}, []byte(body)))
		if err != nil {
			t.Fatalf("%v: %s", err, body)
		}
		return v, true
	}
	// differs tells where a row's family reports a difference between two bodies
	differs := func(key string, o, c string) string {
		r, rec := rows[key], dashNormSearchRows[key]
		if r.req.status != "200" {
			if o != c {
				return "the body"
			}
			return ""
		}
		return dashNormDiffs(t, r.fam, r.fam.masks, rec.flight[0], o, c)
	}

	// inOrder tells a body that answers no label, or one key of one value in each context (searchOnePairLabels)
	inOrder := func(body string) bool {
		labelled, onePair := searchOnePairLabels(t, body)
		return !labelled || onePair
	}
	// each row: it asks what was recorded, the two C sides show no difference, and its guard takes both
	for _, key := range keys {
		r, rec := rows[key], dashNormSearchRows[key]
		if r.req.target != rec.target {
			t.Errorf("%s asks %q, recorded %q", key, r.req.target, rec.target)
		}
		if r.req.guard == nil {
			t.Fatalf("%s has no guard", key)
		}
		if got := differs(key, rec.bodies[0], rec.bodies[1]); got != "" {
			t.Errorf("%s, the recorded pair: differences at %q", key, got)
		}
		for side, body := range rec.bodies {
			v, _ := read(r, body)
			if err := r.req.guard(v); err != nil {
				t.Errorf("the %s guard on C's answer (side %d): %v", key, side, err)
			}
		}
		// a row that answers no label, or one key of one value in each context, is compared in order, whole
		if inOrder(rec.bodies[0]) && len(r.fam.unordered) > 0 {
			t.Errorf("%s answers no label set to order and its family leaves %v unordered", key, r.fam.unordered)
		}
		if !inOrder(rec.bodies[0]) && !slices.Equal(r.fam.unordered, searchLabelsFamily.unordered) {
			t.Errorf("%s answers labels and its family leaves %v unordered", key, r.fam.unordered)
		}
	}

	// each guard refuses the recorded answer of every other row of its case, but for the rows C answers alike
	var alike []string
	for _, key := range keys {
		for _, other := range keys {
			if key == other || strings.SplitN(key, "/", 2)[0] != strings.SplitN(other, "/", 2)[0] {
				continue
			}
			v, ok := read(rows[key], dashNormSearchRows[other].bodies[0])
			if ok && rows[key].req.guard(v) == nil {
				alike = append(alike, key+" < "+other)
			}
		}
	}
	slices.Sort(alike)
	var want []string
	for _, group := range dashNormSearchAlike {
		for _, guard := range group {
			for _, answer := range group {
				if guard != answer {
					want = append(want, guard+" < "+answer)
				}
			}
		}
	}
	slices.Sort(want)
	if !slices.Equal(alike, want) {
		for _, pair := range alike {
			if !slices.Contains(want, pair) {
				t.Errorf("a guard took another row's answer: %s", pair)
			}
		}
		for _, pair := range want {
			if !slices.Contains(alike, pair) {
				t.Errorf("pinned as answered alike, and refused: %s", pair)
			}
		}
	}

	// the named wrong candidates: C's recorded answer with one thing another port would print (old, new). The row's
	// guard refuses it as the oracle's answer, and the row's family reports it as the candidate's
	type plant struct{ what, old, new string }
	const (
		c13, c15, c16 = `"strings":13,"char":0,"total":13`, `"strings":15,"char":0,"total":15`,
			`"strings":16,"char":0,"total":16`
		noContext = `"contexts":{}`
		unsought  = `"contexts":{"q.ctx":{"matched":[]}}`
		alpha     = `"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}}`
		childOnly = `"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,`
		parentToo = `"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,` +
			`"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,`
		echo = `"filters":{"q":"alpha","after":0,"before":0}`
	)
	listed := []plant{{"the child listed", `"nodes":[]`, `"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb",` +
		`"nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}]`}}
	for key, plants := range map[string][]plant{
		"data/v2-all": {
			{"the dimension's id", `"dimensions":["alpha"]`, `"dimensions":["a"]`},
			{"a name not tested", c15, `"strings":14,"char":0,"total":14`},
			{"the labels' tests counted", c15, `"strings":15,"char":12,"total":27`},
			{"matched by its id too", `"matched":["dimensions"]`, `"matched":["id","dimensions"]`},
			{"no searches", `"searches":{` + c15 + `},`, ``},
		},
		"data/v3-child": {{"the parent listed", childOnly, parentToo}},
		"data/plain": {
			{"a name tested although its id matched", c13, c15},
			{"the instance's id stored", `"instances":["q.q_a_name"]`, `"instances":["q.a"]`},
			{"the dimensions' ids stored", `"dimensions":["alpha","a"]`, `"dimensions":["a"]`},
			{"a word matched whole or by its case: nothing found", `"contexts":{"q.ctx":{"family":"fam","matched":[` +
				`"families","instances","dimensions"],"instances":["q.q_a_name"],"dimensions":["alpha","a"]}}`, noContext},
			{"the family named family", `"matched":["families",`, `"matched":["family",`},
			{"the family not printed", `"family":"fam",`, ``},
			{"an empty label list printed", `"dimensions":["alpha","a"]}`, `"dimensions":["alpha","a"],"labels":{}}`},
		},
		"data/label-key": {
			{"the total without the labels' matches", `"char":2,"total":17`, `"char":2,"total":15`},
			{"the labels' tests counted", `"char":2,"total":17`, `"char":12,"total":27`},
			{"the whole label set", `"labels":{"k":`, `"labels":{"_collect_plugin":["fixture-pusher"],"k":`},
			{"labels named label", `"matched":["labels"]`, `"matched":["label"]`},
			{"a family beside the labels", `{"matched":["labels"],`, `{"family":"fam","matched":["labels"],`},
			{"a second context", `]}}},"searches"`, `]}},"x.ctx":{"matched":[]}},"searches"`},
			{"no api", `{"api":2,`, `{`},
		},
		"data/label-value": {
			{"the key's other value too", `"char":1,"total":16`, `"char":2,"total":17`},
			{"values not searched", `"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v2"]}}}`, noContext},
		},
		"data/nomatch": {
			{"an unmatched context kept", noContext, unsought},
			{"the parent dropped", parentToo, childOnly},
			{"nothing counted", c15, `"strings":0,"char":0,"total":0`},
			{"a request echoed without debug", `{"api":2,"nodes"`, `{"api":2,"request":{},"nodes"`},
			{"no api", `{"api":2,`, `{`},
		},
		"data/nomatch-scoped": listed,
		"data/contexts-beside": {
			{"the parent listed", childOnly, parentToo},
			{"the context filtered", alpha, noContext},
		},
		"data/fields": {
			{"matched in the members' order", `"matched":["title","units","families"]`,
				`"matched":["title","families","units"]`},
			{"the family named family", `"families"]`, `"family"]`},
			{"the title not printed", `"title":"title [x]",`, ``},
			{"the first chart's title", `"title":"title [x]"`, `"title":"title a"`},
		},
		"data/cut": {
			{"no cut without a limit", `["alpha","b","... 2 dimensions more"]`, `["alpha","b","z","inc"]`},
			{"the cut after the limit's items", `["alpha","b","... 2 dimensions more"]`,
				`["alpha","b","z","... 1 dimensions more"]`},
			{"the marker's word", `... 2 dimensions more`, `... 2 instances more`},
		},
		"data/cut4": {{"cut at 3 whatever the cardinality", `["alpha","b","z","inc"]`,
			`["alpha","b","... 2 dimensions more"]`}},
		"data/cut2": {{"the limit without its floor of 3", `["alpha","b","... 2 dimensions more"]`,
			`["alpha","... 3 dimensions more"]`}},
		"data/all": {
			{"matched left out", `{"matched":[]}`, `{}`},
			{"the texts tested", `"strings":0,"char":0,"total":0`, c15},
			{"no context", unsought, noContext},
		},
		"data/wordless": {{"a pattern that matches nothing", unsought + `,"searches":{"strings":0,"char":0,"total":0}`,
			noContext + `,"searches":{` + c15 + `}`}},
		"data/star": {{"a pattern that matches everything", `{"matched":[]}`, `{"matched":["id"]}`}},
		"data/debug": {
			{"q after the window", echo, `"filters":{"after":0,"before":0,"q":"alpha"}`},
			{"the options in the request's order", `"options":["debug","instances","dimensions","labels","titles",` +
				`"family","units"]`, `"options":["family","units","titles","labels","instances","dimensions","debug"]`},
			{"the contexts' mode", `"nodes","search"]`, `"nodes","contexts"]`},
			{"no context selector echoed", `"selectors":{"nodes":null,"contexts":null}`, `"selectors":{"nodes":null}`},
			{"no request", `"request":{`, `"asked":{`},
			{"no api", `{"api":2,`, `{`},
		},
		"data/debug-noq": {
			{"q left out", `"filters":{"q":null,`, `"filters":{`},
			{"q as an empty text", `"filters":{"q":null,`, `"filters":{"q":"",`},
		},
		"data/debug-selectors": {
			{"the window made absolute", `"after":-600,"before":0`, `"after":1791476000,"before":1791476600`},
			{"a pattern's text lost", `"contexts":"*ctx"`, `"contexts":null`},
			{"the parent listed", childOnly, parentToo},
		},
		"data/mcp": {
			{"matched printed", `{"q.ctx":{"dimensions":["alpha"]}}`, `{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}}`},
			{"the api printed", `{"nodes":[{"machine_guid"`, `{"api":2,"nodes":[{"machine_guid"`},
			{"a node as elsewhere", `"relationship":"child","connected":true`, `"relationship":"child"`},
		},
		"data/every": {
			{"the labels' tests counted", `"strings":13,"char":12,"total":25`, `"strings":13,"char":0,"total":13`},
			{"a name tested beside its id", `"strings":13,"char":12,"total":25`, `"strings":15,"char":12,"total":27`},
			{"the instances' ids", `"instances":["q.q_a_name","q.two"]`, `"instances":["q.a","q.two"]`},
			{"the dimensions whole", `["alpha","b","... 4 dimensions more"]`, `["alpha","b","z","inc","h","a"]`},
			{"labels before the lists", `"matched":["id","title","units","families","instances","dimensions","labels"]`,
				`"matched":["id","title","units","families","labels","instances","dimensions"]`},
		},
		"data/negative": {{"a negative word as a match", noContext, unsought}, {"nothing tested", c15,
			`"strings":0,"char":0,"total":0`}},
		"data/negative-first": {
			{"the family matched by the later word", `"title":"title [x]","units"`, `"title":"title [x]","family":"fam","units"`},
			{"families among the matched", `["id","title","units","instances",`, `["id","title","units","families","instances",`},
		},
		"data/window-every": {
			{"the parent listed under a window", childOnly, parentToo},
			{"the dimensions whole", `["alpha","b","... 4 dimensions more"]`, `["alpha","b","z","inc","h","a"]`},
		},
		"data/window-negative": listed,
		"data/window-before": append([]plant{{"the context tested", `"strings":0,"char":0,"total":0`,
			`"strings":13,"char":12,"total":25`}}, listed...),
		"data/comma":           {{"the comma not a separator", alpha, noContext}},
		"data/plus":            {{"the plus kept", `"contexts":{"q.ctx":{"title":"title [x]","matched":["title"]}}`, noContext}},
		"data/backslash":       {{"the comma a separator all the same", noContext, alpha}},
		"data/backslash-plain": {{"the backslash kept", alpha, noContext}},
		"data/ampersand":       {{"the parameters split before decoding", alpha, noContext}},
		"data/control":         {{"the newline a separator", noContext, alpha}},
		"data/control-cut": {
			{"the scope read after the byte", parentToo, childOnly},
			{"the word after the byte too", alpha, noContext},
		},
		"data/minify": {{"another count", c15, c13}},
		"data/long-keys": {
			{"the nodes' short keys", `"hostname":"parity-child","nodes_array_index":1`, `"nm":"parity-child","ni":1`},
			{"the status' short key", `"nodes_array_index":1,"status":{"agents_array_index":0`,
				`"nodes_array_index":1,"status":{"ai":0`},
			{"the status named st", `"nodes_array_index":1,"status":{`, `"nodes_array_index":1,"st":{`},
			{"the agent's long keys", `"now":`, `"current_time":`},
			{"the agent's index by its long key", `,"ai":0,"timings"`, `,"agents_array_index":0,"timings"`},
			{"the child numbered 0", `"hostname":"parity-child","nodes_array_index":1`,
				`"hostname":"parity-child","nodes_array_index":0`},
			{"the parent's index by its short key", `"hostname":"parity-parent","nodes_array_index":0`,
				`"hostname":"parity-parent","ni":0`},
			{"another host second", `"hostname":"parity-child","nodes_array_index":1`,
				`"hostname":"parity-child2","nodes_array_index":1`},
			{"another count", c15, c13},
		},
		"merge/union": {
			{"the later host's names alone", `["q.q_a_name","q.q_a_other"]`, `["q.q_a_other"]`},
			{"the first host's names alone", `["q.q_a_name","q.q_a_other"]`, `["q.q_a_name"]`},
			{"the later host's names first", `["q.q_a_name","q.q_a_other"]`, `["q.q_a_other","q.q_a_name"]`},
			{"one host's tests", `"strings":45,"char":0,"total":45`, `"strings":30,"char":0,"total":30`},
		},
		"merge/truncated": {
			{"returned off by one", `"returned":1,"remaining":1`, `"returned":2,"remaining":0`},
			{"no cut", `,"__truncated__":{"total_contexts":2,"returned":1,"remaining":1}`,
				`,"r.ctx":{"family":"rfam","matched":["families"]}`},
			{"the first host's family", `"family":"fam[x]"`, `"family":"fam"`},
		},
		"merge/truncated-mcp": {
			{"no text", `"info":"Cardinality limit reached. Use cardinality_limit parameter to see more results.",`, ``},
			{"matched printed", `{"family":"fam[x]"}`, `{"family":"fam[x]","matched":["families"]}`},
			{"the api printed", `{"nodes":[{"machine_guid"`, `{"api":2,"nodes":[{"machine_guid"`},
		},
		"merge/titles": {
			{"the first host's title", `"title":"[x] [x]"`, `"title":"title [x]"`},
			{"the newcomer's title", `"title":"[x] [x]"`, `"title":"other [x]"`},
		},
		"merge/title-one": {{"every host's titles merged", `"title":"other [x]"`, `"title":"[x] [x]"`}},
		"merge/a-or-b": {
			{"the first host's family", `"family":"fam[x]"`, `"family":"fam"`},
			{"a title that matched nowhere", `{"family":"fam[x]",`, `{"title":"[x] [x]","family":"fam[x]",`},
			{"the dimensions twice", `"dimensions":["alpha","b","a"]},"r.ctx"`, `"dimensions":["alpha","b","a","alpha","b","a"]},"r.ctx"`},
		},
		"merge/labels": {
			{"a chart pair's matches once", `"char":6,"total":51`, `"char":2,"total":47`},
			{"another key in one context", `"r.ctx":{"matched":["labels"],"labels":{"k":[`,
				`"r.ctx":{"matched":["labels"],"labels":{"x":[`},
		},
		"merge/every-shared": {
			{"the limit of 3", `"dimensions":["alpha","b","z","... 3 dimensions more"]`,
				`"dimensions":["alpha","b","... 4 dimensions more"]`},
			{"the cardinality as the limit", `"dimensions":["alpha","b","z","... 3 dimensions more"]`,
				`"dimensions":["alpha","b","z","inc","h","a"]`},
			{"the instances cut at 3", `["q.q_a_name","q.two","q.q_a_other"]`, `["q.q_a_name","q.two","... 1 instances more"]`},
		},
		"labels/four": {
			{"the cut after the limit's values", `"... 2 values more"]`, `"w9","... 1 values more"]`},
			{"the values whole", `"... 2 values more"]`, `"w8","w9"]`},
			{"the marker's word", `... 2 values more`, `... 2 labels more`},
			{"the tests counted", `"char":4,"total":16`, `"char":16,"total":28`},
		},
		"labels/four-whole": {{"the limit of 3", `"char":4,"total":16`, `"char":3,"total":15`}},
		"labels/five": {
			{"the first host's count", `... 3 values more`, `... 2 values more`},
			{"the second child's value not counted", `"char":5,"total":25`, `"char":4,"total":24`},
		},
		"labels/five-whole": {{"the second child's tests", `"strings":20,`, `"strings":19,`}},
		"labels/three": {
			{"a list of the limit's length cut", `"x3"`, `"... 2 values more"`},
		},
		"labels/take": {
			{"the first host's title", `"title":"[x]"`, `"title":"ltitle c[x]"`},
			{"the newcomer's labels lost", `"matched":["title","labels"]`, `"matched":["title"]`},
		},
		"labels/keep": {
			{"the newcomer's title", `"title":"[x]"`, `"title":"mtitle c5"`},
			{"the title not matched", `"matched":["title","labels"]`, `"matched":["labels"]`},
		},
		"labels/every": {
			{"the instances whole", `["l.c1","l.c2","... 3 instances more"]`, `["l.c1","l.c2","l.c3","l.c4","l.c5"]`},
			{"the marker's word", `... 3 instances more`, `... 3 dimensions more`},
			{"the dimension's id", `"dimensions":["d","epsilon"]`, `"dimensions":["d","e"]`},
			{"the first host's units merged", `"units":"lunits"`, `"units":"[x]units"`},
		},
		"labels/fold-ascii": {
			{"no folding beside other bytes", `"contexts":{"l.ctx":{"matched":["labels"],"labels":{"u":["Grün"]}}}`,
				noContext},
			{"the value as bytes of Latin-1", `Grün`, "Gr\u00c3\u00bcn"},
		},
		"labels/fold-bytes": {
			{"the letter folded", noContext, `"contexts":{"l.ctx":{"matched":["labels"],"labels":{"u":["Grün"]}}}`},
		},
		"window/collected": {
			{"the old chart skipped", `"instances":["w.old","w.a"]`, `"instances":["w.a"]`},
			{"the stopped dimension skipped", `"dimensions":["o1","p1","p2"]`, `"dimensions":["o1","p1"]`},
			{"the obsolete dimension met", `"dimensions":["o1","p1","p2"]`, `"dimensions":["o1","p1","... 2 dimensions more"]`},
			{"the obsolete dimension tested", `"strings":9,"char":12,"total":21`, `"strings":10,"char":12,"total":22`},
			{"the parent listed", childOnly, parentToo},
		},
		"window/gone-late": listed,
		"window/stopped": {
			{"the old chart met", `"instances":["w.a"]`, `"instances":["w.old","w.a"]`},
			{"the stopped dimension met", `"dimensions":["p1","p3"]`, `"dimensions":["p1","p2","p3"]`},
			{"the stopped dimension tested", `"strings":7,"char":6,"total":13`, `"strings":8,"char":6,"total":14`},
			{"the old chart's labels", `"strings":7,"char":6,"total":13`, `"strings":7,"char":12,"total":19`},
		},
		"window/stopped-edge": {
			{"the window's first second left out", `"dimensions":["p1","p2","p3"]`, `"dimensions":["p1","p3"]`},
		},
		"window/old": {
			{"the later chart met", `"instances":["w.old"]`, `"instances":["w.old","w.a"]`},
			{"the later chart's dimensions", `"dimensions":["o1"]`, `"dimensions":["o1","p1","... 2 dimensions more"]`},
		},
		"window/between": {
			{"the list's slack of a second", `"matched":["id","title","units","families"]`,
				`"matched":["id","title","units","families","instances"],"instances":["w.old"]`},
			{"empty lists printed", `"matched":["id","title","units","families"]`,
				`"matched":["id","title","units","families"],"instances":[],"dimensions":[]`},
			{"nothing tested", `"strings":4,"char":0,"total":4`, `"strings":0,"char":0,"total":0`},
			{"the first chart's title", `"title":"wtitle [x]"`, `"title":"wtitle old"`},
			{"the parent listed", childOnly, parentToo},
		},
		"restart/stored-labels": {
			{"no stored label matched", `"char":2,"total":17`, `"char":0,"total":15`},
			{"another stored key", `"matched":["labels"],"labels":{"k":[`, `"matched":["labels"],"labels":{"x":[`},
		},
		"restart/stored-name": {
			{"a live chart's name", `"instances":["q_a_name"]`, `"instances":["q.q_a_name"]`},
			{"the id", `"instances":["q_a_name"]`, `"instances":["q.a"]`},
		},
		"restart/nameless/null-name": {
			{"the id for the name", `{"matched":["dimensions"]}`, `{"matched":["dimensions"],"dimensions":["z"]}`},
			{"an empty name stored", `{"matched":["dimensions"]}`, `{"matched":["dimensions"],"dimensions":[""]}`},
			{"an empty list printed", `{"matched":["dimensions"]}`, `{"matched":["dimensions"],"dimensions":[]}`},
			{"the bit lost", `"contexts":{"q.ctx":{"matched":["dimensions"]}}`, noContext},
			{"the empty names not tested", c16, c15},
		},
		"restart/nameless/empty-name": {
			{"the id for the name", `{"matched":["dimensions"]}`, `{"matched":["dimensions"],"dimensions":["inc"]}`},
			{"the empty names not tested", c16, `"strings":14,"char":0,"total":14`},
		},
		"restart/nameless/names-left": {
			{"the ids for the names", `["alpha","b","... 2 dimensions more"]`, `["alpha","b","... 4 dimensions more"]`},
			{"the live names", `"instances":["q_a_name","q.two"]`, `"instances":["q.q_a_name","q.two"]`},
		},
	} {
		r, ok := rows[key]
		if !ok {
			t.Errorf("a wrong candidate of %s, which is no row", key)
			continue
		}
		c := dashNormSearchRows[key].bodies[0]
		for _, p := range plants {
			planted := strings.Replace(c, p.old, p.new, 1)
			if planted == c {
				t.Errorf("%s, %s: C's answer does not hold %s", key, p.what, p.old)
				continue
			}
			v, _ := read(r, planted)
			if err := r.req.guard(v); err == nil {
				t.Errorf("the %s guard took %s", key, p.what)
			}
			if got := differs(key, c, planted); got == "" {
				t.Errorf("%s: its family reports no difference for %s", key, p.what)
			}
		}
	}

	// the timeout's answer is its text, whole
	timeout := rows["data/timeout"]
	for name, c := range map[string]struct {
		body string
		bad  bool
	}{"C's answer": {dashNormSearchRows["data/timeout"].bodies[0], false}, "an answer in JSON": {`{"api":2}`, true},
		"another text": {"timeout", true}} {
		if err := timeout.req.guard(Value{Kind: KindString, Text: c.body}); (err != nil) != c.bad {
			t.Errorf("the timeout guard, %s: %v", name, err)
		}
	}
	if timeout.req.status != "504" || dashNormSearchRows["data/timeout"].bodies[0] != nodesTimeout {
		t.Errorf("the timeout row asks for a %s; C answered %q", timeout.req.status,
			dashNormSearchRows["data/timeout"].bodies[0])
	}

	// the label family: an answer's labels are a set, keys and values; `matched`, the instances, the dimensions and
	// the cut markers are compared in order. Every row that answers labels with something to turn takes them in the
	// opposite order; searchFamily reports that order
	turnedRows := 0
	for _, key := range keys {
		c := dashNormSearchRows[key].bodies[0]
		if !strings.Contains(c, `"labels":{`) {
			continue
		}
		reversed, turned := dashNormSearchReversed(t, c)
		if !turned {
			continue
		}
		turnedRows++
		if got := differs(key, c, reversed); got != "" {
			t.Errorf("%s, the labels in the opposite order: differences at %q", key, got)
		}
		if got := dashNormDiffs(t, searchFamily, searchFamily.masks, [2]int64{}, c, reversed); got == "" {
			t.Errorf("%s, the labels in the opposite order: searchFamily reports no difference", key)
		}
		v, _ := read(rows[key], reversed)
		if err := rows[key].req.guard(v); err != nil {
			t.Errorf("the %s guard on C's answer with its labels in the opposite order: %v", key, err)
		}
	}
	if turnedRows != 19 {
		t.Errorf("%d rows answer labels that can be turned, pinned 19", turnedRows)
	}
	every := dashNormSearchRows["data/every"].bodies[0]
	for name, c := range map[string]struct {
		old, new string
		fam      v2Family
		want     string
	}{
		"a label's value":        {`"v2"`, `"v3"`, searchLabelsFamily, "$.contexts.q.ctx.labels.k[1]"},
		"a value more":           {`"corpus"]`, `"corpus","other"]`, searchLabelsFamily, "$.contexts.q.ctx.labels._collect_module.length"},
		"a label missing":        {`"_collect_module":["corpus"]`, `"x":["corpus"]`, searchLabelsFamily, "$.contexts.q.ctx.labels.<members>"},
		"the instances' order":   {`["q.q_a_name","q.two"]`, `["q.two","q.q_a_name"]`, searchLabelsFamily, "$.contexts.q.ctx.instances[0] $.contexts.q.ctx.instances[1]"},
		"the dimensions' order":  {`["alpha","b",`, `["b","alpha",`, searchLabelsFamily, "$.contexts.q.ctx.dimensions[0] $.contexts.q.ctx.dimensions[1]"},
		"the dimensions' marker": {`... 4 dimensions more`, `... 3 dimensions more`, searchLabelsFamily, "$.contexts.q.ctx.dimensions[2]"},
		"matched's order":        {`"instances","dimensions","labels"]`, `"dimensions","instances","labels"]`, searchLabelsFamily, "$.contexts.q.ctx.matched[4] $.contexts.q.ctx.matched[5]"},
		"the members' order":     {`"title":"title [x]","family":"fam",`, `"family":"fam","title":"title [x]",`, searchLabelsFamily, "$.contexts.q.ctx.<members>"},
		"the counts":             {`"char":12`, `"char":11`, searchLabelsFamily, "$.searches.char"},
		"the durations":          {`"query_ms":`, `"query_ms":1`, searchLabelsFamily, ""},
	} {
		planted := strings.Replace(every, c.old, c.new, 1)
		if planted == every {
			t.Errorf("the label family, %s: C's answer does not hold %s", name, c.old)
		} else if got := dashNormDiffs(t, c.fam, c.fam.masks, [2]int64{}, every, planted); got != c.want {
			t.Errorf("the label family, %s: differences at %q, want %q", name, got, c.want)
		}
	}

	// the contexts' order is compared too: C lists them in the order its walk met them
	shared := dashNormSearchRows["merge/every-shared"].bodies[0]
	turnedContexts, err := ParseJSON([]byte(shared))
	if err != nil {
		t.Fatal(err)
	}
	for i, m := range turnedContexts.Members {
		if m.Key == "contexts" {
			contexts := slices.Clone(m.Value.Members)
			slices.Reverse(contexts)
			turnedContexts.Members[i].Value.Members = contexts
		}
	}
	if got := differs("merge/every-shared", shared, turnedContexts.String()); got != "$.contexts.<members>" {
		t.Errorf("the contexts in the opposite order: differences at %q, want $.contexts.<members>", got)
	}

	// the cut family: of a cut list of its key, the values printed before the marker are each agent's choice among
	// the key's values; how many they are, the marker, and any text that is none of them are compared
	four, five := searchCutFamily("lk", searchLabelValues[:4]...), searchCutFamily("lk", searchLabelValues...)
	cut := func(list string) string {
		return `{"api":2,"contexts":{"l.ctx":{"matched":["labels"],"labels":{"l3":["x1","x2","x3"],"lk":` + list +
			`}}},"searches":{"strings":20,"char":5,"total":25}}`
	}
	for name, c := range map[string]struct {
		fam  v2Family
		o, c string
		want string
	}{
		"two C agents' values":    {five, `["w1","w2","... 3 values more"]`, `["w5","w1","... 3 values more"]`, ""},
		"compared without it":     {searchLabelsFamily, `["w1","w2","... 3 values more"]`, `["w5","w1","... 3 values more"]`, "$.contexts.l.ctx.labels.lk[2]"},
		"a value of another key":  {five, `["w1","w2","... 3 values more"]`, `["w1","x1","... 3 values more"]`, "$.contexts.l.ctx.labels.lk[2]"},
		"a value not the host's":  {four, `["w1","w2","... 2 values more"]`, `["w5","w1","... 2 values more"]`, "$.contexts.l.ctx.labels.lk[2]"},
		"a value twice":           {five, `["w1","w2","... 3 values more"]`, `["w1","w1","... 3 values more"]`, "$.contexts.l.ctx.labels.lk[2]"},
		"one value before it":     {five, `["w1","w2","... 3 values more"]`, `["w1","... 4 values more"]`, "$.contexts.l.ctx.labels.lk[0] $.contexts.l.ctx.labels.lk.length"},
		"another count":           {five, `["w1","w2","... 3 values more"]`, `["w1","w2","... 2 values more"]`, "$.contexts.l.ctx.labels.lk[0]"},
		"the marker first":        {five, `["w1","w2","... 3 values more"]`, `["... 3 values more","w1","w2"]`, "$.contexts.l.ctx.labels.lk[1] $.contexts.l.ctx.labels.lk[2]"},
		"a whole list, reordered": {five, `["w1","w2","w3","w4","w5"]`, `["w5","w4","w3","w2","w1"]`, ""},
		"a whole list, changed":   {five, `["w1","w2","w3","w4","w5"]`, `["w1","w2","w3","w4","w6"]`, "$.contexts.l.ctx.labels.lk[4]"},
		"a marker as a value":     {five, `["w1","w2","w3","w4","w5"]`, `["w1","w2","w3","w4","... 1 values more"]`, "$.contexts.l.ctx.labels.lk[0] $.contexts.l.ctx.labels.lk[1] $.contexts.l.ctx.labels.lk[2] $.contexts.l.ctx.labels.lk[3] $.contexts.l.ctx.labels.lk[4]"},
	} {
		if got := dashNormDiffs(t, c.fam, c.fam.masks, [2]int64{}, cut(c.o), cut(c.c)); got != c.want {
			t.Errorf("the cut family, %s: differences at %q, want %q", name, got, c.want)
		}
	}
	// and the bytes its render writes: the list of its key alone, in any layout; another key's cut list, a list
	// that is not cut and a text that is no list of the key stay as they were written
	for name, c := range map[string]struct{ in, want string }{
		"a cut list":        {`{"lk":["w2","w5","... 3 values more"]}`, `{"lk":["VALUE","VALUE","... 3 values more"]}`},
		"in a pretty body":  {"{\"lk\":[\n  \"w2\",\n  \"w5\",\n  \"... 3 values more\"\n]}", "{\"lk\":[\n  \"VALUE\",\n  \"VALUE\",\n  \"... 3 values more\"\n]}"},
		"after a space":     {`{"lk": ["w2","... 4 values more"]}`, `{"lk": ["VALUE","... 4 values more"]}`},
		"a whole list":      {`{"lk":["w1","w2","w3"]}`, `{"lk":["w1","w2","w3"]}`},
		"an empty list":     {`{"lk":[]}`, `{"lk":[]}`},
		"another key":       {`{"l3":["w1","w2","... 3 values more"]}`, `{"l3":["w1","w2","... 3 values more"]}`},
		"another marker":    {`{"lk":["w1","w2","... 3 dimensions more"]}`, `{"lk":["w1","w2","... 3 dimensions more"]}`},
		"a foreign value":   {`{"lk":["w1","x","... 3 values more"]}`, `{"lk":["VALUE","x","... 3 values more"]}`},
		"a value twice":     {`{"lk":["w1","w1","... 3 values more"]}`, `{"lk":["VALUE","w1","... 3 values more"]}`},
		"the key as a text": {`{"q":"lk","lk":"w1"}`, `{"q":"lk","lk":"w1"}`},
		"no JSON":           {"query timeout", "query timeout"},
	} {
		if got := string(five.render(0, [2]int64{}, []byte(c.in))); got != c.want {
			t.Errorf("the cut render, %s: %s, want %s", name, got, c.want)
		}
	}
	if got := string(four.render(0, [2]int64{}, []byte(`{"lk":["w5","w4","... 2 values more"]}`))); got !=
		`{"lk":["w5","VALUE","... 2 values more"]}` {
		t.Errorf("the cut render of the first child's four values wrote %s", got)
	}

	// the rows' own cut families: `four` takes the first child's four values alone, the rows over every host the
	// second child's too; C's recorded answer with other values printed before its marker
	lkRe := regexp.MustCompile(`"lk":\[[^\]]*\]`)
	for name, c := range map[string]struct{ key, list, want string }{
		"another two of the host's four": {"labels/four", `"lk":["w4","w3","... 2 values more"]`, ""},
		"the second child's value on the first": {"labels/four", `"lk":["w5","w1","... 2 values more"]`,
			"$.contexts.l.ctx.labels.lk[2]"},
		"the second child's value":            {"labels/five", `"lk":["w5","w4","... 3 values more"]`, ""},
		"the second child's value, among all": {"labels/every", `"lk":["w5","w4","... 3 values more"]`, ""},
		"a value of no chart": {"labels/five", `"lk":["w6","w4","... 3 values more"]`,
			"$.contexts.l.ctx.labels.lk[2]"},
	} {
		recorded := dashNormSearchRows[c.key].bodies[0]
		planted := lkRe.ReplaceAllLiteralString(recorded, c.list)
		if !lkRe.MatchString(recorded) {
			t.Errorf("the rows' cut families, %s: C's answer of %s has no list of lk", name, c.key)
			continue
		}
		if got := differs(c.key, recorded, planted); got != c.want {
			t.Errorf("the rows' cut families, %s: differences at %q, want %q", name, got, c.want)
		}
		v, _ := read(rows[c.key], planted)
		if err := rows[c.key].req.guard(v); (err != nil) != (c.want != "") {
			t.Errorf("the rows' cut families, %s: the %s guard: %v", name, c.key, err)
		}
	}

	// the MCP family: the `info` text as each side wrote it
	mcp := dashNormSearchRows["merge/truncated-mcp"].bodies[0]
	escaped := strings.Replace(mcp, `Cardinality limit reached. Use`, `Cardinality limit reached\u002e Use`, 1)
	if escaped == mcp {
		t.Fatalf("C's MCP answer has no text: %s", mcp)
	}
	if got := dashNormDiffs(t, searchMCPFamily, searchMCPFamily.masks, [2]int64{}, mcp, escaped); got != "$.info" {
		t.Errorf("the MCP family, a full stop as its code: differences at %q, want $.info", got)
	}
	if got := dashNormDiffs(t, searchFamily, searchFamily.masks, [2]int64{}, mcp, escaped); got != "" {
		t.Errorf("the same, decoded: differences at %q", got)
	}
	v, _ := read(rows["merge/truncated-mcp"], escaped)
	if err := rows["merge/truncated-mcp"].req.guard(v); err == nil {
		t.Errorf("the truncated-mcp guard took its text in another escape")
	}

	// a letter that is no ASCII is compared as each side wrote it: C writes its bytes, and its escape, which decodes to
	// the same text, is a difference of the strings' escapes (v2Escapes)
	fold := dashNormSearchRows["labels/fold-ascii"].bodies[0]
	foldEscaped := strings.Replace(fold, "Grün", `Gr\u00fcn`, 1)
	if foldEscaped == fold {
		t.Fatalf("C's answer does not hold the value: %s", fold)
	}
	var folds [2]Value
	for i, b := range []string{fold, foldEscaped} {
		folds[i], _ = read(rows["labels/fold-ascii"], b)
	}
	if got := differs("labels/fold-ascii", fold, foldEscaped); got != "" {
		t.Errorf("the value as its escape, decoded: differences at %q", got)
	}
	foldFam := rows["labels/fold-ascii"].fam
	if e := v2Escapes([2][]byte{[]byte(fold), []byte(foldEscaped)}, folds, foldFam); e == "" {
		t.Errorf("the value as its escape: the escapes' comparison reports nothing")
	}
	if e := v2Escapes([2][]byte{[]byte(fold), []byte(fold)}, [2]Value{folds[0], folds[0]}, foldFam); e != "" {
		t.Errorf("C's own bytes twice: %s", e)
	}

	// `minify`: C wrote that answer in one line and the others in many (the record keeps how many lines each body
	// had), and the row's comparison holds the two layouts to each other: a pretty answer beside C's is reported
	minified := dashNormSearchRows["data/minify"]
	if minified.lines != [2]int{1, 1} || dashNormSearchRows["data/v2-all"].lines[0] < 10 {
		t.Errorf("C's answers had %v lines with `minify` and %v without, want one and many", minified.lines,
			dashNormSearchRows["data/v2-all"].lines)
	}
	pretty := strings.Replace(minified.bodies[0], `{"api":2,`, "{\n    \"api\":2,", 1)
	if l := v2Layouts([2][]byte{[]byte(minified.bodies[0]), []byte(pretty)},
		rows["data/minify"].fam.layoutByCount()); l == "" {
		t.Errorf("the minify row's layouts: a pretty answer beside C's shows no difference")
	}
	if l := v2Layouts([2][]byte{[]byte(minified.bodies[0]), []byte(minified.bodies[1])},
		rows["data/minify"].fam.layoutByCount()); l != "" {
		t.Errorf("the minify row's layouts, the recorded pair: %s", l)
	}

	// the helpers: the counts' text, a labelled context's facts one by one
	counts := `{"searches":{"strings":15,"char":2,"total":17}}`
	for name, c := range map[string]struct {
		fact dashFact
		bad  bool
	}{"C's counts": {searchCounts(15, 2), false}, "another count of strings": {searchCounts(14, 2), true},
		"another count of matches": {searchCounts(15, 3), true}} {
		v, _ := ParseJSON([]byte(counts))
		if err := c.fact(v); (err != nil) != c.bad {
			t.Errorf("searchCounts, %s: %v", name, err)
		}
	}
	if err := searchCounts(15, 2)(Value{Kind: KindObject}); err == nil {
		t.Errorf("searchCounts took an answer without searches")
	}
	if total, _ := ParseJSON([]byte(`{"searches":{"strings":15,"char":2,"total":15}}`)); searchCounts(15, 2)(total) == nil {
		t.Errorf("searchCounts took a total without the labels' matches")
	}
}
