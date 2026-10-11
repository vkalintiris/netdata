// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The context APIs of the dashboard (milestone 10): `/api/v2|v3/contexts` (check `api.v2-contexts`) and the
// full-text search `/api/v2|v3/q` (check `api.v2-q`), both answered by C's v2 walk (rrdcontext_to_json_v2,
// database/contexts/api_v2_contexts.c:1297-1564) over the hosts a scope keeps, with nodes, agents and versions (the
// routes: web/api/v2/api_v2_contexts.c:78-83, web/api/v2/api_v2_q.c:5-11). Unless a cite names web/api/v2/, an
// `api_v2_contexts.c` below is database/contexts/'s. The fixture is dashPair's parent, which has no context with the
// pulse off, and dashChild's child, whose one context `q.ctx` holds charts q.a and q.two (streamDataFixture); the
// `merge`, `merge-gone` and `merge-keep` subtests add a second child (child2Host).

var (
	// contextsFamily compares `/api/v2|v3/contexts`: the request's durations masked (infoV2Volatile), and a collected
	// context's `last_entry` as NOW when it is a second of the agent's clock (`now`; C prints the walk's clock there,
	// api_v2_contexts.c:1213, which with a window is the second before the wall's: v2Clock).
	contextsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, now: []string{"last_entry"}}

	// contextsLabelsFamily is contextsFamily with each context's aggregated labels a set, its keys and each key's
	// values (C walks them in the order of their strings' heap addresses, rrdlabels-aggregated.c:130-173; D220 fork 9).
	contextsLabelsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, now: []string{"last_entry"},
		unordered: []string{"contexts.*.labels", "contexts.*.labels.*[]"}}

	// contextsMCPFamily is contextsFamily with the MCP texts compared as each agent wrote them, not as the strings
	// they decode to (mcpTextRender; D232). C writes a control byte, a quote and a backslash escaped, and every other
	// byte as it is, a non-ASCII character's too (buffer_json_strcat, libnetdata/buffer/buffer.h:301-374).
	contextsMCPFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, now: []string{"last_entry"},
		render: mcpTextRender}

	// searchFamily compares `/api/v2|v3/q`: the request's durations masked (infoV2Volatile). Its rows match no
	// label, so no label list is printed (api_v2_contexts.c:1131-1135), or one key of one value in each context,
	// which has no order to be each agent's: nothing is left unordered, the layout compared whole (v2Layouts); a row
	// that matches more is compared by searchLabelsFamily (searchOnePairLabels tells them apart).
	searchFamily = v2Family{masks: infoV2Volatile, settle: dashSettle}

	// searchLabelsFamily is searchFamily with each context's matched labels a set, its keys and each key's values: C
	// keeps both in arrays indexed by their strings' heap addresses and walks them in that order
	// (rrdlabels-aggregated.c:140, :156; D220 fork 9), which two C agents print differently (H37's probes). The
	// instances, the dimensions, `matched` and the cut markers stay ordered.
	searchLabelsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle,
		unordered: []string{"contexts.*.labels", "contexts.*.labels.*[]"}}

	// searchMCPFamily is searchFamily with the MCP text (`info`) compared as each agent wrote it (mcpTextRender; D232).
	searchMCPFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, render: mcpTextRender}
)

// mcpTextRe finds the MCP texts of a contexts answer, its string members `info` (the next steps,
// api_v2_contexts.c:1164, :1284, :1291) and `help` (of the categories' `__info__`, :126), each with the bytes of its
// literal between the quotes.
var mcpTextRe = regexp.MustCompile(`"(info|help)":(\s*)"((?:[^"\\]|\\.)*)"`)

// mcpTextRender writes each MCP text (mcpTextRe) as the JSON string of its own literal, every backslash and quote of
// the literal escaped once more. The string the comparison decodes is then the text as the agent wrote it, so two
// answers agree there only byte for byte: `\u2022` for C's raw bullet, or `\u0022` for its `\"`, is a difference
// (decoded, as compareV2 compares any other string, they are the same text).
func mcpTextRender(_ int, _ [2]int64, body []byte) []byte {
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
// fixture (:636, :275); the options debug (the request, :1382-1441) and mcp (no api and no timings, the nodes' MCP
// form and the next steps, :321-334, :1283-1285; the next steps compared as written, contextsMCPFamily); and a
// context scope and a host selector with no word in them (D233).
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
			base-600, base-300), status: "200", guard: dashGuard(contextsNone)}, contextsFamily},
		{v2Req{name: "debug", target: "/api/v2/contexts?scope_nodes=*&options=debug", status: "200",
			guard: dashGuard(contextsDebugFacts, dashWalkNodes(true), contextsQDefaults)}, contextsFamily},
		{v2Req{name: "mcp", target: "/api/v2/contexts?scope_nodes=*&options=mcp", status: "200",
			guard: dashGuard(contextsMCPFacts, contextsQDefaults)}, contextsMCPFamily},
		// a selector with no word in it is no pattern (D233): C's constructor returns NULL for a text of separators or
		// a lone `!` (string_to_simple_pattern, simple_pattern.h:57-59; simple_pattern.c:107-118, :141-143), which the
		// walk reads as no filter (query_scope.c:52, :115; api_v2_contexts.c:658-659), so each answers as `v2-all`
		// does: every host and the child's context (C's answers, H35's probe P2)
		{v2Req{name: "scope-wordless", target: "/api/v2/contexts?scope_nodes=*&scope_contexts=%7C", status: "200",
			guard: dashGuard(dashWalkNodes(true), contextsFacts)}, contextsFamily},
		{v2Req{name: "nodes-wordless", target: "/api/v2/contexts?scope_nodes=*&nodes=!", status: "200",
			guard: dashGuard(dashWalkNodes(true), contextsFacts)}, contextsFamily},
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
		contextsChild2Gone("both-child2", base),
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

// contextsChild2Gone is the row of the second child's own contexts once it is gone: no longer collected, their last
// entry its data's end (the fixture child's base less child2Earlier, plus 60: base), which the row settles on
// (dashGone).
func contextsChild2Gone(name string, base int64) contextsRow {
	last2 := dashSecs(base - child2Earlier + 60)
	return contextsRow{v2Req{name: name, target: "/api/v2/contexts?options=titles&scope_nodes=" + child2Host.Hostname,
		status: "200", guard: dashGuard(contextsOwn(child2Host, "q.ctx r.ctx"),
			dashMembers([]string{"contexts", "q.ctx"}, "title", `"other [x]"`, "family", `"fam2"`, "units", `"units2"`,
				"priority", "900", "first_entry", dashSecs(base-child2Earlier), "last_entry", last2, "live", "false"),
			contextsChild2R(base, last2, "false"))}, contextsFamily}
}

