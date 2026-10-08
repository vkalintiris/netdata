// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The node APIs of the dashboard (milestone 10): `/api/v2|v3/nodes` and `/api/v2|v3/versions` (check
// `api.v2-nodes`) and `/api/v2|v3/node_instances` (check `api.v2-node-instances`). C answers each with its one v2
// walk (rrdcontext_to_json_v2, database/contexts/api_v2_contexts.c:1297-1564) in the route's mode: nodes and their info
// (api_v2_nodes.c:6), the version hashes alone (api_v2_versions.c:6), or nodes with their instances, the version
// hashes and the agent's info (api_v2_node_instances.c:6-9). The fixture is dashPair's parent and dashChild's child.

// dashNode are the facts of item i of an answer's `nodes`: the host's machine GUID, hostname and index `ni`
// (jsonwrap-v2.c:20-30). C numbers the hosts it keeps in the order of its host index (api_v2_contexts.c:702-708,
// query_scope.c:20), which is insertion order (rrdhost.c:121-125, no DICT_OPTION_ADD_IN_FRONT): localhost first,
// then the child once it connected.
func dashNode(i, ni int, guid, host string) []dashFact {
	at := "[" + strconv.Itoa(i) + "]"
	return []dashFact{
		dashIs(strconv.Quote(guid), "nodes", at, "mg"),
		dashIs(strconv.Quote(host), "nodes", at, "nm"),
		dashIs(strconv.Itoa(ni), "nodes", at, "ni"),
	}
}

// dashParent and dashChildNode are dashNode's facts of the fixture's two hosts.
func dashParent(i, ni int) []dashFact {
	return dashNode(i, ni, parentIdentity.MachineGUID, parentIdentity.Hostname)
}

func dashChildNode(i, ni int) []dashFact {
	return dashNode(i, ni, childHost.MachineGUID, childHost.Hostname)
}

// dashNodeState is the `state` of the host named name in the body of a `/api/v2|v3/nodes` answer: `reachable` for a
// host that is online, `stale` for any other (database/contexts/api_v2_contexts.c:512).
func dashNodeState(body []byte, name string) (string, error) {
	v, err := ParseJSON(body)
	if err != nil {
		return "", err
	}
	nodes, _ := dashMember(v, "nodes")
	for _, node := range nodes.Items {
		if nm, err := dashMember(node, "nm"); err != nil || nm.Kind != KindString || nm.Text != name {
			continue
		}
		state, err := dashMember(node, "state")
		if err != nil || state.Kind != KindString {
			return "", fmt.Errorf("node %s has no state", name)
		}
		return state.Text, nil
	}
	return "", fmt.Errorf("no node %s", name)
}

// dashGoneWait bounds dashGone's wait for one side. C clears a host's online flag as its receiver ends
// (streaming/stream-receiver.c:1466-1467): both C sides read `stale` at the first poll in H34's probe p1.
const dashGoneWait = 30 * time.Second

// dashGonePoll judges one raw answer of dashGone's poll for the host named name: what it says of the host (its
// state, or what is wrong with the answer), and whether the poll ends there: the host is `stale`, or the agent
// answered a whole status line that is not 200 (it does not serve the route: waiting changes nothing). An answer
// without a status line, empty or cut, is asked again.
func dashGonePoll(answer []byte, name string) (got string, done bool) {
	if !bytes.HasPrefix(answer, []byte("HTTP/1.1 200 ")) {
		status, _, whole := bytes.Cut(answer, []byte("\r\n"))
		return "answered " + strconv.Quote(truncateBytes(answer)), whole && bytes.HasPrefix(status, []byte("HTTP/1.1 "))
	}
	state, err := dashNodeState(httpBody(answer), name)
	if err != nil {
		return err.Error(), false
	}
	return state, state == "stale"
}

// dashGone closes a child's two connections (0 the oracle's) and waits until each side's `/api/v3/nodes` lists the
// host `stale`: its receiver ended (rrdhost_is_online, rrdhost.h:462-470). The polls are tagged harness=wait (how many
// there are is each side's timing). The oracle's failure ends the case, which did not run; a candidate's is reported
// (at once when it answers the route with another status than 200) and the rows that follow show how it differs.
//
// The host's contexts are still collected then: C only flags the host there (rrdcontext.c:96-98), and its contexts
// worker takes the collected flags down on its next heartbeat of 1 s (rrdcontext-worker.c:1125-1127,
// rrdcontext-internal.h:16; a second later in the probe). A row that needs them down says so in its guard (`live`
// false) and settles on it.
func dashGone(t *testing.T, p *Pair, host stream.HostInfo, conns [2]*stream.Conn) {
	t.Helper()
	for _, conn := range conns {
		_ = conn.Close()
	}
	target := "/api/v3/nodes?scope_nodes=" + host.Hostname + "&harness=wait"
	for _, side := range p.Each() {
		got := ""
		pollUntil(dashGoneWait, func() bool {
			b, err := v2Exchange(side.Daemon.Addr, v2Req{target: target})
			if err != nil {
				got = err.Error()
				return false
			}
			done := false
			got, done = dashGonePoll(b, host.Hostname)
			return done
		})
		if got == "stale" {
			continue
		}
		if side.Role == Oracle {
			t.Fatalf("oracle: %s: %s is not stale, after %v at most: %s", target, host.Hostname, dashGoneWait, got)
		}
		t.Errorf("candidate: %s: %s is not stale, after %v at most: %s", target, host.Hostname, dashGoneWait, got)
	}
}

