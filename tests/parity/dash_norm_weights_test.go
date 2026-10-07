// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strings"
	"testing"
)

// testDashNormWeights pins the rules of the weights, netdata-streaming and info-tail families on answers recorded
// from C (the oracle as its own candidate, run nd-h30s5-probe2): each rule hides what differed there and nothing
// else.
func testDashNormWeights(t *testing.T) {
	parse := func(name, body string) Value {
		t.Helper()
		v, err := ParseJSON([]byte(body))
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		return v
	}
	diffs := func(o, c Value, masks []Mask, unordered ...string) []string {
		var out []string
		for _, d := range Compare(ApplyMasks(o, masks), ApplyMasks(c, masks), unordered...) {
			out = append(out, d.String())
		}
		return out
	}

	// a v2 node's status duration (jsonwrap-v2.c:14-15): dropped on both sides, whatever its value, and nothing else
	weightsNode := func(ms string) string {
		return `{
    "dictionaries":{
        "nodes":[{
                "mg":"5a1e0000-0000-4000-8000-0000000000bb",
                "nm":"parity-child",
                "ni":0,
                "st":{
                    "ai":0,
                    "code":200,
                    "msg":""` + ms + `
                }
            }]
    }
}`
	}
	for name, c := range map[string]struct{ body, want string }{
		"the oracle's":    {weightsNode(",\n                    \"ms\":0.066"), weightsNode("")},
		"the candidate's": {weightsNode(",\n                    \"ms\":0.09"), weightsNode("")},
		"none at 0":       {weightsNode(""), weightsNode("")},
		"minified":        {`{"st":{"ai":0,"code":200,"msg":"","ms":0.006}}`, `{"st":{"ai":0,"code":200,"msg":""}}`},
		"other durations": {`{"timings":{"query_ms":0.135,"total_ms":0.438}}`,
			`{"timings":{"query_ms":0.135,"total_ms":0.438}}`},
	} {
		for i := range 2 {
			if got := string(weightsRender(i, []byte(c.body))); got != c.want {
				t.Errorf("weightsRender %s, side %d:\n%s\nwant\n%s", name, i, got, c.want)
			}
		}
	}

	// the v2 family: the agents' clock and durations and the node's duration differed (the `anomaly` row); the versions
	// hashes did not, and stay compared (the `two-hosts` row's walk adds the context versions twice)
	weightsAnswer := func(hash, ms, now, query, output, total string) Value {
		return parse("weights answer", string(weightsRender(0, []byte(`{
    "versions":{
        "routing_hard_hash":1,
        "nodes_hard_hash":2,
        "contexts_hard_hash":`+hash+`,
        "contexts_soft_hash":0,
        "alerts_hard_hash":0,
        "alerts_soft_hash":0
    },
    "dictionaries":{
        "nodes":[{
                "mg":"5a1e0000-0000-4000-8000-0000000000bb",
                "nm":"parity-child",
                "ni":0,
                "st":{
                    "ai":0,
                    "code":200,
                    "msg":"",
                    "ms":`+ms+`
                }
            }]
    },
    "agents":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nd":null,
            "nm":"parity-parent",
            "now":`+now+`,
            "ai":0,
            "timings":{
                "prep_ms":0,
                "query_ms":`+query+`,
                "output_ms":`+output+`,
                "total_ms":`+total+`,
                "cloud_ms":`+total+`
            }
        }],
    "correlated_dimensions":1
}`))))
	}
	oracle := weightsAnswer("10", "0.007", "1791313264", "0.135", "0.303", "0.438")
	candidate := weightsAnswer("10", "0.006", "1791313264", "0.126", "0.073", "0.199")
	if d := diffs(oracle, candidate, weightsFamily.masks); d != nil {
		t.Errorf("weightsFamily: the recorded answers differ: %q", d)
	}
	twoHosts := weightsAnswer("20", "0.007", "1791313264", "0.135", "0.303", "0.438")
	if d := diffs(oracle, twoHosts, weightsFamily.masks); len(d) != 1 ||
		!strings.HasPrefix(d[0], "$.versions.contexts_hard_hash:") {
		t.Errorf("weightsFamily: another hash gives %q, want one difference at versions.contexts_hard_hash", d)
	}

	// the v1 family: the query's duration differed (the `mc` row, weights.c:326); the statistics beside it stay compared
	v1 := func(ms, queries string) Value {
		return parse("v1 answer", `{
    "statistics":{
        "query_time_ms":`+ms+`,
        "db_queries":`+queries+`,
        "query_result_points":484,
        "binary_searches":0,
        "db_points_read":484,
        "db_points_per_tier":[484]
    },
    "correlated_dimensions":2
}`)
	}
	if d := diffs(v1("0.274", "4"), v1("0.425", "4"), weightsV1Family.masks); d != nil {
		t.Errorf("weightsV1Family: the recorded answers differ: %q", d)
	}
	if d := diffs(v1("0.274", "4"), v1("0.274", "5"), weightsV1Family.masks); len(d) != 1 ||
		!strings.HasPrefix(d[0], "$.statistics.db_queries:") {
		t.Errorf("weightsV1Family: another query count gives %q, want one difference at statistics.db_queries", d)
	}

	// the streaming table: its expiry as the distance from the head's Date (now + 10, function-netdata-streaming.c:1010)
	for name, c := range map[string]struct{ date, expires, want string }{
		"the oracle's":    {"Tue, 06 Oct 2026 19:02:28 GMT", "1791313358", `"NOW+10"`},
		"the candidate's": {"Tue, 06 Oct 2026 19:02:29 GMT", "1791313359", `"NOW+10"`},
		"a second later":  {"Tue, 06 Oct 2026 19:02:29 GMT", "1791313358", `"NOW+10"`},
		"elsewhere":       {"Tue, 06 Oct 2026 19:02:28 GMT", "1791313353", `"NOW+5"`},
	} {
		resp := "HTTP/1.1 200 OK\r\nDate: " + c.date + "\r\nContent-Type: application/json; charset=utf-8\r\n\r\n" +
			"{\n    \"type\":\"table\",\n    \"expires\":" + c.expires + "\n}"
		_, v, err := fnStreamingDoc([]byte(resp))
		if err != nil {
			t.Errorf("fnStreamingDoc %s: %v", name, err)
			continue
		}
		if e, err := dashMember(v, "expires"); err != nil || e.String() != c.want {
			t.Errorf("fnStreamingDoc %s: expires %s (%v), want %s", name, e, err, c.want)
		}
	}

	// the streaming table's volatile columns, masked by name at each side's own index where they hold a number other
	// than 0: localhost's row and the child's as each side wrote them (an excerpt: seven of the 85 columns,
	// renumbered); localhost's port 0 and null retention length, the reason and the metric count stay compared
	const tableDoc = `{
    "columns":{
        "Node":{"index":0,"type":"string"},
        "InStatus":{"index":1,"type":"string"},
        "InAge":{"index":2,"type":"duration","max":{age.max}},
        "InReason":{"index":3,"type":"string"},
        "InLocalPort":{"index":4,"type":"integer","max":{port.max}},
        "dbDuration":{"index":5,"type":"duration","max":{dur.max}},
        "dbMetrics":{"index":6,"type":"integer","max":{metrics}}
    },
    "data":[["parity-parent","initializing",{local.age},"LOCALHOST",{local.port},{local.dur},0],
        ["parity-rchild","online",{age},"{reason}",{port},{dur},{metrics}]]
}`
	// table is the excerpt with the oracle's recorded cells, but for the cells given as pairs ({cell}, value)
	table := func(cells ...string) Value {
		return parse("table", strings.NewReplacer(append(cells, "{age.max}", "35", "{port.max}", "42925",
			"{dur.max}", "26", "{local.age}", "35", "{local.port}", "0", "{local.dur}", "null", "{age}", "20",
			"{reason}", "CONNECTED", "{port}", "42925", "{dur}", "26", "{metrics}", "73")...).Replace(tableDoc))
	}
	streamingDiffs := func(o, c Value) []string {
		var out []string
		for _, d := range Compare(ApplyMasks(o, fnStreamingMasks(o, fnStreamingVolatile)),
			ApplyMasks(c, fnStreamingMasks(c, fnStreamingVolatile))) {
			out = append(out, d.String())
		}
		return out
	}
	recorded := table()
	if d := streamingDiffs(recorded, table("{age.max}", "34", "{port.max}", "45137", "{dur.max}", "24",
		"{local.age}", "34", "{age}", "18", "{port}", "45137", "{dur}", "24")); d != nil {
		t.Errorf("fnStreamingVolatile: the recorded rows differ: %q", d)
	}
	for name, c := range map[string]struct {
		other Value
		at    []string
	}{
		"another reason": {table("{reason}", "VIRTUAL NODE"), []string{"$.data[1][3]:"}},
		"another metric count": {table("{metrics}", "74"),
			[]string{"$.columns.dbMetrics.max:", "$.data[1][6]:"}},
		"a port on localhost":             {table("{local.port}", "45137"), []string{"$.data[0][4]:"}},
		"a retention length on localhost": {table("{local.dur}", "12"), []string{"$.data[0][5]:"}},
		"a child's port of 0": {table("{port}", "0", "{port.max}", "0"),
			[]string{"$.columns.InLocalPort.max:", "$.data[1][4]:"}},
	} {
		d := streamingDiffs(recorded, c.other)
		ok := len(d) == len(c.at)
		for i := 0; ok && i < len(d); i++ {
			ok = strings.HasPrefix(d[i], c.at[i])
		}
		if !ok {
			t.Errorf("fnStreamingVolatile: %s gives %q, want differences at %q", name, d, c.at)
		}
	}

	// the info tail: the host labels in another order (C adds them from concurrent startup threads) compare equal as
	// a map, and only so
	o := `{"host_labels":{"_aclk_available":"true","_aclk_proxy":"none","_mqtt_version":"5","_os":"linux",` +
		`"_timezone":"Etc/UTC","_abbrev_timezone":"UTC"},"functions":{}}`
	c := `{"host_labels":{"_aclk_proxy":"none","_mqtt_version":"5","_abbrev_timezone":"UTC","_aclk_available":"true",` +
		`"_os":"linux","_timezone":"Etc/UTC"},"functions":{}}`
	if d := diffs(parse("tail", o), parse("tail", c), infoTailFamily.masks, infoTailFamily.unordered...); d != nil {
		t.Errorf("infoTailFamily: the recorded label orders differ: %q", d)
	}
	if l := v2Layouts([2][]byte{[]byte(o), []byte(c)}, true); l != "" {
		t.Errorf("infoTailFamily: the recorded label orders' layouts: %s", l)
	}
	if d := diffs(parse("tail", o), parse("tail", c), infoTailFamily.masks); d == nil {
		t.Errorf("infoTailFamily: without the unordered labels the orders compare equal")
	}
}