// contextsKeepRows are the `merge-keep` subtest's rows, once the second child is gone and the fixture child still
// streams (D238 point 2), the fixture child's base being base; C's answers are H35's probe P2's (and H34's p1's).
//
//   - `child2`, the second child's own contexts: no longer collected (contextsChild2Gone), which the row settles on.
//     It is what the merge reads as "the newcomer is not collected".
//   - `titles`, every host's. The first host's q.ctx is collected and the newcomer's is not: the one input for which
//     C's rules take their first branch and keep the first host's title, family, units and priority as they are
//     (api_v2_contexts.c:843, :862, :881, :897). The flags are ORed (:837), so q.ctx is collected: its last entry is
//     the walk's now and it is live; the retention is still merged (:909-919), so its first entry is the newcomer's,
//     the earlier one. r.ctx, the second child's alone, is not collected: its last entry is its data's end. A port
//     that always merges would print `[x] [x]`, `fam[x]` and the lower priority 900 (`merge`'s facts); one that took
//     the newcomer's flags instead of ORing them would print q.ctx not live, its last entry the fixture's end. As in
//     `merge-gone`, the row judges this merge only where `child2` agreed.
func contextsKeepRows(base int64) []contextsRow {
	return []contextsRow{
		contextsChild2Gone("child2", base),
		{v2Req{name: "titles", target: contextsTitles, status: "200", guard: dashGuard(contextsMergeNodes,
			[]dashFact{
				dashKeys("api nodes contexts versions agents timings"),
				dashKeys("q.ctx r.ctx", "contexts"),
			},
			dashMembers([]string{"contexts", "q.ctx"}, "title", `"title [x]"`, "family", `"fam"`, "units", `"units"`,
				"priority", "1000", "first_entry", dashSecs(base-child2Earlier), "last_entry", `"NOW"`, "live", "true"),
			contextsChild2R(base, dashSecs(base-child2Earlier+60), "false"))}, contextsFamily},
	}
}