// The values of the capabilities that vary by node (aclk_capas.c:41-42, :49, :51, :53), as nodeCapsOf takes them:
// funcs, health and dyncfg each on and off.
const (
	capFuncsOn, capFuncsOff   = `"version":1,"enabled":true`, `"version":0,"enabled":false`
	capHealthOn, capHealthOff = `"version":2,"enabled":true`, `"version":2,"enabled":false`
	capDyncfgOn, capDyncfgOff = `"version":2,"enabled":true`, `"version":2,"enabled":false`
)

// nodeCapsOf is the capability list C gives a node (aclk_capas.c:39-55) with these funcs, health and dyncfg values
// (`"version":V,"enabled":E`), the others as the harness configures them: ML built in but off (`ml` 1/false), metric
// correlations version 1 (weights.c:16).
func nodeCapsOf(funcs, health, dyncfg string) string {
	return `[{"name":"proto","version":1,"enabled":true},{"name":"ml","version":1,"enabled":false},` +
		`{"name":"mc","version":1,"enabled":true},{"name":"ctx","version":1,"enabled":true},` +
		`{"name":"funcs",` + funcs + `},{"name":"http_api_v2","version":7,"enabled":true},` +
		`{"name":"health",` + health + `},{"name":"req_cancel","version":1,"enabled":true},` +
		`{"name":"dyncfg",` + dyncfg + `}]`
}

// nodeCaps is a node's capability list with health off, the harness's default (nodeCapsOf). Functions and dyncfg are
// localhost's own (aclk_capas.c:41-42); a child has them only when it streams functions, which the fixture child does
// not negotiate (stream.CapsLive).
func nodeCaps(local bool) string {
	if local {
		return nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOn)
	}
	return nodeCapsOf(capFuncsOff, capHealthOff, capDyncfgOff)
}

// nodesCapabilities are the capability lists of an answer's nodes and of their instances, in order, each with the
// path that names it.
func nodesCapabilities(v Value) (where []string, lists []Value) {
	nodes, _ := dashMember(v, "nodes")
	for i, node := range nodes.Items {
		if c, err := dashMember(node, "capabilities"); err == nil {
			where, lists = append(where, fmt.Sprintf("nodes[%d]", i)), append(lists, c)
		}
		instances, _ := dashMember(node, "instances")
		for j, instance := range instances.Items {
			if c, err := dashMember(instance, "capabilities"); err == nil {
				where, lists = append(where, fmt.Sprintf("nodes[%d].instances[%d]", i, j)), append(lists, c)
			}
		}
	}
	return where, lists
}

// compareNodesCapabilities is a v2Family check: each node's (and instance's) capabilities by name, as the agents'
// are (compareCapabilities: the Rust agent's known differences, capabilityDiffs, apply when the candidate is
// another binary). The families mask them for this.
func compareNodesCapabilities(t *testing.T, name string, o, c Value) {
	t.Helper()
	where, oLists := nodesCapabilities(o)
	cWhere, cLists := nodesCapabilities(c)
	if strings.Join(where, " ") != strings.Join(cWhere, " ") {
		t.Errorf("%s: capability lists differ\noracle:    %v\ncandidate: %v", name, where, cWhere)
		return
	}
	for i := range where {
		compareCapabilities(t, name+" "+where[i], oLists[i], cLists[i])
	}
}

// nodeCapsMasks hide each capability's values in the lists under prefix (a node's, an instance's), which are compared
// by name (compareNodesCapabilities); the names, and each entry's members and their order, stay compared (C writes
// name, version, enabled: database/contexts/api_v2_contexts.c:437-452).
func nodeCapsMasks(prefix string) []Mask {
	return []Mask{
		{prefix + ".capabilities.[].version", "compared by name"},
		{prefix + ".capabilities.[].enabled", "compared by name"},
	}
}

var (
	// nodesFamily compares `/api/v2|v3/nodes`: the request's durations masked (infoV2Volatile), the host labels a set
	// (C walks them in the order of their entries' heap addresses, rrdlabels.c:36-44, keyed by the label's pointer at
	// :267: in C-against-C run nd-h31s3-probe1 localhost's 37 labels began `_aclk_available _is_k8s_node` on one side
	// and `_aclk_proxy _mqtt_version` on the other; the precedent is rvNodesRules; a label map is flat, so the layout
	// is compared whole), each node's capabilities by name (compareNodesCapabilities, nodeCapsMasks).
	nodesFamily = v2Family{
		masks:     slices.Concat(infoV2Volatile, nodeCapsMasks("nodes.[]")),
		unordered: []string{"nodes.[].labels"},
		flat:      true,
		settle:    dashSettle,
		check:     compareNodesCapabilities,
	}

	// versionsFamily compares `/api/v2|v3/versions`: the hashes, and the request's duration masked.
	versionsFamily = v2Family{masks: infoV2Volatile, settle: dashSettle}
)

