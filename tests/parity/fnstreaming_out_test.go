// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"strconv"
	"testing"
)

// The Out cells of `netdata-streaming` (milestone 10 commit 12, D241 F3: plan 4.3 `out/connected`, `out/denied`, and
// the `never` pair's row): the admin's table on the pairs of TestNodeInstancesStream (runNIStream), whose localhost
// streams to a scripted parent, compared at each of their stages as fnStreamingCompareWith compares a table, with the
// sender's own cells besides (fnStreamingOutVolatile, fnStreamingOutFacts).

// fnStreamingOutVolatile are the columns whose numbers other than 0 differed C against C on these pairs (SA-F's probe
// p1b, two runs), besides fnStreamingVolatile's, masked as those are (cells and maxima, fnStreamingMasks) and held per
// side by fnStreamingOutFacts: the sender's own port (`OutLocalPort` has no `max`, function-netdata-streaming.c:775),
// the data and replication bytes sent on the connection (:266, :268), the parents' last attempt (a microsecond clock,
// :271-287). The counts (67, 13, 12 on every C side), the metadata bytes (12132) and the functions bytes (0) were
// equal C against C and are compared, as TestNodeInstancesStream compares them on the same pairs.
var fnStreamingOutVolatile = []string{
	"OutLocalPort", "OutTrafficData", "OutTrafficReplication", "OutAttemptSince", "OutAttemptAge",
}

// fnStreamingOutHeld are the columns fnStreamingOutFacts holds besides the sender's times: the counts, the sender's
// own port, the bytes sent by type, the parents' last attempt.
var fnStreamingOutHeld = []string{
	"dbMetrics", "dbInstances", "dbContexts", "CollectedMetrics", "CollectedInstances", "CollectedContexts",
	"OutLocalPort", "OutTrafficData", "OutTrafficMetadata", "OutTrafficReplication", "OutTrafficFunctions",
	"OutAttemptSince", "OutAttemptAge",
}

// fnStreamingOutSide is what one side's table of a stream pair is held to besides fnStreamingFacts: the stage, the
// ports its agent listens on and the scripted parent listens on, and the seconds its sender's times may hold: the
// agent's start window, the seconds by which its parents' list was made (never), its sender connected (connected,
// and denied: a refusal is no connection) and the parent refused it (denied).
type fnStreamingOutSide struct {
	stage        string
	listen, stub string
	started      [2]int64
	ready        int64
	connected    [2]int64
	closed       [2]int64
}

// fnStreamingOutSideOf is side i's fnStreamingOutSide of a stream pair's stage (niStreamRun).
func fnStreamingOutSideOf(r *niStreamRun, i int) fnStreamingOutSide {
	s := r.Sides[i]
	return fnStreamingOutSide{stage: r.Stage, listen: s.listen, stub: s.stub, started: s.started, ready: s.ready,
		connected: s.connected, closed: s.closed}
}

// fnStreamingOut compares the admin's table at a stage of TestNodeInstancesStream's pairs (runNIStream's compare):
// fnStreamingCompareWith with the stage's guard (fnStreamingOutGuard), fnStreamingOutVolatile masked and each side's
// table held to fnStreamingOutFacts.
func fnStreamingOut(t *testing.T, r *niStreamRun) {
	sides := [2]fnStreamingOutSide{fnStreamingOutSideOf(r, 0), fnStreamingOutSideOf(r, 1)}
	fnStreamingCompareWith(t, r.Pair, fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
		guard: fnStreamingOutGuard(r.Stage, sides[0].stub), volatile: fnStreamingOutVolatile,
		facts: func(i int, v Value, flight [2]int64) error { return fnStreamingOutFacts(v, sides[i]) }})
}

// fnStreamingOutGuard is the guard of the oracle's table at a stage of the stream pairs, given the scripted parent's
// port: localhost alone (its sender's host, rrdhost_status() of localhost, function-netdata-streaming.c:75-77), as
// C's probe p1b printed it (rrdhost-status.c:240-291, function-netdata-streaming.c:106-138, :229-287):
//   - never: the sender exists and was never queued: offline, NEVER CONNECTED (the reason's 0), no connection, both
//     ends `not connected` (socket-peers.c:8-15), one parent never tried (its reason's text, stream-parents.c:161-172);
//     a stream offline for another reason than NO PARENT TO SEND TO is a warning (:124-128); nothing collected
//     (the pulse is off): initializing;
//   - connected: online with the handshake's reason (the capabilities stored as the reason, stream-connector.c:240),
//     one connection (stream-sender.c:364), one hop, the parent's port, its plain socket and no compression (the
//     scripted parent drops it), the parent's reason SOCKET CONNECTED; normal; collecting (pulse): online;
//   - denied: offline with the refusal's reason DENIED, still one connection, both ends `not connected` again,
//     nothing replicating (completion 0, only a connected stream has one: rrdhost-status.c:265-281), the parent's
//     reason DENIED; a warning.
func fnStreamingOutGuard(stage, stub string) func(Value) error {
	row := fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "InReason": `"LOCALHOST"`}
	add := func(pairs ...string) {
		for i := 0; i+1 < len(pairs); i += 2 {
			row[pairs[i]] = pairs[i+1]
		}
	}
	switch stage {
	case "never":
		add("rowOptions", `{"severity":"warning"}`, "InStatus", `"initializing"`, "OutStatus", `"offline"`,
			"OutReason", `"NEVER CONNECTED"`, "OutConnections", "0", "OutLocalIP", `"not connected"`,
			"OutRemoteIP", `"not connected"`, "OutAttemptHandshake", `["NEVER CONNECTED"]`)
	case "connected":
		add("rowOptions", `{"severity":"normal"}`, "InStatus", `"online"`, "OutStatus", `"online"`,
			"OutReason", `"CONNECTED"`, "OutConnections", "1", "OutHops", "1", "OutRemotePort", stub,
			"OutSSL", `"PLAIN"`, "OutCompression", `"UNCOMPRESSED"`, "OutAttemptHandshake", `["SOCKET CONNECTED"]`)
	case "denied":
		add("rowOptions", `{"severity":"warning"}`, "InStatus", `"online"`, "OutStatus", `"offline"`,
			"OutReason", `"DENIED"`, "OutConnections", "1", "OutLocalIP", `"not connected"`,
			"OutRemoteIP", `"not connected"`, "OutReplCompletion", "0", "OutAttemptHandshake", `["DENIED"]`)
	default:
		return func(Value) error { return fmt.Errorf("harness: no stage %q of the stream pairs", stage) }
	}
	return fnStreamingRows(row)
}