// TestContextsV2API compares `/api/v2|v3/contexts` (check `api.v2-contexts`, D224) on a parent with the fixture
// child: the dashboard's calls, then the options, limits and filters beyond them (D231 F6) and the selectors with no
// word in them (D233); `merge`: on a parent with the fixture child and a second child that brings q.ctx again, over
// an earlier window, and r.ctx (D231 F6 B, D232); `merge-gone`: the same parent once the fixture child is gone, then
// once both are (D232); `merge-keep`: the same parent once the second child alone is gone (D238); then `access`: the
// METRICS ACL's refusal (451) and bearer protection (412) of both routes (web_api_v2.c:30-36, web_api_v3.c:56-62).
// Green on Rust since milestone 10 commit 3.
func TestContextsV2API(t *testing.T) {
	t.Run("data", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		dashChild(t, p, base)
		for _, r := range contextsRows() {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, contextsFamily) })
		}
		for _, r := range slices.Concat(contextsOptionRows(base), contextsCloserRows()) {
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
	t.Run("merge-keep", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		dashChild(t, p, base)
		second := dashChildAs(t, p, base-child2Earlier, child2Host, qOtherCharts, rCharts)
		dashGone(t, p, child2Host, second)
		for _, r := range contextsKeepRows(base) {
			t.Run(r.req.name, func(t *testing.T) { compareV2(t, p, r.req, r.fam) })
		}
	})
	t.Run("three", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		searchThreeFixture(t, p)
		for _, r := range contextsThreeRows() {
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

// searchFixtureLabels are the labels of the fixture's `q.ctx` when each of its two charts' three labels matched: the
// two every pushed chart carries and `k` with both charts' values (compared as a set: searchLabelsFamily).
const searchFixtureLabels = `{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}`

// searchKeys are a search answer's members (api_v2_contexts.c:1379-1380, :1470-1546).
var searchKeys = dashKeys("api nodes contexts searches versions agents timings")

// searchNoNode is the fact of an answer that lists no host.
var searchNoNode = []dashFact{dashIs("[]", "nodes")}

// searchCounts is the fact of the `searches` member (api_v2_contexts.c:1528-1535): `strings` texts tested (each id,
// name, family, title and units counts before it is tested, :167-171), `char` label keys and values that matched (a
// label counts only its matches, rrdlabels.c:1438-1447; api_v2_contexts.c:260-261), and their sum.
func searchCounts(strings, chars int) dashFact {
	return dashIs(fmt.Sprintf(`{"strings":%d,"char":%d,"total":%d}`, strings, chars, strings+chars), "searches")
}

// searchIs is the guard of a search answer: its members (searchKeys), its nodes' facts, `contexts` exactly (compact
// JSON) and its counts (searchCounts).
func searchIs(nodes []dashFact, contexts string, strings, chars int) func(Value) error {
	return dashGuard(nodes, []dashFact{searchKeys, dashIs(contexts, "contexts"), searchCounts(strings, chars)})
}

// searchContext are the facts of the context `id` of a search answer that matched labels: its members, in this
// order (space-separated), each of pairs (name, want) rendered as its want, and its labels as a set (dashSet: C's
// order of the keys and of a key's values is each agent's, searchLabelsFamily).
func searchContext(id, members, labels string, pairs ...string) []dashFact {
	path := []string{"contexts", id}
	return slices.Concat([]dashFact{dashKeys(members, path...), dashSet(labels, append(slices.Clone(path), "labels")...)},
		dashMembers(path, pairs...))
}

// searchLabelled is the guard of a search answer whose contexts matched labels: its members, its nodes' facts, the
// contexts' ids in order (space-separated), the facts of each (searchContext), and its counts.
func searchLabelled(nodes []dashFact, ids string, strings, chars int, contexts ...[]dashFact) func(Value) error {
	return dashGuard(nodes, []dashFact{searchKeys, dashKeys(ids, "contexts")}, slices.Concat(contexts...),
		[]dashFact{searchCounts(strings, chars)})
}

// searchCutRe is the last item of a label's values that C cut: how many it left out (rrdlabels-aggregated.c:157-163).
var searchCutRe = regexp.MustCompile(`^"\.\.\. \d+ values more"$`)

// searchCutFamily is searchLabelsFamily for a search whose matched labels have a key with more values than the
// per-context limit. C then prints the first of them, in the order of their strings' heap addresses, and
// `... N values more` for the rest (rrdlabels-aggregated.c:156-163): which values are printed is each agent's (two C
// agents printed `w1`, `w2` and `w5`, `w1` of the same five: H37's probes). The render writes `VALUE` for each
// value printed before the marker of a cut list of `key` that is one of the key's values (`values`, each once): how
// many are printed and the marker stay compared, and a text the key does not have, or one printed twice, stays as it
// was written. The key's list is found by its member's text: no value of the fixture holds a `]`.
func searchCutFamily(key string, values ...string) v2Family {
	list := regexp.MustCompile(`"` + regexp.QuoteMeta(key) + `":(\s*)\[([^\]]*)\]`)
	fam := searchLabelsFamily
	fam.render = func(_ int, _ [2]int64, body []byte) []byte {
		return list.ReplaceAllFunc(body, func(m []byte) []byte {
			g := list.FindSubmatch(m)
			items := jsonStringRe.FindAll(g[2], -1)
			if len(items) == 0 || !searchCutRe.Match(items[len(items)-1]) {
				return m
			}
			n, seen := 0, map[string]bool{}
			inner := jsonStringRe.ReplaceAllFunc(g[2], func(item []byte) []byte {
				n++
				text := string(item[1 : len(item)-1])
				if n == len(items) || !slices.Contains(values, text) || seen[text] {
					return item
				}
				seen[text] = true
				return []byte(`"VALUE"`)
			})
			return []byte(`"` + key + `":` + string(g[1]) + `[` + string(inner) + `]`)
		})
	}
	return fam
}

// searchEvery are the facts of the fixture's `q.ctx` when every text of it matches (`q=**`, searchDataRows): the
// three texts, the seven names of `matched` in C's order (api_v2_contexts.c:1071-1086), the charts' names, the
// dimensions' names as given (C's JSON list: alpha, b, z, inc, h of q.a, then q.two's new `a`, cut at the limit),
// and the three label keys with their values, as a set.
func searchEvery(dimensions string) []dashFact {
	return searchContext("q.ctx", "title family units matched instances dimensions labels",
		searchFixtureLabels,
		"title", `"title [x]"`, "family", `"fam"`, "units", `"units"`,
		"matched", `["id","title","units","families","instances","dimensions","labels"]`,
		"instances", `["q.q_a_name","q.two"]`, "dimensions", dimensions)
}

// searchEcho is the `request` member of a search with `options=debug` (api_v2_contexts.c:1382-1441): the route's
// modes in the writer's order (:714-750), `debug` and the route's six default options in the options table's order
// (web/api/v2/api_v2_q.c:5-11; web/api/maps/contexts_options.c), the scopes and selectors as the request's texts
// (null: not given), then the filters, `q` first (:1422-1425).
func searchEcho(scopeNodes, scopeContexts, nodes, contexts, q string, after, before int64) dashFact {
	return dashIs(fmt.Sprintf(`{"mode":["versions","agents","nodes","search"],"options":["debug","instances",`+
		`"dimensions","labels","titles","family","units"],"scope":{"scope_nodes":%s,"scope_contexts":%s},`+
		`"selectors":{"nodes":%s,"contexts":%s},"filters":{"q":%s,"after":%d,"before":%d}}`, scopeNodes, scopeContexts,
		nodes, contexts, q, after, before), "request")
}

// searchDataRows are check `api.v2-q`'s rows on the fixture child beyond the dashboard's two (D234 F9, and review
// R104's requests that only a run settles), the fixture's base being base. C's answers are H37's probes' (C
// against C). The fixture: context `q.ctx` (family `fam`, units `units`, title `title [x]`, its charts' titles
// merged); q.a named `q.q_a_name` with dimensions a (named alpha), b, z, inc, h and the label k=v1; q.two with a, b
// and k=v2; both charts carry `_collect_plugin=fixture-pusher` and `_collect_module=corpus`. A search that matches
// nothing tests 15 texts (searchFacts).
//
//   - `plain`: a word is a part of a text whatever its case (api_v2_contexts.c:1319; simple_pattern.c:150-156,
//     :209-214). `A` matches the family, q.a's id and two dimensions' ids: the names are what is stored (:227, :242),
//     and a name is not tested once its id matched (:223-224, :238-239): 13 tests. C answers `q=a` the same (probe).
//   - `label-key`, `label-value`: a label's key and value are both tested, and the pair that matched joins its
//     context's labels (rrdlabels.c:1430-1456): `k` matches both charts' key, `v2` one chart's value.
//   - `nomatch`, `nomatch-scoped`: a context nothing of which matches is not stored and does not count for its host
//     (api_v2_contexts.c:280-285). Without a context pattern and a window every host is listed all the same
//     (:656-658); with a context scope a host needs a context that counts (:663-674): none is listed.
//   - `contexts-beside`: `contexts=` beside `q` filters no context (the walk only marks what it selects,
//     query_scope.c:102-124, and the callback does not read the mark, api_v2_contexts.c:270), but it makes a host
//     need a counted context (:658): the parent, which has none, is dropped.
//   - `fields`: the context's family, units and title each matched: the three members (:1060-1067), then `matched`
//     in C's order (title, units, families; :1071-1086).
//   - `cut`, `cut4`, `cut2`: a list longer than the per-context limit prints one item fewer than the limit and the
//     count of the rest (:1097-1102, :1118-1123). The limit is 3, or `cardinality` divided by the contexts shown when
//     that is more (:1034-1040): 4 for `cardinality=4` (all four names), still 3 for `cardinality=2` (a port without
//     the floor of 3 prints `alpha` and `... 3 dimensions more`).
//   - `all`, `wordless`, `star`: no `q`, a `q` of separators, and a lone `*` are no pattern (simple_pattern.h:57-61,
//     simple_pattern.c:107-143; D233): nothing is searched and every context in scope is stored with no match.
//   - `debug`, `debug-noq`, `debug-selectors`: the request echoed (searchEcho); without a `q` its `filters.q` is
//     null; with a window the parent is dropped.
//   - `mcp`: no `api` and no `timings`, the nodes in their MCP form, and no `matched` (:1070).
//   - `minify`: the answer in one line (the route's defaults have no `minify`, web/api/v2/api_v2_q.c:5-11; the
//     comparison holds the two layouts to each other, v2Layouts).
//   - `long-keys`: with `options=long-json-keys` the nodes are written with their long key names
//     (libnetdata/json/json-keys.c:75-142), and the agent's entry keeps the short ones, which C writes as literals
//     (`mg`, `nd`, `nm`, `now`, `ai`: api_v2_contexts_agents.c:22-28).
//   - `every` (`q=**`): a pattern that matches every text (simple_pattern.c:41-69, :235): 13 tests, and each of the
//     two charts' three labels matches by its key and by its value, 12 (searchEvery); the six dimensions' names are
//     cut at the default limit of 3 (:1034-1040, :1118-1123).
//   - `every-whole` (`q=**&cardinality=6`): a limit of 6 for the one context: the six dimensions' names whole, the
//     hidden `h` among them (the search tests no flag, :217-244).
//   - `negative` (`q=!*`): a negative word never counts as a match (simple_pattern.c:296-298): nothing is kept and 15
//     texts are tested, the labels' matches none.
//   - `negative-first` (`q=!fam,*`): the first word that matches a text decides (simple_pattern.c:288-301): the
//     family is refused, everything else matches: no `family` member, `matched` without `families`.
//   - `window-every`, `window-negative`, `window-before`: with a window a host is listed only for a context that
//     counts (api_v2_contexts.c:656-658, :663-674): the parent is dropped. The child's charts are collected, so each
//     reaches the walk's clock and a window inside the fixture's data meets them all (:218, :235, :275); a window
//     before the data misses the host (:636) and nothing is tested.
//   - `comma`, `plus`, `backslash`, `backslash-plain`, `ampersand`: `q` is read from the decoded query
//     (web_client.c:2114-2126; url_decode_r, libnetdata/url/url.c:200-246). An encoded comma is a separator after
//     decoding (`alpha` matches beside `nomatch`); a plus is a space (:229), which is no separator of a web pattern
//     (simple_pattern.h:55): `title [x]` matches the title; a backslash takes the separator's meaning from the comma
//     after it (simple_pattern.c:121-136): one word `alpha,nomatch`, which matches nothing; before another byte it is
//     dropped (`al\pha` is `alpha`); an encoded `&` ends the value, since C splits the parameters after decoding
//     (the FIXME of web_client.c:2114-2117): the `q=alpha` after it is the request's `q` (the last one wins).
//   - `control`, `control-cut`: the escape of a byte that is not printable ends the decoding, and the request's text
//     with it (url.c:221-227, :245-247; the caller does not read the failure, web_client.c:2119). `nomatch%0Aalpha` is
//     `nomatch`: a newline, though a separator of a web pattern (simple_pattern.h:55), never parts two words of a
//     URL. And after `alpha%09` the host scope is not read: `alpha` is found with every host listed.
//   - `timeout`: a time that is up before the first host answers 504 (nodesTimeout).
func searchDataRows(base int64) []contextsRow {
	all, child := dashWalkNodes(true), dashWalkNodes(false)
	window := func(after, before int64) string { return fmt.Sprintf("&after=%d&before=%d", base+after, base+before) }
	const (
		alpha    = `{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}}`
		unsought = `{"q.ctx":{"matched":[]}}`
		cut      = "/api/v2/q?q=alpha|b|z|inc"
		cut3     = `{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","... 2 dimensions more"]}}`
		labels   = "matched labels"
		// the six dimensions' names under the default limit of 3: two, and the count of the other four
		dims3 = `["alpha","b","... 4 dimensions more"]`
	)
	row := func(name, target string, guard func(Value) error) contextsRow {
		return contextsRow{v2Req{name: name, target: target, status: "200", guard: guard}, searchFamily}
	}
	labelled := func(name, target string, guard func(Value) error) contextsRow {
		return contextsRow{v2Req{name: name, target: target, status: "200", guard: guard}, searchLabelsFamily}
	}
	return []contextsRow{
		row("plain", "/api/v2/q?q=A", searchIs(all, `{"q.ctx":{"family":"fam","matched":["families","instances",`+
			`"dimensions"],"instances":["q.q_a_name"],"dimensions":["alpha","a"]}}`, 13, 0)),
		labelled("label-key", "/api/v2/q?q=k", searchLabelled(all, "q.ctx", 15, 2,
			searchContext("q.ctx", labels, `{"k":["v1","v2"]}`, "matched", `["labels"]`))),
		row("label-value", "/api/v2/q?q=v2", searchLabelled(all, "q.ctx", 15, 1,
			searchContext("q.ctx", labels, `{"k":["v2"]}`, "matched", `["labels"]`))),
		row("nomatch", "/api/v2/q?q=nomatch", searchIs(all, "{}", 15, 0)),
		row("nomatch-scoped", "/api/v2/q?q=nomatch&scope_contexts=q.*", searchIs(searchNoNode, "{}", 15, 0)),
		row("contexts-beside", "/api/v2/q?q=alpha&contexts=nomatch", searchIs(child, alpha, 15, 0)),
		row("fields", "/api/v2/q?q=fam|units|title", searchIs(all, `{"q.ctx":{"title":"title [x]","family":"fam",`+
			`"units":"units","matched":["title","units","families"]}}`, 15, 0)),
		row("cut", cut, searchIs(all, cut3, 15, 0)),
		row("cut4", cut+"&cardinality=4", searchIs(all,
			`{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","z","inc"]}}`, 15, 0)),
		row("cut2", cut+"&cardinality=2", searchIs(all, cut3, 15, 0)),
		row("all", "/api/v2/q", searchIs(all, unsought, 0, 0)),
		row("wordless", "/api/v2/q?q=,", searchIs(all, unsought, 0, 0)),
		row("star", "/api/v2/q?q=*", searchIs(all, unsought, 0, 0)),
		row("debug", "/api/v2/q?q=alpha&options=debug", dashGuard(all, []dashFact{
			dashKeys("api request nodes contexts searches versions agents timings"),
			searchEcho("null", "null", "null", "null", `"alpha"`, 0, 0), dashIs(alpha, "contexts"), searchCounts(15, 0)})),
		row("debug-noq", "/api/v2/q?options=debug", dashGuard(all, []dashFact{
			dashKeys("api request nodes contexts searches versions agents timings"),
			searchEcho("null", "null", "null", "null", "null", 0, 0), dashIs(unsought, "contexts"), searchCounts(0, 0)})),
		row("debug-selectors", "/api/v2/q?q=alpha&options=debug&scope_nodes=*&nodes=*&scope_contexts=q.*&"+
			"contexts=*ctx&cardinality=7&timeout=20000&after=-600&before=0", dashGuard(child, []dashFact{
			dashKeys("api request nodes contexts searches versions agents timings"),
			searchEcho(`"*"`, `"q.*"`, `"*"`, `"*ctx"`, `"alpha"`, -600, 0), dashIs(alpha, "contexts"),
			searchCounts(15, 0)})),
		row("mcp", "/api/v2/q?q=alpha&options=mcp", dashGuard([]dashFact{
			dashKeys("nodes contexts searches versions agents"),
			dashIs("["+mcpParent+","+mcpNode(childHost, "child")+"]", "nodes"),
			dashIs(`{"q.ctx":{"dimensions":["alpha"]}}`, "contexts"), searchCounts(15, 0)})),
		row("minify", "/api/v2/q?q=alpha&options=minify", searchIs(all, alpha, 15, 0)),
		row("long-keys", "/api/v2/q?q=alpha&options=long-json-keys", dashGuard([]dashFact{searchKeys,
			dashKeys("machine_guid hostname nodes_array_index status", "nodes", "[0]"),
			dashIs(strconv.Quote(parentIdentity.Hostname), "nodes", "[0]", "hostname"),
			dashIs(strconv.Quote(childHost.MachineGUID), "nodes", "[1]", "machine_guid"),
			dashIs(strconv.Quote(childHost.Hostname), "nodes", "[1]", "hostname"),
			dashIs("1", "nodes", "[1]", "nodes_array_index"),
			dashIs(`{"agents_array_index":0,"code":200,"msg":""}`, "nodes", "[1]", "status"),
			dashAbsent("nodes", "[2]"),
			dashKeys("mg nd nm now ai timings", "agents", "[0]"),
			dashIs(alpha, "contexts"), searchCounts(15, 0)})),
		labelled("every", "/api/v2/q?q=**", searchLabelled(all, "q.ctx", 13, 12, searchEvery(dims3))),
		labelled("every-whole", "/api/v2/q?q=**&cardinality=6", searchLabelled(all, "q.ctx", 13, 12,
			searchEvery(`["alpha","b","z","inc","h","a"]`))),
		row("negative", "/api/v2/q?q=!*", searchIs(all, "{}", 15, 0)),
		labelled("negative-first", "/api/v2/q?q=!fam,*", searchLabelled(all, "q.ctx", 13, 12,
			searchContext("q.ctx", "title units matched instances dimensions labels",
				searchFixtureLabels,
				"title", `"title [x]"`, "units", `"units"`,
				"matched", `["id","title","units","instances","dimensions","labels"]`,
				"instances", `["q.q_a_name","q.two"]`, "dimensions", dims3))),
		labelled("window-every", "/api/v2/q?q=**"+window(10, 20), searchLabelled(child, "q.ctx", 13, 12,
			searchEvery(dims3))),
		row("window-negative", "/api/v2/q?q=!*"+window(10, 20), searchIs(searchNoNode, "{}", 15, 0)),
		row("window-before", "/api/v2/q?q=**"+window(-600, -300), searchIs(searchNoNode, "{}", 0, 0)),
		row("comma", "/api/v2/q?q=nomatch%2Calpha", searchIs(all, alpha, 15, 0)),
		row("plus", "/api/v2/q?q=title+%5Bx%5D", searchIs(all, `{"q.ctx":{"title":"title [x]","matched":["title"]}}`,
			15, 0)),
		row("backslash", "/api/v2/q?q=alpha%5C%2Cnomatch", searchIs(all, "{}", 15, 0)),
		row("backslash-plain", "/api/v2/q?q=al%5Cpha", searchIs(all, alpha, 15, 0)),
		row("ampersand", "/api/v2/q?q=nomatch%26q=alpha", searchIs(all, alpha, 15, 0)),
		row("control", "/api/v2/q?q=nomatch%0Aalpha", searchIs(all, "{}", 15, 0)),
		row("control-cut", "/api/v2/q?q=alpha%09nomatch&scope_nodes="+childHost.Hostname, searchIs(all, alpha, 15, 0)),
		{v2Req{name: "timeout", target: "/api/v2/q?q=a&timeout=-1", status: "504", guard: dashText(nodesTimeout)},
			searchFamily},
	}
}

// searchMergeRows are check `api.v2-q`'s rows on a parent with the fixture child and the second child
// (child2Host: `q.ctx` again, its charts titled `other a` and `other two`, family `fam2`, units `units2`, q.a named
// `q.q_a_other`, and `r.ctx`; the same dimensions and labels as the fixture's): what one context matched on two
// hosts is merged (contexts_conflict_callback, api_v2_contexts.c:830-965). C's answers are H37's probes'.
//
//   - `union`: `q_a` matches q.a's name on each child: the two names, the first host's first (:926-936). 45 tests:
//     15 a context (searchFacts), three contexts.
//   - `truncated`, `truncated-mcp`: both contexts match `fam`; after `cardinality=1` contexts the rest is counted
//     (`__truncated__`, :1047-1055), and an MCP caller gets the `info` text (:1146-1148), compared as written.
//   - `titles`: `title|other` matches each child's own title of q.ctx (`title [x]`, `other [x]`), so the context is
//     kept on both hosts and its titles are merged (:840-857; string_2way_merge): `[x] [x]`.
//   - `title-one`: `other` matches the second child's title alone; the fixture child's q.ctx matches nothing and is
//     not stored (:280-285), so nothing is merged: the second child's own title (a port that merged every host's
//     context would print `[x] [x]`).
//   - `a-or-b` (R104's request 9 as written): the families merged (`fam[x]`), the two charts' names, and the
//     dimensions' names in the order met (alpha, b, then q.two's `a`); no title, which matched on neither host.
//   - `labels`: both children's charts carry k=v1 and k=v2: each context's pairs once (the values merged as a set,
//     rrdlabels-aggregated.c:176-235); two matches a chart pair, three contexts: 6.
//   - `every-shared`: with `cardinality=8` and two contexts shown each list may hold 4 items (:1034-1040): the three
//     names of q.ctx's charts whole, the six dimensions cut after three.
func searchMergeRows() []contextsRow {
	nodes := contextsMergeNodes
	const (
		truncated = `"__truncated__":{"total_contexts":2,"returned":1,"remaining":1}}`
		k         = `{"k":["v1","v2"]}`
		every     = searchFixtureLabels
		matched   = `["id","title","units","families","instances","dimensions","labels"]`
		members   = "title family units matched instances dimensions labels"
		dims4     = `["alpha","b","z","... 3 dimensions more"]`
	)
	row := func(name, target string, guard func(Value) error) contextsRow {
		return contextsRow{v2Req{name: name, target: target, status: "200", guard: guard}, searchFamily}
	}
	return []contextsRow{
		row("union", "/api/v2/q?q=q_a", searchIs(nodes,
			`{"q.ctx":{"matched":["instances"],"instances":["q.q_a_name","q.q_a_other"]}}`, 45, 0)),
		row("truncated", "/api/v2/q?q=fam&cardinality=1", searchIs(nodes,
			`{"q.ctx":{"family":"fam[x]","matched":["families"]},`+truncated, 45, 0)),
		{v2Req{name: "truncated-mcp", target: "/api/v2/q?q=fam&cardinality=1&options=mcp", status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes contexts info searches versions agents"),
				dashIs("["+mcpParent+","+mcpNode(childHost, "child")+","+mcpNode(child2Host, "child")+"]", "nodes"),
				dashIs(`{"q.ctx":{"family":"fam[x]"},`+truncated, "contexts"),
				mcpWritten([]string{"info"},
					"Cardinality limit reached. Use cardinality_limit parameter to see more results."),
				searchCounts(45, 0)})}, searchMCPFamily},
		row("titles", "/api/v2/q?q=title|other", searchIs(nodes, `{"q.ctx":{"title":"[x] [x]","matched":["title",`+
			`"instances"],"instances":["q.q_a_other"]},"r.ctx":{"title":"r title [x]","matched":["title"]}}`, 45, 0)),
		row("title-one", "/api/v2/q?q=other", searchIs(nodes, `{"q.ctx":{"title":"other [x]","matched":["title",`+
			`"instances"],"instances":["q.q_a_other"]}}`, 45, 0)),
		row("a-or-b", "/api/v2/q?q=a|b", searchIs(nodes, `{"q.ctx":{"family":"fam[x]","matched":["families",`+
			`"instances","dimensions"],"instances":["q.q_a_name","q.q_a_other"],"dimensions":["alpha","b","a"]},`+
			`"r.ctx":{"family":"rfam","matched":["families","instances","dimensions"],"instances":["r.r_a_name"],`+
			`"dimensions":["alpha","b","a"]}}`, 39, 0)),
		{v2Req{name: "labels", target: "/api/v2/q?q=k", status: "200", guard: searchLabelled(nodes, "q.ctx r.ctx", 45, 6,
			searchContext("q.ctx", "matched labels", k, "matched", `["labels"]`),
			searchContext("r.ctx", "matched labels", k, "matched", `["labels"]`))}, searchLabelsFamily},
		{v2Req{name: "every-shared", target: "/api/v2/q?q=**&cardinality=8", status: "200",
			guard: searchLabelled(nodes, "q.ctx r.ctx", 39, 36,
				searchContext("q.ctx", members, every, "title", `"[x] [x]"`, "family", `"fam[x]"`, "units", `"units"`,
					"matched", matched, "instances", `["q.q_a_name","q.two","q.q_a_other"]`, "dimensions", dims4),
				searchContext("r.ctx", members, every, "title", `"r title [x]"`, "family", `"rfam"`, "units",
					`"runits"`, "matched", matched, "instances", `["r.r_a_name","r.two"]`, "dimensions", dims4))},
			searchLabelsFamily},
	}
}