// niSide is what one side's node-instance answers take from its own run, for niIngestRender: the port the agent
// listens on, the seconds it started in (from its launch on) and the seconds the fixture child's connection was
// opened in (dashLinkAs).
type niSide struct {
	listen          string
	started, opened [2]int64
}

// niStartSlack is how many seconds after its launch an agent may read its start time: C takes it at the top of main()
// (daemon/main.c:338), the launch's own second in every run seen. The slack is for a loaded box; it costs only
// niReady's wait.
const niStartSlack = 5

// niReady waits until both agents' start windows (their launch and niStartSlack seconds after it) are over, so that a
// child connected from then on connects in a later second than either agent started in: a start time and a connection
// time are then told apart by their seconds alone.
func niReady(p *Pair) {
	last := max(p.Oracle.LaunchStartedAt.Unix(), p.Candidate.LaunchStartedAt.Unix()) + niStartSlack
	time.Sleep(time.Until(time.Unix(last+1, 0)))
}

// niSides are the two sides' own values (niSide) for a pair whose fixture child's connections were opened in the
// seconds opened (0 the oracle's).
func niSides(p *Pair, opened [2][2]int64) [2]niSide {
	var sides [2]niSide
	for i, side := range p.Each() {
		_, port, _ := strings.Cut(side.Daemon.Addr, ":")
		launch := side.Daemon.LaunchStartedAt.Unix()
		sides[i] = niSide{listen: port, started: [2]int64{launch, launch + niStartSlack}, opened: opened[i]}
	}
	return sides
}

// nodeInstancesFamily compares `/api/v2|v3/node_instances` of a pair whose sides' own values are sides: the agent's
// info as `api.v2-info` does on a fresh dbengine (infoV2Volatile, infoV2Fresh: in C-against-C run nd-h29s3-probe1
// `from` was 1791312187 against 1791312190 and `retention` 5 against 2), each instance's capabilities by name
// (nodeCapsMasks), its `db.last_time` as NOW when it is a second of the agent's clock (`now`), and what C takes from
// each side's own clock and sockets written by what it is (niIngestRender): nothing of an ingestion is masked, and
// the cloud status's start and age, which the render names too, are not masked here either.
func nodeInstancesFamily(sides [2]niSide) v2Family {
	envelope := slices.DeleteFunc(slices.Clone(infoV2Volatile), func(m Mask) bool {
		return m.Pattern == "agents.[].cloud.since" || m.Pattern == "agents.[].cloud.age"
	})
	return v2Family{
		masks:  slices.Concat(envelope, infoV2Fresh, nodeCapsMasks("nodes.[].instances.[]")),
		settle: dashSettle,
		now:    []string{"last_time"},
		render: niIngestRender(sides),
		check:  compareNodesCapabilities,
	}
}

var (
	// niPortRe is an ingestion source's end and its port: `"[ip]:port"`, then `:SSL` on TLS
	// (database/contexts/api_v2_contexts.c:366-371).
	niPortRe = regexp.MustCompile(`"(local|remote)":(\s*"\[[^\]"]*\]:)([0-9]+)`)
	// niAgeRe is a start and its age, in C's order: an ingestion's (database/contexts/api_v2_contexts.c:343-344) and
	// the cloud status's (claim/cloud-status.c:74-75).
	niAgeRe = regexp.MustCompile(`"since":(\s*)([0-9]+),(\s*)"age":(\s*)([0-9]+)`)
	// niPeerRe is a port as a socket has one: a number above 0, written without a leading 0.
	niPeerRe = regexp.MustCompile(`^[1-9][0-9]*$`)
)