// fnStreamingOutFacts holds one side's table of a stream pair to what C fixes between the cells of fnStreamingOutHeld
// (fnStreamingOutVolatile's among them, which the comparison masks), and the sender's times to their windows (side):
//   - the counts are whole numbers, each db column's `max` the largest of its cells and each Collected column's the db
//     column's (function-netdata-streaming.c:706, :712, :718);
//   - the bytes sent by type are whole numbers, each `max` the largest of its cells (:96-101, :809-835);
//   - the parents' last attempt is the milliseconds of a microsecond clock (`since_ut / 1000`) and its age the
//     seconds from its whole second to the table's clock (:271-287), both null or neither, each `max` the largest of
//     its cells; the attempt lies in the side's window: the parents' list made (never: [start, ready],
//     stream-parents.c:940), the connection (connected), the refusal (denied);
//   - OutSince is the agent's start before any connection (never: rrdhost-status.c:289-290), the connection's second
//     after it (stream-sender.c:365; a refusal is no connection: denied too);
//   - OutLocalPort is the sender socket's own port, neither the agent's nor the parent's, where there is one
//     (connected, socket-peers.c:40-50), else 0.
func fnStreamingOutFacts(v Value, side fnStreamingOutSide) error {
	data, err := dashMember(v, "data")
	if err != nil {
		return err
	}
	attempt, since := side.connected, side.connected
	switch side.stage {
	case "never":
		attempt, since = [2]int64{side.started[0], side.ready}, side.started
	case "denied":
		attempt = side.closed
	}
	top := map[string]int64{}
	for r := range data.Items {
		cell := map[string]int64{}
		null := map[string]bool{}
		for _, name := range append([]string{"InSince", "InAge", "OutSince"}, fnStreamingOutHeld...) {
			c, err := fnStreamingCell(v, r, name)
			if err != nil {
				return err
			}
			if cell[name], null[name], err = fnStreamingWhole(c); err != nil || cell[name] < 0 {
				return fmt.Errorf("row %d: %s is %s, want a whole number or null", r, name, c)
			}
			if !null[name] {
				top[name] = max(top[name], cell[name])
			}
		}
		clock := cell["InSince"]/1000 + cell["InAge"]
		if null["OutAttemptSince"] != null["OutAttemptAge"] ||
			(!null["OutAttemptSince"] && (cell["OutAttemptSince"]/1000+cell["OutAttemptAge"] != clock ||
				cell["OutAttemptSince"]/1000 < attempt[0] || cell["OutAttemptSince"]/1000 > attempt[1])) {
			return fmt.Errorf("row %d: OutAttemptSince %d and OutAttemptAge %d: not the milliseconds of an attempt in "+
				"[%d, %d] and the seconds from its second to the table's clock %d", r, cell["OutAttemptSince"],
				cell["OutAttemptAge"], attempt[0], attempt[1], clock)
		}
		if s := cell["OutSince"] / 1000; null["OutSince"] || s < since[0] || s > since[1] {
			return fmt.Errorf("row %d: OutSince %d is no second of [%d, %d] (%s)", r, cell["OutSince"], since[0], since[1],
				side.stage)
		}
		port := cell["OutLocalPort"]
		if p := strconv.FormatInt(port, 10); (side.stage == "connected" && (port <= 0 || port > 65535 || p == side.listen ||
			p == side.stub)) || (side.stage != "connected" && port != 0) {
			return fmt.Errorf("row %d: OutLocalPort %d, want the sender socket's own port connected, 0 otherwise (%s)", r,
				port, side.stage)
		}
		for _, name := range fnStreamingOutHeld {
			if name != "OutLocalPort" && name != "OutAttemptSince" && name != "OutAttemptAge" && null[name] {
				return fmt.Errorf("row %d: %s is null", r, name)
			}
		}
	}
	for _, name := range fnStreamingOutHeld {
		want, what := top[name], "the largest cell"
		switch name {
		case "OutLocalPort":
			if _, err := dashAt(v, "columns", name, "max"); err == nil {
				return fmt.Errorf("columns.%s has a max: C passes none (function-netdata-streaming.c:775)", name)
			}
			continue
		case "CollectedMetrics", "CollectedInstances", "CollectedContexts":
			want, what = top["db"+name[len("Collected"):]], "the db column's largest cell"
		}
		m, err := dashAt(v, "columns", name, "max")
		if err != nil {
			return err
		}
		if x, perr := strconv.ParseFloat(m.Text, 64); m.Kind != KindNumber || perr != nil || x != float64(want) {
			return fmt.Errorf("columns.%s.max is %s, %s is %d", name, m, what, want)
		}
	}
	return nil
}