// searchDim is one dimension of a search fixture's chart: its id, its name (empty: none), the last second of its
// data relative to the fixture's base (0: its chart's), and whether the child declares it obsolete after its data: C
// then takes its metric for no longer collected while its chart still is (rrdmetric_updated_rrddim_flags,
// rrdcontext-metric.c:318-328).
type searchDim struct {
	id, name string
	to       int64
	obsolete bool
}

// searchChart is one chart of a search fixture: its id (`type.id`), title, units, family and context, its
// dimensions, its labels (key, value), and the seconds of its data relative to the fixture's base: one sample a
// second in (base+from, base+to]; obsolete, when the child declares the chart obsolete as a whole after its data
// (its CHART line again with the option: plugins.d/pluginsd_parser.c:529-530).
type searchChart struct {
	id, title, units, family, context string
	dims                              []searchDim
	labels                            [][2]string
	from, to                          int64
	obsolete                          bool
}

// searchChildAs connects a child as `host` to each side (dashConnectAs), sends it the charts with their data, the
// same base on both sides, then sleeps 2.5 s (as dashChildAs does). It hands back the two connections.
func searchChildAs(t *testing.T, p *Pair, base int64, host stream.HostInfo, charts ...searchChart) [2]*stream.Conn {
	t.Helper()
	conns := dashConnectAs(t, p, host)
	for _, conn := range conns {
		for _, c := range charts {
			chart := func(options string) {
				conn.Linef("CHART '%s' '' '%s' '%s' '%s' '%s' line 1000 1 '%s' fixture-pusher corpus", c.id, c.title,
					c.units, c.family, c.context, options)
			}
			chart("")
			for _, d := range c.dims {
				conn.Linef("DIMENSION '%s' '%s' absolute 1 1 ''", d.id, d.name)
			}
			for _, l := range c.labels {
				conn.Linef("CLABEL '%s' '%s' 2", l[0], l[1])
			}
			conn.Linef("CLABEL_COMMIT")
			for s := c.from + 1; s <= c.to; s++ {
				conn.Linef("BEGIN2 '%s' 1 %d #", c.id, base+s)
				for _, d := range c.dims {
					if d.to == 0 || s <= d.to {
						conn.Linef("SET2 '%s' %d %d A", d.id, s, s)
					}
				}
				conn.Linef("END2")
			}
			// a dimension is declared again under its chart's line, now with the option
			if slices.ContainsFunc(c.dims, func(d searchDim) bool { return d.obsolete }) {
				chart("")
				for _, d := range c.dims {
					if d.obsolete {
						conn.Linef("DIMENSION '%s' '%s' absolute 1 1 'obsolete'", d.id, d.name)
					}
				}
			}
			if c.obsolete {
				chart("obsolete")
			}
		}
		if err := conn.Flush(); err != nil {
			t.Fatal(err)
		}
	}
	time.Sleep(2500 * time.Millisecond)
	return conns
}