// niIngestRender renders what a node-instance answer takes from the side's own run (sides[i]), each value by what it
// is and only where it is that:
//   - an ingestion source's `local` port reads LISTEN when it is the port the agent listens on, and its `remote` port
//     PEER when it is a port (1 to 65535) other than that one: the child's own end, which the kernel chose
//     (socket-peers.c:22-50, database/contexts/api_v2_contexts.c:366-371); the address and any suffix are kept;
//   - a `since` reads START when it is a second the agent started in: localhost's ingestion (rrdhost-status.c:176,
//     :202: `netdata_start_time`) and the cloud status of an agent that never connected (claim/cloud-status.c:38-44);
//     it reads CONNECTED when it is a second the child's connection was opened in: a child's ingestion
//     (rrdhost-status.c:163-169, streaming/stream-receiver.c:1416). niReady keeps the two apart;
//   - the `age` after a `since` reads "NOW-SINCE" where it is the body's `now` (dashNowRe) less that start, as C
//     computes it (database/contexts/api_v2_contexts.c:344, from the walk's one `now`, :495;
//     claim/cloud-status.c:75).
//
// Any other port, start or age is left as the agent wrote it, for the comparison.
func niIngestRender(sides [2]niSide) func(i int, _ [2]int64, body []byte) []byte {
	return func(i int, _ [2]int64, body []byte) []byte {
		side := sides[i]
		body = niPortRe.ReplaceAllFunc(body, func(m []byte) []byte {
			g := niPortRe.FindSubmatch(m)
			switch end, port := string(g[1]), string(g[3]); {
			case end == "local" && port == side.listen:
				return []byte(`"local":` + string(g[2]) + "LISTEN")
			case end == "remote" && port != side.listen && niPeerRe.MatchString(port):
				if n, err := strconv.Atoi(port); err == nil && n <= 65535 {
					return []byte(`"remote":` + string(g[2]) + "PEER")
				}
			}
			return m
		})
		n, clocked := int64(0), false
		if now := dashNowRe.FindSubmatch(body); now != nil {
			x, err := strconv.ParseInt(string(now[1]), 10, 64)
			n, clocked = x, err == nil
		}
		return niAgeRe.ReplaceAllFunc(body, func(m []byte) []byte {
			g := niAgeRe.FindSubmatch(m)
			since, err1 := strconv.ParseInt(string(g[2]), 10, 64)
			age, err2 := strconv.ParseInt(string(g[5]), 10, 64)
			if err1 != nil || err2 != nil {
				return m
			}
			start, span := string(g[2]), string(g[5])
			switch {
			case since >= side.started[0] && since <= side.started[1]:
				start = `"START"`
			case since >= side.opened[0] && since <= side.opened[1]:
				start = `"CONNECTED"`
			}
			if clocked && since+age == n {
				span = `"NOW-SINCE"`
			}
			return []byte(`"since":` + string(g[1]) + start + "," + string(g[3]) + `"age":` + string(g[4]) + span)
		})
	}
}

// versionsGuard judges a versions answer: its members alone (api_v2_versions.c:6, the VERSIONS mode;
// api_v2_contexts.c:1379-1380, :1538-1539, :1545-1546) and the hashes (jsonwrap-v2.c:65-73): routing 1; the host
// index's version 2, one insert each for localhost and the child (dictionary-statistics.h:43-61); the contexts'
// versions above 0 once the child's context exists (query_scope.c:64); nothing queued for a hub on an unclaimed
// agent (contexts_soft_hash 0, rrdcontext-queues.c:42-44); no alert and no transition with health off.
var versionsGuard = dashGuard(
	[]dashFact{dashKeys("api versions timings"), dashAbove(0, "versions", "contexts_hard_hash")},
	dashMembers([]string{"versions"}, "routing_hard_hash", "1", "nodes_hard_hash", "2", "contexts_soft_hash", "0",
		"alerts_hard_hash", "0", "alerts_soft_hash", "0"),
)

// nodesChildFacts are the facts of the fixture child as item i of `/api/v2|v3/nodes`, numbered ni
// (database/contexts/api_v2_contexts.c:493-518): the version the child's User-Agent named (stream.go:138,
// stream-receiver-connection.c:458-466), no host label and no system info (it sends none), online so reachable
// (api_v2_contexts.c:512) and health off (api_v2_contexts.c:463-480), and its capabilities (nodeCaps).
func nodesChildFacts(i, ni int) []dashFact {
	at := []string{"nodes", "[" + strconv.Itoa(i) + "]"}
	return slices.Concat(dashChildNode(i, ni),
		[]dashFact{dashKeys("mg nm ni v labels hw os state health capabilities", at...)},
		dashMembers(at, "v", `"1.0"`, "labels", "{}", "state", `"reachable"`, "health", `{"status":"disabled"}`,
			"capabilities", nodeCaps(false)))
}

// nodesDebug is the request a nodes or versions answer echoes with `options=debug`, after `api`
// (database/contexts/api_v2_contexts.c:1382-1441): its modes (in the writer's order, :714-750: the routes ask
// `nodes` with `nodes-info`, or `versions` alone, and no option of their own: api_v2_nodes.c:6,
// api_v2_versions.c:6), the options, the host scope and selector as asked (null: none; these modes echo no context
// scope), and the window as the walk reads it (0 and 0: none).
func nodesDebug(modes, nodes string, after, before int64) string {
	return fmt.Sprintf(`{"mode":[%s],"options":["debug"],"scope":{"scope_nodes":null},"selectors":{"nodes":%s},`+
		`"filters":{"after":%d,"before":%d}}`, modes, nodes, after, before)
}

// nodesTimeout is the answer of a v2 walk whose time is up before a host is visited (`timeout=-1`: the walk compares
// its clock with the deadline at every host, api_v2_contexts.c:640-642, and answers 504 with this text,
// :1451-1462).
const nodesTimeout = "query timeout"

