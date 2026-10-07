// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"slices"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// The context APIs of the dashboard (milestone 10): `/api/v2|v3/contexts` (check `api.v2-contexts`) and the
// full-text search `/api/v2|v3/q` (check `api.v2-q`), both answered by C's v2 walk (rrdcontext_to_json_v2,
// database/contexts/api_v2_contexts.c:1297-1564) over the hosts a scope keeps, with nodes, agents and versions (the
// routes: web/api/v2/api_v2_contexts.c:78-83, web/api/v2/api_v2_q.c:5-11). Unless a cite names web/api/v2/, an
// `api_v2_contexts.c` below is database/contexts/'s. The fixture is dashPair's parent, which has no context with the
// pulse off, and dashChild's child, whose one context `q.ctx` holds charts q.a and q.two (streamDataFixture).

var (
	// contextsFamily compares `/api/v2|v3/contexts`: the request's durations masked (infoV2Volatile), and a collected
	// context's `last_entry` as NOW when it is a second of the request's flight (`now`; C prints the walk's clock
	// there, api_v2_contexts.c:1213).
	contextsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle, now: []string{"last_entry"}}

	// searchFamily compares `/api/v2|v3/q`: the request's durations masked (infoV2Volatile). The search rows here
	// match no label, so no aggregated label list is printed (api_v2_contexts.c:1131-1135) and none is left unordered.
	searchFamily = v2Family{masks: infoV2Volatile, settle: dashSettle}
)

// dashWalkNodes are the facts of a walk's `nodes` (CONTEXTS_V2_NODES with the AGENTS mode and without node
// instances: each with the walk's status, api_v2_contexts.c:490-491, jsonwrap-v2.c:8-18): the parent and the child
// for an answer of every host, the child alone, numbered 0, for one whose scope names it (query_scope.c:36-48). A
// host is kept without a matching context (api_v2_contexts.c:656-658: no context pattern), so the parent is listed.
func dashWalkNodes(all bool) []dashFact {
	st := `{"ai":0,"code":200,"msg":""}`
	if all {
		return slices.Concat(dashParent(0, 0), dashChildNode(1, 1),
			[]dashFact{dashIs(st, "nodes", "[0]", "st"), dashIs(st, "nodes", "[1]", "st"), dashAbsent("nodes", "[2]")})
	}
	return slices.Concat(dashChildNode(0, 0), []dashFact{dashIs(st, "nodes", "[0]", "st"), dashAbsent("nodes", "[1]")})
}

// contextsFacts are the facts of a contexts answer: its members, and `contexts` exactly the fixture's `q.ctx` with
// the route's default options (web/api/v2/api_v2_contexts.c:80-82; printed at api_v2_contexts.c:1199-1217): its
// family and units, the lowest priority of its collected charts (q.a's 1000, rrdcontext-worker.c:635-727), collected
// so its last entry is the walk's now (NOW) and live.
var contextsFacts = slices.Concat(
	[]dashFact{
		dashKeys("api nodes contexts versions agents timings"),
		dashKeys("q.ctx", "contexts"),
		dashKeys("family units priority first_entry last_entry live", "contexts", "q.ctx"),
	},
	dashMembers([]string{"contexts", "q.ctx"}, "family", `"fam"`, "units", `"units"`, "priority", "1000",
		"last_entry", `"NOW"`, "live", "true"))

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

// TestContextsV2API compares `/api/v2|v3/contexts` (check `api.v2-contexts`, D224) on a parent with the fixture
// child; then `access`: the METRICS ACL's refusal (451) and bearer protection (412) of both routes (web_api_v2.c:30-36,
// web_api_v3.c:56-62). Red on Rust until commit 3.
func TestContextsV2API(t *testing.T) {
	t.Run("data", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		dashChild(t, p, dashBase())
		for _, r := range contextsRows() {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, contextsFamily) })
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