// The label fixture (R104's requests 8 and 9): on the fixture child four charts of `l.ctx`, each with one value of
// the label `lk` (w1 to w4) and one of `l3` (x1, x2, x3 and x1 again); on the second child a fifth chart of `l.ctx`
// with other texts, a dimension `e` named epsilon, a fifth value of `lk`, the first of `l3`, a label of its own,
// `only2`, and a label `u` whose value has a letter that is no ASCII (`Grün`: its ü is the bytes c3 bc).
var (
	searchLabelValues = []string{"w1", "w2", "w3", "w4", "w5"}

	searchLabelCharts = func() []searchChart {
		var charts []searchChart
		for i, x := range []string{"x1", "x2", "x3", "x1"} {
			charts = append(charts, searchChart{id: fmt.Sprintf("l.c%d", i+1), title: fmt.Sprintf("ltitle c%d", i+1),
				units: "lunits", family: "lfam", context: "l.ctx", dims: []searchDim{{id: "d"}},
				labels: [][2]string{{"lk", searchLabelValues[i]}, {"l3", x}}, to: 10})
		}
		return charts
	}()

	searchLabelChart2 = searchChart{id: "l.c5", title: "mtitle c5", units: "munits", family: "mfam", context: "l.ctx",
		dims:   []searchDim{{id: "d"}, {id: "e", name: "epsilon"}},
		labels: [][2]string{{"lk", "w5"}, {"l3", "x1"}, {"only2", "z"}, {"u", "Grün"}}, to: 10}
)