// nodesRows are check `api.v2-nodes`'s requests, the fixture child's base being base.
func nodesRows(base int64) []v2Req {
	parent := []string{"nodes", "[0]"}
	members := dashKeys("api nodes timings")
	// every host, localhost first, with its info (no agents, no versions: the NODES_INFO mode)
	both := slices.Concat([]dashFact{dashAbsent("nodes", "[2]")}, dashParent(0, 0),
		dashMembers(parent, "state", `"reachable"`, "capabilities", nodeCaps(true)), nodesChildFacts(1, 1))
	// the child alone, numbered 0
	child := slices.Concat([]dashFact{dashAbsent("nodes", "[1]")}, nodesChildFacts(0, 0))
	debug := dashKeys("api request nodes timings")
	const modes = `"nodes","nodes-info"`
	return []v2Req{
		{name: "v3-nodes", target: "/api/v3/nodes", status: "200", guard: dashGuard([]dashFact{members}, both)},
		// a host scope that names the child: it alone, numbered 0 (query_scope.c:36-48 skip the others before
		// api_v2_contexts.c:704 numbers it)
		{name: "v2-nodes-child", target: "/api/v2/nodes?scope_nodes=" + childHost.Hostname, status: "200",
			guard: dashGuard([]dashFact{dashKeys("api nodes timings"), dashAbsent("nodes", "[1]")},
				nodesChildFacts(0, 0))},
		// a context scope (web/api/v2/api_v2_contexts.c:27-30, D231): a host is kept only with a matching context
		// (api_v2_contexts.c:656-658, :663-674), so localhost, which has none with the pulse off, is dropped and the
		// child alone is numbered 0
		{name: "v2-nodes-ctx", target: "/api/v2/nodes?scope_contexts=" + qCharts.context, status: "200",
			guard: dashGuard([]dashFact{dashKeys("api nodes timings"), dashAbsent("nodes", "[1]")},
				nodesChildFacts(0, 0))},
		// a context scope that matches no context (D232): no host is kept, the child neither (the same lines), so
		// the row above holds because the scope matched q.ctx, not because the child has some context
		{name: "v2-nodes-nomatch", target: "/api/v2/nodes?scope_contexts=nomatch", status: "200",
			guard: dashGuard(nodesNone)},
		// The route's other parameters (web/api/v2/api_v2_contexts.c:23-42), as C answered them in H35's probe P2.
		// `options=debug` echoes the request: the route's two modes and no option but debug.
		{name: "v3-nodes-debug", target: "/api/v3/nodes?options=debug", status: "200", guard: dashGuard(
			[]dashFact{debug, dashIs(nodesDebug(modes, "null", 0, 0), "request")}, both)},
		// `nodes=` selects among the hosts in scope (query_scope.c:52-62; the walk skips a host it does not select,
		// api_v2_contexts.c:629-632): the child alone, numbered 0 (a host is numbered when it is kept, :702-708),
		// and the selector echoed
		{name: "v3-nodes-sel", target: "/api/v3/nodes?options=debug&nodes=" + childHost.Hostname, status: "200",
			guard: dashGuard([]dashFact{debug,
				dashIs(nodesDebug(modes, strconv.Quote(childHost.Hostname), 0, 0), "request")}, child)},
		{name: "v3-nodes-sel-none", target: "/api/v3/nodes?nodes=nomatch", status: "200", guard: dashGuard(nodesNone)},
		// `contexts=` filters no context, but a host without any context is dropped (:270, :656-674): localhost,
		// with the pulse off
		{name: "v3-nodes-ctx-miss", target: "/api/v3/nodes?contexts=nomatch", status: "200",
			guard: dashGuard([]dashFact{members}, child)},
		// a window keeps the hosts whose retention meets it (:636; rrdhost-collection.c:31-35, rrdcontext.h:722-724:
		// an online host's ends now, rrdhost.h:607-619): both hosts for the fixture's own minute; for five minutes
		// that end before the fixture's first sample, localhost alone (it has no data, so its retention begins at 0),
		// the child's data beginning after them, and the window echoed as it was asked
		{name: "v3-nodes-window", target: fmt.Sprintf("/api/v3/nodes?after=%d&before=%d", base, base+60), status: "200",
			guard: dashGuard([]dashFact{members}, both)},
		{name: "v3-nodes-window-miss", target: fmt.Sprintf("/api/v3/nodes?options=debug&after=%d&before=%d", base-600,
			base-300), status: "200", guard: dashGuard([]dashFact{debug, dashAbsent("nodes", "[1]"),
			dashIs(nodesDebug(modes, "null", base-600, base-300), "request")}, dashParent(0, 0))},
		// a cardinality limit cuts no node list (the limit is the contexts' and their lists', :1178-1263)
		{name: "v3-nodes-card1", target: "/api/v3/nodes?cardinality=1", status: "200",
			guard: dashGuard([]dashFact{members}, both)},
		// a time that is up before the first host (nodesTimeout). A timeout above 0 is no row: whether a walk of two
		// hosts takes longer than a millisecond is each side's timing (`timeout=1` answered 200 on both C sides).
		{name: "v3-nodes-timeout", target: "/api/v3/nodes?timeout=-1", status: "504", guard: dashText(nodesTimeout)},
		// a host scope with no word in it is no scope (D233: string_to_simple_pattern returns NULL for a text of
		// separators alone, simple_pattern.h:57-59, which query_scope.c:36 reads as every host): `v3-nodes`' answer
		{name: "v3-nodes-wordless", target: "/api/v3/nodes?scope_nodes=,", status: "200",
			guard: dashGuard([]dashFact{members}, both)},
	}
}

