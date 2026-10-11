// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"strconv"
	"testing"
	"time"
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
// and denied: a refusal is no connection), the parent refused it (denied) and it probed its parents (banned).
type fnStreamingOutSide struct {
	stage        string
	listen, stub string
	started      [2]int64
	ready        int64
	connected    [2]int64
	closed       [2]int64
	probed       [2]int64
	// socket is the sender's own port at its connection (niStreamSide.socket), empty where the harness has none
	socket string
}

// fnStreamingOutSideOf is side i's fnStreamingOutSide of a stream pair's stage (niStreamRun).
func fnStreamingOutSideOf(r *niStreamRun, i int) fnStreamingOutSide {
	s := r.Sides[i]
	return fnStreamingOutSide{stage: r.Stage, listen: s.listen, stub: s.stub, started: s.started, ready: s.ready,
		connected: s.connected, closed: s.closed, probed: s.probed, socket: s.socket}
}

// collected are the seconds the side's database first time may be in: niStreamSide.collected's window of its first
// collection, the one TestNodeInstancesStream reads localhost's `db.first_time` in (niFirstRender); zeros for a pair
// without a collection (never).
func (s fnStreamingOutSide) collected() [2]int64 {
	return niStreamSide{niSide: niSide{started: s.started}, connected: s.connected, probed: s.probed}.collected()
}

// fnStreamingOut compares the admin's table at a stage of TestNodeInstancesStream's pairs (runNIStream's compare):
// fnStreamingCompareWith with the stage's guard (fnStreamingOutGuardFor), its columns masked (fnStreamingOutVolatileOf)
// and its cells (fnStreamingOutMasksOf), and each side's table held to fnStreamingOutFacts.
func fnStreamingOut(t *testing.T, r *niStreamRun) {
	sides := [2]fnStreamingOutSide{fnStreamingOutSideOf(r, 0), fnStreamingOutSideOf(r, 1)}
	// a table asked in the second of its parents' last attempt reads that attempt's age 0 on one side and 1 on the
	// other, and the masks keep a 0 (fnStreamingMasks): the stages whose rows come right after that attempt (the
	// probes that banned the parents, the disconnect) ask in a later second
	if end := fnStreamingOutAttemptEnd(sides); end > 0 {
		time.Sleep(time.Until(time.Unix(end+1, 0)))
	}
	fnStreamingCompareWith(t, r.Pair, fnStreamingOutAsk(r, sides))
}

// fnStreamingOutAsk is fnStreamingOut's comparison of r's stage, given its sides (fnStreamingOutSideOf).
func fnStreamingOutAsk(r *niStreamRun, sides [2]fnStreamingOutSide) fnStreamingAsk {
	return fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
		guard: fnStreamingOutGuardFor(r.pair, r.Stage, sides[0].stub), volatile: fnStreamingOutVolatileOf(r.Stage),
		facts: func(i int, v Value, flight [2]int64) error { return fnStreamingOutFacts(v, sides[i]) },
		masks: fnStreamingOutMasksOf(r.Stage)}
}

// fnStreamingOutMasksOf are the masks a stage's tables take besides the columns' (fnStreamingAsk.masks): at `reset`
// the reason of a reset (fnStreamingResetMasks), nil elsewhere.
func fnStreamingOutMasksOf(stage string) func(Value) []Mask {
	if stage == "reset" {
		return fnStreamingResetMasks
	}
	return nil
}

// fnStreamingResetMasks are the masks of each row's OutReason and OutAttemptHandshake where the first is one of the
// texts C gives a reset (niStreamResetReasons: which one, the path that saw the reset says) and the second that text
// alone (`["<OutReason>"]`: one call writes both, stream-parents.c:103-108); fnStreamingOutFacts holds the same on
// each side.
func fnStreamingResetMasks(v Value) []Mask {
	data, err := dashMember(v, "data")
	if err != nil {
		return nil
	}
	cols := fnStreamingColumns(v)
	var masks []Mask
	for r := range data.Items {
		reason, err1 := fnStreamingCell(v, r, "OutReason")
		attempts, err2 := fnStreamingCell(v, r, "OutAttemptHandshake")
		if err1 != nil || err2 != nil || !fnStreamingResetPair(reason, attempts) {
			continue
		}
		for _, n := range []string{"OutReason", "OutAttemptHandshake"} {
			masks = append(masks, Mask{fmt.Sprintf("data.[%d].[%d]", r, cols[n]), "reset"})
		}
	}
	return masks
}