// searchLabelRows are check `api.v2-q`'s rows on the label fixture (searchLabelCharts on the fixture child,
// searchLabelChart2 on the second child). C's answers are H37's probes'. A search of `l.ctx` that matches
// nothing tests 12 texts on the fixture child (the context's four, four charts' ids, four dimensions' ids) and 8 on
// the second (the context's four, the chart's id, `d`, and `e` with its name).
//
//   - `four`, `four-whole`: one host, one key with four values. The default limit of 3 prints two of them and
//     `... 2 values more` (rrdlabels-aggregated.c:156-163; which two is each agent's: searchCutFamily);
//     `cardinality=4` prints the four, as a set.
//   - `five`, `five-whole`: every host: the second child's value joins the first child's four
//     (rrdlabels_aggregated_merge, :176-235): two and `... 3 values more`; with `cardinality=5` the five.
//   - `three`: a key with as many values as the limit is printed whole (x1 is two charts' and the second child's:
//     once).
//   - `take`: the fixture child's context matches by its title and no label, the second child's by a label: the
//     kept entry takes the newcomer's labels (api_v2_contexts.c:950-952).
//   - `keep`: the other way round: the kept entry's labels stay, and the title, which matched on the second child
//     alone, is the two hosts' merged (`[x]`: the titles share no text at their ends).
//   - `every`: all of it at once: the five charts' ids cut after two (`... 3 instances more`, :1097-1102), the
//     dimensions' names (d and epsilon), then the labels (:1131-1135), `lk` cut and the other keys whole.
//   - `fold-ascii`, `fold-bytes` (the plan's question 13): a word's case is folded for ASCII letters alone (libc's
//     strcasestr in the C locale, simple_pattern.c:209-214: the daemon sets no locale). `GRüN` matches the value
//     `Grün`; `grÜn`, whose capital is other bytes (c3 9c), matches nothing.
func searchLabelRows() []contextsRow {
	child := dashWalkNodes(false)
	nodes := contextsMergeNodes
	const (
		labels  = "matched labels"
		matched = `["labels"]`
		lk      = "/api/v2/q?q=lk"
	)
	four, five := searchCutFamily("lk", searchLabelValues[:4]...), searchCutFamily("lk", searchLabelValues...)
	one := "&scope_nodes=" + childHost.Hostname
	row := func(name, target string, fam v2Family, guard func(Value) error) contextsRow {
		return contextsRow{v2Req{name: name, target: target, status: "200", guard: guard}, fam}
	}
	return []contextsRow{
		row("four", lk+one, four, searchIs(child,
			`{"l.ctx":{"matched":["labels"],"labels":{"lk":["VALUE","VALUE","... 2 values more"]}}}`, 12, 4)),
		row("four-whole", lk+"&cardinality=4"+one, searchLabelsFamily, searchLabelled(child, "l.ctx", 12, 4,
			searchContext("l.ctx", labels, `{"lk":["w1","w2","w3","w4"]}`, "matched", matched))),
		row("five", lk, five, searchIs(nodes,
			`{"l.ctx":{"matched":["labels"],"labels":{"lk":["VALUE","VALUE","... 3 values more"]}}}`, 20, 5)),
		row("five-whole", lk+"&cardinality=5", searchLabelsFamily, searchLabelled(nodes, "l.ctx", 20, 5,
			searchContext("l.ctx", labels, `{"lk":["w1","w2","w3","w4","w5"]}`, "matched", matched))),
		row("three", "/api/v2/q?q=l3", searchLabelsFamily, searchLabelled(nodes, "l.ctx", 20, 5,
			searchContext("l.ctx", labels, `{"l3":["x1","x2","x3"]}`, "matched", matched))),
		row("take", "/api/v2/q?q=ltitle|only2", searchFamily, searchLabelled(nodes, "l.ctx", 20, 1,
			searchContext("l.ctx", "title matched labels", `{"only2":["z"]}`, "title", `"[x]"`,
				"matched", `["title","labels"]`))),
		row("keep", "/api/v2/q?q=w1|mtitle", searchFamily, searchLabelled(nodes, "l.ctx", 20, 1,
			searchContext("l.ctx", "title matched labels", `{"lk":["w1"]}`, "title", `"[x]"`,
				"matched", `["title","labels"]`))),
		row("every", "/api/v2/q?q=**", five, searchLabelled(nodes, "l.ctx", 19, 44,
			searchContext("l.ctx", "title family units matched instances dimensions labels",
				`{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],`+
					`"lk":["VALUE","VALUE","... 3 values more"],"l3":["x1","x2","x3"],"only2":["z"],"u":["Grün"]}`,
				"title", `"[x]"`, "family", `"[x]fam"`, "units", `"lunits"`,
				"matched", `["id","title","units","families","instances","dimensions","labels"]`,
				"instances", `["l.c1","l.c2","... 3 instances more"]`, "dimensions", `["d","epsilon"]`))),
		row("fold-ascii", "/api/v2/q?q=GR%C3%BCN", searchFamily, searchLabelled(nodes, "l.ctx", 20, 1,
			searchContext("l.ctx", labels, `{"u":["Grün"]}`, "matched", matched))),
		row("fold-bytes", "/api/v2/q?q=gr%C3%9Cn", searchFamily, searchIs(nodes, "{}", 20, 0)),
	}
}