// nodesNone are the facts of a nodes answer that keeps no host.
var nodesNone = []dashFact{dashKeys("api nodes timings"), dashIs("[]", "nodes")}

// nodesHealthOn are the facts of `/api/v3/nodes` in `health.api`'s `endpoints` case (D231 F1), whose localhost runs
// healthAPIConf's three alerts on its chart, settled with ha_low WARNING and the two others CLEAR: localhost's health
// `online` (the status's RUNNING) with the five-way count C takes of its alerts at the request
// (api_v2_contexts.c:463-480; rrdhost-status.c:314-366: each alert of a collected chart by its status), and its
// capabilities with health on (aclk_capas.c:51).
var nodesHealthOn = slices.Concat(dashParent(0, 0), dashMembers([]string{"nodes", "[0]"},
	"health", `{"status":"online","alerts":{"critical":0,"warning":1,"clear":2,"undefined":0,"uninitialized":0}}`,
	"capabilities", nodeCapsOf(capFuncsOn, capHealthOn, capDyncfgOn)))

// fnStreamNodesFamily compares `/api/v3/nodes` on `fn.stream`'s two parents, whose addresses are addrs (0 the
// oracle's): nodesFamily, with each side's own address written PARENT before the comparison. A C child labels its
// host with its destination (`_streams_to`, database/rrdhost-labels.c:229-230), its own parent's address.
func fnStreamNodesFamily(addrs [2]string) v2Family {
	fam := nodesFamily
	fam.render = func(i int, _ [2]int64, body []byte) []byte {
		return bytes.ReplaceAll(body, []byte(addrs[i]), []byte("PARENT"))
	}
	return fam
}

// fnStreamNodesFacts are the facts of `/api/v3/nodes` on a parent of `fn.stream`'s `calls` topology (D231 F2), as C
// printed them (C against C, H33's probe p1): localhost, then the C child (`stream.rchild`'s identity) and its vnode,
// numbered in the order the parent made their hosts (the vnode's sender waits for the child, fnStreamCalls), each
// reachable with health off. Both negotiated functions on their own receivers (funcs 1/true,
// streaming/stream-receiver-api.c:15-20); the child lists the `config` method and the vnode does not (dyncfg 2/true
// and 2/false, daemon/dyncfg/dyncfg.c:531-536). The child names its destination, this parent, in `_streams_to`.
var fnStreamNodesFacts = slices.Concat(
	[]dashFact{dashKeys("api nodes timings"), dashAbsent("nodes", "[3]"),
		dashIs(`"PARENT"`, "nodes", "[1]", "labels", "_streams_to")},
	dashParent(0, 0), dashMembers([]string{"nodes", "[0]"}, "capabilities", nodeCaps(true)),
	dashNode(1, 1, rchildGUID, rchildHostname),
	dashMembers([]string{"nodes", "[1]"}, "state", `"reachable"`, "health", `{"status":"disabled"}`,
		"capabilities", nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOn)),
	dashNode(2, 2, rvGUID, rvName),
	dashMembers([]string{"nodes", "[2]"}, "state", `"reachable"`, "health", `{"status":"disabled"}`,
		"capabilities", nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOff)),
)

// versionsScoped judges a versions answer whose host scope keeps no host with a context: versionsGuard's members and
// hashes, but the contexts' version 0 (the sum over the hosts in scope, query_scope.c:64); the host index's version
// is the agent's, whatever the scope.
var versionsScoped = dashGuard([]dashFact{dashKeys("api versions timings")},
	dashMembers([]string{"versions"}, "routing_hard_hash", "1", "nodes_hard_hash", "2", "contexts_hard_hash", "0",
		"contexts_soft_hash", "0", "alerts_hard_hash", "0", "alerts_soft_hash", "0"))

