// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The context APIs of the dashboard (milestone 10): `/api/v2|v3/contexts` (check `api.v2-contexts`) and the
// full-text search `/api/v2|v3/q` (check `api.v2-q`), both answered by C's v2 walk (rrdcontext_to_json_v2,
// database/contexts/api_v2_contexts.c:1297-1564) over the hosts a scope keeps, with nodes, agents and versions (the
// routes: web/api/v2/api_v2_contexts.c:78-83, web/api/v2/api_v2_q.c:5-11). Unless a cite names web/api/v2/, an
// `api_v2_contexts.c` below is database/contexts/'s. The fixture is dashPair's parent, which has no context with the
// pulse off, and dashChild's child, whose one context `q.ctx` holds charts q.a and q.two (streamDataFixture); the
// `merge` and `merge-gone` subtests add a second child (child2Host).

var (
	// contextsFamily compares `/api/v2|v3/contexts`: the request's durations masked (infoV2Volatile), and a collected
	// context's `last_entry` as NOW when it is a second of the request's flight (`now`; C prints the walk's clock
	// there, api_v2_contexts.c:1213).
	contextsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, now: []string{"last_entry"}}

	// contextsLabelsFamily is contextsFamily with each context's aggregated labels a set, its keys and each key's
	// values (C walks them in the order of their strings' heap addresses, rrdlabels-aggregated.c:130-173; D220 fork 9).
	contextsLabelsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, now: []string{"last_entry"},
		unordered: []string{"contexts.*.labels", "contexts.*.labels.*[]"}}

	// contextsWindowFamily compares `/api/v2|v3/contexts` asked with a window: contextsFamily without `now`, since
	// C's walk then takes the second before its clock (libnetdata.c:549-555, api_v2_contexts.c:1368-1374) and prints
	// it as the agent's `now` (masked, infoV2Volatile).
	contextsWindowFamily = v2Family{masks: infoV2Volatile, settle: dashSettle}

	// contextsMCPFamily is contextsFamily with the MCP texts compared as each agent wrote them, not as the strings
	// they decode to (mcpTextRender; D232). C writes a control byte, a quote and a backslash escaped, and every other
	// byte as it is, a non-ASCII character's too (buffer_json_strcat, libnetdata/buffer/buffer.h:301-374).
	contextsMCPFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, now: []string{"last_entry"},
		render: mcpTextRender}

	// searchFamily compares `/api/v2|v3/q`: the request's durations masked (infoV2Volatile). The search rows here
	// match no label, so no aggregated label list is printed (api_v2_contexts.c:1131-1135) and none is left unordered.
	searchFamily = v2Family{masks: infoV2Volatile, settle: dashSettle}
)

// mcpTextRe finds the MCP texts of a contexts answer, its string members `info` (the next steps,
// api_v2_contexts.c:1164, :1284, :1291) and `help` (of the categories' `__info__`, :126), each with the bytes of its
// literal between the quotes.
var mcpTextRe = regexp.MustCompile(`"(info|help)":(\s*)"((?:[^"\\]|\\.)*)"`)

