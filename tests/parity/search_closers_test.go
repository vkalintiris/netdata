// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The closer rows of `api.v2-q` and `api.v2-contexts` (BACKLOG, "From the close of M10 commit 6", B1, B3, B5, B7,
// B8): `q` sent to the contexts mode, a limit over three contexts, a cut list under `options=mcp`, a context kept by
// one host and gone from another, and a chart declared obsolete as a whole. Each case's rows are C's answers as the
// closer drafter's probes saw them, C against C.

// sCharts are the third context of the `three` fixture, on the second child beside q.ctx and r.ctx: s.ctx, with
// texts of its own (as rCharts').
var sCharts = dataCharts{typ: "s", name: "s_a_name", titleA: "s title a", titleTwo: "s title two", units: "sunits",
	family: "sfam", context: "s.ctx", priority: 1200}

// searchThreeFixture is the `merge` fixture with a third context: the fixture child (q.ctx), then the second child
// (q.ctx again with its other texts, r.ctx and s.ctx, its data a minute earlier). It hands back the fixture child's
// base and connections.
func searchThreeFixture(t *testing.T, p *Pair) (base int64, first [2]*stream.Conn) {
	t.Helper()
	base = dashBase()
	first = dashChild(t, p, base)
	dashChildAs(t, p, base-child2Earlier, child2Host, qOtherCharts, rCharts, sCharts)
	return base, first
}

// searchThreeNodes are the facts of the `three` fixture's hosts in a walk's answer (as the `merge` fixture's: the
// parent, the fixture child and the second child, each with the walk's status).
var searchThreeNodes = contextsMergeNodes

// contextsThreeTruncated is the `__truncated__` member of a walk that printed two of the three contexts
// (api_v2_contexts.c:1047-1055 for the search, :1178-1191 for the contexts): of `total_contexts`, `returned` and
// `remaining`, the two last differ, so a writer that swapped them prints {3,1,2}.
const contextsThreeTruncated = `{"total_contexts":3,"returned":2,"remaining":1}`

// contextsCloserRows are `api.v2-contexts`' closer rows on the `data` fixture (B1): `q` is read by the search mode
// alone (web/api/v2/api_v2_contexts.c:31-32, the parser; database/contexts/api_v2_contexts.c:279, the walk), so the
// contexts route lists q.ctx as `v2-all` does, though nothing of it matches `nomatch`. A port whose walk searched any
// request that carries a pattern, with a parser that read `q` on every route, would list no context.
func contextsCloserRows() []contextsRow {
	return []contextsRow{
		{v2Req{name: "q-ignored", target: "/api/v2/contexts?scope_nodes=*&q=nomatch", status: "200",
			guard: dashGuard(dashWalkNodes(true), contextsFacts)}, contextsFamily},
	}
}

// contextsThreeRows are `api.v2-contexts`' rows on the `three` fixture (the contexts block's closer of
// `__truncated__`): a limit of two contexts over three.
func contextsThreeRows() []contextsRow {
	return []contextsRow{
		{v2Req{name: "truncated2", target: "/api/v2/contexts?scope_nodes=*&cardinality=2", status: "200",
			guard: dashGuard(searchThreeNodes, []dashFact{
				dashKeys("api nodes contexts versions agents timings"),
				dashKeys("q.ctx r.ctx __truncated__", "contexts"),
				dashIs(contextsThreeTruncated, "contexts", "__truncated__"),
			})}, contextsFamily},
	}
}

// searchDataCloserRows are `api.v2-q`'s closer rows on the `data` fixture (B5): `q=**` under `options=mcp`, whose
// lists are cut as without it (api_v2_contexts.c:1090-1129, the per-context limit of 3): q.ctx's texts and lists with
// no `matched` (:1070), the dimensions cut after two, the labels whole (two values of `k`), compared as a set
// (searchLabelsFamily); no `info` (no context limit, :1146-1148), no `api` and no `timings`; the nodes in their MCP
// form; 13 texts tested and 12 label matches, as `every`.
func searchDataCloserRows() []contextsRow {
	return []contextsRow{
		{v2Req{name: "every-mcp", target: "/api/v2/q?q=**&options=mcp", status: "200", guard: dashGuard(
			[]dashFact{
				dashKeys("nodes contexts searches versions agents"),
				dashIs("["+mcpParent+","+mcpNode(childHost, "child")+"]", "nodes"),
				dashKeys("q.ctx", "contexts"),
				searchCounts(13, 12),
			},
			searchContext("q.ctx", "title family units instances dimensions labels", searchFixtureLabels,
				"title", `"title [x]"`, "family", `"fam"`, "units", `"units"`, "instances", `["q.q_a_name","q.two"]`,
				"dimensions", `["alpha","b","... 4 dimensions more"]`))}, searchLabelsFamily},
	}
}