// versionsRows are check `api.v2-nodes`'s version requests (the two routes share the callback), the fixture child's
// base being base; the parameters' rows are C's answers of H35's probe P2.
func versionsRows(base int64) []v2Req {
	return []v2Req{
		{name: "v2-versions", target: "/api/v2/versions", status: "200", guard: versionsGuard},
		{name: "v3-versions", target: "/api/v3/versions", status: "200", guard: versionsGuard},
		// `options=debug` echoes the request: the route's one mode and no option but debug (nodesDebug)
		{name: "v2-versions-debug", target: "/api/v2/versions?options=debug", status: "200", guard: dashGuard(
			[]dashFact{dashKeys("api request versions timings"), dashIs(nodesDebug(`"versions"`, "null", 0, 0), "request"),
				dashAbove(0, "versions", "contexts_hard_hash")})},
		// the hashes are those of the hosts in scope (query_scope.c:20-84): localhost alone has no context, and a
		// scope that matches no host has none either
		{name: "v2-versions-scope", target: "/api/v2/versions?scope_nodes=" + parentIdentity.Hostname, status: "200",
			guard: versionsScoped},
		{name: "v2-versions-scope-none", target: "/api/v2/versions?scope_nodes=nomatch", status: "200",
			guard: versionsScoped},
		// `nodes=` changes no hash: they are summed before it is read (api_v2_contexts.c:628-646)
		{name: "v2-versions-nodes", target: "/api/v2/versions?nodes=nomatch", status: "200", guard: versionsGuard},
		// the context scope and selector are not read in this mode (web/api/v2/api_v2_contexts.c:27-30), and neither a
		// window nor a cardinality limit changes a hash
		{name: "v2-versions-ignored", target: fmt.Sprintf("/api/v2/versions?scope_contexts=nomatch&contexts=nomatch"+
			"&cardinality=1&after=%d&before=%d", base-600, base-300), status: "200", guard: versionsGuard},
		{name: "v2-versions-timeout", target: "/api/v2/versions?timeout=-1", status: "504",
			guard: dashText(nodesTimeout)},
	}
}

// versionsMCP is C's whole answer to `/api/v3/versions?options=mcp` on an agent alone with no context: `mcp` leaves
// out the api member and the timings (api_v2_contexts.c:1379-1380, :1545-1546), so nothing in it is a clock or a
// duration; one host (nodes_hard_hash 1) and, with the pulse off, no context (contexts_hard_hash 0).
const versionsMCP = "{\n    \"versions\":{\n        \"routing_hard_hash\":1,\n        \"nodes_hard_hash\":1,\n" +
	"        \"contexts_hard_hash\":0,\n        \"contexts_soft_hash\":0,\n        \"alerts_hard_hash\":0,\n" +
	"        \"alerts_soft_hash\":0\n    }\n}\n"

// TestNodesAPI compares the nodes and versions routes (check `api.v2-nodes`, D224): the dashboard's node list
// (`/api/v3/nodes`), a host scope, a context scope that matches the child's context and one that matches none (D231,
// D232), the routes' other parameters (the request echoed with `options=debug`, a host selector, a context selector,
// a window, a cardinality limit, a time that is up) and a host scope with no word in it (D233), and both version
// routes with theirs, on a parent with the fixture child; then `access`: the ACL refusal (451) and bearer
// protection (412) of each route (web_api.c:82-89), but `/api/v3/versions`, which no ACL guards (HTTP_ACL_NOCHECK,
// web_api_v3.c:142-147; `/api/v2/versions` has the NODES ACL, web_api_v2.c:99-104). Green on Rust since milestone 10
// commit 2.
func TestNodesAPI(t *testing.T) {
	t.Run("data", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		base := dashBase()
		dashChild(t, p, base)
		for _, r := range nodesRows(base) {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, nodesFamily) })
		}
		for _, r := range versionsRows(base) {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, versionsFamily) })
		}
	})
	t.Run("access", func(t *testing.T) {
		rows := append(accessRoutes("/api/v3/nodes", "/api/v2/nodes", "/api/v2/versions"),
			accessRow{conf: accessACL.name, name: "v3-versions", request: accessGet("/api/v3/versions?options=mcp"),
				want: [2]string{"HTTP/1.1 200 OK\r\n", "\r\n\r\n" + versionsMCP}},
			accessRow{conf: accessBearer.name, name: "v3-versions", request: accessGet("/api/v3/versions"),
				want: accessAnonymous})
		accessRows(t, []accessConf{accessACL, accessBearer}, rows)
	})
}

// niChildFacts are the facts of the fixture child as item i of `/api/v2|v3/node_instances`, numbered ni, with its
// one instance (api_v2_contexts.c:524-571): the walk's status (jsonwrap-v2.c:8-18); its database queryable and live
// in the stream's memory mode (rrdhost-status.c:114-149, :385-390; dashPair's `ram`), until now; its first
// connection (`id`, :234), one hop, a child that is online (:179-185, :225-228) with the receiver's negotiated
// capabilities (stream.CapsLive); the fixture's 7 dimensions of 2 charts of 1 context (streamDataFixture) both
// stored and collected; no sender (api_v2_contexts.c:383-384), ML and health off, no function and no dyncfg
// (aclk_capas.c:41-42; rrdhost-status.c:405). What the ingestion takes from the side's own run reads as
// niIngestRender names it: connected in the seconds the fixture child's connection was opened in, from a port of the
// child's own to the port the agent listens on, both on the loopback address.
func niChildFacts(i, ni int) []dashFact {
	node := []string{"nodes", "[" + strconv.Itoa(i) + "]"}
	inst := append(slices.Clone(node), "instances", "[0]")
	db, ingest := append(slices.Clone(inst), "db"), append(slices.Clone(inst), "ingest")
	return slices.Concat(dashChildNode(i, ni),
		[]dashFact{
			dashKeys("mg nm ni instances", node...),
			dashAbsent(append(slices.Clone(node), "instances", "[1]")...),
			dashKeys("st db ingest ml health functions capabilities dyncfg", inst...),
		},
		dashMembers(inst, "st", `{"ai":0,"code":200,"msg":""}`, "ml", `{"status":"disabled","type":"disabled"}`,
			"health", `{"status":"disabled"}`, "functions", "{}", "capabilities", nodeCaps(false),
			"dyncfg", `{"status":"unavailable"}`),
		dashMembers(db, "status", `"online"`, "liveness", `"live"`, "mode", `"ram"`, "last_time", `"NOW"`,
			"metrics", "7", "instances", "2", "contexts", "1"),
		dashMembers(ingest, "id", "1", "hops", "1", "type", `"child"`, "status", `"online"`, "since", `"CONNECTED"`,
			"age", `"NOW-SINCE"`, "metrics", "7", "instances", "2", "contexts", "1"),
		[]dashFact{dashIs(`{"local":"[127.0.0.1]:LISTEN","remote":"[127.0.0.1]:PEER",`+
			`"capabilities":["VCAPS","HLABELS","CLABELS","INTERPOLATED"]}`, append(slices.Clone(ingest), "source")...)})
}

