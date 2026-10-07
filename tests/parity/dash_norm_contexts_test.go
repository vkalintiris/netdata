// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strings"
	"testing"
)

// testDashNormContexts pins the rules of the node and context families (nodesFamily, nodeInstancesFamily,
// contextsFamily) and their guards' facts on C's answers, recorded C against C (the oracle as its own candidate, run
// nd-h29s3-probe1, 18:43Z) and trimmed to the members under test.
func testDashNormContexts(t *testing.T) {
	// C's capability lists of localhost and of the fixture child, as recorded
	const (
		capsLocal = `[{"name":"proto","version":1,"enabled":true},{"name":"ml","version":1,"enabled":false},` +
			`{"name":"mc","version":1,"enabled":true},{"name":"ctx","version":1,"enabled":true},` +
			`{"name":"funcs","version":1,"enabled":true},{"name":"http_api_v2","version":7,"enabled":true},` +
			`{"name":"health","version":2,"enabled":false},{"name":"req_cancel","version":1,"enabled":true},` +
			`{"name":"dyncfg","version":2,"enabled":true}]`
		capsChild = `[{"name":"proto","version":1,"enabled":true},{"name":"ml","version":1,"enabled":false},` +
			`{"name":"mc","version":1,"enabled":true},{"name":"ctx","version":1,"enabled":true},` +
			`{"name":"funcs","version":0,"enabled":false},{"name":"http_api_v2","version":7,"enabled":true},` +
			`{"name":"health","version":2,"enabled":false},{"name":"req_cancel","version":1,"enabled":true},` +
			`{"name":"dyncfg","version":2,"enabled":false}]`
	)
	if nodeCaps(true) != capsLocal || nodeCaps(false) != capsChild {
		t.Errorf("nodeCaps is\n%s\n%s, C recorded\n%s\n%s", nodeCaps(true), nodeCaps(false), capsLocal, capsChild)
	}
	caps := strings.NewReplacer("CAPS_LOCAL", capsLocal, "CAPS_CHILD", capsChild)
	const (
		agent    = `{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":`
		stOK     = `"st":{"ai":0,"code":200,"msg":""}`
		parentNd = `{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent",`
		childNd  = `{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child",`
		versions = `"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,` +
			`"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0}`
		// `/api/v2/contexts?scope_nodes=parity-child`
		ctx = `{"api":2,"nodes":[` + childNd + `"ni":0,` + stOK + `}],"contexts":{"q.ctx":{"family":"fam",` +
			`"units":"units","priority":1000,"first_entry":1791312060,"last_entry":1791312192,"live":true}},` +
			versions + `,"agents":[` + agent + `1791312192,"ai":0,"timings":{"prep_ms":0,"query_ms":0.022,` +
			`"output_ms":0.019,"total_ms":0.041,"cloud_ms":0.041}}],"timings":{"routing_ms":0,"node_max_ms":0,` +
			`"total_ms":0.041}}`
		// `/api/v2/contexts?scope_nodes=*` before the child connected
		ctxAlone = `{"api":2,"nodes":[` + parentNd + `"ni":0,` + stOK + `}],"contexts":{},"versions":{` +
			`"routing_hard_hash":1,"nodes_hard_hash":1,"contexts_hard_hash":0,"contexts_soft_hash":0,` +
			`"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[` + agent + `1791312190,"ai":0,"timings":{` +
			`"prep_ms":0,"query_ms":0.038,"output_ms":0.071,"total_ms":0.109,"cloud_ms":0.109}}],` +
			`"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.109}}`
	)

	// a family's `now` (maskNowKeys): a second of the request's flight where C prints its walk's clock, and nothing
	// else; the body's own `now` is not trusted (a walk on the wrong clock throughout is not hidden)
	flight := [2]int64{1791312192, 1791312192}
	for name, c := range map[string]struct {
		in     string
		flight [2]int64
		want   string
	}{
		"the walk's clock": {ctx, flight, strings.Replace(ctx, `"last_entry":1791312192`, `"last_entry":"NOW"`, 1)},
		"a second before": {strings.Replace(ctx, `"last_entry":1791312192`, `"last_entry":1791312191`, 1), flight,
			""},
		"a flight over two seconds": {strings.Replace(ctx, `"last_entry":1791312192`, `"last_entry":1791312191`, 1),
			[2]int64{1791312191, 1791312192}, strings.Replace(ctx, `"last_entry":1791312192`, `"last_entry":"NOW"`, 1)},
		"the body's clock, out of flight": {ctx, [2]int64{1791312195, 1791312196}, ""},
		"another member":                  {strings.Replace(ctx, `"last_entry"`, `"last_seen"`, 1), flight, ""},
		"pretty": {"\"last_entry\":1791312192,\n    \"now\":1791312192", flight,
			"\"last_entry\":\"NOW\",\n    \"now\":1791312192"},
	} {
		want := c.want
		if want == "" {
			want = c.in
		}
		if got := string(contextsFamily.normalise(0, c.flight, []byte(c.in))); got != want {
			t.Errorf("contextsFamily's now, %s: %s, want %s", name, got, want)
		}
	}
	// and the first agent's `now` itself must be a second of the flight (v2NowInFlight)
	ctxDoc, err := ParseJSON([]byte(ctx))
	if err != nil {
		t.Fatal(err)
	}
	for name, c := range map[string]struct {
		flight [2]int64
		bad    bool
	}{
		"in flight":     {flight, false},
		"a second late": {[2]int64{1791312193, 1791312194}, true},
		"a second soon": {[2]int64{1791312190, 1791312191}, true},
	} {
		if err := v2NowInFlight(ctxDoc, c.flight); (err != nil) != c.bad {
			t.Errorf("v2NowInFlight, %s: %v", name, err)
		}
	}
	if err := v2NowInFlight(Value{Kind: KindObject}, flight); err != nil {
		t.Errorf("v2NowInFlight without agents: %v", err)
	}

	// `/api/v3/node_instances`, both sides of one C-against-C round (each instance's functions and the agent's
	// application and counts left out): each side's ports, start, and its tier's start and retention
	ni := func(local, remote, start, age, from, retention, expected, expectedHuman string) string {
		return caps.Replace(`{"api":2,"nodes":[` + parentNd + `"ni":0,"instances":[{` + stOK + `,"db":{` +
			`"status":"initializing","liveness":"stale","mode":"dbengine","first_time":0,"last_time":1791312192,` +
			`"metrics":0,"instances":0,"contexts":0},"ingest":{"id":0,"hops":0,"type":"localhost",` +
			`"status":"initializing","since":` + start + `,"age":` + age + `,"metrics":0,"instances":0,` +
			`"contexts":0},"ml":{"status":"disabled","type":"disabled"},"health":{"status":"disabled"},` +
			`"capabilities":CAPS_LOCAL,"dyncfg":{"status":"online"}}]},` + childNd + `"ni":1,"instances":[{` +
			stOK + `,"db":{"status":"online","liveness":"live","mode":"ram","first_time":1791312060,` +
			`"last_time":1791312192,"metrics":7,"instances":2,"contexts":1},"ingest":{"id":1,"hops":1,` +
			`"type":"child","status":"online","since":1791312190,"age":2,"metrics":7,"instances":2,"contexts":1,` +
			`"source":{"local":"[127.0.0.1]:` + local + `","remote":"[127.0.0.1]:` + remote + `",` +
			`"capabilities":["VCAPS","HLABELS","CLABELS","INTERPOLATED"]}},"ml":{"status":"disabled",` +
			`"type":"disabled"},"health":{"status":"disabled"},"capabilities":CAPS_CHILD,` +
			`"dyncfg":{"status":"unavailable"}}]}],` + versions + `,"agents":[` + agent + `1791312192,"ai":0,` +
			`"cloud":{"id":0,"status":"available","since":` + start + `,"age":` + age + `,` +
			`"url":"https://app.netdata.cloud","reason":"Agent is not claimed yet"},"nodes":{"total":2,` +
			`"receiving":1,"sending":0,"archived":0},"db_size":[{"tier":0,"granularity":"1s","metrics":0,` +
			`"samples":0,"disk_used":8192,"disk_max":1073741824,"disk_percent":0,"from":` + from +
			`,"to":1791312192,"retention":` + retention + `,"retention_human":"1m","requested_retention":0,` +
			`"requested_retention_human":"off","expected_retention":` + expected + `,"expected_retention_human":"` +
			expectedHuman + `"}]}]}`)
	}
	niO := ni("38929", "34050", "1791312184", "8", "1791312187", "5", "655360", "7d15h")
	niC := ni("42091", "39074", "1791312187", "5", "1791312190", "2", "262144", "3d1h")
	for name, c := range map[string]struct {
		candidate string
		fam       v2Family
		masks     []Mask
		want      string
	}{
		// what differs once the envelope's clocks are masked: the family masks exactly that
		"the envelope's masks alone": {niC, v2Family{}, infoV2Volatile,
			"$.nodes[0].instances[0].ingest.since $.nodes[0].instances[0].ingest.age " +
				"$.nodes[1].instances[0].ingest.source.local $.nodes[1].instances[0].ingest.source.remote " +
				"$.agents[0].db_size[0].from $.agents[0].db_size[0].retention " +
				"$.agents[0].db_size[0].expected_retention $.agents[0].db_size[0].expected_retention_human"},
		"the family": {niC, nodeInstancesFamily, nodeInstancesFamily.masks, ""},
		"a child's metrics": {strings.Replace(niC, `"metrics":7`, `"metrics":8`, 1), nodeInstancesFamily,
			nodeInstancesFamily.masks, "$.nodes[1].instances[0].db.metrics"},
		"a child's last time a second behind": {strings.Replace(niC, `"last_time":1791312192,"metrics":7`,
			`"last_time":1791312191,"metrics":7`, 1), nodeInstancesFamily, nodeInstancesFamily.masks,
			"$.nodes[1].instances[0].db.last_time"},
		"a capability, compared by name": {strings.Replace(niC, `"name":"funcs","version":0,"enabled":false`,
			`"name":"funcs","version":1,"enabled":true`, 1), nodeInstancesFamily, nodeInstancesFamily.masks, ""},
		// the ingestion's source and age: only the port and the start are each side's
		"a source without its brackets": {strings.Replace(niC, `"local":"[127.0.0.1]:`, `"local":"127.0.0.1:`, 1),
			nodeInstancesFamily, nodeInstancesFamily.masks, "$.nodes[1].instances[0].ingest.source.local"},
		"a source on another address": {strings.Replace(niC, `"remote":"[127.0.0.1]:`, `"remote":"[::1]:`, 1),
			nodeInstancesFamily, nodeInstancesFamily.masks, "$.nodes[1].instances[0].ingest.source.remote"},
		"a source with a suffix": {strings.Replace(niC, `"remote":"[127.0.0.1]:39074"`,
			`"remote":"[127.0.0.1]:39074:SSL"`, 1), nodeInstancesFamily, nodeInstancesFamily.masks,
			"$.nodes[1].instances[0].ingest.source.remote"},
		"an age that is not now - since": {strings.Replace(niC, `"since":1791312190,"age":2,`,
			`"since":1791312190,"age":3,`, 1), nodeInstancesFamily, nodeInstancesFamily.masks,
			"$.nodes[1].instances[0].ingest.age"},
	} {
		if got := dashNormDiffs(t, c.fam, c.masks, flight, niO, c.candidate); got != c.want {
			t.Errorf("node_instances, %s: differences at %q, want %q", name, got, c.want)
		}
	}
	got := string(nodeInstancesFamily.normalise(0, flight, []byte(niO)))
	if strings.Count(got, `"last_time":"NOW"`) != 2 || !strings.Contains(got, `"first_time":1791312060`) ||
		!strings.Contains(got, `"local":"[127.0.0.1]:PORT"`) ||
		!strings.Contains(got, `"since":1791312190,"age":"NOW-SINCE"`) ||
		!strings.Contains(got, `"since":1791312184,"age":"NOW-SINCE"`) {
		t.Errorf("node_instances' normalisation: %s", got)
	}

	// `/api/v3/nodes` (localhost's labels and system info trimmed): the labels a set, the capabilities by name
	nodes := caps.Replace(`{"api":2,"nodes":[` + parentNd + `"ni":0,"v":"v2.11.0-458-g1e97a0fc9e",` +
		`"labels":{"_os":"linux","_hostname":"parity-parent","_is_parent":"true"},"state":"reachable",` +
		`"health":{"status":"disabled"},"capabilities":CAPS_LOCAL},` + childNd + `"ni":1,"v":"1.0","labels":{},` +
		`"hw":{"architecture":"","cpu_frequency":"","cpus":"","memory":"","disk_space":"","virtualization":"",` +
		`"container":""},"os":{"id":"","nm":"","v":"","kernel":{"nm":"","v":""}},"state":"reachable",` +
		`"health":{"status":"disabled"},"capabilities":CAPS_CHILD}]}`)
	reordered := strings.Replace(nodes, `{"_os":"linux","_hostname":"parity-parent","_is_parent":"true"}`,
		`{"_is_parent":"true","_hostname":"parity-parent","_os":"linux"}`, 1)
	mlOn := strings.Replace(nodes, `"name":"ml","version":1,"enabled":false`, `"name":"ml","version":1,"enabled":true`,
		1)
	for name, c := range map[string]struct {
		candidate string
		fam       v2Family
		want      string
	}{
		"the same answer":                  {nodes, nodesFamily, ""},
		"the labels in another order":      {reordered, nodesFamily, ""},
		"the labels' order, compared":      {reordered, v2Family{}, "$.nodes[0].labels.<members>"},
		"a label's value":                  {strings.Replace(nodes, `"_os":"linux"`, `"_os":"other"`, 1), nodesFamily, "$.nodes[0].labels._os"},
		"a capability, compared by name":   {mlOn, nodesFamily, ""},
		"a capability, without the family": {mlOn, v2Family{}, "$.nodes[0].capabilities[1].enabled"},
		"the child's version":              {strings.Replace(nodes, `"v":"1.0"`, `"v":"1.1"`, 1), nodesFamily, "$.nodes[1].v"},
	} {
		if got := dashNormDiffs(t, c.fam, c.fam.masks, flight, nodes, c.candidate); got != c.want {
			t.Errorf("nodes, %s: differences at %q, want %q", name, got, c.want)
		}
	}

	// the lists compareNodesCapabilities compares by name
	for name, c := range map[string]struct{ body, where, last string }{
		"nodes":          {nodes, "nodes[0] nodes[1]", capsChild},
		"node instances": {niO, "nodes[0].instances[0] nodes[1].instances[0]", capsChild},
		"none":           {ctxAlone, "", ""},
	} {
		v, err := ParseJSON([]byte(c.body))
		if err != nil {
			t.Fatal(err)
		}
		where, lists := nodesCapabilities(v)
		last := ""
		if len(lists) > 0 {
			last = lists[len(lists)-1].String()
		}
		if strings.Join(where, " ") != c.where || last != c.last {
			t.Errorf("nodesCapabilities, %s: %v, the last %s", name, where, last)
		}
	}

	// the facts' paths: keys with dots, items, and what is not there
	v, err := ParseJSON(contextsFamily.normalise(0, flight, []byte(ctx)))
	if err != nil {
		t.Fatal(err)
	}
	alone, err := ParseJSON(contextsFamily.normalise(0, [2]int64{1791312190, 1791312190}, []byte(ctxAlone)))
	if err != nil {
		t.Fatal(err)
	}
	for name, c := range map[string]struct {
		fact dashFact
		on   Value
		want string
	}{
		"a key with dots":      {dashIs(`"fam"`, "contexts", "q.ctx", "family"), v, ""},
		"an item":              {dashIs(`"parity-child"`, "nodes", "[0]", "nm"), v, ""},
		"an item past the end": {dashIs(`"x"`, "nodes", "[1]", "nm"), v, `no nodes.[1], want "x"`},
		"a value":              {dashIs("1001", "contexts", "q.ctx", "priority"), v, "contexts.q.ctx.priority is 1000, want 1001"},
		"absent":               {dashAbsent("nodes", "[1]"), v, ""},
		"present":              {dashAbsent("nodes", "[0]", "st", "ai"), v, "nodes.[0].st.ai is 0, want nothing there"},
		"above":                {dashAbove(0, "versions", "contexts_hard_hash"), v, ""},
		"not above":            {dashAbove(0, "versions", "contexts_hard_hash"), alone, "versions.contexts_hard_hash is 0, want a number above 0"},
		"keys":                 {dashKeys("api nodes contexts versions agents timings"), v, ""},
		"other keys":           {dashKeys("api nodes"), v, "the answer has members [api nodes contexts versions agents timings], want [api nodes]"},
		"no object":            {dashKeys("x", "nodes", "[0]", "nm"), v, "nodes.[0].nm has members [], want [x]"},
	} {
		got := ""
		if err := c.fact(c.on); err != nil {
			got = err.Error()
		}
		if got != c.want {
			t.Errorf("fact, %s: %q, want %q", name, got, c.want)
		}
	}

	// the contexts guards take C's answer with the child and refuse it without the child
	if err := dashGuard(dashWalkNodes(false), contextsFacts)(v); err != nil {
		t.Errorf("the child scope's guard on C's answer: %v", err)
	}
	for name, guard := range map[string]func(Value) error{
		"every host":   dashGuard(dashWalkNodes(true)),
		"the contexts": dashGuard(contextsFacts),
	} {
		if err := guard(alone); err == nil {
			t.Errorf("the %s guard took C's answer without the child", name)
		}
	}
}