// The window fixture (R104's request 4): two charts of `w.ctx` on the fixture child. `w.old`'s data ends a minute
// before the base, in (base-120, base-60]; `w.a`'s is the minute after the base, (base, base+60]: its dimension p2
// only until base+20, and its dimension p3 declared obsolete after its last sample.
var searchWindowCharts = []searchChart{
	{id: "w.old", title: "wtitle old", units: "wunits", family: "wfam", context: "w.ctx",
		dims: []searchDim{{id: "o1"}}, labels: [][2]string{{"wl", "old"}}, from: -120, to: -60},
	{id: "w.a", title: "wtitle a", units: "wunits", family: "wfam", context: "w.ctx",
		dims: []searchDim{{id: "p1"}, {id: "p2", to: 20}, {id: "p3", obsolete: true}}, labels: [][2]string{{"wl", "a"}},
		to: 60},
}

// searchWindowed is the guard of `q=**` on the window fixture: the child alone, `w.ctx` with its three texts and
// what the window left of its charts: `matched` (C's JSON), then, where instances is not empty, the charts' ids, the
// dimensions' ids and the values of the label `wl` (each a JSON list), and the counts.
func searchWindowed(matched, instances, dimensions, wl string, strings, chars int) func(Value) error {
	child := dashWalkNodes(false)
	texts := []string{"title", `"wtitle [x]"`, "family", `"wfam"`, "units", `"wunits"`, "matched", matched}
	if instances == "" {
		return dashGuard(child, []dashFact{searchKeys, dashKeys("w.ctx", "contexts"),
			dashKeys("title family units matched", "contexts", "w.ctx")},
			dashMembers([]string{"contexts", "w.ctx"}, texts...), []dashFact{searchCounts(strings, chars)})
	}
	return searchLabelled(child, "w.ctx", strings, chars, searchContext("w.ctx",
		"title family units matched instances dimensions labels",
		`{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":`+wl+`}`,
		append(texts, "instances", instances, "dimensions", dimensions)...))
}

// searchWindowRows are check `api.v2-q`'s rows on the window fixture (searchWindowCharts), the fixture's base being
// base (R104's request 4): the search's own window tests, of each instance and of each metric (api_v2_contexts.c:218,
// :235), which have no slack and read a collected object's retention as reaching the walk's clock. C's answers are
// H37's probes'. `collected` is asked while the child streams; the others once it is gone (dashGone),
// when each chart and each dimension has the retention of its own data, and the rows settle on that.
//
//   - `collected`: a window after every sample, [base+61, base+70]. What is collected reaches the walk's clock and
//     is met: both charts, and o1, p1 and p2, whose last samples are older. p3 is no longer collected (obsolete): its
//     own retention ended a second before the window, and it is skipped before it is counted.
//   - `gone-late`: the same window once the child is gone: the host's own retention misses it (:636): no host, and
//     nothing is tested.
//   - `stopped`: [base+21, base+60]: `w.old` ended before it and is skipped whole, its dimension and labels with
//     it; of `w.a`, p2's last sample (base+20) is a second before the window: p1 and p3.
//   - `stopped-edge`: [base+20, base+60]: p2's last second is the window's first: it is met (no slack, both ends
//     inclusive).
//   - `old`: [base-60, base-50]: `w.old`'s last second is the window's first: it alone (`w.a` starts after it).
//   - `between`: [base-59, base-50]: no chart has data there, but the context's own retention spans it (:275): its
//     four texts are tested and match, and it is printed with no instance, no dimension and no label.
func searchWindowRows(base int64) (live, gone []contextsRow) {
	window := func(after, before int64) string {
		return fmt.Sprintf("/api/v2/q?q=**&after=%d&before=%d", base+after, base+before)
	}
	const (
		every = `["id","title","units","families","instances","dimensions","labels"]`
		texts = `["id","title","units","families"]`
	)
	// a row that answers labels, and one that answers none
	labelled := func(name, target string, guard func(Value) error) contextsRow {
		return contextsRow{v2Req{name: name, target: target, status: "200", guard: guard}, searchLabelsFamily}
	}
	row := func(name, target string, guard func(Value) error) contextsRow {
		return contextsRow{v2Req{name: name, target: target, status: "200", guard: guard}, searchFamily}
	}
	live = []contextsRow{
		labelled("collected", window(61, 70), searchWindowed(every, `["w.old","w.a"]`, `["o1","p1","p2"]`,
			`["old","a"]`, 9, 12)),
	}
	gone = []contextsRow{
		row("gone-late", window(61, 70), searchIs(searchNoNode, "{}", 0, 0)),
		labelled("stopped", window(21, 60), searchWindowed(every, `["w.a"]`, `["p1","p3"]`, `["a"]`, 7, 6)),
		labelled("stopped-edge", window(20, 60), searchWindowed(every, `["w.a"]`, `["p1","p2","p3"]`, `["a"]`, 8, 6)),
		labelled("old", window(-60, -50), searchWindowed(every, `["w.old"]`, `["o1"]`, `["old"]`, 6, 6)),
		row("between", window(-59, -50), searchWindowed(texts, "", "", "", 4, 0)),
	}
	return live, gone
}

// searchStoreWait is how long the `restart` case leaves both agents to store the child's charts, dimensions and
// labels before it stops them: the metadata writer's first job runs 6 s after it starts (as sqlite_restart_test.go
// waits). A wait too short shows at the rows' guards.
const searchStoreWait = 10 * time.Second