// niLocalIngest are the facts of localhost's ingestion, the instance at path inst: initializing with the pulse off
// (rrdhost-status.c:171-173), begun when the agent started (:202: a host without a connection time gets
// `netdata_start_time`), which niIngestRender names START, its age the walk's clock less that.
func niLocalIngest(inst []string) []dashFact {
	return dashMembers(append(slices.Clone(inst), "ingest"), "type", `"localhost"`, "status", `"initializing"`,
		"since", `"START"`, "age", `"NOW-SINCE"`)
}

// niAgentFacts are the facts of the agent's info in `/api/v2|v3/node_instances` (api_v2_contexts_agents.c:11-121;
// the AGENTS_INFO mode): its members, its hosts, localhost and the child it receives, and its cloud status, of an
// agent that never connected: begun when the agent started (claim/cloud-status.c:38-44, :74-75), as niIngestRender
// names it.
var niAgentFacts = slices.Concat(
	[]dashFact{dashKeys("mg nd nm now ai application cloud nodes metrics instances contexts capabilities api db_size "+
		"timings", "agents", "[0]")},
	dashMembers([]string{"agents", "[0]"}, "mg", strconv.Quote(parentIdentity.MachineGUID),
		"nodes", `{"total":2,"receiving":1,"sending":0,"archived":0}`),
	dashMembers([]string{"agents", "[0]", "cloud"}, "status", `"available"`, "since", `"START"`, "age", `"NOW-SINCE"`))

// nodeInstancesRows are check `api.v2-node-instances`'s requests.
func nodeInstancesRows() []v2Req {
	parent := []string{"nodes", "[0]", "instances", "[0]"}
	return []v2Req{
		{name: "v2-ni-child", target: "/api/v2/node_instances?scope_nodes=" + childHost.Hostname, status: "200",
			guard: dashGuard([]dashFact{dashKeys("api nodes versions agents timings"), dashAbsent("nodes", "[1]")},
				niChildFacts(0, 0), niAgentFacts)},
		// every host: localhost has no data with the pulse off, so its database and ingestion are initializing
		// (rrdhost-status.c:124-130, :171-173), in the parent's own dbengine
		{name: "v3-ni", target: "/api/v3/node_instances", status: "200", guard: dashGuard(
			[]dashFact{dashKeys("api nodes versions agents timings"), dashAbsent("nodes", "[2]")},
			dashParent(0, 0),
			dashMembers(append(slices.Clone(parent), "db"), "status", `"initializing"`, "mode", `"dbengine"`,
				"last_time", `"NOW"`, "contexts", "0"),
			niLocalIngest(parent),
			dashMembers(parent, "capabilities", nodeCaps(true)),
			niChildFacts(1, 1), niAgentFacts)},
	}
}

// TestNodeInstancesAPI compares `/api/v2|v3/node_instances` (check `api.v2-node-instances`, D224): a host scope
// naming the child, then every host, on a parent with the fixture child; then `access`: the NODES ACL's refusal
// (451) and bearer protection (412) of both routes (web_api_v2.c:91-96, web_api_v3.c:126-131). Red on Rust until
// commit 11.
func TestNodeInstancesAPI(t *testing.T) {
	t.Run("data", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		niReady(p)
		_, opened := dashChildLinkAs(t, p, dashBase(), childHost, qCharts)
		fam := nodeInstancesFamily(niSides(p, opened))
		for _, r := range nodeInstancesRows() {
			t.Run(r.name, func(t *testing.T) { compareV2(t, p, r, fam) })
		}
	})
	t.Run("access", func(t *testing.T) {
		accessRows(t, []accessConf{accessACL, accessBearer},
			accessRoutes("/api/v2/node_instances", "/api/v3/node_instances"))
	})
}