// mcpTextRender writes each MCP text (mcpTextRe) as the JSON string of its own literal, every backslash and quote of
// the literal escaped once more. The string the comparison decodes is then the text as the agent wrote it, so two
// answers agree there only byte for byte: `\u2022` for C's raw bullet, or `\u0022` for its `\"`, is a difference
// (decoded, as compareV2 compares any other string, they are the same text).
func mcpTextRender(_ int, body []byte) []byte {
	escape := strings.NewReplacer(`\`, `\\`, `"`, `\"`)
	return mcpTextRe.ReplaceAllFunc(body, func(m []byte) []byte {
		g := mcpTextRe.FindSubmatch(m)
		return []byte(`"` + string(g[1]) + `":` + string(g[2]) + `"` + escape.Replace(string(g[3])) + `"`)
	})
}

// mcpWritten holds when the MCP text at path, left as the agent wrote it (mcpTextRender), holds each of parts byte
// for byte.
func mcpWritten(path []string, parts ...string) dashFact {
	return func(v Value) error {
		got, err := dashAt(v, path...)
		if err != nil {
			return fmt.Errorf("%v, want a text that holds %q as written", err, parts)
		}
		if got.Kind != KindString {
			return fmt.Errorf("%s is %s, want a text that holds %q as written", dashPath(path), got, parts)
		}
		for _, part := range parts {
			if !strings.Contains(got.Text, part) {
				return fmt.Errorf("%s, as written, does not hold %q: %s", dashPath(path), part, got.Text)
			}
		}
		return nil
	}
}

// mcpBullet is U+2022 as C writes it in a JSON string: its three UTF-8 bytes.
const mcpBullet = "\xe2\x80\xa2"

// dashWalkStatus are the facts that each of the first n hosts of a walk's `nodes` has the walk's status (the AGENTS
// mode without node instances: api_v2_contexts.c:490-491, jsonwrap-v2.c:8-18).
func dashWalkStatus(n int) []dashFact {
	var facts []dashFact
	for i := range n {
		facts = append(facts, dashIs(`{"ai":0,"code":200,"msg":""}`, "nodes", "["+strconv.Itoa(i)+"]", "st"))
	}
	return facts
}

// dashWalkNodes are the facts of a walk's `nodes` (CONTEXTS_V2_NODES with the AGENTS mode and without node
// instances: each with the walk's status, api_v2_contexts.c:490-491, jsonwrap-v2.c:8-18): the parent and the child
// for an answer of every host, the child alone, numbered 0, for one whose scope names it (query_scope.c:36-48). A
// host is kept without a matching context (api_v2_contexts.c:656-658: no context pattern), so the parent is listed.
func dashWalkNodes(all bool) []dashFact {
	if all {
		return slices.Concat(dashParent(0, 0), dashChildNode(1, 1), dashWalkStatus(2),
			[]dashFact{dashAbsent("nodes", "[2]")})
	}
	return slices.Concat(dashChildNode(0, 0), dashWalkStatus(1), []dashFact{dashAbsent("nodes", "[1]")})
}

// contextsQDefaults are the facts of the fixture's `q.ctx` with the route's default options
// (web/api/v2/api_v2_contexts.c:80-82; printed at api_v2_contexts.c:1199-1217): its family and units, the lowest
// priority of its collected charts (q.a's 1000, rrdcontext-worker.c:635-727), collected so its last entry is the
// walk's now (NOW) and live.
var contextsQDefaults = dashMembers([]string{"contexts", "q.ctx"}, "family", `"fam"`, "units", `"units"`,
	"priority", "1000", "last_entry", `"NOW"`, "live", "true")

// contextsFacts are the facts of a contexts answer: its members, and `contexts` exactly the fixture's `q.ctx` with
// the route's default options (contextsQDefaults).
var contextsFacts = slices.Concat(
	[]dashFact{
		dashKeys("api nodes contexts versions agents timings"),
		dashKeys("q.ctx", "contexts"),
		dashKeys("family units priority first_entry last_entry live", "contexts", "q.ctx"),
	},
	contextsQDefaults)

// contextsRows are check `api.v2-contexts`'s requests: the dashboard's (every host), a scope by hostname, and one by
// machine GUID (query_scope.c:37-39).
func contextsRows() []v2Req {
	return []v2Req{
		{name: "v2-all", target: "/api/v2/contexts?scope_nodes=*", status: "200",
			guard: dashGuard(dashWalkNodes(true), contextsFacts)},
		{name: "v2-child", target: "/api/v2/contexts?scope_nodes=" + childHost.Hostname, status: "200",
			guard: dashGuard(dashWalkNodes(false), contextsFacts)},
		{name: "v3-guid", target: "/api/v3/contexts?scope_nodes=" + childHost.MachineGUID, status: "200",
			guard: dashGuard(dashWalkNodes(false), contextsFacts)},
	}
}

// contextsRow is a contexts request with the family its answers are compared by.
type contextsRow struct {
	req v2Req
	fam v2Family
}

// contextsLists asks every host's contexts with their lists (`options` adds to the route's defaults,
// web/api/v2/api_v2_contexts.c:33-34, :80-82): the title, then the dimensions, the labels and the instances of the
// context's charts (api_v2_contexts.c:967-1020, printed at :1194-1271).
const contextsLists = "/api/v2/contexts?scope_nodes=*&options=titles,labels,instances,dimensions"

// contextsListFacts are the facts of the fixture's `q.ctx` asked with its lists (contextsLists): its defaults
// (contextsQDefaults) and title (the charts' titles merged, string.c:459-500), and the lists as C prints them under a
// cardinality limit (web/api/v2/api_v2_contexts.c:41-42): the dimensions and the instances exactly (first-seen
// order: q.a's dimensions, then q.two's new `a`; the charts' names), the labels as a set (dashSet).
func contextsListFacts(dims, labels, instances string) []dashFact {
	q := []string{"contexts", "q.ctx"}
	return slices.Concat(
		[]dashFact{
			dashKeys("api nodes contexts versions agents timings"),
			dashKeys("q.ctx", "contexts"),
			dashKeys("title family units priority first_entry last_entry live dimensions labels instances", q...),
			dashSet(labels, append(slices.Clone(q), "labels")...),
		},
		contextsQDefaults, dashMembers(q, "title", `"title [x]"`, "dimensions", dims, "instances", instances))
}

// contextsNone are the facts of a contexts answer that keeps no host and no context.
var contextsNone = []dashFact{dashKeys("api nodes contexts versions agents timings"), dashIs("[]", "nodes"),
	dashIs("{}", "contexts")}

// contextsOptionRows are check `api.v2-contexts`' rows beyond the dashboard's call (D231 F6), on the fixture whose
// data starts after base: the lists, under a cardinality limit of 1 and of 2 (a list is cut only when it holds more
// than the limit, and then after limit - 1 items: api_v2_contexts.c:1228-1234, :1257-1263,
// rrdlabels-aggregated.c:157-163); a context filter that matches nothing (it filters no context but drops a host
// without one, api_v2_contexts.c:270, :656-658, :663-674), a context scope that matches nothing, a window before the
// fixture (:636, :275); and the options debug (the request, :1382-1441) and mcp (no api and no timings, the nodes'
// MCP form and the next steps, :321-334, :1283-1285; the next steps compared as written, contextsMCPFamily).
func contextsOptionRows(base int64) []contextsRow {
	full := `{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}`
	return []contextsRow{
		{v2Req{name: "full", target: contextsLists, status: "200", guard: dashGuard(dashWalkNodes(true),
			contextsListFacts(`["alpha","b","z","inc","h","a"]`, full, `["q.q_a_name","q.two"]`))},
			contextsLabelsFamily},
		{v2Req{name: "card1", target: contextsLists + "&cardinality=1", status: "200", guard: dashGuard(
			dashWalkNodes(true), contextsListFacts(`["... 6 dimensions more"]`,
				`{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["... 2 values more"]}`,
				`["... 2 instances more"]`))}, contextsLabelsFamily},
		{v2Req{name: "card2", target: contextsLists + "&cardinality_limit=2", status: "200", guard: dashGuard(
			dashWalkNodes(true), contextsListFacts(`["alpha","... 5 dimensions more"]`, full,
				`["q.q_a_name","q.two"]`))}, contextsLabelsFamily},
		{v2Req{name: "contexts-miss", target: "/api/v2/contexts?scope_nodes=*&contexts=nomatch", status: "200",
			guard: dashGuard(dashWalkNodes(false), contextsFacts)}, contextsFamily},
		{v2Req{name: "scope-miss", target: "/api/v2/contexts?scope_nodes=*&scope_contexts=nomatch", status: "200",
			guard: dashGuard(contextsNone)}, contextsFamily},
		{v2Req{name: "window-miss", target: fmt.Sprintf("/api/v2/contexts?scope_nodes=*&after=%d&before=%d",
			base-600, base-300), status: "200", guard: dashGuard(contextsNone)}, contextsWindowFamily},
		{v2Req{name: "debug", target: "/api/v2/contexts?scope_nodes=*&options=debug", status: "200",
			guard: dashGuard(contextsDebugFacts, dashWalkNodes(true), contextsQDefaults)}, contextsFamily},
		{v2Req{name: "mcp", target: "/api/v2/contexts?scope_nodes=*&options=mcp", status: "200",
			guard: dashGuard(contextsMCPFacts, contextsQDefaults)}, contextsMCPFamily},
	}
}

// contextsDebugFacts are the facts of the fixture's contexts with `options=debug`: the request echoed after `api`
// (api_v2_contexts.c:1382-1441): the route's modes (in the writer's order, :714-750) and its default options beside
// debug (in the options table's order, web/api/maps/contexts_options.c:9-28, :51-62), and no scope but the hosts'.
var contextsDebugFacts = []dashFact{
	dashKeys("api request nodes contexts versions agents timings"),
	dashKeys("q.ctx", "contexts"),
	dashIs(`{"mode":["versions","agents","nodes","contexts"],"options":["debug","priorities","retention","liveness",`+
		`"family","units"],"scope":{"scope_nodes":"*","scope_contexts":null},"selectors":{"nodes":null,`+
		`"contexts":null},"filters":{"after":0,"before":0}}`, "request"),
}

// mcpNode is a host as an mcp answer lists it (api_v2_contexts.c:321-334): no node id on an unclaimed agent, online.
func mcpNode(host stream.HostInfo, relationship string) string {
	return `{"machine_guid":` + strconv.Quote(host.MachineGUID) + `,"hostname":` + strconv.Quote(host.Hostname) +
		`,"relationship":"` + relationship + `","connected":true}`
}

// mcpParent is the fixture's parent as an mcp answer lists it.
var mcpParent = mcpNode(stream.HostInfo{Hostname: parentIdentity.Hostname, MachineGUID: parentIdentity.MachineGUID},
	"localhost")

// contextsMCPFacts are the facts of the fixture's contexts with `options=mcp`: no `api` and no `timings`
// (api_v2_contexts.c:1379-1380, :1545-1546), the hosts in their mcp form, q.ctx, then the next steps (`info`,
// :1283-1285; web/mcp/mcp.h:36-47) as C writes them (mcpWritten; H34's probe p1): an apostrophe as it is, a newline
// as `\n`, the bullet as its UTF-8 bytes and a quote as `\"`. The whole text is compared as written
// (contextsMCPFamily).
var contextsMCPFacts = []dashFact{
	dashKeys("nodes contexts info versions agents"),
	dashIs("["+mcpParent+","+mcpNode(childHost, "child")+"]", "nodes"),
	dashKeys("q.ctx", "contexts"),
	mcpWritten([]string{"info"},
		`Next Steps: Query time-series data with the 'query_metrics' tool`,
		`views:\n   - 'group_by: dimension' will aggregate`,
		`two formats:\n      `+mcpBullet+` String format: 'labels: key1:value1|key1:value2|key2:value3'`,
		`\n      `+mcpBullet+` Structured format: 'labels: {\"key1\": [\"value1\", \"value2\"], `+
			`\"key2\": \"value3\"}' (array values are ORed`),
}

// child2Host is the second child of the `merge` and `merge-gone` subtests.
var child2Host = stream.HostInfo{Hostname: "parity-child2", MachineGUID: "5a1e0000-0000-4000-8000-0000000000cc"}

// The second child's charts: q.ctx again, with other titles, units and family, and a lower priority (q.a's 900
// against the fixture child's 1000), then a second context, r.ctx.
var (
	qOtherCharts = dataCharts{typ: "q", name: "q_a_other", titleA: "other a", titleTwo: "other two",
		units: "units2", family: "fam2", context: "q.ctx", priority: 900}
	rCharts = dataCharts{typ: "r", name: "r_a_name", titleA: "r title a", titleTwo: "r title two", units: "runits",
		family: "rfam", context: "r.ctx", priority: 1100}
)

// child2Earlier is how many seconds before the fixture child's the second child's data starts and ends (D232): its
// fixture's base is the fixture child's less this, so the two hosts' q.ctx hold different windows, (base-60, base]
// against (base, base+60]. A merged context's first entry is then the earlier host's, here the newcomer's and not the
// first host's (the minimum, api_v2_contexts.c:909-913); its last entry is the later host's, here the first host's and
// not the newcomer's (the maximum, :915-918), which C prints once no host collects the context (:1213).
const child2Earlier = 60

// dashSecs is a second as an answer prints it.
func dashSecs(t int64) string { return strconv.FormatInt(t, 10) }

// contextsMergeRows are the `merge` subtest's rows (D231 F6 B), the fixture child's base being base: every host's
// contexts with the titles, so q.ctx is the two children's merged (contexts_conflict_callback,
// api_v2_contexts.c:830-965); a limit of one context (`__truncated__`, :1178-1191); and the same limit with mcp, which
// groups the contexts by category (rrdcontext_categorize_and_output, :70-165), its texts compared as written
// (contextsMCPFamily).
func contextsMergeRows(base int64) []contextsRow {
	return []contextsRow{
		{v2Req{name: "titles", target: contextsTitles, status: "200", guard: dashGuard(contextsMergeFacts(base))},
			contextsFamily},
		{v2Req{name: "truncated", target: "/api/v2/contexts?scope_nodes=*&cardinality=1", status: "200",
			guard: dashGuard(contextsTruncatedFacts(base))}, contextsFamily},
		{v2Req{name: "categorized", target: "/api/v2/contexts?scope_nodes=*&options=mcp&cardinality=1", status: "200",
			guard: dashGuard(contextsCategorizedFacts)}, contextsMCPFamily},
	}
}

// contextsTitles asks every host's contexts with their titles.
const contextsTitles = "/api/v2/contexts?scope_nodes=*&options=titles"

// contextsMergeNodes are the facts of the two children's parent's hosts in a contexts answer: localhost, the fixture
// child and the second child, numbered in that order (the host index's), each with the walk's status.
var contextsMergeNodes = slices.Concat(dashParent(0, 0), dashChildNode(1, 1),
	dashNode(2, 2, child2Host.MachineGUID, child2Host.Hostname), []dashFact{dashAbsent("nodes", "[3]")},
	dashWalkStatus(3))

// contextsMergedQ are the facts of q.ctx merged over the two children with the default options
// (contexts_conflict_callback, api_v2_contexts.c:830-965), the fixture child's base being base. Both children's q.ctx
// are collected, so each rule takes its last branch: the families merged (string_2way_merge, string.c:459-500: "fam"
// and "fam2"), the first child's units kept (:879-893), the lower priority (:895-907), the earlier first entry, the
// second child's (:909-913, child2Earlier); collected, so its last entry is the walk's now (NOW) and live.
func contextsMergedQ(base int64) []dashFact {
	return dashMembers([]string{"contexts", "q.ctx"}, "family", `"fam[x]"`, "units", `"units"`, "priority", "900",
		"first_entry", dashSecs(base-child2Earlier), "last_entry", `"NOW"`, "live", "true")
}

// contextsChild2R are the facts of r.ctx, the second child's alone, asked with the titles: its first entry the second
// child's (child2Earlier), its last entry and liveness as given.
func contextsChild2R(base int64, last, live string) []dashFact {
	return dashMembers([]string{"contexts", "r.ctx"}, "title", `"r title [x]"`, "family", `"rfam"`, "units", `"runits"`,
		"priority", "1100", "first_entry", dashSecs(base-child2Earlier), "last_entry", last, "live", live)
}

// contextsMergeFacts, contextsTruncatedFacts and contextsCategorizedFacts are the `merge` subtest's guards: q.ctx
// merged, with the titles merged too ("title [x]" and "other [x]", each its charts' titles merged by the contexts
// worker), and r.ctx the second child's alone; one context, then `__truncated__` (api_v2_contexts.c:1178-1191);
// with mcp, the hosts in their mcp form and the two contexts grouped by their category, the id up to its only dot,
// each in full (3 samples a category at the least, :70-165), then the categories' next steps (`info`,
// :1159-1165), the texts as C writes them (mcpWritten; H34's probe p1: an apostrophe as it is, a newline as `\n`).
func contextsMergeFacts(base int64) []dashFact {
	return slices.Concat(contextsMergeNodes, contextsMergedQ(base), []dashFact{
		dashKeys("api nodes contexts versions agents timings"),
		dashKeys("q.ctx r.ctx", "contexts"),
		dashIs(`"[x] [x]"`, "contexts", "q.ctx", "title"),
	}, contextsChild2R(base, `"NOW"`, "true"))
}

func contextsTruncatedFacts(base int64) []dashFact {
	return slices.Concat(contextsMergeNodes, contextsMergedQ(base), []dashFact{
		dashKeys("api nodes contexts versions agents timings"),
		dashKeys("q.ctx __truncated__", "contexts"),
		dashIs(`{"total_contexts":2,"returned":1,"remaining":1}`, "contexts", "__truncated__"),
	})
}

var contextsCategorizedFacts = slices.Concat([]dashFact{
	dashKeys("nodes contexts info versions agents"),
	dashIs("["+mcpParent+","+mcpNode(childHost, "child")+","+mcpNode(child2Host, "child")+"]", "nodes"),
	dashKeys("__info__ q r", "contexts"),
	dashKeys("status total_contexts categories samples_per_category help", "contexts", "__info__"),
	dashIs(`["q.ctx"]`, "contexts", "q"),
	dashIs(`["r.ctx"]`, "contexts", "r"),
	mcpWritten([]string{"contexts", "__info__", "help"},
		`Use 'metrics' parameter with specific patterns like 'system.*' to get full details`),
	mcpWritten([]string{"info"},
		`grouped into categories to minimize size.\nNext Steps: repeat the 'list_metrics' call with a pattern`),
}, dashMembers([]string{"contexts", "__info__"}, "status", `"categorized"`, "total_contexts", "2",
	"categories", "2", "samples_per_category", "3"))

// contextsOwn are the facts of one child's own contexts asked with the titles (`scope_nodes=<its hostname>`): the
// child alone, numbered 0, with the walk's status, and exactly the contexts named in ids (space-separated).
func contextsOwn(host stream.HostInfo, ids string) []dashFact {
	return slices.Concat(dashNode(0, 0, host.MachineGUID, host.Hostname), dashWalkStatus(1), []dashFact{
		dashAbsent("nodes", "[1]"),
		dashKeys("api nodes contexts versions agents timings"),
		dashKeys(ids, "contexts"),
	})
}

// contextsGoneRows are the `merge-gone` subtest's rows once the fixture child is gone and the second child still
// streams (D232), the fixture child's base being base; C's answers are H34's probe p1's.
//
//   - `child`, the fixture child's own contexts: q.ctx is no longer collected (`live` false, its last entry its
//     data's end, base+60), which the row settles on (dashGone). It is what the merge reads as "old not collected".
//   - `child2`, the second child's own: q.ctx collected, with the title, the family and the units that a merge
//     letting the collected newcomer win would print.
//   - `titles`, every host's. C ORs the newcomer's flags into the kept entry before its rules read them
//     (api_v2_contexts.c:837), so "old not collected, new collected" is never true there (:846, :865, :884, :900)
//     and q.ctx is merged as when both are collected (contextsMergeFacts): the titles and the families merged, the
//     first child's units, the lower priority; and it is collected, so its last entry is the walk's now and it is
//     live. A merge that read the flags as its comments say ("keep new") would print the second child's own title,
//     family and units (`child2`'s facts); one that did not OR them at all would print it not live. The row's guard
//     is `merge`'s, since C answers the same in both states: it judges this merge only where `child` agreed, so
//     read a green `titles` beside a red `child` as not run.
func contextsGoneRows(base int64) []contextsRow {
	q, own := []string{"contexts", "q.ctx"}, "/api/v2/contexts?options=titles&scope_nodes="
	return []contextsRow{
		{v2Req{name: "child", target: own + childHost.Hostname, status: "200", guard: dashGuard(
			contextsOwn(childHost, "q.ctx"),
			dashMembers(q, "title", `"title [x]"`, "family", `"fam"`, "units", `"units"`, "priority", "1000",
				"first_entry", dashSecs(base), "last_entry", dashSecs(base+60), "live", "false"))}, contextsFamily},
		{v2Req{name: "child2", target: own + child2Host.Hostname, status: "200", guard: dashGuard(
			contextsOwn(child2Host, "q.ctx r.ctx"),
			dashMembers(q, "title", `"other [x]"`, "family", `"fam2"`, "units", `"units2"`, "priority", "900",
				"first_entry", dashSecs(base-child2Earlier), "last_entry", `"NOW"`, "live", "true"),
			contextsChild2R(base, `"NOW"`, "true"))}, contextsFamily},
		{v2Req{name: "titles", target: contextsTitles, status: "200", guard: dashGuard(contextsMergeFacts(base))},
			contextsFamily},
	}
}

// contextsBothGoneRows are the `merge-gone` subtest's rows once the second child is gone too, so that no host
// collects q.ctx (the fixture child's base being base; C's answers are H34's probe p1's).
//
//   - `both-child2`, the second child's own contexts: no longer collected, their last entry its data's end (base),
//     which the row settles on (dashGone).
//   - `both-titles`, every host's: with neither collected the rules merge as before (the last branches), and C
//     prints the merged retention's end instead of its clock (api_v2_contexts.c:1213): the later of the two hosts'
//     last entries (:915-918), the fixture child's base+60 and not the newcomer's base.
func contextsBothGoneRows(base int64) []contextsRow {
	q := []string{"contexts", "q.ctx"}
	last2 := dashSecs(base - child2Earlier + 60)
	return []contextsRow{
		{v2Req{name: "both-child2", target: "/api/v2/contexts?options=titles&scope_nodes=" + child2Host.Hostname,
			status: "200", guard: dashGuard(contextsOwn(child2Host, "q.ctx r.ctx"),
				dashMembers(q, "title", `"other [x]"`, "family", `"fam2"`, "units", `"units2"`, "priority", "900",
					"first_entry", dashSecs(base-child2Earlier), "last_entry", last2, "live", "false"),
				contextsChild2R(base, last2, "false"))}, contextsFamily},
		{v2Req{name: "both-titles", target: contextsTitles, status: "200", guard: dashGuard(contextsMergeNodes,
			[]dashFact{
				dashKeys("api nodes contexts versions agents timings"),
				dashKeys("q.ctx r.ctx", "contexts"),
			},
			dashMembers(q, "title", `"[x] [x]"`, "family", `"fam[x]"`, "units", `"units"`, "priority", "900",
				"first_entry", dashSecs(base-child2Earlier), "last_entry", dashSecs(base+60), "live", "false"),
			contextsChild2R(base, last2, "false"))}, contextsFamily},
	}
}

// TestContextsV2API compares `/api/v2|v3/contexts` (check `api.v2-contexts`, D224) on a parent with the fixture
// child: the dashboard's calls, then the options, limits and filters beyond them (D231 F6); `merge`: on a parent
// with the fixture child and a second child that brings q.ctx again, over an earlier window, and r.ctx (D231 F6 B,
// D232); `merge-gone`: the same parent once the fixture child is gone, then once both are (D232); then `access`: the
// METRICS ACL's refusal (451) and bearer protection (412) of both routes (web_api_v2.c:30-36, web_api_v3.c:56-62). Red
// on Rust until commit 3.
func TestContextsV2API(t *testing.T) {
	t.Run("data", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		dashChild(t, p, base)
		for _, r := range contextsRows() {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, contextsFamily) })
		}
		for _, r := range contextsOptionRows(base) {
			t.Run(r.req.name, func(t *testing.T) { compareV2(t, p, r.req, r.fam) })
		}
	})
	t.Run("merge", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		dashChild(t, p, base)
		dashChildAs(t, p, base-child2Earlier, child2Host, qOtherCharts, rCharts)
		for _, r := range contextsMergeRows(base) {
			t.Run(r.req.name, func(t *testing.T) { compareV2(t, p, r.req, r.fam) })
		}
	})
	t.Run("merge-gone", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		first := dashChild(t, p, base)
		second := dashChildAs(t, p, base-child2Earlier, child2Host, qOtherCharts, rCharts)
		dashGone(t, p, childHost, first)
		for _, r := range contextsGoneRows(base) {
			t.Run(r.req.name, func(t *testing.T) { compareV2(t, p, r.req, r.fam) })
		}
		dashGone(t, p, child2Host, second)
		for _, r := range contextsBothGoneRows(base) {
			t.Run(r.req.name, func(t *testing.T) { compareV2(t, p, r.req, r.fam) })
		}
	})
	t.Run("access", func(t *testing.T) {
		accessRows(t, []accessConf{accessACL, accessBearer}, accessRoutes("/api/v2/contexts", "/api/v3/contexts"))
	})
}