// fnStreamingResetPair tells whether reason is a text C gives a reset (niStreamResetReasons) and attempts the one
// parent's text, the same.
func fnStreamingResetPair(reason, attempts Value) bool {
	return reason.Kind == KindString && slices.Contains(niStreamResetReasons, reason.Text) &&
		attempts.Kind == KindArray && len(attempts.Items) == 1 && attempts.Items[0].Kind == KindString &&
		attempts.Items[0].Text == reason.Text
}

// fnStreamingOutAttemptEnd is the last second the parents' last attempt of a `banned` or `reset` stage may lie in on
// either side (fnStreamingOut waits past it), 0 for the other stages, whose rows come seconds after their attempt.
func fnStreamingOutAttemptEnd(sides [2]fnStreamingOutSide) int64 {
	end := int64(0)
	for _, s := range sides {
		switch s.stage {
		case "banned":
			end = max(end, s.probed[1])
		case "reset":
			end = max(end, s.closed[1])
		}
	}
	return end
}

// fnStreamingOutVolatileOf are the columns masked at a stage besides fnStreamingVolatile's: fnStreamingOutVolatile,
// and the chart definitions' bytes where they are compressed (compressed: stream-sender-commit.c:171-173, they
// varied C against C) or how far a young connection got (reset).
func fnStreamingOutVolatileOf(stage string) []string {
	if stage == "compressed" || stage == "reset" {
		return slices.Concat(fnStreamingOutVolatile, []string{"OutTrafficMetadata"})
	}
	return fnStreamingOutVolatile
}

// fnStreamingOutConnected tells whether a stage reads its sender connected (fnStreamingOutFacts: its own port).
func fnStreamingOutConnected(stage string) bool { return stage == "connected" || stage == "compressed" }

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
	return fnStreamingOutGuardFor("out", stage, stub)
}

// fnStreamingOutGuardFor is fnStreamingOutGuard of a stage of the pair named pair (runNIStream), as C printed each
// (SA-F's probe p1, H41):
//   - never2's never: as never's, with the two parents' reasons (stream_parent_handshake_error_to_json(),
//     stream-parents.c:161-172);
//   - tls's connected: the ends on IPv6 (`::1`, socket-peers.c) and the socket TLS (`SSL`, rrdhost-status.c:254);
//     its denied: the socket gone, so plain again (nd_sock_is_ssl() of a closed socket);
//   - zip's compressed: replicating (the stub asks no chart's replication: every chart waits, completion 100 and 13
//     instances, rrdhost-status.c:265-281), compressed (the stub took the compressions offered: the compressor
//     initialized, :277);
//   - reset: offline, the second connection counted, the ends cleared; a warning. Its reason and the parent's are
//     left to fnStreamingOutFacts: a reset reads DISCONNECT SOCKET ERROR or DISCONNECTED SOCKET CLOSED BY REMOTE END
//     by the path that saw it (niStreamResetReasons; C against C, SA-F2's pass c1);
//   - banned: offline with NO PARENT TO SEND TO, which is no warning (function-netdata-streaming.c:124-128), and each
//     parent's reason its ban's (stream-parents.c:647-656, :694-701: PARENT IS LOCALHOST, ALREADY CONNECTED).
func fnStreamingOutGuardFor(pair, stage, stub string) func(Value) error {
	row := fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "InReason": `"LOCALHOST"`}
	add := func(pairs ...string) {
		for i := 0; i+1 < len(pairs); i += 2 {
			row[pairs[i]] = pairs[i+1]
		}
	}
	switch stage {
	case "never":
		attempts := `["NEVER CONNECTED"]`
		if pair == "never2" {
			attempts = `["NEVER CONNECTED","NEVER CONNECTED"]`
		}
		add("rowOptions", `{"severity":"warning"}`, "InStatus", `"initializing"`, "OutStatus", `"offline"`,
			"OutReason", `"NEVER CONNECTED"`, "OutConnections", "0", "OutLocalIP", `"not connected"`,
			"OutRemoteIP", `"not connected"`, "OutAttemptHandshake", attempts)
	case "connected":
		add("rowOptions", `{"severity":"normal"}`, "InStatus", `"online"`, "OutStatus", `"online"`,
			"OutReason", `"CONNECTED"`, "OutConnections", "1", "OutHops", "1", "OutRemotePort", stub,
			"OutSSL", `"PLAIN"`, "OutCompression", `"UNCOMPRESSED"`, "OutAttemptHandshake", `["SOCKET CONNECTED"]`)
		if pair == "tls" {
			add("OutLocalIP", `"::1"`, "OutRemoteIP", `"::1"`, "OutSSL", `"SSL"`)
		}
	case "compressed":
		add("rowOptions", `{"severity":"normal"}`, "InStatus", `"online"`, "OutStatus", `"replicating"`,
			"OutReason", `"CONNECTED"`, "OutConnections", "1", "OutHops", "1", "OutRemotePort", stub,
			"OutReplCompletion", "100", "OutReplInstances", "13", "OutSSL", `"PLAIN"`, "OutCompression",
			`"COMPRESSED"`, "OutAttemptHandshake", `["SOCKET CONNECTED"]`)
	case "denied":
		add("rowOptions", `{"severity":"warning"}`, "InStatus", `"online"`, "OutStatus", `"offline"`,
			"OutReason", `"DENIED"`, "OutConnections", "1", "OutLocalIP", `"not connected"`,
			"OutRemoteIP", `"not connected"`, "OutReplCompletion", "0", "OutAttemptHandshake", `["DENIED"]`)
		if pair == "tls" {
			add("OutSSL", `"PLAIN"`)
		}
	case "reset":
		add("rowOptions", `{"severity":"warning"}`, "InStatus", `"online"`, "OutStatus", `"offline"`,
			"OutConnections", "2", "OutLocalIP", `"not connected"`, "OutRemoteIP", `"not connected"`,
			"OutReplCompletion", "0", "OutSSL", `"PLAIN"`)
	case "banned":
		add("rowOptions", `{"severity":"normal"}`, "InStatus", `"online"`, "OutStatus", `"offline"`,
			"OutReason", `"NO PARENT TO SEND TO"`, "OutConnections", "0", "OutLocalIP", `"not connected"`,
			"OutRemoteIP", `"not connected"`, "OutAttemptHandshake", `["LOCALHOST","ALREADY CONNECTED"]`)
	default:
		return func(Value) error { return fmt.Errorf("harness: no stage %q of the stream pairs", stage) }
	}
	return fnStreamingRows(row)
}

