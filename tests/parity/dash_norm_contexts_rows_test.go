// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"slices"
	"strconv"
	"strings"
	"testing"
)

// testDashNormContextsRows pins the families and guards of `api.v2-contexts`' rows beyond the dashboard's calls
// (D231 F6, D232, D233, D238: contextsLabelsFamily, contextsMCPFamily, dashSet and the rows' facts) on C's
// answers, recorded C against C (the oracle as its own candidate: H33's probe p1, 14:00Z, for the `data` rows; H34's
// probe p1, 23:22-23:23Z, for the MCP texts and the two children's rows) and assembled from their parts: each
// answer's members as C printed them, the timings of one of them.
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
		// q.ctx's defaults on the fixture, and on the merge's two children: the fixture child's data from
		// 1791415200 (baseMerge), the second child's from a minute earlier
		q = `"family":"fam","units":"units","priority":1000,"first_entry":1791381420,"last_entry":1791381592,` +
			`"live":true`
		qMerge = `"family":"fam[x]","units":"units","priority":900,"first_entry":1791415140,` +
			`"last_entry":1791415366,"live":true`
		rMerge = `"title":"r title [x]","family":"rfam","units":"runits","priority":1100,"first_entry":1791415140,`
		labels = `{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}`
		// the seconds of the two pairs' answers, and the fixture child's base in the merge's pair
		now, nowMerge, baseMerge int64 = 1791381592, 1791415366, 1791415200
		// the MCP texts as C wrote them: a context list's next steps, a categorized list's, the categories' help
		nextSteps = `Next Steps: Query time-series data with the 'query_metrics' tool, using different aggregations ` +
			`to inspect different views:\n   - 'group_by: dimension' will aggregate all time-series by the ` +
			`listed dimensions\n   - 'group_by: instance' will aggregate all time-series by the listed ` +
			`instances\n   - 'group_by: label, group_by_label: {label_key}' will aggregate by the listed ` +
			`label values\n\nDimensions, instances and labels can also be used for filtering in ` +
			`'query_metrics':\n   - 'dimensions: dimension1|dimension2|*dimension*' will select only the ` +
			`time-series with the given dimension\n   - 'instances: instance1|instance2|*instance*' will ` +
			`select only the time-series with the given instance\n   - 'labels' can be specified in two ` +
			`formats:\n      ` + mcpBullet +
			` String format: 'labels: key1:value1|key1:value2|key2:value3' (values with same key are ORed, ` +
			`different keys are ANDed)\n      ` + mcpBullet +
			` Structured format: 'labels: {\"key1\": [\"value1\", \"value2\"], \"key2\": \"value3\"}' (array ` +
			`values are ORed, different keys are ANDed)`
		grouped = `The response has been grouped into categories to minimize size.\nNext Steps: repeat the ` +
			`'list_metrics' call with a pattern to match what is interesting, or run 'get_metrics_details' ` +
			`to get more information for the contexts of interest.`
		help = `Results grouped by category with samples. Use 'metrics' parameter with specific patterns like ` +
			`'system.*' to get full details for a category.`
	)
	// hashes are the versions of the two children's parent, its context dictionaries' version as given: 13 for the
	// fixture child's, 26 for the second child's, 39 for every host's
	hashes := func(contexts int) string {
		return `"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":` + strconv.Itoa(contexts) +
			`,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0}`
	}
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
	mcp := body(now, `"nodes":[`+mcpP+`,`+mcpC+`]`, `"contexts":{"q.ctx":{`+q+`}}`, `"info":"`+nextSteps+`"`,
		version, agent)
	merge := body(nowMerge, `"api":2`, nodes3, `"contexts":{"q.ctx":{"title":"[x] [x]",`+qMerge+`},"r.ctx":{`+
		rMerge+`"last_entry":1791415366,"live":true}}`, hashes(39), agent, timings)
	truncated := body(nowMerge, `"api":2`, nodes3, `"contexts":{"q.ctx":{`+qMerge+`},"__truncated__":{`+
		`"total_contexts":2,"returned":1,"remaining":1}}`, hashes(39), agent, timings)
	categorized := body(nowMerge, `"nodes":[`+mcpP+`,`+mcpC+`,`+mcpC2+`]`, `"contexts":{"__info__":{`+
		`"status":"categorized","total_contexts":2,"categories":2,"samples_per_category":3,"help":"`+help+`"},`+
		`"q":["q.ctx"],"r":["r.ctx"]}`, `"info":"`+grouped+`"`, hashes(39), agent)

	// parse is an answer as v2Round judges it: normalised by fam, asked in the second at
	parse := func(fam v2Family, b string, at int64) Value {
		t.Helper()
		v, err := ParseJSON(fam.normalise(0, [2]int64{at, at}, [2]int64{at, at}, []byte(b)))
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

	// a window's answer: C's walk clock is the second before the request's (the recorded `window-miss` answer, asked
	// in second `now`, says now - 1). The agent's clock is held to that second (v2Clock), not to the flight's; what
	// the answer lists is compared
	windowTarget := "/api/v2/contexts?scope_nodes=*&after=1791380820&before=1791381120"
	if err := v2NowInFlight(parse(contextsFamily, window, now), contextsFamily.v2Clock(windowTarget, flight)); err != nil {
		t.Errorf("a window's clock, the second before the request: %v", err)
	}
	if err := v2NowInFlight(parse(contextsFamily, window, now), flight); err == nil {
		t.Errorf("v2NowInFlight took a window's clock for the wall's")
	}
	if err := v2NowInFlight(parse(contextsFamily, none, now), contextsFamily.v2Clock(windowTarget, flight)); err == nil {
		t.Errorf("v2NowInFlight took the wall's clock for a window's")
	}
	for name, c := range map[string]struct{ candidate, want string }{
		"the recorded pair": {strings.Replace(window, `"query_ms":0.039`, `"query_ms":0.003`, 1), ""},
		"the child listed":  {miss, "$.nodes.length $.contexts.<members>"},
	} {
		if got := dashNormDiffs(t, contextsFamily, contextsFamily.masks, flight, window, c.candidate); got != c.want {
			t.Errorf("contexts window, %s: differences at %q, want %q", name, got, c.want)
		}
	}

	// the MCP family: the texts as each side wrote them. C's two sides wrote the same bytes; another escape of the
	// same text is a difference, which the comparison of the decoded strings (contextsFamily) does not see
	uBullet := strings.Replace(mcp, mcpBullet, `\u2022`, 1)
	uQuote := strings.Replace(mcp, `\"key1\"`, `\u0022key1\u0022`, 1)
	uNewline := strings.Replace(mcp, `views:\n`, `views:\u000a`, 1)
	uTool := strings.Replace(mcp, `'query_metrics' tool`, `\u0027query_metrics\u0027 tool`, 1)
	uApostrophe := strings.Replace(categorized, `'metrics'`, `\u0027metrics\u0027`, 1)
	uGrouped := strings.Replace(categorized, `size.\n`, `size.\u000A`, 1)
	otherText := strings.Replace(mcp, `ANDed)"`, `ANDed.)"`, 1)
	otherPriority := strings.Replace(mcp, `"priority":1000`, `"priority":1`, 1)
	mcpTimings := strings.Replace(mcp, `"query_ms":0.039`, `"query_ms":0.021`, 1)
	categorizedTimings := strings.Replace(categorized, `"query_ms":0.039`, `"query_ms":0.012`, 1)
	for name, c := range map[string]struct {
		oracle, candidate string
		fam               v2Family
		want              string
	}{
		"the recorded pair":                {mcp, mcpTimings, contextsMCPFamily, ""},
		"the bullet escaped":               {mcp, uBullet, contextsMCPFamily, "$.info"},
		"the bullet escaped, decoded":      {mcp, uBullet, contextsFamily, ""},
		"a quote as its code":              {mcp, uQuote, contextsMCPFamily, "$.info"},
		"a quote as its code, decoded":     {mcp, uQuote, contextsFamily, ""},
		"a newline as its code":            {mcp, uNewline, contextsMCPFamily, "$.info"},
		"a newline as its code, decoded":   {mcp, uNewline, contextsFamily, ""},
		"apostrophes as their code":        {mcp, uTool, contextsMCPFamily, "$.info"},
		"apostrophes, decoded":             {mcp, uTool, contextsFamily, ""},
		"another text":                     {mcp, otherText, contextsMCPFamily, "$.info"},
		"a context beside the text":        {mcp, otherPriority, contextsMCPFamily, "$.contexts.q.ctx.priority"},
		"the categories' recorded pair":    {categorized, categorizedTimings, contextsMCPFamily, ""},
		"the help's apostrophes escaped":   {categorized, uApostrophe, contextsMCPFamily, "$.contexts.__info__.help"},
		"the help's apostrophes, decoded":  {categorized, uApostrophe, contextsFamily, ""},
		"the categories' newline in caps":  {categorized, uGrouped, contextsMCPFamily, "$.info"},
		"the categories' newline, decoded": {categorized, uGrouped, contextsFamily, ""},
	} {
		if got := dashNormDiffs(t, c.fam, c.fam.masks, flight, c.oracle, c.candidate); got != c.want {
			t.Errorf("contexts mcp, %s: differences at %q, want %q", name, got, c.want)
		}
	}
	// the render leaves each text's own bytes as the string's text, and nothing else of a body changes
	for name, c := range map[string]struct {
		in   string
		path []string
		want string
	}{
		"the next steps":  {mcp, []string{"info"}, nextSteps},
		"the categories'": {categorized, []string{"info"}, grouped},
		"the help":        {categorized, []string{"contexts", "__info__", "help"}, help},
		"after a space":   {`{"info": "a\nb\"c"}`, []string{"info"}, `a\nb\"c`},
		"a backslash":     {`{"help":"a\\b"}`, []string{"help"}, `a\\b`},
	} {
		got, err := dashAt(parse(contextsMCPFamily, c.in, now), c.path...)
		if err != nil || got.Text != c.want {
			t.Errorf("mcpTextRender, %s: %q (%v), want %q", name, got.Text, err, c.want)
		}
	}
	// and the bytes it writes: the text's own escaped once more, everything around it kept, the space after a
	// colon too (the layouts' comparison reads it)
	for name, c := range map[string]struct{ in, want string }{
		"no escape":      {`{"info": "a"}`, `{"info": "a"}`},
		"escapes":        {`{"info": "a\nb\"c"}`, `{"info": "a\\nb\\\"c"}`},
		"another member": {`{"help":"x","other":"a\nb"}`, `{"help":"x","other":"a\nb"}`},
		"a text in pretty form": {"{\n    \"info\":\"a\\tb\",\n    \"x\":1\n}",
			"{\n    \"info\":\"a\\\\tb\",\n    \"x\":1\n}"},
		"no text":              {all, all},
		"an answer of no JSON": {"Unsupported API command: contexts", "Unsupported API command: contexts"},
	} {
		if got := string(mcpTextRender(0, [2]int64{}, []byte(c.in))); got != c.want {
			t.Errorf("mcpTextRender, %s: %s, want %s", name, got, c.want)
		}
	}
	if decoded, err := dashAt(parse(contextsFamily, mcp, now), "info"); err != nil || decoded.Text == nextSteps ||
		!strings.Contains(decoded.Text, "\u2022 String format") {
		t.Errorf("the next steps decoded: %q (%v)", decoded.Text, err)
	}
	// the texts' guards take C's bytes alone: not another escape of them, and not the decoded text
	for name, c := range map[string]struct {
		row, in string
		fam     v2Family
		at      int64
	}{
		"the bullet escaped":             {"mcp", uBullet, contextsMCPFamily, now},
		"a quote as its code":            {"mcp", uQuote, contextsMCPFamily, now},
		"a newline as its code":          {"mcp", uNewline, contextsMCPFamily, now},
		"apostrophes as their code":      {"mcp", uTool, contextsMCPFamily, now},
		"C's answer, decoded":            {"mcp", mcp, contextsFamily, now},
		"the help's apostrophes escaped": {"categorized", uApostrophe, contextsMCPFamily, nowMerge},
		"the categories' newline":        {"categorized", uGrouped, contextsMCPFamily, nowMerge},
		"the categories', decoded":       {"categorized", categorized, contextsFamily, nowMerge},
	} {
		var guard func(Value) error
		for _, r := range slices.Concat(contextsOptionRows(1791381420), contextsMergeRows(baseMerge)) {
			if r.req.name == c.row {
				guard = r.req.guard
			}
		}
		if guard == nil {
			t.Errorf("no row %s", c.row)
		} else if err := guard(parse(c.fam, c.in, c.at)); err == nil {
			t.Errorf("the %s guard took %s", c.row, name)
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
	for _, r := range contextsMergeRows(baseMerge) {
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
		// a selector with no word in it filters nothing: the dashboard's answer, not a filter's that matches nothing
		"scope-wordless": {all, none, now},
		"nodes-wordless": {all, miss, now},
		"debug":          {debug, strings.Replace(debug, `"debug",`, `"debug","titles",`, 1), now},
		"mcp":            {mcp, all, now},
		"titles":         {merge, strings.Replace(merge, `"[x] [x]"`, `"other [x]"`, 1), nowMerge},
		"truncated":      {truncated, merge, nowMerge},
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
	if len(rows) != 13 {
		t.Errorf("%d rows, pinned 13", len(rows))
	}
	// a host kept without a context is refused where none may be
	kept := body(now, `"api":2`, `"nodes":[`+parent+`"ni":0,`+st+`]`, `"contexts":{}`, version, agent, timings)
	for _, name := range []string{"scope-miss", "window-miss"} {
		if err := rows[name].req.guard(parse(rows[name].fam, kept, now)); err == nil {
			t.Errorf("the %s guard took localhost without a context", name)
		}
	}
	// a merge that keeps the newcomer's title, family or units, the higher priority, or the first host's first entry
	// (a minute after the second child's) is refused; so is a context that is not live
	for right, wrong := range map[string]string{
		`"live":true},"r.ctx"`:                      `"live":false},"r.ctx"`,
		`"live":true}}`:                             `"live":false}}`,
		`"title":"[x] [x]"`:                         `"title":"other [x]"`,
		`"family":"fam[x]"`:                         `"family":"fam2"`,
		`"units":"units"`:                           `"units":"units2"`,
		`"priority":900`:                            `"priority":1000`,
		`"priority":900,"first_entry":1791415140`:   `"priority":900,"first_entry":1791415200`,
		`"priority":1100,"first_entry":1791415140,`: `"priority":1100,"first_entry":1791415200,`,
		`"title":"r title [x]"`:                     `"title":"r title a"`,
		`"family":"rfam"`:                           `"family":"fam"`,
		`"units":"runits"`:                          `"units":"units"`,
		`"priority":1100`:                           `"priority":1101`,
	} {
		if err := rows["titles"].req.guard(parse(contextsFamily, strings.Replace(merge, right, wrong, 1),
			nowMerge)); err == nil {
			t.Errorf("the merge guard took %s", wrong)
		}
	}
	if err := rows["truncated"].req.guard(parse(contextsFamily, strings.Replace(truncated,
		`"first_entry":1791415140`, `"first_entry":1791415200`, 1), nowMerge)); err == nil {
		t.Errorf("the truncated guard took the first host's first entry")
	}

	// `merge-gone` (D232), another pair of the same probe whose base was the same minute's: C's answers once the
	// fixture child was gone (the first right after it read `stale`, its context still collected; the others a
	// second later), then once both were
	const (
		nodes2  = `"nodes":[` + child2 + `"ni":0,` + st + `]`
		ownQ    = `"title":"title [x]","family":"fam","units":"units","priority":1000,"first_entry":1791415200,`
		own2Q   = `"title":"other [x]","family":"fam2","units":"units2","priority":900,"first_entry":1791415140,`
		mergedQ = `"title":"[x] [x]","family":"fam[x]","units":"units","priority":900,"first_entry":1791415140,`
		// the seconds of the answers: the first after `stale`, the gone rows', the rows' once both were gone
		nowStale, nowGone, nowBoth int64 = 1791415379, 1791415380, 1791415381
	)
	contexts := func(q, r string) string {
		if r == "" {
			return `"contexts":{"q.ctx":{` + q + `}}`
		}
		return `"contexts":{"q.ctx":{` + q + `},"r.ctx":{` + rMerge + r + `}}`
	}
	const live, liveGone = `"last_entry":NOW,"live":true`, `"last_entry":1791415380,"live":true`
	stillCollected := body(nowStale, `"api":2`, nodesC, contexts(ownQ+live, ""), hashes(13), agent, timings)
	goneChild := body(nowGone, `"api":2`, nodesC, contexts(ownQ+`"last_entry":1791415260,"live":false`, ""),
		hashes(13), agent, timings)
	goneChild2 := body(nowGone, `"api":2`, nodes2, contexts(own2Q+liveGone, liveGone), hashes(26), agent, timings)
	goneTitles := body(nowGone, `"api":2`, nodes3, contexts(mergedQ+liveGone, liveGone), hashes(39), agent, timings)
	const dead2 = `"last_entry":1791415200,"live":false`
	bothChild2 := body(nowBoth, `"api":2`, nodes2, contexts(own2Q+dead2, dead2), hashes(26), agent, timings)
	bothTitles := body(nowBoth, `"api":2`, nodes3, contexts(mergedQ+`"last_entry":1791415260,"live":false`, dead2),
		hashes(39), agent, timings)
	// a merge without C's OR of the flags: the collected newcomer's title, family, units and priority
	keepNew := body(nowGone, `"api":2`, nodes3, contexts(own2Q+liveGone, liveGone), hashes(39), agent, timings)
	// a merge whose rules read C's flags but whose kept entry does not take the newcomer's: it stays not collected
	notCollected := body(nowGone, `"api":2`, nodes3, contexts(mergedQ+`"last_entry":1791415260,"live":false`,
		liveGone), hashes(39), agent, timings)
	gone := map[string]contextsRow{}
	for _, r := range slices.Concat(contextsGoneRows(baseMerge), contextsBothGoneRows(baseMerge)) {
		gone[r.req.name] = r
	}
	for name, c := range map[string]struct {
		c       string
		at      int64
		refused map[string]string
	}{
		"child": {goneChild, nowGone, map[string]string{
			"the context still collected": stillCollected,
			"the second child's":          goneChild2,
			"the second child listed too": strings.Replace(goneChild, nodesC, `"nodes":[`+child+`"ni":0,`+st+`,`+
				child2+`"ni":1,`+st+`]`, 1),
			"a second context": strings.Replace(goneChild, `"live":false}}`, `"live":false},"r.ctx":{`+rMerge+
				dead2+`}}`, 1),
		}},
		"child2": {goneChild2, nowGone, map[string]string{
			"the second child gone": bothChild2,
			"the merged answer":     goneTitles,
			"the first entry of the first host": strings.Replace(goneChild2,
				`"priority":900,"first_entry":1791415140`, `"priority":900,"first_entry":1791415200`, 1),
		}},
		"titles": {goneTitles, nowGone, map[string]string{
			"the collected newcomer's":   keepNew,
			"an entry not collected":     notCollected,
			"the newcomer's title alone": strings.Replace(goneTitles, `"title":"[x] [x]"`, `"title":"other [x]"`, 1),
			"the newcomer's family":      strings.Replace(goneTitles, `"family":"fam[x]"`, `"family":"fam2"`, 1),
			"the newcomer's units":       strings.Replace(goneTitles, `"units":"units"`, `"units":"units2"`, 1),
			"the first host's priority":  strings.Replace(goneTitles, `"priority":900`, `"priority":1000`, 1),
			"the first host's first entry": strings.Replace(goneTitles, `"priority":900,"first_entry":1791415140`,
				`"priority":900,"first_entry":1791415200`, 1),
		}},
		"both-child2": {bothChild2, nowBoth, map[string]string{
			"the contexts still collected": goneChild2,
			"q.ctx live at its data's end": strings.Replace(bothChild2, own2Q+dead2,
				own2Q+`"last_entry":1791415200,"live":true`, 1),
			"r.ctx still collected": strings.Replace(bothChild2, `"priority":1100,"first_entry":1791415140,`+dead2,
				`"priority":1100,"first_entry":1791415140,`+liveGone, 1),
		}},
		"both-titles": {bothTitles, nowBoth, map[string]string{
			"the contexts still collected": goneTitles,
			"q.ctx live at its data's end": strings.Replace(bothTitles, `"last_entry":1791415260,"live":false`,
				`"last_entry":1791415260,"live":true`, 1),
			"the newcomer's last entry": strings.Replace(bothTitles, `"last_entry":1791415260`,
				`"last_entry":1791415200`, 1),
			"the first host's first entry": strings.Replace(bothTitles, `"priority":900,"first_entry":1791415140`,
				`"priority":900,"first_entry":1791415200`, 1),
			"the first host's last entry in r.ctx": strings.Replace(bothTitles,
				`"priority":1100,"first_entry":1791415140,`+dead2,
				`"priority":1100,"first_entry":1791415140,"last_entry":1791415260,"live":false`, 1),
			"the newcomer's title": strings.Replace(bothTitles, `"title":"[x] [x]"`, `"title":"other [x]"`, 1),
		}},
	} {
		r, ok := gone[name]
		if !ok {
			t.Errorf("no row %s", name)
			continue
		}
		if err := r.req.guard(parse(r.fam, c.c, c.at)); err != nil {
			t.Errorf("the merge-gone %s guard on C's answer: %v", name, err)
		}
		for what, refused := range c.refused {
			if refused == c.c {
				t.Errorf("the merge-gone %s guard: %s is C's answer", name, what)
			}
			if err := r.req.guard(parse(r.fam, refused, c.at)); err == nil {
				t.Errorf("the merge-gone %s guard took %s", name, what)
			}
		}
	}
	if len(gone) != 5 {
		t.Errorf("%d merge-gone rows, pinned 5", len(gone))
	}
	// and each fact of a merge-gone guard alone: C's answer with one member another's (old, new) is refused
	rFacts := [][2]string{{`"title":"r title [x]"`, `"title":"r title a"`}, {`"family":"rfam"`, `"family":"fam"`},
		{`"units":"runits"`, `"units":"units"`}, {`"priority":1100`, `"priority":1101`}}
	for name, c := range map[string]struct {
		c       string
		at      int64
		members [][2]string
	}{
		"child": {goneChild, nowGone, [][2]string{{`"title":"title [x]"`, `"title":"[x] [x]"`},
			{`"family":"fam"`, `"family":"fam[x]"`}, {`"units":"units"`, `"units":"units2"`},
			{`"priority":1000`, `"priority":900`}, {`"first_entry":1791415200`, `"first_entry":1791415140`},
			{`"ni":0`, `"ni":1`}, {`"msg":""`, `"msg":"x"`}, {`"api":2,`, ``}}},
		"child2": {goneChild2, nowGone, slices.Concat(rFacts, [][2]string{
			{`"title":"other [x]"`, `"title":"[x] [x]"`}, {`"family":"fam2"`, `"family":"fam[x]"`},
			{`"units":"units2"`, `"units":"units"`}, {`"priority":900`, `"priority":1000`},
			{`"first_entry":1791415140,"last_entry":1791415380,"live":true}}`,
				`"first_entry":1791415200,"last_entry":1791415380,"live":true}}`}})},
		"titles": {goneTitles, nowGone, slices.Concat(rFacts, [][2]string{
			{`"first_entry":1791415140,"last_entry":1791415380,"live":true}}`,
				`"first_entry":1791415200,"last_entry":1791415380,"live":true}}`},
			{nodes3, nodes}, {`"api":2,`, ``}, {`},"r.ctx":{` + rMerge + liveGone + `}}`, `}}`}})},
		"both-child2": {bothChild2, nowBoth, slices.Concat(rFacts, [][2]string{
			{`"title":"other [x]"`, `"title":"[x] [x]"`}, {`"family":"fam2"`, `"family":"fam[x]"`},
			{`"units":"units2"`, `"units":"units"`}, {`"priority":900`, `"priority":1000`},
			{`"priority":900,"first_entry":1791415140`, `"priority":900,"first_entry":1791415200`},
			{`"priority":1100,"first_entry":1791415140`, `"priority":1100,"first_entry":1791415200`},
			{`"ni":0`, `"ni":2`}, {`"api":2,`, ``}, {`},"r.ctx":{` + rMerge + dead2 + `}}`, `}}`}})},
		"both-titles": {bothTitles, nowBoth, slices.Concat(rFacts, [][2]string{
			{`"family":"fam[x]"`, `"family":"fam"`}, {`"units":"units"`, `"units":"units2"`},
			{`"priority":900`, `"priority":1000`},
			{`"priority":1100,"first_entry":1791415140`, `"priority":1100,"first_entry":1791415200`},
			{nodes3, nodes}, {`"api":2,`, ``}, {`},"r.ctx":{` + rMerge + dead2 + `}}`, `}}`}})},
	} {
		for _, m := range c.members {
			planted := strings.Replace(c.c, m[0], m[1], 1)
			if planted == c.c {
				t.Errorf("the merge-gone %s guard: C's answer does not hold %s", name, m[0])
			} else if err := gone[name].req.guard(parse(gone[name].fam, planted, c.at)); err == nil {
				t.Errorf("the merge-gone %s guard took %s for %s", name, m[1], m[0])
			}
		}
	}
	// `merge-keep` (D238): C's answers once the second child alone was gone (H35's probe P2, the same members in
	// this pair's seconds). The first host's q.ctx is collected and the newcomer's is not, so the first host's
	// title, family, units and priority are kept; the retention's start is still the newcomer's
	const keptQ = `"title":"title [x]","family":"fam","units":"units","priority":1000,"first_entry":1791415140,`
	keepTitles := body(nowGone, `"api":2`, nodes3, contexts(keptQ+liveGone, dead2), hashes(39), agent, timings)
	keep := map[string]contextsRow{}
	for _, r := range contextsKeepRows(baseMerge) {
		keep[r.req.name] = r
	}
	for name, c := range map[string]struct {
		c       string
		refused map[string]string
	}{
		"child2": {bothChild2, map[string]string{
			"the contexts still collected": goneChild2,
			"q.ctx live at its data's end": strings.Replace(bothChild2, own2Q+dead2,
				own2Q+`"last_entry":1791415200,"live":true`, 1),
			"the merged answer": keepTitles,
		}},
		"titles": {keepTitles, map[string]string{
			// a port that always merges: `merge`'s title, family and priority
			"always merged": body(nowGone, `"api":2`, nodes3, contexts(mergedQ+liveGone, dead2), hashes(39), agent,
				timings),
			// a port that takes the newcomer's flags: not collected, its last entry the fixture's end
			"the newcomer's flags": body(nowGone, `"api":2`, nodes3, contexts(keptQ+
				`"last_entry":1791415260,"live":false`, dead2), hashes(39), agent, timings),
			"the second child still collected": goneTitles,
			"the merged title alone":           strings.Replace(keepTitles, `"title":"title [x]"`, `"title":"[x] [x]"`, 1),
			"the merged family alone":          strings.Replace(keepTitles, `"family":"fam"`, `"family":"fam[x]"`, 1),
			"the newcomer's units":             strings.Replace(keepTitles, `"units":"units"`, `"units":"units2"`, 1),
			"the lower priority":               strings.Replace(keepTitles, `"priority":1000`, `"priority":900`, 1),
			"the first host's first entry": strings.Replace(keepTitles, `"priority":1000,"first_entry":1791415140`,
				`"priority":1000,"first_entry":1791415200`, 1),
			"not live at the walk's clock": strings.Replace(keepTitles, keptQ+liveGone,
				keptQ+`"last_entry":1791415380,"live":false`, 1),
			"live at its data's end": strings.Replace(keepTitles, keptQ+liveGone,
				keptQ+`"last_entry":1791415260,"live":true`, 1),
			"r.ctx still collected": strings.Replace(keepTitles, `"priority":1100,"first_entry":1791415140,`+dead2,
				`"priority":1100,"first_entry":1791415140,`+liveGone, 1),
			"r.ctx's last entry the fixture's": strings.Replace(keepTitles,
				`"priority":1100,"first_entry":1791415140,`+dead2,
				`"priority":1100,"first_entry":1791415140,"last_entry":1791415260,"live":false`, 1),
			"two hosts":       strings.Replace(keepTitles, nodes3, nodes, 1),
			"without r.ctx":   strings.Replace(keepTitles, `},"r.ctx":{`+rMerge+dead2+`}}`, `}}`, 1),
			"without its api": strings.Replace(keepTitles, `"api":2,`, ``, 1),
		}},
	} {
		r, ok := keep[name]
		if !ok {
			t.Errorf("no merge-keep row %s", name)
			continue
		}
		if err := r.req.guard(parse(r.fam, c.c, nowGone)); err != nil {
			t.Errorf("the merge-keep %s guard on C's answer: %v", name, err)
		}
		for what, refused := range c.refused {
			if refused == c.c {
				t.Errorf("the merge-keep %s guard: %s is C's answer", name, what)
			}
			if err := r.req.guard(parse(r.fam, refused, nowGone)); err == nil {
				t.Errorf("the merge-keep %s guard took %s", name, what)
			}
		}
	}
	if len(keep) != 2 {
		t.Errorf("%d merge-keep rows, pinned 2", len(keep))
	}
	if got := dashNormDiffs(t, contextsFamily, contextsFamily.masks, [2]int64{nowGone, nowGone}, keepTitles,
		strings.Replace(keepTitles, `"query_ms":0.039`, `"query_ms":0.02`, 1)); got != "" {
		t.Errorf("contexts merge-keep, the recorded pair: differences at %q", got)
	}
	if got := dashNormDiffs(t, contextsFamily, contextsFamily.masks, [2]int64{nowGone, nowGone}, keepTitles,
		body(nowGone, `"api":2`, nodes3, contexts(mergedQ+liveGone, dead2), hashes(39), agent, timings)); got !=
		"$.contexts.q.ctx.title $.contexts.q.ctx.family $.contexts.q.ctx.priority" {
		t.Errorf("contexts merge-keep against a port that always merges: differences at %q", got)
	}

	// the two C sides' answers of a gone row show no difference; an entry that is not collected keeps its last
	// entry's second, which is compared
	for name, c := range map[string]struct{ oracle, candidate, want string }{
		"the recorded pair": {goneChild, strings.Replace(goneChild, `"query_ms":0.039`, `"query_ms":0.03`, 1), ""},
		"a last entry a second later": {goneChild, strings.Replace(goneChild, `"last_entry":1791415260`,
			`"last_entry":1791415261`, 1), "$.contexts.q.ctx.last_entry"},
		"the merged pair": {goneTitles, strings.Replace(goneTitles, `"query_ms":0.039`, `"query_ms":0.071`, 1), ""},
		"the merge without the OR": {goneTitles, keepNew, "$.contexts.q.ctx.title $.contexts.q.ctx.family " +
			"$.contexts.q.ctx.units"},
		"both gone, the merged pair": {bothTitles, strings.Replace(bothTitles, `"query_ms":0.039`, `"query_ms":0.02`,
			1), ""},
		"both gone, the newcomer's last entry": {bothTitles, strings.Replace(bothTitles, `"last_entry":1791415260`,
			`"last_entry":1791415200`, 1), "$.contexts.q.ctx.last_entry"},
		"both gone, the second child's pair": {bothChild2, strings.Replace(bothChild2, `"query_ms":0.039`,
			`"query_ms":0.006`, 1), ""},
	} {
		if got := dashNormDiffs(t, contextsFamily, contextsFamily.masks, [2]int64{nowGone, nowGone}, c.oracle,
			c.candidate); got != c.want {
			t.Errorf("contexts merge-gone, %s: differences at %q, want %q", name, got, c.want)
		}
	}
}