// searchNamesAway are the statements that write two stored dimension names of q.a away in a stopped agent's
// metadata database, z's to NULL and inc's to an empty text (each id is one row's: q.two has neither dimension).
const searchNamesAway = `UPDATE dimension SET name = NULL WHERE id = 'z'; UPDATE dimension SET name = '' WHERE id = 'inc';`

// searchRestart stops both agents, runs edit on each side's cache directory (nil: nothing), and starts both again
// on the directories they left.
func searchRestart(t *testing.T, p *Pair, edit func(cache string)) {
	t.Helper()
	for _, cache := range stopBoth(t, p) {
		if edit != nil {
			edit(cache)
		}
	}
	restartBoth(t, p)
}

// searchStoredRows are check `api.v2-q`'s rows on a parent that restarted after the fixture child left (R104's
// request 5): the child is an archived host whose instances and metrics are loaded from the metadata database
// (rrdcontext-loading.c:16-141; a metric without retention is not loaded, :19-26, so the case keeps the child's
// data in dbengine). C's answers are H37's probes'.
//
//   - `stored-labels`: an instance's labels are loaded when they are first asked for (rrdinstance_labels,
//     rrdcontext-instance.c:38-48): the two charts' `k`. (The row settles as any row does: an agent that answered
//     its first ask without them and a later one with them would pass.)
//   - `stored-name`: the instance's name is the stored one as it is (rrdcontext-loading.c:116): `q_a_name`, without
//     the chart type a live chart's name starts with (`q.q_a_name`, searchMergeRows' `union`).
func searchStoredRows() []contextsRow {
	all := dashWalkNodes(true)
	return []contextsRow{
		{v2Req{name: "stored-labels", target: "/api/v2/q?q=k", status: "200", guard: searchLabelled(all, "q.ctx", 15, 2,
			searchContext("q.ctx", "matched labels", `{"k":["v1","v2"]}`, "matched", `["labels"]`))}, searchLabelsFamily},
		{v2Req{name: "stored-name", target: "/api/v2/q?q=q_a", status: "200", guard: searchIs(all,
			`{"q.ctx":{"matched":["instances"],"instances":["q_a_name"]}}`, 15, 0)}, searchFamily},
	}
}

// searchNamelessRows are check `api.v2-q`'s rows once two stored dimension names of q.a were written away, z's to
// NULL and inc's to an empty text, and both parents restarted (R104's request 6; searchNamesAway, run by the
// metadata crate's tool on each stopped side's database: execDB). C loads either as
// no name (string_strdupz, libnetdata/string/string.c:321-326; rrdcontext-loading.c:61), which is not its id, so the
// name is tested when the id missed, and never matches. C's answers are H37's probes'.
//
//   - `null-name`, `empty-name`: the id matches (`z`, `inc`): the context is kept and `matched` says `dimensions`,
//     but the name to store is empty, which C's dictionary refuses (dictionary-item.h:430-435): no `dimensions`
//     member. 16 tests: the fixture's 15 with alpha's name, less the name of the id that matched, plus the two empty
//     names.
//   - `names-left`: every text: the four names that are left, cut after two (`... 2 dimensions more`).
func searchNamelessRows() []contextsRow {
	all := dashWalkNodes(true)
	const nameless = `{"q.ctx":{"matched":["dimensions"]}}`
	return []contextsRow{
		{v2Req{name: "null-name", target: "/api/v2/q?q=z", status: "200", guard: searchIs(all, nameless, 16, 0)},
			searchFamily},
		{v2Req{name: "empty-name", target: "/api/v2/q?q=inc", status: "200", guard: searchIs(all, nameless, 16, 0)},
			searchFamily},
		{v2Req{name: "names-left", target: "/api/v2/q?q=**", status: "200", guard: searchLabelled(all, "q.ctx", 13, 12,
			searchContext("q.ctx", "title family units matched instances dimensions labels",
				searchFixtureLabels,
				"title", `"title [x]"`, "family", `"fam"`, "units", `"units"`,
				"matched", `["id","title","units","families","instances","dimensions","labels"]`,
				"instances", `["q_a_name","q.two"]`, "dimensions", `["alpha","b","... 2 dimensions more"]`))},
			searchLabelsFamily},
	}
}

// TestSearchAPI compares the full-text search `/api/v2|v3/q` (check `api.v2-q`, D224, D234 F9) on a parent with the
// fixture child: the dashboard's calls, then the words, limits, options and windows beyond them (searchDataRows);
// `merge`: with the second child of `api.v2-contexts`' merge rows (searchMergeRows); `labels`: two children whose
// charts carry five values of one label key (searchLabelRows); `window`: a child with a chart and a dimension that
// stopped early, asked while it streams and once it is gone (searchWindowRows); `restart`: the parents restarted
// after the fixture child, stored in dbengine, left, then again with two stored dimension names written away
// (searchStoredRows, searchNamelessRows); then `access`: the METRICS ACL's refusal (451) and bearer protection (412)
// of both routes (web_api_v2.c:39-45, web_api_v3.c:66-72). Green on Rust since commit 6.
func TestSearchAPI(t *testing.T) {
	rows := func(t *testing.T, p *Pair, list []contextsRow) {
		t.Helper()
		for _, r := range list {
			t.Run(r.req.name, func(t *testing.T) { compareV2(t, p, r.req, r.fam) })
		}
	}
	t.Run("data", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		dashChild(t, p, base)
		for _, r := range searchRows() {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, searchFamily) })
		}
		rows(t, p, slices.Concat(searchDataRows(base), searchDataCloserRows()))
	})
	t.Run("merge", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		dashChild(t, p, base)
		dashChildAs(t, p, base-child2Earlier, child2Host, qOtherCharts, rCharts)
		rows(t, p, searchMergeRows())
	})
	t.Run("labels", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		searchChildAs(t, p, base, childHost, searchLabelCharts...)
		searchChildAs(t, p, base, child2Host, searchLabelChart2)
		rows(t, p, searchLabelRows())
	})
	t.Run("window", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		conns := searchChildAs(t, p, base, childHost, searchWindowCharts...)
		live, gone := searchWindowRows(base)
		rows(t, p, live)
		dashGone(t, p, childHost, conns)
		rows(t, p, gone)
	})
	t.Run("restart", func(t *testing.T) {
		p := dashPair(t, daemon.Options{StreamMemoryMode: "dbengine"})
		conns := dashChild(t, p, dashBase())
		time.Sleep(searchStoreWait)
		dashGone(t, p, childHost, conns)
		searchRestart(t, p, nil)
		rows(t, p, searchStoredRows())
		t.Run("nameless", func(t *testing.T) {
			searchRestart(t, p, func(cache string) { execDB(t, filepath.Join(cache, "netdata-meta.db"), searchNamesAway) })
			rows(t, p, searchNamelessRows())
		})
	})
	// the closer rows' cases (search_closers_test.go)
	for _, c := range searchCloserCases() {
		t.Run(c.name, func(t *testing.T) { c.run(t, rows) })
	}
	t.Run("access", func(t *testing.T) {
		accessRows(t, []accessConf{accessACL, accessBearer}, accessRoutes("/api/v2/q?q=alpha", "/api/v3/q?q=alpha"))
	})
}