// searchThreeRows are `api.v2-q`'s rows on the `three` fixture while both children stream (B3): `fam` matches the
// family of each of the three contexts; a limit of two prints q.ctx (the two hosts' families merged, `fam[x]`) and
// r.ctx, then `__truncated__` (contextsThreeTruncated). 60 texts tested: 15 a context on a host, four.
func searchThreeRows() []contextsRow {
	return []contextsRow{
		{v2Req{name: "truncated3", target: "/api/v2/q?q=fam&cardinality=2", status: "200", guard: searchIs(
			searchThreeNodes, `{"q.ctx":{"family":"fam[x]","matched":["families"]},"r.ctx":{"family":"rfam",`+
				`"matched":["families"]},"__truncated__":`+contextsThreeTruncated+`}`, 60, 0)}, searchFamily},
	}
}

// searchFirstGoneRows are `api.v2-q`'s rows on the `three` fixture once the fixture child is gone and the second
// child streams (B7), the fixture child's base being base:
//
//   - `first-gone-child`, the fixture child's own contexts on the contexts route: q.ctx no longer collected (`live`
//     false), which the row settles on (contextsGoneRows' `child`). It is the state the next row needs, which its
//     own answer cannot show.
//   - `first-gone`: `title|other` matches the fixture child's q.ctx by its title (`title [x]`) and the second
//     child's by its own (`other [x]`) and q.a's name; C ORs the newcomer's flags into the kept entry before its
//     rules read them (api_v2_contexts.c:837), so "old not collected, new collected" is never true there and the
//     titles are merged as when both are collected (`[x] [x]`, :840-857). A port that read the flags as the
//     comments say ("keep new") would print `other [x]`. r.ctx and s.ctx match by their titles. 60 texts tested.
func searchFirstGoneRows(base int64) []contextsRow {
	q := []string{"contexts", "q.ctx"}
	return []contextsRow{
		{v2Req{name: "first-gone-child", target: "/api/v2/contexts?options=titles&scope_nodes=" + childHost.Hostname,
			status: "200", guard: dashGuard(contextsOwn(childHost, "q.ctx"),
				dashMembers(q, "title", `"title [x]"`, "family", `"fam"`, "units", `"units"`, "priority", "1000",
					"first_entry", dashSecs(base), "last_entry", dashSecs(base+60), "live", "false"))}, contextsFamily},
		{v2Req{name: "first-gone", target: "/api/v2/q?q=title|other", status: "200", guard: searchIs(searchThreeNodes,
			`{"q.ctx":{"title":"[x] [x]","matched":["title","instances"],"instances":["q.q_a_other"]},`+
				`"r.ctx":{"title":"r title [x]","matched":["title"]},"s.ctx":{"title":"s title [x]","matched":["title"]}}`,
			60, 0)}, searchFamily},
	}
}

// searchKeepRows are `api.v2-q`'s rows on the `merge` fixture once the second child alone is gone (B7, the "keep"
// branch): the fixture child's q.ctx is collected and the newcomer's is not, the one input for which C's rules keep
// the first host's title as it is (api_v2_contexts.c:843): `title [x]`, where a port that always merges prints
// `[x] [x]` (`merge/titles`). What matched is ORed all the same (:922-936): the title on the first host, q.a's name
// on the second. r.ctx, the second child's alone, matches by its title. The row settles until the second child's
// contexts are no longer collected (dashGone). 45 texts tested.
func searchKeepRows() []contextsRow {
	return []contextsRow{
		{v2Req{name: "second-gone", target: "/api/v2/q?q=title|other", status: "200", guard: searchIs(contextsMergeNodes,
			`{"q.ctx":{"title":"title [x]","matched":["title","instances"],"instances":["q.q_a_other"]},`+
				`"r.ctx":{"title":"r title [x]","matched":["title"]}}`, 45, 0)}, searchFamily},
	}
}

