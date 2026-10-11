// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// `netdata-streaming` called down the stream (`fn.netdata-streaming`'s `down`; R111's "not verifiable" 1; D220 fork
// 7): a parent runs a child's built-in by sending the call to the child's receiver (`send_to_plugin_cb =
// send_to_child`, stream-receiver.c:647-651), and the child runs it on its sender's stream thread
// (stream-sender-execute.c), which reads the sender's own status for its row (rrdhost_status(), rrdhost-status.c:
// 240-291): a port that took the sender's lock there would stall the call and the stream with it.

// fnStreamingDownLimit is how long a call down the stream may take on either side: C answered every one in under 0.1 s
// (SA-F's probe p1); a call that waits on the sender's own stream thread would wait until the parent's timeout
// (the built-in's 10 s, fnWait).
const fnStreamingDownLimit = 3 * time.Second

// fnStreamingDownChild starts side i's child under its parent at parent: stream.rchild's identity (rchildHostname),
// alloc, one tier, the pulse on (a sender starts at the first collection), compression on (the C parent decodes the
// child's zstd, TestRChild/compressed), and bearer tokens for its own machine GUID (fnWriteTokens' two, signed for
// the child), so that its own table can be asked too.
func fnStreamingDownChild(t *testing.T, bin string, role Role, parent string) *daemon.Daemon {
	t.Helper()
	id := daemon.Identity{Hostname: rchildHostname, StreamKey: cChildKey, MachineGUID: rchildGUID}
	o := daemon.Options{Binary: bin, RunDir: runDir(t, role), StorageTiers: 1, DBMode: "alloc", Identity: &id,
		NoStreamKey: true, StreamTo: &daemon.StreamTo{Destination: parent, APIKey: parentIdentity.StreamKey,
			Compression: true, Extra: "    reconnect delay = 5\n"}}
	dir := filepath.Join(o.RunDir, "lib", "bearer_tokens")
	if err := os.MkdirAll(dir, 0o750); err != nil {
		t.Fatal(err)
	}
	for _, b := range []bearerToken{fnAdminToken, fnMemberToken} {
		if err := os.WriteFile(filepath.Join(dir, b.token), b.file(rchildGUID, b.signature(t, rchildGUID, false)),
			0o640); err != nil {
			t.Fatal(err)
		}
	}
	d, err := daemon.Start(o)
	if err != nil {
		t.Fatalf("parity: start %s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	return d
}

// fnStreamingDownSide is what one side's child's table is held to besides fnStreamingFacts (fnStreamingDownFacts):
// the port its parent listens on, the child's own port of its stream to that parent (niHeldSockets), the seconds the
// child connected in (from its launch to the second its parent was first seen listing its netdata-streaming: the
// sender takes its connection's second, stream-sender.c:365, before it sends its functions, :382, :178) and whether
// its sender has sent a function's answer before this table (the call before it).
type fnStreamingDownSide struct {
	parent, socket string
	connected      [2]int64
	answered       bool
}

// fnStreamingDownSocket is the child's own port of the one stream socket the child pid holds to the parent's port.
func fnStreamingDownSocket(pid int, parent string) (string, error) {
	held, err := niHeldSockets(pid)
	if err != nil {
		return "", err
	}
	var found []string
	for s := range held {
		if local, remote, _ := strings.Cut(s, ">"); remote == parent {
			found = append(found, local)
		}
	}
	if len(found) != 1 {
		return "", fmt.Errorf("the child (PID %d) holds %d sockets to its parent's port %s: %v", pid, len(found), parent,
			found)
	}
	return found[0], nil
}

// fnStreamingDownGuard is the guard of the oracle's table of the C child, called down the stream: its localhost alone
// (the child has no child of its own: function-netdata-streaming.c:75-77), collecting (the pulse) and streaming to
// the C parent (rrdhost-status.c:240-291): online with the handshake's reason, one connection, one hop, the parent's
// port, plain (no TLS) and compressed (the C parent accepted the child's zstd: stream-compression.c), the parent's
// reason SOCKET CONNECTED (stream-connector.c:236-238); normal (function-netdata-streaming.c:107-138).
func fnStreamingDownGuard(parent string) func(Value) error {
	return fnStreamingRows(fnStreamingRow{"Node": strconv.Quote(rchildHostname), "rowOptions": `{"severity":"normal"}`,
		"InReason": `"LOCALHOST"`, "InStatus": `"online"`, "OutStatus": `"online"`, "OutReason": `"CONNECTED"`,
		"OutConnections": "1", "OutHops": "1", "OutRemoteIP": `"127.0.0.1"`, "OutRemotePort": parent,
		"OutSSL": `"PLAIN"`, "OutCompression": `"COMPRESSED"`, "OutAttemptHandshake": `["SOCKET CONNECTED"]`})
}

// fnStreamingDownVolatile are the columns masked besides fnStreamingVolatile's and fnStreamingOutVolatile's on this
// topology: the parent's port (each side's own parent), and the bytes of the chart definitions and of the functions'
// answers, which compression writes (stream-sender-commit.c:171-173: the bytes counted are the compressed ones) and
// which differ C against the port by the compressors' blocks. Each is held per side by fnStreamingDownFacts.
var fnStreamingDownVolatile = []string{"OutRemotePort", "OutTrafficMetadata", "OutTrafficFunctions"}

// fnStreamingDownFacts holds side's child's table, called down the stream, to what C fixes for a sender connected
// to a real parent, where the comparison masks (fnStreamingFacts holds fnStreamingVolatile's columns):
//   - OutRemotePort is the parent's port and OutLocalPort the child's own end of that socket (socket-peers.c:22-50);
//   - OutSince and the parents' last attempt are seconds of the child's connection (stream-sender.c:365;
//     stream-parents.c:854), OutSince a whole second, the attempt's age from its whole second to the table's clock;
//   - the metadata bytes are above 0 (the chart definitions went first); the functions' bytes are 0 before any
//     function's answer was sent and above 0 after one (stream-sender-execute.c: the answer queued as FUNCTIONS
//     traffic, stream-traffic-types.h:10-18);
//   - the `max` of each column fnStreamingDown masks besides fnStreamingVolatile's (fnStreamingOutVolatile,
//     fnStreamingDownVolatile) is C's (fnStreamingMaxima: on this one-row table, its cell; none for OutLocalPort).
func fnStreamingDownFacts(v Value, side fnStreamingDownSide) error {
	cell := func(col string) (int64, error) {
		c, err := fnStreamingCell(v, 0, col)
		if err != nil {
			return 0, err
		}
		n, null, err := fnStreamingWhole(c)
		if err == nil && null {
			err = fmt.Errorf("row 0: %s is null", col)
		}
		return n, err
	}
	got := map[string]int64{}
	for _, col := range []string{"OutRemotePort", "OutLocalPort", "OutSince", "OutAttemptSince", "OutAttemptAge",
		"InSince", "InAge", "OutTrafficMetadata", "OutTrafficFunctions"} {
		n, err := cell(col)
		if err != nil {
			return err
		}
		got[col] = n
	}
	if p := strconv.FormatInt(got["OutRemotePort"], 10); p != side.parent {
		return fmt.Errorf("row 0: OutRemotePort %s, want the parent's port %s", p, side.parent)
	}
	if p := strconv.FormatInt(got["OutLocalPort"], 10); p != side.socket {
		return fmt.Errorf("row 0: OutLocalPort %s, want the child's own end of its stream, %s", p, side.socket)
	}
	clock := got["InSince"]/1000 + got["InAge"]
	for _, col := range []string{"OutSince", "OutAttemptSince"} {
		if s := got[col] / 1000; s < side.connected[0] || s > side.connected[1] {
			return fmt.Errorf("row 0: %s %d is no second of the connection [%d, %d]", col, got[col], side.connected[0],
				side.connected[1])
		}
	}
	if got["OutSince"]%1000 != 0 || got["OutAttemptSince"]/1000+got["OutAttemptAge"] != clock {
		return fmt.Errorf("row 0: OutSince %d and OutAttemptSince %d with its age %d: not a whole second, and its age "+
			"to the table's clock %d", got["OutSince"], got["OutAttemptSince"], got["OutAttemptAge"], clock)
	}
	if got["OutTrafficMetadata"] <= 0 || (got["OutTrafficFunctions"] > 0) != side.answered {
		return fmt.Errorf("row 0: OutTrafficMetadata %d, OutTrafficFunctions %d: the definitions sent, and a function's "+
			"answer sent before this table: %t", got["OutTrafficMetadata"], got["OutTrafficFunctions"], side.answered)
	}
	return fnStreamingMaxima(v, slices.Concat(fnStreamingOutVolatile, fnStreamingDownVolatile))
}

// fnStreamingDownSteady is how long a child must read complete with its counts unchanged before its table is asked
// (fnStreamingDownComplete): five passes of the pulse that makes its last charts (update every 1 s). In SA-F4's
// probe p1 (four C children and two C children beside k117 and k118, polled every 0.5 s) the counts' last change came
// 1.0 s after the first bytes of collected data on every C child (1.5 s on both Rust children), and no reading was
// complete before its counts were final.
const fnStreamingDownSteady = 5 * time.Second

// fnStreamingDownState is a child's localhost as its own `/api/v3/node_instances` reads it (rrdhost_status(),
// rrdhost-status.c): its database's counts (`db`: metrics, instances, contexts; the contexts' counts, :117-145), its
// collected ones (`ingest`, :154-208), its sender's status (`stream.status`: online, replicating while a chart's
// replication runs, offline unconnected, :265-282; the Function's OutStatus) and the bytes of collected data its
// sender has sent on this connection (`stream.destination.traffic.data`, :256-262).
type fnStreamingDownState struct {
	db, collected [3]int64
	status        string
	data          int64
}

// fnStreamingDownStateOf reads v, a child's own `/api/v3/node_instances`, as fnStreamingDownState.
func fnStreamingDownStateOf(v Value) (fnStreamingDownState, error) {
	var s fnStreamingDownState
	read := func(to *int64, path ...string) error {
		x, err := dashAt(v, niStreamAt(path...)...)
		if err != nil {
			return err
		}
		n, err := strconv.ParseInt(x.Text, 10, 64)
		if x.Kind != KindNumber || err != nil || n < 0 {
			return fmt.Errorf("%s is %s, want a count", strings.Join(path, "."), x)
		}
		*to = n
		return nil
	}
	for k, count := range []string{"metrics", "instances", "contexts"} {
		if err := errors.Join(read(&s.db[k], "db", count), read(&s.collected[k], "ingest", count)); err != nil {
			return s, err
		}
	}
	status, err := dashAt(v, niStreamAt("stream", "status")...)
	if err != nil || status.Kind != KindString {
		return s, fmt.Errorf("stream.status is %s, want a string (%v)", status, err)
	}
	s.status = status.Text
	return s, read(&s.data, "stream", "destination", "traffic", "data")
}

// complete tells why s is no complete child, nil when it is one (fnStreamingDownComplete): its sender online, every
// count above 0 and collected, and its collected data flowing. A C child stores its pulse's charts at its first
// collection and counts them collected a pass later; once connected, the pulse adds netdata.network_streaming at its
// first pass after the stream's bytes (pulse-network.c:272-300; its context netdata.network is network_api's: 2
// metrics, 1 instance, no context) and netdata.db_points_results at its first pass after a query generated points
// (pulse-queries.c:221-243): on a child that nobody queries, the replication answers to the parent's requests
// (stream-replication-sender.c:297, :562; pulse-queries.c:120), the same answers after which a chart's data is sent
// (command-begin-set-end-init.c:71-79). A younger child reads every count stored collected and no data (H41's pass
// c1: 67/13/12 against the oracle's child's 73/14/13, its data 0), and a child replicating reads data above 0 with
// its counts short (p1: C children `replicating` with 984 and 985 bytes, 73/14/13 stored, 67/13/12 collected).
func (s fnStreamingDownState) complete() error {
	switch {
	case s.status != "online":
		return fmt.Errorf("counts %v: the sender is %s, want online", s.db, s.status)
	case slices.Contains(s.db[:], 0):
		return fmt.Errorf("counts %v: a count is 0", s.db)
	case s.db != s.collected:
		return fmt.Errorf("counts %v, collected %v: not every count stored is collected", s.db, s.collected)
	case s.data <= 0:
		return fmt.Errorf("counts %v: no collected data sent yet", s.db)
	}
	return nil
}

// fnStreamingDownRead is one reading of a child (fnStreamingDownSettled): when it was answered, and its state.
type fnStreamingDownRead struct {
	at    time.Time
	state fnStreamingDownState
}

// fnStreamingDownComplete judges a child's readings, oldest first: nil when the last reads a complete child that has
// stayed so: every reading of the steady before it complete (fnStreamingDownState.complete) with the last one's
// counts, and the readings going back that far.
func fnStreamingDownComplete(reads []fnStreamingDownRead, steady time.Duration) error {
	if len(reads) == 0 {
		return errors.New("no reading")
	}
	last := reads[len(reads)-1]
	for _, r := range slices.Backward(reads) {
		err := r.state.complete()
		if err == nil && r.state.db != last.state.db {
			err = fmt.Errorf("counts %v apart from the last reading's %v", r.state.db, last.state.db)
		}
		if err != nil {
			return fmt.Errorf("still growing %v before the last reading: %w", last.at.Sub(r.at).Round(time.Millisecond), err)
		}
		if last.at.Sub(r.at) >= steady {
			return nil
		}
	}
	return fmt.Errorf("counts %v complete for %v, want %v", last.state.db,
		last.at.Sub(reads[0].at).Round(time.Millisecond), steady)
}

// fnStreamingDownApart judges both children's states (o the oracle's, c the candidate's): nil when they read the same
// counts, so that the tables asked are of children alike.
func fnStreamingDownApart(o, c fnStreamingDownState) error {
	if o.db != c.db {
		return fmt.Errorf("the children's counts are apart: the oracle's %v, the candidate's %v", o.db, c.db)
	}
	return nil
}

// fnStreamingDownAsk is the state of the child at addr (its own `/api/v3/node_instances`, tagged harness=wait).
func fnStreamingDownAsk(addr string) (fnStreamingDownState, error) {
	b, err := v2Exchange(addr, v2Req{target: "/api/v3/node_instances?harness=wait"})
	if err != nil {
		return fnStreamingDownState{}, err
	}
	v, err := ParseJSON(httpBody(b))
	if err != nil {
		return fnStreamingDownState{}, err
	}
	return fnStreamingDownStateOf(v)
}

// fnStreamingDownSettled reads the child at addr (fnStreamingDownAsk) up to limit until its readings hold
// fnStreamingDownComplete over steady; it returns why they never did, nil when they did.
func fnStreamingDownSettled(addr string, limit, steady time.Duration) error {
	var reads []fnStreamingDownRead
	err := errors.New("no reading")
	pollUntil(limit, func() bool {
		s, e := fnStreamingDownAsk(addr)
		if e != nil {
			err = e
			return false
		}
		reads = append(reads, fnStreamingDownRead{time.Now(), s})
		err = fnStreamingDownComplete(reads, steady)
		return err == nil
	})
	return err
}

// fnStreamingDownTogether reads both children (addr: the oracle's, the candidate's) up to limit until each reads
// complete (fnStreamingDownState.complete) and their counts are equal (fnStreamingDownApart); it returns the last
// round's failures: the oracle's (its child not complete) and the candidate's (its child not complete, or apart).
func fnStreamingDownTogether(addr [2]string, limit time.Duration) (oracle, candidate error) {
	pollUntil(limit, func() bool {
		var s [2]fnStreamingDownState
		var errs [2]error
		for i := range addr {
			if s[i], errs[i] = fnStreamingDownAsk(addr[i]); errs[i] == nil {
				errs[i] = s[i].complete()
			}
		}
		oracle, candidate = errs[0], errs[1]
		if oracle == nil && candidate == nil {
			candidate = fnStreamingDownApart(s[0], s[1])
		}
		return oracle == nil && candidate == nil
	})
	return oracle, candidate
}

// fnStreamingDown (TestFnNetdataStreaming/down): two C parents (the oracle's binary on both sides: `ram`, one tier,
// the pulse off, the bearer tokens) and under each a child (fnStreamingDownChild): a C child under the oracle's
// parent, the candidate under the other. Once each parent has its child online and lists the child's
// netdata-streaming, each child has read complete for fnStreamingDownSteady (fnStreamingDownSettled: up to 60 s) and
// both read complete with the same counts (fnStreamingDownTogether: up to 60 s; an oracle's child that does not is
// the test's end, a candidate's an error and the calls still compared), the admin's
// `/host/<child>/api/v1/function?function=netdata-streaming` is asked through each
// parent twice (`call1`, `call2`) and compared as the Function's tables are (fnStreamingCompareWith with via: the
// pair is the children's), each answer within fnStreamingDownLimit; then each parent must still have its child
// online with its data up to the clock (the stream went on). The second call's table holds the first's answer in
// OutTrafficFunctions.
func fnStreamingDown(t *testing.T) {
	bins := binaries(t)
	parents := startPairWith(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true},
		parentIdentity, [2]string{bins[0], bins[0]}, [2]string{}, [2]Role{"fnsd-p-oracle", "fnsd-p-candidate"},
		fnWriteTokens)
	var children Pair
	var sides [2]fnStreamingDownSide
	var via [2]string
	for i, side := range parents.Each() {
		c := fnStreamingDownChild(t, bins[i], Role("fnsd-c-"+string(side.Role)), side.Daemon.Addr)
		if i == 0 {
			children.Oracle = c
		} else {
			children.Candidate = c
		}
		_, port, _ := strings.Cut(side.Daemon.Addr, ":")
		sides[i].parent, via[i] = port, side.Daemon.Addr
	}
	for i, side := range parents.Each() {
		x := &fnHTTPSide{role: side.Role, d: side.Daemon}
		c := children.Each()[i].Daemon
		var err error
		switch {
		case !fnStreamOnline(side.Daemon.Addr, rchildGUID, 90*time.Second):
			err = errors.New("not online on its parent")
		case !x.listed(t, []fnListed{{fnChildHost, "netdata-streaming"}}, fnBuiltinsUsers[2].header):
			err = errors.New("its netdata-streaming not listed on its parent")
		}
		sides[i].connected = [2]int64{c.LaunchStartedAt.Unix(), time.Now().Unix()}
		if err == nil {
			err = fnStreamingDownSettled(c.Addr, 60*time.Second, fnStreamingDownSteady)
		}
		socket, serr := fnStreamingDownSocket(c.PID(), sides[i].parent)
		sides[i].socket = socket
		switch err = errors.Join(err, serr); {
		case err == nil:
		case side.Role == Oracle:
			t.Fatalf("oracle: the C child is not online, listed and settled on its parent in time: %v", err)
		default:
			t.Errorf("candidate: the child is not online, listed and settled on its parent in time: %v", err)
		}
	}
	// the children compared are alike: each still complete and both with the same counts
	switch o, c := fnStreamingDownTogether([2]string{children.Oracle.Addr, children.Candidate.Addr}, 60*time.Second); {
	case o != nil:
		t.Fatalf("oracle: the C child is not complete: %v", o)
	case c != nil:
		t.Errorf("candidate: the child is not complete and alike the oracle's: %v", c)
	}
	target := fnChildHost + "/api/v1/function?function=netdata-streaming"
	for _, call := range []string{"call1", "call2"} {
		t.Run(call, func(t *testing.T) {
			fnStreamingCompareWith(t, &children, fnStreamingAsk{target: target,
				guard: fnStreamingDownGuard(sides[0].parent), via: via, limit: fnStreamingDownLimit,
				volatile: append(append([]string{}, fnStreamingOutVolatile...), fnStreamingDownVolatile...),
				facts:    func(i int, v Value, flight [2]int64) error { return fnStreamingDownFacts(v, sides[i]) }})
		})
		for i := range sides {
			sides[i].answered = true
		}
	}
	// the stream went on: each parent has its child online, its last stored second at most 3 s before the clock
	for _, side := range parents.Each() {
		last := int64(0)
		ok := fnStreamOnline(side.Daemon.Addr, rchildGUID, 10*time.Second) && pollUntil(10*time.Second, func() bool {
			b, err := v2Exchange(side.Daemon.Addr, v2Req{target: "/api/v3/node_instances?scope_nodes=" + rchildHostname +
				"&harness=wait"})
			if err != nil {
				return false
			}
			v, err := ParseJSON(httpBody(b))
			if err != nil {
				return false
			}
			l, err := dashAt(v, "nodes", "[0]", "instances", "[0]", "db", "last_time")
			if err != nil {
				return false
			}
			last, _ = strconv.ParseInt(l.Text, 10, 64)
			return last >= time.Now().Unix()-3
		})
		switch {
		case ok:
		case side.Role == Oracle:
			t.Fatalf("oracle: the parent's last second of the child is %d, the stream stalled", last)
		default:
			t.Errorf("candidate: the parent's last second of the child is %d, the stream stalled", last)
		}
	}
}