// searchFacts are the facts of a search for `alpha` (a case-insensitive substring, api_v2_contexts.c:1319;
// simple_pattern.c:44-48, :150-156): its members, `q.ctx` alone matched by one dimension's name (the fixture's `a`
// named `alpha`, streamDataFixture; api_v2_contexts.c:238-243, printed at :1071-1129), and the count of string
// searches (:167-171) of the child's one context: its id, family, title and units (:196-212), its charts' ids and
// q.a's name (q.two has none: 3, :222-224), its dimensions' ids and alpha's name (6 of q.a, 2 of q.two: 8,
// :238-239), 15 in all; labels count only their matches (rrdlabels.c:1430-1447), none here.
var searchFacts = []dashFact{
	dashKeys("api nodes contexts searches versions agents timings"),
	dashIs(`{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}}`, "contexts"),
	dashIs(`{"strings":15,"char":0,"total":15}`, "searches"),
}

// searchRows are check `api.v2-q`'s requests: the search scoped to the child, then over every host.
func searchRows() []v2Req {
	return []v2Req{
		{name: "v3-child", target: "/api/v3/q?q=*alpha*&scope_nodes=" + childHost.Hostname, status: "200",
			guard: dashGuard(dashWalkNodes(false), searchFacts)},
		{name: "v2-all", target: "/api/v2/q?q=*alpha*", status: "200",
			guard: dashGuard(dashWalkNodes(true), searchFacts)},
	}
}

// TestSearchAPI compares the full-text search `/api/v2|v3/q` (check `api.v2-q`, D224) on a parent with the fixture
// child; then `access`: the METRICS ACL's refusal (451) and bearer protection (412) of both routes
// (web_api_v2.c:39-45, web_api_v3.c:66-72). Red on Rust until commit 6.
func TestSearchAPI(t *testing.T) {
	t.Run("data", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		dashChild(t, p, dashBase())
		for _, r := range searchRows() {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, searchFamily) })
		}
	})
	t.Run("access", func(t *testing.T) {
		accessRows(t, []accessConf{accessACL, accessBearer}, accessRoutes("/api/v2/q?q=alpha", "/api/v3/q?q=alpha"))
	})
}