// fnStreamingOutFacts holds one side's table of a stream pair to what C fixes between the cells of fnStreamingOutHeld
// (fnStreamingOutVolatile's among them, which the comparison masks), and the sender's times to their windows (side):
//   - the counts, the bytes sent by type, the parents' last attempt and its age are whole numbers, each column's `max`
//     C's (fnStreamingMaxima: the largest of its cells, a Collected count's the db column's);
//   - the parents' last attempt is the milliseconds of a microsecond clock (`since_ut / 1000`) and its age the
//     seconds from its whole second to the table's clock (:271-287), both null or neither; the attempt lies in the
//     side's window: the parents' list made (never: [start, ready], stream-parents.c:940), the connection
//     (connected), the refusal (denied), the probes (banned: stream-parents.c:653, :698), the disconnect (reset: its
//     whole second, stream-parents.c:103-108);
//   - OutSince is the agent's start before any connection (never, banned: rrdhost-status.c:289-290), the very second
//     localhost's InSince reads (both netdata_start_time: :175-177, :202, :289-290), and the connection's second
//     after it (stream-sender.c:365; a refusal or a disconnect writes none: denied and reset too);
//   - dbFrom is a second of the side's first collection (collected: with the pulse on, the first point localhost
//     stored, rrdhost_retention(), rrdhost-status.c:121-122, function-netdata-streaming.c:155), 0 without a
//     collection (never: nothing stored, :124-130);
//   - OutLocalPort is the sender socket's own port where there is one (connected, socket-peers.c:40-50): its
//     session's port with the scripted parent (niStreamSide.niIsSocket), else 0;
//   - at `reset`, OutReason is a text C gives a reset and OutAttemptHandshake that text alone (fnStreamingResetPair:
//     the disconnect writes the host's and the parent's reason at once, stream-parents.c:103-108).
func fnStreamingOutFacts(v Value, side fnStreamingOutSide) error {
	data, err := dashMember(v, "data")
	if err != nil {
		return err
	}
	attempt, since := side.connected, side.connected
	switch side.stage {
	case "never":
		attempt, since = [2]int64{side.started[0], side.ready}, side.started
	case "banned":
		attempt, since = side.probed, side.started
	case "denied", "reset":
		attempt = side.closed
	}
	first := side.collected()
	for r := range data.Items {
		cell := map[string]int64{}
		null := map[string]bool{}
		for _, name := range append([]string{"InSince", "InAge", "OutSince", "dbFrom"}, fnStreamingOutHeld...) {
			c, err := fnStreamingCell(v, r, name)
			if err != nil {
				return err
			}
			if cell[name], null[name], err = fnStreamingWhole(c); err != nil || cell[name] < 0 {
				return fmt.Errorf("row %d: %s is %s, want a whole number or null", r, name, c)
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
		if (side.stage == "never" || side.stage == "banned") && cell["OutSince"] != cell["InSince"] {
			return fmt.Errorf("row %d: OutSince %d is not InSince %d: a sender that never connected reads the agent's "+
				"start, as localhost's ingestion does", r, cell["OutSince"], cell["InSince"])
		}
		if s := cell["dbFrom"] / 1000; null["dbFrom"] || (first[1] == 0 && cell["dbFrom"] != 0) ||
			(first[1] != 0 && (s < first[0] || s > first[1])) {
			return fmt.Errorf("row %d: dbFrom %d is not the milliseconds of a second of the first collection [%d, %d] "+
				"(0 without one)", r, cell["dbFrom"], first[0], first[1])
		}
		port := cell["OutLocalPort"]
		sock := niStreamSide{niSide: niSide{listen: side.listen}, stub: side.stub, socket: side.socket}
		if p := strconv.FormatInt(port, 10); (fnStreamingOutConnected(side.stage) && !sock.niIsSocket(p)) ||
			(!fnStreamingOutConnected(side.stage) && port != 0) {
			return fmt.Errorf("row %d: OutLocalPort %d, want the sender socket's own port connected, 0 otherwise (%s)", r,
				port, side.stage)
		}
		for _, name := range fnStreamingOutHeld {
			if name != "OutLocalPort" && name != "OutAttemptSince" && name != "OutAttemptAge" && null[name] {
				return fmt.Errorf("row %d: %s is null", r, name)
			}
		}
		if side.stage == "reset" {
			reason, err1 := fnStreamingCell(v, r, "OutReason")
			attempts, err2 := fnStreamingCell(v, r, "OutAttemptHandshake")
			if err1 != nil || err2 != nil || !fnStreamingResetPair(reason, attempts) {
				return fmt.Errorf("row %d: OutReason %s and OutAttemptHandshake %s: not a reset's reason, the parent's "+
					"the same (%v %v)", r, reason, attempts, err1, err2)
			}
		}
	}
	return fnStreamingMaxima(v, fnStreamingOutHeld)
}

// fnStreamingMaxima holds the `max` of each of v's columns names to the one C passes with it (function-netdata-
// streaming.c: computed at :41-101, :156-286, passed at :584-860): the largest of its cells, each a whole number of 0
// or more or null (left out), 0 without any; none for OutLocalPort (:773-775: NAN); a Collected count's is the db
// column's (:706, :712, :718). The comparison masks a maximum other than 0 where it masks its column
// (fnStreamingMasks): this holds it on each side instead.
func fnStreamingMaxima(v Value, names []string) error {
	data, err := dashMember(v, "data")
	if err != nil {
		return err
	}
	largest := func(name string) (int64, error) {
		top := int64(0)
		for r := range data.Items {
			c, err := fnStreamingCell(v, r, name)
			if err != nil {
				return 0, err
			}
			n, null, err := fnStreamingWhole(c)
			if err != nil || n < 0 {
				return 0, fmt.Errorf("row %d: %s is %s, want a whole number or null", r, name, c)
			}
			if !null {
				top = max(top, n)
			}
		}
		return top, nil
	}
	for _, name := range names {
		col, what := name, "the largest cell"
		switch name {
		case "OutLocalPort":
			if _, err := dashAt(v, "columns", name, "max"); err == nil {
				return fmt.Errorf("columns.%s has a max: C passes none (function-netdata-streaming.c:775)", name)
			}
			continue
		case "CollectedMetrics", "CollectedInstances", "CollectedContexts":
			col, what = "db"+name[len("Collected"):], "the db column's largest cell"
		}
		want, err := largest(col)
		if err != nil {
			return err
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
