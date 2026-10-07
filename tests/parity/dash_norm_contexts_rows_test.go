// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strconv"
	"strings"
	"testing"
)

// testDashNormContextsRows pins the families and guards of `api.v2-contexts`' rows beyond the dashboard's calls
// (D231 F6: contextsLabelsFamily, contextsWindowFamily, dashSet and the rows' facts) on C's answers, recorded C
// against C (the oracle as its own candidate, H33's probe p1, 14:00Z) and assembled from their parts: each answer's
// members as C printed them, the timings and versions of one of them.
func testDashNormContextsRows(t *testing.T) {
	const (
		parent = `{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent",`
		child  = `{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child",`
		child2 = `{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2",`
		st     = `"st":{"ai":0,"code":200,"msg":""}}`
		nodes  = `"nodes":[` + parent + `"ni":0,` + st + `,` + child + `"ni":1,` + st + `]`
		nodesC = `"nodes":[` + child + `"ni":0,` + st + `]`
		nodes3 = `"nodes":[` + parent + `"ni":0,` + st + `,` + child + `"ni":1,` + st + `,` + child2 + `"ni":2,` +
			st + `]`
		mcpP = `{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent",` +
			`"relationship":"localhost","connected":true}`
		mcpC = `{"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb","hostname":"parity-child",` +
			`"relationship":"child","connected":true}`
		mcpC2 = `{"machine_guid":"5a1e0000-0000-4000-8000-0000000000cc","hostname":"parity-child2",` +
			`"relationship":"child","connected":true}`
		version = `"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,` +
			`"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0}`
		agent = `"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":NOW,` +
			`"ai":0,"timings":{"prep_ms":0,"query_ms":0.039,"output_ms":0.019,"total_ms":0.058,"cloud_ms":0.058}}]`
		timings = `"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.058}`
		// q.ctx's defaults on the fixture, and on the merge's two children
		q = `"family":"fam","units":"units","priority":1000,"first_entry":1791381420,"last_entry":1791381592,` +
			`"live":true`
		qMerge = `"family":"fam[x]","units":"units","priority":900,"first_entry":1791381480,` +
			`"last_entry":1791381605,"live":true`
		labels = `{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}`
		// the seconds of the two pairs' answers
		now, nowMerge int64 = 1791381592, 1791381605
	)
	// body is an answer of these members, the agent's clock at the second at
	body := func(at int64, members ...string) string {
		return strings.ReplaceAll("{"+strings.Join(members, ",")+"}", "NOW", strconv.FormatInt(at, 10))
	}
	lists := func(dims, labels, instances string) string {
		return `"contexts":{"q.ctx":{"title":"title [x]",` + q + `,"dimensions":` + dims + `,"labels":` + labels +
			`,"instances":` + instances + `}}`
	}
	full := body(now, `"api":2`, nodes, lists(`["alpha","b","z","inc","h","a"]`, labels, `["q.q_a_name","q.two"]`),
		version, agent, timings)
	card1 := body(now, `"api":2`, nodes, lists(`["... 6 dimensions more"]`,
		`{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["... 2 values more"]}`,
		`["... 2 instances more"]`), version, agent, timings)
	card2 := body(now, `"api":2`, nodes, lists(`["alpha","... 5 dimensions more"]`, labels, `["q.q_a_name","q.two"]`),
		version, agent, timings)
	all := body(now, `"api":2`, nodes, `"contexts":{"q.ctx":{`+q+`}}`, version, agent, timings)
	miss := body(now, `"api":2`, nodesC, `"contexts":{"q.ctx":{`+q+`}}`, version, agent, timings)
	none := body(now, `"api":2`, `"nodes":[]`, `"contexts":{}`, version, agent, timings)
	// a window's walk: the agent's clock the second before the request's
	window := body(now-1, `"api":2`, `"nodes":[]`, `"contexts":{}`, version, agent, timings)
	debug := body(now, `"api":2`, `"request":{"mode":["versions","agents","nodes","contexts"],"options":["debug",`+
		`"priorities","retention","liveness","family","units"],"scope":{"scope_nodes":"*","scope_contexts":null},`+
		`"selectors":{"nodes":null,"contexts":null},"filters":{"after":0,"before":0}}`, nodes,
		`"contexts":{"q.ctx":{`+q+`}}`, version, agent, timings)
	mcp := body(now, `"nodes":[`+mcpP+`,`+mcpC+`]`, `"contexts":{"q.ctx":{`+q+`}}`, `"info":"Next Steps: ..."`,
		version, agent)
	merge := body(nowMerge, `"api":2`, nodes3, `"contexts":{"q.ctx":{"title":"[x] [x]",`+qMerge+`},"r.ctx":{`+
		`"title":"r title [x]","family":"rfam","units":"runits","priority":1100,"first_entry":1791381480,`+
		`"last_entry":1791381605,"live":true}}`, version, agent, timings)
	truncated := body(nowMerge, `"api":2`, nodes3, `"contexts":{"q.ctx":{`+qMerge+`},"__truncated__":{`+
		`"total_contexts":2,"returned":1,"remaining":1}}`, version, agent, timings)
	categorized := body(nowMerge, `"nodes":[`+mcpP+`,`+mcpC+`,`+mcpC2+`]`, `"contexts":{"__info__":{`+
		`"status":"categorized","total_contexts":2,"categories":2,"samples_per_category":3,"help":"Results ..."},`+
		`"q":["q.ctx"],"r":["r.ctx"]}`, `"info":"The response has been grouped into categories ..."`, version, agent)

	// parse is an answer as v2Round judges it: normalised by fam, asked in the second at
	parse := func(fam v2Family, b string, at int64) Value {
		t.Helper()
		v, err := ParseJSON(fam.normalise(0, [2]int64{at, at}, []byte(b)))
		if err != nil {
			t.Fatalf("%v: %s", err, b)
		}
		return v
	}
	flight := [2]int64{now, now}

	// the lists' family: the labels a set, keys and values; the dimensions and instances in order
	reordered := strings.Replace(full, labels, `{"k":["v2","v1"],"_collect_module":["corpus"],`+
		`"_collect_plugin":["fixture-pusher"]}`, 1)
	otherTimings := strings.Replace(full, `"query_ms":0.039`, `"query_ms":0.021`, 1)
	for name, c := range map[string]struct {
		oracle, candidate string
		fam               v2Family
		want              string
	}{
		"the recorded pair":           {full, otherTimings, contextsLabelsFamily, ""},
		"the labels in another order": {full, reordered, contextsLabelsFamily, ""},
		"the labels' order, compared": {full, reordered, contextsFamily,
			"$.contexts.q.ctx.labels.<members> $.contexts.q.ctx.labels.k[0] $.contexts.q.ctx.labels.k[1]"},
		"a label's value": {full, strings.Replace(full, `"v2"]`, `"v3"]`, 1), contextsLabelsFamily,
			"$.contexts.q.ctx.labels.k[1]"},
		"a label's value missing": {full, strings.Replace(full, `"k":["v1","v2"]`, `"k":["v1"]`, 1),
			contextsLabelsFamily, "$.contexts.q.ctx.labels.k.length"},
		"a label missing": {full, strings.Replace(full, `"_collect_module":["corpus"],`, ``, 1),
			contextsLabelsFamily, "$.contexts.q.ctx.labels.<members>"},
		"the dimensions in another order": {full, strings.Replace(full, `["alpha","b",`, `["b","alpha",`, 1),
			contextsLabelsFamily, "$.contexts.q.ctx.dimensions[0] $.contexts.q.ctx.dimensions[1]"},
		"the instances in another order": {full, strings.Replace(full, `["q.q_a_name","q.two"]`,
			`["q.two","q.q_a_name"]`, 1), contextsLabelsFamily,
			"$.contexts.q.ctx.instances[0] $.contexts.q.ctx.instances[1]"},
		"a limit's message": {card1, strings.Replace(card1, `"... 2 values more"`, `"... 1 values more"`, 1),
			contextsLabelsFamily, "$.contexts.q.ctx.labels.k[0]"},
	} {
		if got := dashNormDiffs(t, c.fam, c.fam.masks, flight, c.oracle, c.candidate); got != c.want {
			t.Errorf("contexts lists, %s: differences at %q, want %q", name, got, c.want)
		}
	}

	// the window's family: no `now` (C's walk clock is the second before the request's, and its contexts' last
	// entries with it), so the agent's clock is not held to the flight; what it lists is compared
	if len(contextsWindowFamily.now) != 0 {
		t.Errorf("contextsWindowFamily writes %v as NOW", contextsWindowFamily.now)
	}
	if err := v2NowInFlight(parse(contextsWindowFamily, window, now), flight); err == nil {
		t.Errorf("v2NowInFlight took a window's clock, the second before the request")
	}
	for name, c := range map[string]struct{ candidate, want string }{
		"the recorded pair": {strings.Replace(window, `"query_ms":0.039`, `"query_ms":0.003`, 1), ""},
		"the child listed":  {miss, "$.nodes.length $.contexts.<members>"},
	} {
		if got := dashNormDiffs(t, contextsWindowFamily, contextsWindowFamily.masks, flight, window,
			c.candidate); got != c.want {
			t.Errorf("contexts window, %s: differences at %q, want %q", name, got, c.want)
		}
	}

	// dashSet: a set of the value's members and items
	set := dashSet(labels, "contexts", "q.ctx", "labels")
	for name, c := range map[string]struct {
		in   string
		fact dashFact
		bad  bool
	}{
		"the recorded labels": {full, set, false},
		"another order":       {reordered, set, false},
		"another value":       {strings.Replace(full, `"v2"]`, `"v3"]`, 1), set, true},
		"one value more":      {strings.Replace(full, `"v2"]`, `"v2","v3"]`, 1), set, true},
		"no labels":           {all, set, true},
		"a bad want":          {full, dashSet(`{"k":`, "contexts"), true},
	} {
		if err := c.fact(parse(contextsLabelsFamily, c.in, now)); (err != nil) != c.bad {
			t.Errorf("dashSet, %s: %v", name, err)
		}
	}

	// each row's guard takes C's answer and refuses the answer of a row that differs from it where it judges
	rows := map[string]contextsRow{}
	for _, r := range contextsOptionRows(1791381420) {
		rows[r.req.name] = r
	}
	for _, r := range contextsMergeRows() {
		rows[r.req.name] = r
	}
	for name, c := range map[string]struct {
		c, refused string
		at         int64
	}{
		"full":          {full, card2, now},
		"card1":         {card1, full, now},
		"card2":         {card2, card1, now},
		"contexts-miss": {miss, all, now},
		"scope-miss":    {none, miss, now},
		"window-miss":   {window, all, now},
		"debug":         {debug, strings.Replace(debug, `"debug",`, `"debug","titles",`, 1), now},
		"mcp":           {mcp, all, now},
		"titles":        {merge, strings.Replace(merge, `"[x] [x]"`, `"other [x]"`, 1), nowMerge},
		"truncated":     {truncated, merge, nowMerge},
		"categorized": {categorized, strings.Replace(categorized, `"samples_per_category":3`,
			`"samples_per_category":0`, 1), nowMerge},
	} {
		r, ok := rows[name]
		if !ok {
			t.Errorf("no row %s", name)
			continue
		}
		if err := r.req.guard(parse(r.fam, c.c, c.at)); err != nil {
			t.Errorf("the %s guard on C's answer: %v", name, err)
		}
		if err := r.req.guard(parse(r.fam, c.refused, c.at)); err == nil {
			t.Errorf("the %s guard took a wrong answer", name)
		}
	}
	if len(rows) != 11 {
		t.Errorf("%d rows, pinned 11", len(rows))
	}
	// a host kept without a context is refused where none may be
	kept := body(now, `"api":2`, `"nodes":[`+parent+`"ni":0,`+st+`]`, `"contexts":{}`, version, agent, timings)
	for _, name := range []string{"scope-miss", "window-miss"} {
		if err := rows[name].req.guard(parse(rows[name].fam, kept, now)); err == nil {
			t.Errorf("the %s guard took localhost without a context", name)
		}
	}
	// a merge that keeps the newcomer's title, family or units, or the higher priority, is refused
	for right, wrong := range map[string]string{
		`"title":"[x] [x]"`: `"title":"other [x]"`,
		`"family":"fam[x]"`: `"family":"fam2"`,
		`"units":"units"`:   `"units":"units2"`,
		`"priority":900`:    `"priority":1000`,
	} {
		if err := rows["titles"].req.guard(parse(contextsFamily, strings.Replace(merge, right, wrong, 1),
			nowMerge)); err == nil {
			t.Errorf("the merge guard took %s", wrong)
		}
	}
}