// searchObsoleteCharts are the `obsolete` fixture (B8; H37's probe p1's o.ctx): two charts of `o.ctx` on the fixture
// child, each with data in (base, base+60]: o.live, whose dimension d2 is declared obsolete after its data, and
// o.obs, declared obsolete as a whole after its data (its CHART line again with the option).
var searchObsoleteCharts = []searchChart{
	{id: "o.live", title: "otitle live", units: "ounits", family: "ofam", context: "o.ctx",
		dims: []searchDim{{id: "d1"}, {id: "d2", obsolete: true}}, labels: [][2]string{{"ol", "live"}}, to: 60},
	{id: "o.obs", title: "otitle obs", units: "ounits", family: "ofam", context: "o.ctx",
		dims: []searchDim{{id: "e1"}}, labels: [][2]string{{"ol", "obs"}}, to: 60, obsolete: true},
}

// searchObsoleteRows are `api.v2-q`'s rows on the `obsolete` fixture while the child streams, its base being base
// (B8): `q=**` over a window after every sample, [base+61, base+70]. A chart declared obsolete stays collected while
// a dimension of it is (the contexts worker derives an instance's collected flag from its metrics', and the chart's
// option archives the instance alone: rrdcontext-instance.c:518-530), so o.obs reaches the walk's clock and is met
// with its dimension e1 and its label (api_v2_contexts.c:218, :235); d2, declared obsolete itself, is no longer
// collected, its own retention ended before the window, and it is skipped. A port that took the chart's option for
// the end of its collection would list o.live alone. The child alone is listed (a window); 8 texts tested (the
// context's four, two charts' ids, d1 and e1) and 12 label matches (two charts' three labels, each by key and value).
func searchObsoleteRows(base int64) []contextsRow {
	return []contextsRow{
		{v2Req{name: "obsolete-chart", target: fmt.Sprintf("/api/v2/q?q=**&after=%d&before=%d", base+61, base+70),
			status: "200", guard: searchLabelled(dashWalkNodes(false), "o.ctx", 8, 12, searchContext("o.ctx",
				"title family units matched instances dimensions labels",
				`{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"ol":["live","obs"]}`,
				"title", `"otitle [x]"`, "family", `"ofam"`, "units", `"ounits"`,
				"matched", `["id","title","units","families","instances","dimensions","labels"]`,
				"instances", `["o.live","o.obs"]`, "dimensions", `["d1","e1"]`))}, searchLabelsFamily},
	}
}

// searchCloserCase is one of `api.v2-q`'s closer cases: a pair of its own, its fixture and its rows (ask: TestSearchAPI's
// row runner).
type searchCloserCase struct {
	name string
	run  func(t *testing.T, ask func(t *testing.T, p *Pair, rows []contextsRow))
}

// searchCloserCases are `api.v2-q`'s closer cases, after the check's own: `three` (B3, then B7 once the fixture
// child is gone), `keep` (B7's keep branch) and `obsolete` (B8).
func searchCloserCases() []searchCloserCase {
	return []searchCloserCase{
		{"three", func(t *testing.T, ask func(t *testing.T, p *Pair, rows []contextsRow)) {
			p := dashPair(t, daemon.Options{})
			base, first := searchThreeFixture(t, p)
			ask(t, p, searchThreeRows())
			dashGone(t, p, childHost, first)
			ask(t, p, searchFirstGoneRows(base))
		}},
		{"keep", func(t *testing.T, ask func(t *testing.T, p *Pair, rows []contextsRow)) {
			p := dashPair(t, daemon.Options{})
			base := dashBase()
			dashChild(t, p, base)
			second := dashChildAs(t, p, base-child2Earlier, child2Host, qOtherCharts, rCharts)
			dashGone(t, p, child2Host, second)
			ask(t, p, searchKeepRows())
		}},
		{"obsolete", func(t *testing.T, ask func(t *testing.T, p *Pair, rows []contextsRow)) {
			p := dashPair(t, daemon.Options{})
			base := dashBase()
			searchChildAs(t, p, base, childHost, searchObsoleteCharts...)
			ask(t, p, searchObsoleteRows(base))
		}},
	}
}
