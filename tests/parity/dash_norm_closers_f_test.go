// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"net"
	"os"
	"slices"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The pins of SA-F's closer rows (H41): the stream rows' tightenings and the stream's other closers, each judge on
// answers C gave (the recorded C pairs of dash_norm_fnstreaming_data_test.go, and SA-F's own recordings), then on
// named wrong candidates, each one thing wrong in a recorded answer.

// testDashNormClosersFOutFacts: fnStreamingOutFacts' tightenings (R111 G-W2, G-W6) on C's recorded tables of the
// stream pairs: localhost's dbFrom held to its first collection's window (niStreamSide.collected) and, in `never`,
// OutSince held to InSince.
func testDashNormClosersFOutFacts(t *testing.T) {
	cell := func(resp, col string) int64 { return fnsInt(t, fnsCellOf(t, resp, 0, col)) }
	c, cs := fnsOutConnected[1], fnsOutConnectedSides[1]
	d, ds := fnsOutDenied[0], fnsOutDeniedSides[0]
	n, ns := fnsOutNever[1], fnsOutNeverSides[1]
	// C's tables hold, and C's first times lie inside the window, not at its ends (2 and 1 s from its end)
	for name, x := range map[string]struct {
		resp string
		side fnStreamingOutSide
	}{"never": {n.resp, ns}, "connected": {c.resp, cs}, "denied": {d.resp, ds},
		"never 0": {fnsOutNever[0].resp, fnsOutNeverSides[0]}, "connected 0": {fnsOutConnected[0].resp,
			fnsOutConnectedSides[0]}, "denied 1": {fnsOutDenied[1].resp, fnsOutDeniedSides[1]}} {
		if err := fnStreamingOutFacts(fnsDoc(t, x.resp), x.side); err != nil {
			t.Errorf("C's %s table: %v", name, err)
		}
	}
	if w := cs.collected(); w != [2]int64{cs.started[0], cs.connected[1] - niStreamFirstSlack} {
		t.Errorf("the connected side's first collection window: %v", w)
	}
	if w := ns.collected(); w != [2]int64{} {
		t.Errorf("the never side's first collection window: %v, want none", w)
	}
	// dbFrom written as a second s, dbDuration and the maxima kept consistent with it: fnStreamingFacts accepts each
	// one, so the window alone judges it
	from := func(resp string, s int64) string {
		to := cell(resp, "dbTo")
		dur, top := "null", "0"
		if s > 0 && to > s*1000 {
			dur = strconv.FormatInt(to/1000-s, 10)
			top = dur
		}
		return fnsWith(t, resp, fnsCell{0, "dbFrom", strconv.FormatInt(s*1000, 10)}, fnsCell{-1, "dbFrom",
			strconv.FormatInt(s*1000, 10)}, fnsCell{0, "dbDuration", dur}, fnsCell{-1, "dbDuration", top})
	}
	w := cs.collected()
	for name, x := range map[string]struct {
		rec  fnsRecorded
		s    int64
		side fnStreamingOutSide
		ok   bool
	}{
		"connected: the window's first second":         {c, w[0], cs, true},
		"connected: the window's last second":          {c, w[1], cs, true},
		"connected: a second after the window":         {c, w[1] + 1, cs, false},
		"connected: a second before the launch":        {c, w[0] - 1, cs, false},
		"connected: dbFrom the connection's second":    {c, cell(c.resp, "OutSince") / 1000, cs, false},
		"connected: dbFrom the table's clock less one": {c, cell(c.resp, "dbTo")/1000 - 1, cs, false},
		"connected: no first time while collecting":    {c, 0, cs, false},
		"denied: dbFrom the refusal's second":          {d, ds.closed[0], ds, false},
		"never: a first time without a collection":     {n, ns.started[0] + 1, ns, false},
	} {
		resp := from(x.rec.resp, x.s)
		if err := fnStreamingFacts(fnsDoc(t, resp), x.rec.port, x.rec.flight); err != nil {
			t.Errorf("harness: %s is refused by fnStreamingFacts already: %v", name, err)
		}
		err := fnStreamingOutFacts(fnsDoc(t, resp), x.side)
		switch {
		case x.ok && err != nil:
			t.Errorf("fnStreamingOutFacts, %s: %v", name, err)
		case !x.ok && (err == nil || !strings.Contains(err.Error(), "row 0: dbFrom")):
			t.Errorf("fnStreamingOutFacts, %s: %v, want the first collection's refusal", name, err)
		}
	}
	// never: OutSince read as another second of the start window than InSince (OutAge kept consistent)
	in := cell(n.resp, "InSince")
	for name, x := range map[string]struct {
		resp string
		ok   bool
	}{
		"never: OutSince a second after InSince": {fnsWith(t, n.resp, fnsCell{0, "OutSince", strconv.FormatInt(in+1000, 10)},
			fnsCell{0, "OutAge", strconv.FormatInt(cell(n.resp, "OutAge")-1, 10)}), false},
		"never: OutSince InSince": {n.resp, true},
	} {
		err := fnStreamingOutFacts(fnsDoc(t, x.resp), ns)
		switch {
		case x.ok && err != nil:
			t.Errorf("fnStreamingOutFacts, %s: %v", name, err)
		case !x.ok && (err == nil || !strings.Contains(err.Error(), "is not InSince")):
			t.Errorf("fnStreamingOutFacts, %s: %v, want InSince's refusal", name, err)
		}
	}
	// connected: OutSince is the connection's, not InSince (the rule is never's alone)
	if cell(c.resp, "OutSince") == cell(c.resp, "InSince") {
		t.Errorf("harness: C's connected table reads OutSince as InSince")
	}
}

// fnsSwapped is a port written with its two bytes swapped: a port read without ntohs(), in 1 to 65535 and not the
// agent's or the parent's, which the general rule of the port words takes.
func fnsSwapped(t *testing.T, port string) string {
	t.Helper()
	n := fnsInt(t, port)
	return strconv.FormatInt((n&0xff)<<8|n>>8, 10)
}

// testDashNormClosersFSocket: the port word SOCKET held to the sender's own end of its session with the scripted
// parent (niStreamSide.socket, niSessionPort), in TestNodeInstancesStream's render and in the Function's facts, on
// C's recorded `out/connected` pair; the parsers of /proc behind it; and niSessionPort itself on sessions this test's
// own process holds.
func testDashNormClosersFSocket(t *testing.T) {
	const key = "out/connected/v3-ni"
	_, _, dst, _, where := niStreamPaths()
	rec, ok := niStreamRecorded[key]
	if !ok {
		t.Fatalf("harness: no recorded row %s", key)
	}
	var ports [2]string
	for i := range ports {
		_, ports[i], _, _ = niPortOf(strings.Trim(niStreamValue(t, rec.body[i], dst("local")...), `"`))
	}
	sockets := rec
	for i := range sockets.sides {
		sockets.sides[i].socket = ports[i]
	}
	row := niStreamRowOf(t, sockets, key)
	local := where(dst("local"))
	with := func(i int, port string) string {
		return niStreamEdit(t, rec.body[i], dst("local"), `"[127.0.0.1]:`+port+`"`, "", false)
	}
	// C's pair, each side's port its session's, and the oracle's guard
	if got := niStreamNormDiffs(t, row.fam, row.req.target, rec.flight, rec.body[0], rec.body[1]); got != "" {
		t.Errorf("C's pair with its sessions' ports: differs at %q", got)
	}
	v, err := ParseJSON(row.fam.normalise(0, row.fam.v2Clock(row.req.target, rec.flight[0]), rec.flight[0],
		[]byte(rec.body[0])))
	if err != nil || row.req.guard(v) != nil {
		t.Errorf("C's answer with its session's port: the guard refuses it: %v %v", err, row.req.guard(v))
	}
	general := niStreamRowOf(t, rec, key)
	for name, c := range map[string]struct {
		candidate     string
		want, general string
	}{
		"the port's bytes swapped":       {with(1, fnsSwapped(t, ports[1])), local, ""},
		"the oracle's session's port":    {with(1, ports[0]), local, ""},
		"a port one above the session's": {with(1, strconv.FormatInt(fnsInt(t, ports[1])+1, 10)), local, ""},
		"the scripted parent's port":     {with(1, rec.sides[1].stub), local, local},
	} {
		if got := niStreamNormDiffs(t, row.fam, row.req.target, rec.flight, rec.body[0], c.candidate); got != c.want {
			t.Errorf("%s: differences at %q, want %q", name, got, c.want)
		}
		// the general rule, where the harness has no session: what it takes
		if got := niStreamNormDiffs(t, general.fam, general.req.target, rec.flight, rec.body[0], c.candidate); got !=
			c.general {
			t.Errorf("%s, by the general rule: differences at %q, want %q", name, got, c.general)
		}
	}
	// the oracle's own port is held too: its guard refuses a port that is not its session's
	v, err = ParseJSON(row.fam.normalise(0, row.fam.v2Clock(row.req.target, rec.flight[0]), rec.flight[0],
		[]byte(with(0, fnsSwapped(t, ports[0])))))
	if err != nil || row.req.guard(v) == nil || !strings.Contains(row.req.guard(v).Error(), "local") {
		t.Errorf("the oracle's port not its session's: the guard says %v (%v)", row.req.guard(v), err)
	}
	// niIsSocket's two rules
	side := niStreamSide{niSide: niSide{listen: "100"}, stub: "200", socket: "300"}
	for port, want := range map[string]bool{"300": true, "301": false, "100": false, "200": false, "0300": false} {
		if got := side.niIsSocket(port); got != want {
			t.Errorf("niIsSocket(%s) with a session = %t, want %t", port, got, want)
		}
	}
	side.socket = ""
	for port, want := range map[string]bool{"300": true, "65535": true, "65536": false, "0": false, "100": false,
		"200": false, "0300": false, "x": false} {
		if got := side.niIsSocket(port); got != want {
			t.Errorf("niIsSocket(%s) without a session = %t, want %t", port, got, want)
		}
	}
	// the Function's OutLocalPort, by the same rule (fnStreamingOutFacts)
	c, cs := fnsOutConnected[1], fnsOutConnectedSides[1]
	own := fnsCellOf(t, c.resp, 0, "OutLocalPort")
	cs.socket = own
	if err := fnStreamingOutFacts(fnsDoc(t, c.resp), cs); err != nil {
		t.Errorf("C's connected table with its session's port: %v", err)
	}
	for name, port := range map[string]string{"the port's bytes swapped": fnsSwapped(t, own),
		"the other side's session's port": fnsCellOf(t, fnsOutConnected[0].resp, 0, "OutLocalPort")} {
		bad := fnsWith(t, c.resp, fnsCell{0, "OutLocalPort", port})
		if err := fnStreamingOutFacts(fnsDoc(t, bad), cs); err == nil ||
			!strings.Contains(err.Error(), "row 0: OutLocalPort") {
			t.Errorf("fnStreamingOutFacts, OutLocalPort %s: %v", name, err)
		}
		general := cs
		general.socket = ""
		if err := fnStreamingOutFacts(fnsDoc(t, bad), general); err != nil {
			t.Errorf("fnStreamingOutFacts by the general rule, OutLocalPort %s: %v", name, err)
		}
	}
	// /proc's texts: the header skipped, IPv4 and IPv6 ends, a line too short or with a bad port left out
	sockets2 := map[string]string{}
	niTCPSockets("  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n"+
		"   0: 0100007F:AEC0 0100007F:8F73 01 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0\n"+
		"   1: 00000000000000000000000001000000:1F90 00000000000000000000000001000000:C35A 01 0:0 00:0 0 1000 0 77\n"+
		"   2: 0100007F:ZZZZ 0100007F:8F73 01 00000000:00000000 00:00000000 00000000  1000        0 99 1 0\n"+
		"   3: 0100007F:0050\n"+
		"   4: 0100007F:0051 0100007F:0052 01 00000000:00000000\n", sockets2)
	if want := map[string]string{"4242": "44736>36723", "77": "8080>50010"}; !maps.Equal(sockets2, want) {
		t.Errorf("niTCPSockets: %v, want %v", sockets2, want)
	}
	for link, want := range map[string]string{"socket:[4242]": "4242", "socket:[]": "", "pipe:[4242]": "",
		"socket:[4242": "", "/dev/null": ""} {
		if got, ok := niSocketInode(link); got != want && ok || ok != (want != "") {
			t.Errorf("niSocketInode(%q) = %q, %t", link, got, ok)
		}
	}
	// niSessionPort on sessions this process holds: one found by its own port, two refused, none for a process that
	// holds none
	stub, err := stream.StartParent(nil)
	if err != nil {
		t.Fatal(err)
	}
	defer stub.Close()
	dial := func() net.Conn {
		c, err := net.Dial("tcp", stub.Addr())
		if err != nil {
			t.Fatal(err)
		}
		if _, err := c.Write([]byte("STREAM key=k&hostname=h&machine_guid=g&ver=0 HTTP/1.1\r\n\r\n")); err != nil {
			t.Fatal(err)
		}
		return c
	}
	first := dial()
	defer first.Close()
	if stub.WaitSession(1, 5*time.Second) == nil {
		t.Fatal("harness: the stub saw no session")
	}
	want := strconv.Itoa(first.LocalAddr().(*net.TCPAddr).Port)
	if got, err := niSessionPort(stub, os.Getpid()); got != want || err != nil {
		t.Errorf("niSessionPort of the one session this process holds: %q %v, want %s", got, err, want)
	}
	if got, err := niSessionPort(stub, os.Getppid()); err == nil {
		t.Errorf("niSessionPort of a process that holds no session: %q", got)
	}
	second := dial()
	defer second.Close()
	if stub.WaitSession(2, 5*time.Second) == nil {
		t.Fatal("harness: the stub saw no second session")
	}
	if got, err := niSessionPort(stub, os.Getpid()); err == nil ||
		!strings.Contains(err.Error(), "holds 2 open sessions") {
		t.Errorf("niSessionPort of two sessions this process holds: %q %v", got, err)
	}
	// a session that ended is no sender's any more
	_ = second.Close()
	if !pollUntil(5*time.Second, func() bool { return stub.Sessions()[1].Closed() }) {
		t.Fatal("harness: the stub did not see the second session end")
	}
	if got, err := niSessionPort(stub, os.Getpid()); got != want || err != nil {
		t.Errorf("niSessionPort with one session ended: %q %v, want %s", got, err, want)
	}
	// nor is a session the parent ended while the child's end stays held (CLOSE_WAIT: its socket still listed)
	third := dial()
	defer third.Close()
	ended := stub.WaitSession(3, 5*time.Second)
	if ended == nil {
		t.Fatal("harness: the stub saw no third session")
	}
	_ = ended.Close()
	if !pollUntil(5*time.Second, ended.Closed) {
		t.Fatal("harness: the stub did not mark the session it ended")
	}
	if got, err := niSessionPort(stub, os.Getpid()); got != want || err != nil {
		t.Errorf("niSessionPort with a session the parent ended: %q %v, want %s", got, err, want)
	}
}

// testDashNormClosersFPeer: the port word PEER held to the fixture child's own end (niSide.peer, stream.Conn's local
// address) in niIngestRender, on C's recorded pairs of TestNodeInstancesAPI's rows that print the child's source.
func testDashNormClosersFPeer(t *testing.T) {
	rows := niRows()
	for _, name := range []string{"v2-ni-child", "v3-ni", "v3-ni-rfc3339", "v2-ni-long", "v3-ni-window"} {
		r, ok := niRecordedPairs[name]
		if !ok {
			t.Fatalf("harness: no recorded pair %s", name)
		}
		var peers [2]string
		for i := range peers {
			m := niPortRe.FindAllStringSubmatch(r.body[i], -1)
			for _, g := range m {
				if g[1] == "remote" {
					peers[i] = g[3]
				}
			}
			if peers[i] == "" {
				t.Fatalf("harness: %s: side %d's answer names no source", name, i)
			}
		}
		sides := r.sides
		for i := range sides {
			sides[i].peer = peers[i]
		}
		if got := niJudged(t, nodeInstancesFamily(sides), rows[name].guard, r, r.body[0], r.body[1]); got != nil {
			t.Errorf("%s: C's pair with the child's own ports: %q", name, got)
		}
		swap := func(i int, port string) string {
			return strings.Replace(r.body[i], `]:`+peers[i]+`"`, `]:`+port+`"`, 1)
		}
		for what, c := range map[string]string{"the port's bytes swapped": swap(1, fnsSwapped(t, peers[1])),
			"the oracle's child's port": swap(1, peers[0])} {
			if c == r.body[1] {
				t.Fatalf("harness: %s, %s changed nothing", name, what)
			}
			if got := niJudged(t, nodeInstancesFamily(sides), rows[name].guard, r, r.body[0], c); len(got) == 0 {
				t.Errorf("%s, %s: no difference", name, what)
			}
			// the general rule, where the harness has no connection of the child: it takes them
			if got := niJudged(t, nodeInstancesFamily(r.sides), rows[name].guard, r, r.body[0], c); got != nil {
				t.Errorf("%s, %s, by the general rule: %q", name, what, got)
			}
		}
		// the oracle's own port is held too
		if got := niJudged(t, nodeInstancesFamily(sides), rows[name].guard, r, swap(0, fnsSwapped(t, peers[0])),
			r.body[1]); len(got) == 0 {
			t.Errorf("%s: the oracle's child's port swapped: no problem", name)
		}
	}
	// niIsPeer's two rules
	side := niSide{listen: "100", peer: "300"}
	for port, want := range map[string]bool{"300": true, "301": false, "100": false, "0300": false} {
		if got := side.niIsPeer(port); got != want {
			t.Errorf("niIsPeer(%s) with the child's port = %t, want %t", port, got, want)
		}
	}
	side.peer = ""
	for port, want := range map[string]bool{"300": true, "65535": true, "65536": false, "0": false, "100": false,
		"0300": false} {
		if got := side.niIsPeer(port); got != want {
			t.Errorf("niIsPeer(%s) without the child's port = %t, want %t", port, got, want)
		}
	}
}

// fcCase is a named wrong candidate of a row of fcNIStream: the candidate's recorded answer with the value at path
// written (or the member dropped, with the next_in that goes with a next_check), and where the row must show it.
type fcCase struct {
	key    string
	path   []string
	value  string
	drop   bool
	want   string
	prefix bool // want is the start of every path reported (a `<layout>` besides), not the one path
}

// testDashNormClosersFNIStream: the rows of TestNodeInstancesStream's pairs tls, zip, reset and never2 on C's recorded
// pairs (fcNIStream): the oracle's guard holds C's answer, C's pair shows no difference, each named wrong candidate is
// reported where it is wrong, and a wrong oracle is refused by its guard.
func testDashNormClosersFNIStream(t *testing.T) {
	_, st, dst, par, where := niStreamPaths()
	for key := range fcNIStream {
		rec := fcNIStream[key]
		row := niStreamRowOf(t, rec, key)
		v, err := ParseJSON(row.fam.normalise(0, row.fam.v2Clock(row.req.target, rec.flight[0]), rec.flight[0],
			[]byte(rec.body[0])))
		if err != nil {
			t.Fatalf("%s: %v", key, err)
		}
		if err := row.req.guard(v); err != nil {
			t.Errorf("%s: the guard refuses C's answer: %v", key, err)
		}
		if got := niStreamNormDiffs(t, row.fam, row.req.target, rec.flight, rec.body[0], rec.body[1]); got != "" {
			t.Errorf("%s: C's pair differs at %q", key, got)
		}
	}
	second := func(key string) string { return fcNIStream[key].second }
	kept := func(counter string) string {
		return niStreamValue(t, fcNIStream["tls/denied/v3-ni"].body[1], dst("traffic", counter)...)
	}
	cases := map[string]fcCase{
		// tls
		"tls connected: the local end without :SSL": {key: "tls/connected/v3-ni", path: dst("local"),
			value: `"[::1]:` + fcNIStream["tls/connected/v3-ni"].sides[1].socket + `"`, want: where(dst("local"))},
		"tls connected: the remote end on IPv4": {key: "tls/connected/v3-ni", path: dst("remote"),
			value: `"[127.0.0.1]:` + fcNIStream["tls/connected/v3-ni"].sides[1].stub + `:SSL"`, want: where(dst("remote"))},
		"tls connected: the parent's destination without :SSL": {key: "tls/connected/v3-ni", path: par("destination"),
			value: `"[::1]:` + fcNIStream["tls/connected/v3-ni"].sides[1].stub + `"`, want: where(par("destination"))},
		"tls denied: the TLS flag kept after the disconnect (m48)": {key: "tls/denied/v3-ni", path: dst("local"),
			value: `"[not connected]:0:SSL"`, want: where(dst("local"))},
		// zip
		"zip: compression off": {key: "zip/compressed/v3-ni", path: dst("traffic", "compression"), value: "false",
			want: where(dst("traffic", "compression"))},
		"zip: the negotiated names without the compressions": {key: "zip/compressed/v3-ni", path: dst("capabilities"),
			value: niStreamNegotiated, want: where(dst("capabilities")), prefix: true},
		"zip: online": {key: "zip/compressed/v3-ni", path: st("status"), value: `"online"`, want: where(st("status"))},
		"zip: a chart fewer waiting": {key: "zip/compressed/v3-ni", path: st("replication", "instances"), value: "12",
			want: where(st("replication", "instances"))},
		"zip: no definitions sent": {key: "zip/compressed/v3-ni", path: dst("traffic", "metadata"), value: "0",
			want: where(dst("traffic", "metadata"))},
		// reset
		"reset: the parent's reason the handshake's": {key: "reset/reset/v3-ni", path: par("last_handshake"),
			value: `"SOCKET CONNECTED"`, want: where(par("last_handshake"))},
		"reset: the parent not postponed": {key: "reset/reset/v3-ni", path: par("next_check"), drop: true,
			want: where(par()), prefix: true},
		"reset: the reconnect counted": {key: "reset/reset/v3-ni", path: st("id"), value: "3", want: where(st("id"))},
		"reset: the ends kept": {key: "reset/reset/v3-ni", path: dst("remote"),
			value: `"[127.0.0.1]:` + fcNIStream["reset/reset/v3-ni"].sides[1].stub + `"`, want: where(dst("remote"))},
		// the traffic not zeroed at the reconnect: a first connection's bytes, as C keeps them after a refusal (the tls
		// pair's at denied); C's reset read 0 for both
		"reset: the data bytes kept from the first connection": {key: "reset/reset/v3-ni", path: dst("traffic", "data"),
			value: kept("data"), want: where(dst("traffic", "data"))},
		"reset: the replication bytes kept from the first connection": {key: "reset/reset/v3-ni",
			path: dst("traffic", "replication"), value: kept("replication"), want: where(dst("traffic", "replication"))},
		// never2
		"never2: the second parent left out": {key: "never2/never/v3-ni", path: dst("parents"),
			value: "[" + niStreamValue(t, fcNIStream["never2/never/v3-ni"].body[1], par()...) + "]",
			want:  where(dst("parents")), prefix: true},
	}
	for name, c := range cases {
		rec := fcNIStream[c.key]
		row := niStreamRowOf(t, rec, c.key)
		candidate := rec.body[1]
		if c.drop {
			candidate = niStreamEdit(t, niStreamEdit(t, candidate, c.path, "", "", true), par("next_in"), "", "", true)
		} else {
			candidate = niStreamEdit(t, candidate, c.path, c.value, "", false)
		}
		got := niStreamNormDiffs(t, row.fam, row.req.target, rec.flight, rec.body[0], candidate)
		ok := got == c.want
		if c.prefix {
			ok = got != ""
			for _, p := range strings.Fields(got) {
				ok = ok && (strings.HasPrefix(p, c.want) || p == "<layout>")
			}
		}
		if !ok {
			t.Errorf("%s: differences at %q, want %q (prefix %t)", name, got, c.want, c.prefix)
		}
		// the same mistake on the oracle's side: its guard refuses it
		oracle := rec.body[0]
		if c.drop {
			oracle = niStreamEdit(t, niStreamEdit(t, oracle, c.path, "", "", true), par("next_in"), "", "", true)
		} else {
			oracle = niStreamEdit(t, oracle, c.path, c.value, "", false)
		}
		v, err := ParseJSON(row.fam.normalise(0, row.fam.v2Clock(row.req.target, rec.flight[0]), rec.flight[0],
			[]byte(oracle)))
		if err != nil || row.req.guard(v) == nil {
			t.Errorf("%s on the oracle: its guard takes it (%v)", name, err)
		}
	}
	// a reset's reason: C's other text of a reset, in both places, is no difference (SA-F2's pass c1: one C side read
	// each); a failed read's reason, or the parent's reason apart from the host's, is reported where it is, and the
	// oracle's guard refuses it
	rk := "reset/reset/v3-ni"
	rrec := fcNIStream[rk]
	rrow := niStreamRowOf(t, rrec, rk)
	both := func(b, text string) string {
		return niStreamEdit(t, niStreamEdit(t, b, st("reason"), text, "", false), par("last_handshake"), text, "", false)
	}
	closedBy, readFailed := `"DISCONNECTED SOCKET CLOSED BY REMOTE END"`, `"DISCONNECTED SOCKET READ FAILED"`
	for name, c := range map[string]struct {
		candidate func(string) string
		want      []string
	}{
		"C's other text of a reset": {func(b string) string { return both(b, closedBy) }, nil},
		"a failed read's reason": {func(b string) string { return both(b, readFailed) },
			[]string{where(st("reason")), where(par("last_handshake"))}},
		"the parent's reason the other text of a reset": {func(b string) string {
			return niStreamEdit(t, b, par("last_handshake"), closedBy, "", false)
		}, []string{where(par("last_handshake"))}},
		"the host's reason the other text of a reset": {func(b string) string {
			return niStreamEdit(t, b, st("reason"), closedBy, "", false)
		}, []string{where(par("last_handshake"))}},
	} {
		got := strings.Fields(niStreamNormDiffs(t, rrow.fam, rrow.req.target, rrec.flight, rrec.body[0],
			c.candidate(rrec.body[1])))
		slices.Sort(got)
		want := slices.Clone(c.want)
		slices.Sort(want)
		if !slices.Equal(got, want) {
			t.Errorf("reset, %s: differences at %q, want %q", name, got, want)
		}
		v, err := ParseJSON(rrow.fam.normalise(0, rrow.fam.v2Clock(rrow.req.target, rrec.flight[0]), rrec.flight[0],
			[]byte(c.candidate(rrec.body[0]))))
		if err != nil {
			t.Fatalf("reset, %s on the oracle: %v", name, err)
		}
		if refused := rrow.req.guard(v); (refused == nil) != (c.want == nil) {
			t.Errorf("reset, %s on the oracle: its guard says %v, want a refusal %t", name, refused, c.want != nil)
		}
	}
	// the reset stage's windows are apart: the connection's second before the disconnect's
	for i, s := range fcNIStream["reset/reset/v3-ni"].sides {
		if !s.young || s.closed[0] <= s.connected[1] {
			t.Errorf("reset, side %d: the disconnect %v is not after the connection %v (young %t)", i, s.closed,
				s.connected, s.young)
		}
	}
	// never2's second parent is the stub r.second names
	if s := second("never2/never/v3-ni"); s == "" || s == fcNIStream["never2/never/v3-ni"].sides[0].stub {
		t.Errorf("never2: the second stub's port %q", second("never2/never/v3-ni"))
	}
}

// testDashNormClosersFWords: the render's new words on hand-made streams: a young connection's definitions COUNT and
// its data and replication by the SENT rule, the compressed definitions SENT, the functions' bytes as written; the wire
// of each pair; the stage waits.
func testDashNormClosersFWords(t *testing.T) {
	traffic := func(compression bool, side niStreamSide) map[string]string {
		v, err := ParseJSON([]byte(fmt.Sprintf(`{"destination":{"traffic":{"compression":%t,"data":5,"metadata":7,`+
			`"functions":9,"replication":0}}}`, compression)))
		if err != nil {
			t.Fatal(err)
		}
		return niStreamWords(side, v, 0, false, [2]int64{}, "")
	}
	for name, c := range map[string]struct {
		compression bool
		side        niStreamSide
		want        map[string]string
	}{
		"plain": {false, niStreamSide{}, map[string]string{"destination.traffic.data": `"SENT"`}},
		"compressed": {true, niStreamSide{compressed: true}, map[string]string{"destination.traffic.data": `"SENT"`,
			"destination.traffic.metadata": `"SENT"`}},
		// a candidate that says it compresses on a plain pair keeps its definitions' bytes compared
		"said compressed": {true, niStreamSide{}, map[string]string{"destination.traffic.data": `"SENT"`}},
	} {
		if got := traffic(c.compression, c.side); !maps.Equal(got, c.want) {
			t.Errorf("niStreamWords, %s: %v, want %v", name, got, c.want)
		}
	}
	// a young connection (reset): its definitions' bytes COUNT whatever they hold, its data and replication answers
	// SENT above 0 and compared at 0 (C's reset read 0 for both on every side recorded), the functions' as written
	for name, c := range map[string]struct {
		counts string
		want   map[string]string
	}{
		"C's reset": {`"data":0,"metadata":7437,"functions":0,"replication":0`,
			map[string]string{"destination.traffic.metadata": `"COUNT"`}},
		"no definitions yet": {`"data":0,"metadata":0,"functions":0,"replication":0`,
			map[string]string{"destination.traffic.metadata": `"COUNT"`}},
		"data kept": {`"data":14573,"metadata":7437,"functions":0,"replication":0`,
			map[string]string{"destination.traffic.data": `"SENT"`, "destination.traffic.metadata": `"COUNT"`}},
		"replication kept": {`"data":0,"metadata":7437,"functions":0,"replication":5207`,
			map[string]string{"destination.traffic.replication": `"SENT"`, "destination.traffic.metadata": `"COUNT"`}},
		"a function's answer": {`"data":0,"metadata":7437,"functions":9,"replication":0`,
			map[string]string{"destination.traffic.metadata": `"COUNT"`}},
	} {
		v, err := ParseJSON([]byte(`{"destination":{"traffic":{"compression":false,` + c.counts + `}}}`))
		if err != nil {
			t.Fatal(err)
		}
		if got := niStreamWords(niStreamSide{young: true}, v, 0, false, [2]int64{}, ""); !maps.Equal(got, c.want) {
			t.Errorf("niStreamWords, young, %s: %v, want %v", name, got, c.want)
		}
	}
	// a reset's reason: RESET on a young connection for C's two texts, the parent's where it is the host's
	reasons := func(young bool, reason, parent string) map[string]string {
		v, err := ParseJSON([]byte(fmt.Sprintf(`{"reason":%q,"destination":{"parents":[{"last_handshake":%q}]}}`,
			reason, parent)))
		if err != nil {
			t.Fatal(err)
		}
		return niStreamWords(niStreamSide{young: young}, v, 0, false, [2]int64{}, "")
	}
	reset := map[string]string{"reason": `"RESET"`, "destination.parents.[0].last_handshake": `"RESET"`}
	poll, closedBy := niStreamResetReasons[0], niStreamResetReasons[1]
	failed := "DISCONNECTED SOCKET READ FAILED"
	for name, c := range map[string]struct {
		young          bool
		reason, parent string
		want           map[string]string
	}{
		"the poll's error":         {true, poll, poll, reset},
		"a reset read or written":  {true, closedBy, closedBy, reset},
		"a failed read":            {true, failed, failed, map[string]string{}},
		"the parent's another":     {true, poll, closedBy, map[string]string{"reason": `"RESET"`}},
		"a disconnect, not young":  {false, poll, poll, map[string]string{}},
		"the host refused, reset?": {true, "DENIED", poll, map[string]string{}},
	} {
		if got := reasons(c.young, c.reason, c.parent); !maps.Equal(got, c.want) {
			t.Errorf("niStreamWords, %s: %v, want %v", name, got, c.want)
		}
	}
	if want := []string{"DISCONNECT SOCKET ERROR", "DISCONNECTED SOCKET CLOSED BY REMOTE END"}; !slices.Equal(
		niStreamResetReasons, want) {
		t.Errorf("niStreamResetReasons: %q", niStreamResetReasons)
	}
	for pair, want := range map[string][3]string{"tls": {"[::1]", "[::1]", ":SSL"},
		"out": {"[127.0.0.1]", "127.0.0.1", ""},
		"zip": {"[127.0.0.1]", "127.0.0.1", ""}, "reset": {"[127.0.0.1]", "127.0.0.1", ""}} {
		end, host, ssl := niStreamWire(&niStreamRun{pair: pair})
		if got := [3]string{end, host, ssl}; got != want {
			t.Errorf("niStreamWire(%s) = %v, want %v", pair, got, want)
		}
	}
	stub, err := stream.StartParent(nil)
	if err != nil {
		t.Fatal(err)
	}
	defer stub.Close()
	for name, x := range map[string]struct {
		opts func() bool
	}{
		"tls": {func() bool {
			o := niStreamOutOptions("tls", stub)
			return o.StreamTo.Destination == stub.Addr()+":SSL" && strings.Contains(o.StreamTo.Extra,
				"ssl skip certificate verification = yes") && !o.StreamTo.Compression
		}},
		"zip": {func() bool {
			o := niStreamOutOptions("zip", stub)
			return o.StreamTo.Destination == stub.Addr() && o.StreamTo.Compression
		}},
		"out": {func() bool {
			o := niStreamOutOptions("out", stub)
			return o.StreamTo.Destination == stub.Addr() && !o.StreamTo.Compression &&
				o.StreamTo.Extra == "    reconnect delay = 5\n"
		}},
	} {
		if !x.opts() {
			t.Errorf("niStreamOutOptions(%s) is not the pair's", name)
		}
	}
	for s, want := range map[string][3]bool{
		`{"status":"online","id":1}`:      {true, false, true},
		`{"status":"online","id":2}`:      {true, false, false},
		`{"status":"online","id":0}`:      {false, false, false},
		`{"status":"replicating","id":1}`: {false, true, false},
		`{"status":"offline","id":1}`:     {false, false, false},
	} {
		v, err := ParseJSON([]byte(s))
		if err != nil {
			t.Fatal(err)
		}
		got := [3]bool{niStreamCountedAs("online")(v), niStreamCountedAs("replicating")(v), niStreamOnlineAs("1")(v)}
		if got != want {
			t.Errorf("%s: online and counted, replicating and counted, online as 1: %v, want %v", s, got, want)
		}
	}
}

// fcFnDiffsAt are fcFnDiffs of a stage of the stream pairs: its columns (fnStreamingOutVolatileOf) and its cells
// (fnStreamingOutMasksOf) masked, as fnStreamingOut asks fnStreamingRound to.
func fcFnDiffsAt(t *testing.T, o, c, stage string) []string {
	t.Helper()
	all := slices.Concat(fnStreamingVolatile, fnStreamingOutVolatileOf(stage))
	var docs [2]Value
	for i, resp := range []string{o, c} {
		v := fnsDoc(t, resp)
		masks := fnStreamingMasks(v, all)
		if more := fnStreamingOutMasksOf(stage); more != nil {
			masks = append(masks, more(v)...)
		}
		docs[i] = ApplyMasks(v, masks)
	}
	var out []string
	for _, d := range Compare(docs[0], docs[1]) {
		out = append(out, d.String())
	}
	return out
}

// fcFnDiffs are the paths where two recorded tables differ as fnStreamingRound compares them, with volatile masked
// besides fnStreamingVolatile.
func fcFnDiffs(t *testing.T, o, c string, volatile []string) []string {
	t.Helper()
	all := slices.Concat(fnStreamingVolatile, volatile)
	ov, cv := fnsDoc(t, o), fnsDoc(t, c)
	var out []string
	for _, d := range Compare(ApplyMasks(ov, fnStreamingMasks(ov, all)), ApplyMasks(cv, fnStreamingMasks(cv, all))) {
		out = append(out, d.String())
	}
	return out
}

// testDashNormClosersFFnOut: the Function at the stream pairs' new stages on C's recorded tables (fcFnOut): the
// stage's guard (fnStreamingOutGuardFor) holds C's oracle, both sides' tables hold fnStreamingFacts and
// fnStreamingOutFacts, C's pair is equal under the stage's masks (fnStreamingOutVolatileOf), each named wrong
// candidate is reported, and fnStreamingOutSideOf carries a run's windows and sockets.
func testDashNormClosersFFnOut(t *testing.T) {
	for key, x := range fcFnOut {
		pair, stage, _ := strings.Cut(key, "/")
		if err := fnStreamingOutGuardFor(pair, stage, x.sides[0].stub)(fnsDoc(t, x.rec[0].resp)); err != nil {
			t.Errorf("%s: the guard refuses C's table: %v", key, err)
		}
		for i := range x.rec {
			v := fnsDoc(t, x.rec[i].resp)
			if err := fnStreamingFacts(v, x.rec[i].port, x.rec[i].flight); err != nil {
				t.Errorf("%s, C's table %d: fnStreamingFacts: %v", key, i, err)
			}
			if err := fnStreamingOutFacts(v, x.sides[i]); err != nil {
				t.Errorf("%s, C's table %d: fnStreamingOutFacts: %v", key, i, err)
			}
		}
		if d := fcFnDiffsAt(t, x.rec[0].resp, x.rec[1].resp, stage); d != nil {
			t.Errorf("%s: C's tables differ: %q", key, d)
		}
	}
	// a stage the guard does not know is refused whatever the table, by the harness's own refusal
	for _, stage := range []string{"", "no-such-stage"} {
		err := fnStreamingOutGuardFor("out", stage, "1")(fnsDoc(t, fcFnOut["reset/reset"].rec[0].resp))
		if err == nil || !strings.Contains(err.Error(), "harness: no stage") {
			t.Errorf("fnStreamingOutGuardFor, the stage %q: %v, want the harness's refusal", stage, err)
		}
	}
	// the reset stage's counters are how far each connection got: another definitions' count is no difference
	rr := fcFnOut["reset/reset"]
	more := fnsInt(t, fnsCellOf(t, rr.rec[1].resp, 0, "OutTrafficMetadata")) + 8574
	if d := fcFnDiffsAt(t, rr.rec[0].resp, fnsWith(t, rr.rec[1].resp, fnsCell{0, "OutTrafficMetadata",
		strconv.FormatInt(more, 10)}, fnsCell{-1, "OutTrafficMetadata", strconv.FormatInt(more, 10)}), "reset"); d != nil {
		t.Errorf("reset: another definitions' count: %q", d)
	}
	// what C's two zip tables differ by, unmasked: the compressed definitions' bytes
	z := fcFnOut["zip/compressed"]
	if d := fcFnDiffs(t, z.rec[0].resp, z.rec[1].resp, fnStreamingOutVolatile); len(d) == 0 {
		t.Errorf("zip: C's two tables are equal without the metadata's mask: its mask is not C's need")
	}
	cell := func(resp, col string) int64 { return fnsInt(t, fnsCellOf(t, resp, 0, col)) }
	for name, c := range map[string]struct {
		key, col, value string
		guard           bool // the oracle's guard refuses it too
	}{
		"tls connected: plain":                {"tls/connected", "OutSSL", `"PLAIN"`, true},
		"tls connected: the remote on IPv4":   {"tls/connected", "OutRemoteIP", `"127.0.0.1"`, true},
		"tls denied: the TLS flag kept (m48)": {"tls/denied", "OutSSL", `"SSL"`, true},
		"zip: uncompressed":                   {"zip/compressed", "OutCompression", `"UNCOMPRESSED"`, true},
		"zip: online":                         {"zip/compressed", "OutStatus", `"online"`, true},
		"zip: no chart waiting":               {"zip/compressed", "OutReplInstances", "0", true},
		"reset: a failed read's reason":       {"reset/reset", "OutReason", `"DISCONNECTED SOCKET READ FAILED"`, false},
		"reset: the reconnect counted":        {"reset/reset", "OutConnections", "3", true},
		"reset: the parent's reason another": {"reset/reset", "OutAttemptHandshake", `["SOCKET CONNECTED"]`,
			false},
		"reset: the parent's reason the other reset's": {"reset/reset", "OutAttemptHandshake",
			`["DISCONNECTED SOCKET CLOSED BY REMOTE END"]`, false},
		"never2: one parent": {"never2/never", "OutAttemptHandshake", `["NEVER CONNECTED"]`, true},
		"banned: a warning":  {"ban/banned", "rowOptions", `{"severity":"warning"}`, true},
		"banned: the bans in the other order": {"ban/banned", "OutAttemptHandshake",
			`["ALREADY CONNECTED","LOCALHOST"]`, true},
		"banned: the reason NEVER CONNECTED": {"ban/banned", "OutReason", `"NEVER CONNECTED"`, true},
	} {
		x := fcFnOut[c.key]
		pair, stage, _ := strings.Cut(c.key, "/")
		if c.guard {
			if err := fnStreamingOutGuardFor(pair, stage, x.sides[0].stub)(fnsDoc(t, fnsWith(t, x.rec[0].resp,
				fnsCell{0, c.col, c.value}))); err == nil {
				t.Errorf("%s: the oracle's guard takes it", name)
			}
		}
		if d := fcFnDiffsAt(t, x.rec[0].resp, fnsWith(t, x.rec[1].resp, fnsCell{0, c.col, c.value}), stage); len(d) == 0 {
			t.Errorf("%s: the comparison shows no difference", name)
		}
	}
	// a reset's reason (fnStreamingResetMasks, fnStreamingResetPair): C's other text of a reset in both cells is no
	// difference (SA-F2's pass c1: one C side read each), and the facts take it on either side; another text, or the
	// parent's apart from the host's, the facts refuse on either side (the oracle's guard leaves both cells to them)
	rr0, rs0 := fcFnOut["reset/reset"].rec[0], fcFnOut["reset/reset"].sides[0]
	other := func(resp string) string {
		return fnsWith(t, resp, fnsCell{0, "OutReason", `"DISCONNECTED SOCKET CLOSED BY REMOTE END"`},
			fnsCell{0, "OutAttemptHandshake", `["DISCONNECTED SOCKET CLOSED BY REMOTE END"]`})
	}
	if d := fcFnDiffsAt(t, rr.rec[0].resp, other(rr.rec[1].resp), "reset"); d != nil {
		t.Errorf("reset: C's other text of a reset: %q", d)
	}
	if d := fcFnDiffs(t, rr.rec[0].resp, other(rr.rec[1].resp), fnStreamingOutVolatileOf("reset")); len(d) != 2 ||
		!strings.Contains(d[0], "CLOSED BY REMOTE END") || !strings.Contains(d[1], "CLOSED BY REMOTE END") {
		t.Errorf("reset: without the stage's cell masks the other text of a reset differs in its two cells: %q", d)
	}
	if err := fnStreamingOutGuardFor("reset", "reset", rs0.stub)(fnsDoc(t, other(rr0.resp))); err != nil {
		t.Errorf("reset: the oracle's guard refuses C's other text of a reset: %v", err)
	}
	for i, x := range [2]struct {
		resp string
		side fnStreamingOutSide
	}{{rr0.resp, rs0}, {rr.rec[1].resp, rr.sides[1]}} {
		if err := fnStreamingOutFacts(fnsDoc(t, other(x.resp)), x.side); err != nil {
			t.Errorf("reset, side %d: C's other text of a reset: %v", i, err)
		}
		for what, cells := range map[string][]fnsCell{
			"a failed read's reason": {{0, "OutReason", `"DISCONNECTED SOCKET READ FAILED"`},
				{0, "OutAttemptHandshake", `["DISCONNECTED SOCKET READ FAILED"]`}},
			"the parent's reason the other reset's": {{0, "OutAttemptHandshake",
				`["DISCONNECTED SOCKET CLOSED BY REMOTE END"]`}},
			"two parents' reasons": {{0, "OutAttemptHandshake", `["DISCONNECT SOCKET ERROR","DISCONNECT SOCKET ERROR"]`}},
			"no reason":            {{0, "OutReason", "null"}},
		} {
			err := fnStreamingOutFacts(fnsDoc(t, fnsWith(t, x.resp, cells...)), x.side)
			if err == nil || !strings.Contains(err.Error(), "not a reset's reason") {
				t.Errorf("reset, side %d, %s: fnStreamingOutFacts says %v", i, what, err)
			}
		}
	}
	// the masks: both cells of C's reset table, none where the text is no reset's or the parent's is apart, none at
	// another stage
	if got := fnStreamingResetMasks(fnsDoc(t, rr0.resp)); len(got) != 2 || got[0].Reason != "reset" {
		t.Errorf("fnStreamingResetMasks of C's reset table: %v", got)
	}
	for what, resp := range map[string]string{
		"a failed read": fnsWith(t, rr0.resp, fnsCell{0, "OutReason", `"DISCONNECTED SOCKET READ FAILED"`},
			fnsCell{0, "OutAttemptHandshake", `["DISCONNECTED SOCKET READ FAILED"]`}),
		"the parent's apart":  fnsWith(t, rr0.resp, fnsCell{0, "OutAttemptHandshake", `["SOCKET CONNECTED"]`}),
		"C's connected table": fcFnOut["tls/connected"].rec[0].resp,
		"C's denied table":    fcFnOut["tls/denied"].rec[0].resp,
	} {
		if got := fnStreamingResetMasks(fnsDoc(t, resp)); len(got) != 0 {
			t.Errorf("fnStreamingResetMasks, %s: %v", what, got)
		}
	}
	for stage, want := range map[string]bool{"reset": true, "connected": false, "denied": false, "never": false,
		"banned": false, "compressed": false} {
		if got := fnStreamingOutMasksOf(stage) != nil; got != want {
			t.Errorf("fnStreamingOutMasksOf(%s) set: %t, want %t", stage, got, want)
		}
	}
	// fnStreamingOut's comparison of the reset stage (fnStreamingOutAsk): its guard, columns, cells and facts
	ask := fnStreamingOutAsk(&niStreamRun{pair: "reset", Stage: "reset"}, fcFnOut["reset/reset"].sides)
	switch {
	case ask.target != "/api/v1/function?function=netdata-streaming":
		t.Errorf("fnStreamingOutAsk: target %q", ask.target)
	case ask.guard(fnsDoc(t, rr0.resp)) != nil || ask.guard(fnsDoc(t, fcFnOut["tls/denied"].rec[0].resp)) == nil:
		t.Errorf("fnStreamingOutAsk: not the reset stage's guard")
	case !slices.Equal(ask.volatile, fnStreamingOutVolatileOf("reset")):
		t.Errorf("fnStreamingOutAsk: volatile %v", ask.volatile)
	case ask.masks == nil || len(ask.masks(fnsDoc(t, rr0.resp))) != 2:
		t.Errorf("fnStreamingOutAsk: not the reset's masks")
	case ask.facts(1, fnsDoc(t, other(rr.rec[1].resp)), rr.rec[1].flight) != nil:
		t.Errorf("fnStreamingOutAsk: its facts refuse C's other text of a reset")
	case ask.facts(1, fnsDoc(t, fnsWith(t, rr.rec[1].resp, fnsCell{0, "OutReason", `"DENIED"`})), rr.rec[1].flight) == nil:
		t.Errorf("fnStreamingOutAsk: its facts take a refusal's reason at the reset")
	}
	if ask := fnStreamingOutAsk(&niStreamRun{pair: "out", Stage: "connected"},
		fcFnOut["reset/reset"].sides); ask.masks != nil {
		t.Errorf("fnStreamingOutAsk: masks at connected")
	}
	// the facts of the new stages, each on the candidate's table made wrong in one thing (its age kept consistent)
	r, rs := fcFnOut["reset/reset"].rec[1], fcFnOut["reset/reset"].sides[1]
	b, bs := fcFnOut["ban/banned"].rec[1], fcFnOut["ban/banned"].sides[1]
	n, ns := fcFnOut["never2/never"].rec[1], fcFnOut["never2/never"].sides[1]
	at := func(resp string, since int64) string { // the attempt moved to the milliseconds since, its age kept to the clock
		clock := cell(resp, "OutAttemptSince")/1000 + cell(resp, "OutAttemptAge")
		return fnsWith(t, resp, fnsCell{0, "OutAttemptSince", strconv.FormatInt(since, 10)},
			fnsCell{-1, "OutAttemptSince", strconv.FormatInt(since, 10)},
			fnsCell{0, "OutAttemptAge", strconv.FormatInt(clock-since/1000, 10)},
			fnsCell{-1, "OutAttemptAge", strconv.FormatInt(clock-since/1000, 10)})
	}
	outSince := func(resp string, ms int64) string {
		clock := cell(resp, "OutSince")/1000 + cell(resp, "OutAge")
		return fnsWith(t, resp, fnsCell{0, "OutSince", strconv.FormatInt(ms, 10)},
			fnsCell{-1, "OutSince", strconv.FormatInt(ms, 10)},
			fnsCell{0, "OutAge", strconv.FormatInt(clock-ms/1000, 10)},
			fnsCell{-1, "OutAge", strconv.FormatInt(clock-ms/1000, 10)})
	}
	pf, pd := strconv.FormatInt(bs.probed[0]*1000, 10), strconv.FormatInt(cell(b.resp, "dbTo")/1000-bs.probed[0], 10)
	probedFirst := fnsWith(t, b.resp, fnsCell{0, "dbFrom", pf}, fnsCell{-1, "dbFrom", pf}, fnsCell{0, "dbDuration", pd},
		fnsCell{-1, "dbDuration", pd})
	for name, c := range map[string]struct {
		resp string
		side fnStreamingOutSide
		want string
	}{
		"reset: the attempt at the connection": {at(r.resp, rs.connected[0]*1000), rs, "row 0: OutAttemptSince"},
		"reset: OutSince the disconnect's":     {outSince(r.resp, rs.closed[0]*1000), rs, "row 0: OutSince"},
		"reset: a sender's port kept": {fnsWith(t, r.resp, fnsCell{0, "OutLocalPort", "40000"}), rs,
			"row 0: OutLocalPort"},
		"banned: the probes before the launch": {at(b.resp, (bs.started[0]-1)*1000), bs, "row 0: OutAttemptSince"},
		"banned: OutSince a second after InSince": {outSince(b.resp, cell(b.resp, "InSince")+1000), bs,
			"is not InSince"},
		"banned: dbFrom the probes' second": {probedFirst, bs, "row 0: dbFrom"},
		"never2: the attempt after the parents' list was made": {at(n.resp, (ns.ready+1)*1000), ns,
			"row 0: OutAttemptSince"},
	} {
		if err := fnStreamingOutFacts(fnsDoc(t, c.resp), c.side); err == nil || !strings.Contains(err.Error(), c.want) {
			t.Errorf("fnStreamingOutFacts, %s: %v, want %q", name, err, c.want)
		}
	}
	// the stage's sides as runNIStream hands them to the Function: every window, the socket
	run := &niStreamRun{Stage: "connected", Sides: [2]niStreamSide{{niSide: niSide{listen: "1", started: [2]int64{10, 15}},
		stub: "2", ready: 12, connected: [2]int64{20, 21}, closed: [2]int64{30, 31}, probed: [2]int64{40, 41},
		socket: "3"}}}
	want := fnStreamingOutSide{stage: "connected", listen: "1", stub: "2", started: [2]int64{10, 15}, ready: 12,
		connected: [2]int64{20, 21}, closed: [2]int64{30, 31}, probed: [2]int64{40, 41}, socket: "3"}
	if got := fnStreamingOutSideOf(run, 0); got != want {
		t.Errorf("fnStreamingOutSideOf: %+v, want %+v", got, want)
	}
	if got := want.collected(); got != [2]int64{10, 16} {
		t.Errorf("the Function's first collection window: %v, want [10 16] (the connection's end less 5)", got)
	}
	want.connected = [2]int64{}
	if got := want.collected(); got != [2]int64{10, 36} {
		t.Errorf("the Function's first collection window of a banned side: %v, want [10 36] (the probes' end less 5)", got)
	}
	// the stages that wait past their attempt's second before the table is asked
	for stage, want := range map[string]int64{"banned": 41, "reset": 31, "connected": 0, "denied": 0, "never": 0,
		"compressed": 0} {
		s := fnStreamingOutSide{stage: stage, closed: [2]int64{30, 31}, probed: [2]int64{40, 41}}
		if got := fnStreamingOutAttemptEnd([2]fnStreamingOutSide{s, s}); got != want {
			t.Errorf("fnStreamingOutAttemptEnd(%s) = %d, want %d", stage, got, want)
		}
	}
	late := fnStreamingOutSide{stage: "banned", probed: [2]int64{40, 45}}
	if got := fnStreamingOutAttemptEnd([2]fnStreamingOutSide{{stage: "banned", probed: [2]int64{40, 41}}, late}); got != 45 {
		t.Errorf("fnStreamingOutAttemptEnd of two sides: %d, want the later side's 45", got)
	}
	for stage, want := range map[string][]string{"compressed": {"OutTrafficMetadata"}, "reset": {"OutTrafficMetadata"},
		"connected": nil, "denied": nil, "never": nil, "banned": nil} {
		if got := fnStreamingOutVolatileOf(stage); !slices.Equal(got, slices.Concat(fnStreamingOutVolatile, want)) {
			t.Errorf("fnStreamingOutVolatileOf(%s) = %v", stage, got)
		}
	}
}

// testDashNormClosersFDown: the call down the stream on C's recorded tables (fcFnDown, fcFnDownSides): the guard holds
// C's oracle, both sides' tables hold fnStreamingFacts, fnStreamingSince and fnStreamingDownFacts, C's pair is equal
// under the topology's masks; named wrong candidates, the masked columns' maxima among them; the round's `via` and
// `limit` on stub agents; and fnStreamingDownSocket on a socket this process holds.
func testDashNormClosersFDown(t *testing.T) {
	volatile := slices.Concat(fnStreamingOutVolatile, fnStreamingDownVolatile)
	for _, call := range []string{"call1", "call2"} {
		rec, sides := fcFnDown[call], fcFnDownSides[call]
		if err := fnStreamingDownGuard(sides[0].parent)(fnsDoc(t, rec[0].resp)); err != nil {
			t.Errorf("%s: the guard refuses C's table: %v", call, err)
		}
		for i := range rec {
			v := fnsDoc(t, rec[i].resp)
			if err := fnStreamingFacts(v, rec[i].port, rec[i].flight); err != nil {
				t.Errorf("%s, C's table %d: fnStreamingFacts: %v", call, i, err)
			}
			if err := fnStreamingSince(v, [2]int64{rec[i].launch, rec[i].launch + niStartSlack}, nil); err != nil {
				t.Errorf("%s, C's table %d: fnStreamingSince: %v", call, i, err)
			}
			if err := fnStreamingDownFacts(v, sides[i]); err != nil {
				t.Errorf("%s, C's table %d: fnStreamingDownFacts: %v", call, i, err)
			}
		}
		if d := fcFnDiffs(t, rec[0].resp, rec[1].resp, volatile); d != nil {
			t.Errorf("%s: C's tables differ: %q", call, d)
		}
	}
	if fcFnDownSides["call1"][0].answered || !fcFnDownSides["call2"][0].answered {
		t.Errorf("harness: the recorded sides do not say which call follows an answer")
	}
	c1, s1 := fcFnDown["call1"][1], fcFnDownSides["call1"][1]
	c2, s2 := fcFnDown["call2"][1], fcFnDownSides["call2"][1]
	other := fcFnDownSides["call1"][0].parent
	// both of a column's cell and maximum written n
	both := func(resp, col, n string) string { return fnsWith(t, resp, fnsCell{0, col, n}, fnsCell{-1, col, n}) }
	since := fnsInt(t, fnsCellOf(t, c1.resp, 0, "OutSince"))
	clock := since/1000 + fnsInt(t, fnsCellOf(t, c1.resp, 0, "OutAge"))
	early := strconv.FormatInt((s1.connected[0]-1)*1000, 10)
	earlyAge := strconv.FormatInt(clock-s1.connected[0]+1, 10)
	cell := func(resp, col string) int64 { return fnsInt(t, fnsCellOf(t, resp, 0, col)) }
	// the attempt at the millisecond after the second sec, its age kept to the table's clock (cells and maxima)
	attemptAt := func(resp string, sec int64) string {
		at := cell(resp, "OutAttemptSince")/1000 + cell(resp, "OutAttemptAge")
		return both(both(resp, "OutAttemptSince", strconv.FormatInt(sec*1000+1, 10)), "OutAttemptAge",
			strconv.FormatInt(at-sec, 10))
	}
	c0, s0 := fcFnDown["call1"][0], fcFnDownSides["call1"][0]
	plusOne := func(resp, col string) string { return both(resp, col, strconv.FormatInt(cell(resp, col)+1, 10)) }
	noMax := func(resp, col string) string {
		head, body, _ := strings.Cut(resp, "\r\n\r\n")
		return fnsLength(head, niStreamEdit(t, body, []string{"columns", col, "max"}, "", "", true))
	}
	for name, x := range map[string]struct {
		resp string
		side fnStreamingDownSide
		want string
	}{
		"the other side's parent": {both(c1.resp, "OutRemotePort", other), s1, "row 0: OutRemotePort"},
		"the port's bytes swapped": {fnsWith(t, c1.resp, fnsCell{0, "OutLocalPort", fnsSwapped(t, s1.socket)}), s1,
			"row 0: OutLocalPort"},
		"a function's answer sent before the first call": {both(c1.resp, "OutTrafficFunctions", "17"), s1,
			"row 0: OutTrafficMetadata"},
		"the first call's answer not counted": {both(c2.resp, "OutTrafficFunctions", "0"), s2,
			"row 0: OutTrafficMetadata"},
		"no definitions sent": {both(c1.resp, "OutTrafficMetadata", "0"), s1, "row 0: OutTrafficMetadata"},
		"OutSince before the child's launch": {both(both(c1.resp, "OutSince", early), "OutAge", earlyAge), s1,
			"row 0: OutSince"},
		"the attempt before the child's launch": {attemptAt(c1.resp, s1.connected[0]-1), s1, "row 0: OutAttemptSince"},
		"the attempt after the child was seen connected": {attemptAt(c0.resp, s0.connected[1]+1), s0,
			"row 0: OutAttemptSince"},
		"the attempt's age not to the table's clock": {plusOne(c1.resp, "OutAttemptAge"), s1, "with its age"},
		"OutSince not a whole second":                {plusOne(c1.resp, "OutSince"), s1, "not a whole second"},
		"OutLocalPort with a max": {fnsReplace(t, c2.resp, `"OutLocalPort":{`, `"OutLocalPort":{`+"\n"+
			`            "max":1,`, 1), s2, "columns.OutLocalPort has a max"},
		"a negative data count": {both(c2.resp, "OutTrafficData", "-1"), s2,
			"row 0: OutTrafficData is -1, want a whole number"},
		"the data column without its max": {noMax(c2.resp, "OutTrafficData"), s2, "no columns.OutTrafficData.max"},
	} {
		if err := fnStreamingDownFacts(fnsDoc(t, x.resp), x.side); err == nil || !strings.Contains(err.Error(), x.want) {
			t.Errorf("fnStreamingDownFacts, %s: %v, want %q", name, err, x.want)
		}
	}
	// the masked columns' maxima (call2: every masked cell above 0), each one above its one cell
	for _, col := range []string{"OutRemotePort", "OutTrafficData", "OutTrafficMetadata", "OutTrafficReplication",
		"OutTrafficFunctions", "OutAttemptSince", "OutAttemptAge"} {
		up := fnsWith(t, c2.resp, fnsCell{-1, col, strconv.FormatInt(cell(c2.resp, col)+1, 10)})
		if err := fnStreamingDownFacts(fnsDoc(t, up), s2); err == nil || !strings.Contains(err.Error(),
			"columns."+col+".max is") {
			t.Errorf("fnStreamingDownFacts, the maximum of %s one above its cell: %v", col, err)
		}
	}
	// what the comparison hides and what it compares on this topology (call2: every masked cell above 0 on both sides)
	for col, masked := range map[string]bool{"OutRemotePort": true, "OutTrafficMetadata": true,
		"OutTrafficFunctions": true, "OutCompression": false, "OutCapabilities": false, "dbMetrics": false} {
		v := fnsCellOf(t, c2.resp, 0, col)
		changed := `"UNCOMPRESSED"`
		switch col {
		case "OutRemotePort", "OutTrafficMetadata", "OutTrafficFunctions", "dbMetrics":
			changed = strconv.FormatInt(fnsInt(t, v)+1, 10)
		case "OutCapabilities":
			changed = `["VCAPS"]`
		}
		cells := []fnsCell{{0, col, changed}}
		if col != "OutCompression" && col != "OutCapabilities" {
			cells = append(cells, fnsCell{-1, col, changed})
		}
		if d := fcFnDiffs(t, fcFnDown["call2"][0].resp, fnsWith(t, c2.resp, cells...), volatile); (d == nil) != masked {
			t.Errorf("down: %s changed: %q, want it masked: %t", col, d, masked)
		}
	}
	// the round asks each side through via, within limit: stub agents writing C's recorded call1 tables at the
	// second they answer in; the pair's own agents answer 404
	rec := fcFnDown["call1"]
	notFound := func(int64) string { return `{"status":404}` }
	pair := &Pair{Oracle: dashNormStub(t, stubAnswer{body: notFound, status: "404 Not Found"}.raw),
		Candidate: dashNormStub(t, stubAnswer{body: notFound, status: "404 Not Found"}.raw)}
	pair.Oracle.LaunchStartedAt = time.Unix(rec[0].launch, 0)
	pair.Candidate.LaunchStartedAt = time.Unix(rec[1].launch, 0)
	via := func(delay [2]time.Duration) [2]string {
		var out [2]string
		for i := range rec {
			c := fnsClockOf(t, rec[i].resp)
			out[i] = dashNormStub(t, stubAnswer{body: c.at, delay: delay[i]}.raw).Addr
		}
		return out
	}
	down := fcFnDownSides["call1"]
	ask := func(via [2]string, limit time.Duration) fnStreamingAsk {
		return fnStreamingAsk{target: "/host/x/api/v1/function?function=netdata-streaming",
			guard: fnStreamingDownGuard(down[0].parent), via: via, limit: limit,
			volatile: volatile, facts: func(i int, v Value, _ [2]int64) error { return fnStreamingDownFacts(v, down[i]) }}
	}
	if r := fnStreamingRound(pair, ask(via([2]time.Duration{}), time.Second)); r.fatal != "" || len(r.problems) > 0 {
		t.Errorf("the round through via: %q %q", r.fatal, r.problems)
	}
	if r := fnStreamingRound(pair, ask([2]string{}, time.Second)); !strings.Contains(r.fatal, "oracle: answered") {
		t.Errorf("the round without via asks the pair's own agents: %q", r.fatal)
	}
	slow := 300 * time.Millisecond
	if r := fnStreamingRound(pair, ask(via([2]time.Duration{0, slow}), 200*time.Millisecond)); r.fatal != "" ||
		!strings.Contains(strings.Join(r.problems, "\n"), "candidate: answered in") {
		t.Errorf("the round, a candidate slower than the limit: %q %q", r.fatal, r.problems)
	}
	if r := fnStreamingRound(pair, ask(via([2]time.Duration{slow, 0}), 200*time.Millisecond)); !strings.Contains(
		r.fatal, "oracle: answered in") {
		t.Errorf("the round, an oracle slower than the limit: %q", r.fatal)
	}
	// the round masks ask.masks' cells of each side's table: a candidate's OutCompression apart is no difference with
	// that cell masked, and one without
	plain := fnsWith(t, rec[1].resp, fnsCell{0, "OutCompression", `"UNCOMPRESSED"`})
	apart := [2]string{via([2]time.Duration{})[0],
		dashNormStub(t, stubAnswer{body: fnsClockOf(t, plain).at}.raw).Addr}
	compression := func(v Value) []Mask {
		return []Mask{{fmt.Sprintf("data.[0].[%d]", fnStreamingColumns(v)["OutCompression"]), "compression"}}
	}
	masked := ask(apart, time.Second)
	masked.masks = compression
	if r := fnStreamingRound(pair, masked); r.fatal != "" || len(r.problems) > 0 {
		t.Errorf("the round with the cell masked: %q %q", r.fatal, r.problems)
	}
	if r := fnStreamingRound(pair, ask(apart, time.Second)); r.fatal != "" ||
		!strings.Contains(strings.Join(r.problems, "\n"), `"COMPRESSED"`) {
		t.Errorf("the round without the cell masked: %q %q", r.fatal, r.problems)
	}
	// fnStreamingDownSocket: the one socket this process holds to a listener's port, and two refused
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	_, lport, _ := net.SplitHostPort(l.Addr().String())
	dial := func() net.Conn {
		c, err := net.Dial("tcp", l.Addr().String())
		if err != nil {
			t.Fatal(err)
		}
		return c
	}
	first := dial()
	defer first.Close()
	if got, err := fnStreamingDownSocket(os.Getpid(), lport); err != nil ||
		got != strconv.Itoa(first.LocalAddr().(*net.TCPAddr).Port) {
		t.Errorf("fnStreamingDownSocket of the one socket: %q %v", got, err)
	}
	second := dial()
	defer second.Close()
	if got, err := fnStreamingDownSocket(os.Getpid(), lport); err == nil {
		t.Errorf("fnStreamingDownSocket of two sockets: %q", got)
	}
	t.Run("settle", testDashNormClosersFDownSettle)
}

// testDashNormClosersFDownSettle: the down row's settle (SA-F4) on C's recorded readings of its four C children in
// probe p1 (fcFnDownSeries, fcFnDownBodies): fnStreamingDownStateOf reads C's answers as the series records them;
// fnStreamingDownComplete passes each C child no sooner than fnStreamingDownSteady after its first complete reading
// and with its final counts, and refuses named wrong readings (a child still growing, no data yet as in H41's pass
// c1, replicating, disconnected, a count changed or the data gone within the window, a window too short);
// fnStreamingDownSteady keeps its margin over the gaps C recorded; fnStreamingDownApart refuses the children apart;
// fnStreamingDownAsk, fnStreamingDownSettled and fnStreamingDownTogether on stub agents answering C's bodies.
func testDashNormClosersFDownSettle(t *testing.T) {
	type state = fnStreamingDownState
	stateOf := func(body string) (state, error) {
		v, err := ParseJSON([]byte(body))
		if err != nil {
			t.Fatalf("harness: %v", err)
		}
		return fnStreamingDownStateOf(v)
	}
	recorded := func(series string, ms int64) state {
		for _, r := range fcFnDownSeries[series] {
			if r.at.UnixMilli() == ms {
				return r.state
			}
		}
		t.Fatalf("harness: no reading of %s at %d", series, ms)
		return state{}
	}
	// C's three answers read as their series records them, and what each one is
	for name, want := range map[string]string{"young": "not every count stored is collected", "complete": "",
		"replicating": "the sender is replicating, want online"} {
		b := fcFnDownBodies[name]
		s, err := stateOf(b.body)
		if err != nil || s != recorded(b.series, b.at) {
			t.Errorf("fnStreamingDownStateOf, C's %s answer: %+v %v, want %+v", name, s, err, recorded(b.series, b.at))
		}
		if err := s.complete(); (err == nil) != (want == "") || (err != nil && !strings.Contains(err.Error(), want)) {
			t.Errorf("C's %s answer: complete() %v, want %q", name, err, want)
		}
	}
	at := func(keys ...string) []string { return niStreamAt(keys...) }
	edit := func(body string, path []string, value string) string {
		return niStreamEdit(t, body, path, value, "", false)
	}
	young, complete := fcFnDownBodies["young"].body, fcFnDownBodies["complete"].body
	// named wrong answers: complete() and fnStreamingDownStateOf refuse each
	noData := young
	for _, count := range []string{"metrics", "instances", "contexts"} {
		noData = edit(noData, at("ingest", count), niStreamValue(t, young, at("db", count)...))
	}
	for name, x := range map[string]struct{ body, want string }{
		"no data yet, every count stored collected (H41's pass c1)": {noData, "no collected data sent yet"},
		"complete counts, replicating": {edit(complete, at("stream", "status"), `"replicating"`),
			"the sender is replicating"},
		"disconnected, its bytes kept": {edit(complete, at("stream", "status"), `"offline"`), "the sender is offline"},
		"an instance not collected":    {edit(complete, at("ingest", "instances"), "13"), "not every count stored"},
		"no contexts": {edit(edit(complete, at("db", "contexts"), "0"), at("ingest", "contexts"), "0"),
			"a count is 0"},
		"a count written as a string": {edit(complete, at("db", "metrics"), `"73"`), `db.metrics is "73", want a count`},
		"a negative data count":       {edit(complete, at("stream", "destination", "traffic", "data"), "-1"), "data is -1"},
		"no data count": {niStreamEdit(t, complete, at("stream", "destination", "traffic", "data"), "", "", true),
			"no nodes.[0].instances.[0].stream.destination.traffic.data"},
		"the status a number": {edit(complete, at("stream", "status"), "1"), "stream.status is 1, want a string"},
	} {
		s, err := stateOf(x.body)
		if err == nil {
			err = s.complete()
		}
		if err == nil || !strings.Contains(err.Error(), x.want) {
			t.Errorf("%s: %v, want %q", name, err, x.want)
		}
	}
	// each C child: no reading complete before its counts are final, their last change soon after the data's first
	// bytes; the poll passes it fnStreamingDownSteady after its first complete reading (within a reading of 0.5 s),
	// with its final counts, and no sooner
	steady := fnStreamingDownSteady
	var gap time.Duration
	passed := map[string]int{}
	for key, series := range fcFnDownSeries {
		final := series[len(series)-1].state
		first := slices.IndexFunc(series, func(r fnStreamingDownRead) bool { return r.state.complete() == nil })
		data := slices.IndexFunc(series, func(r fnStreamingDownRead) bool { return r.state.data > 0 })
		changed := 0
		for k := 1; k < len(series); k++ {
			if series[k].state.db != series[k-1].state.db || series[k].state.collected != series[k-1].state.collected {
				changed = k
			}
		}
		if first < 0 || data < 0 || series[first].state.db != final.db || changed > first {
			t.Fatalf("harness: %s: first complete %d, data %d, last change %d", key, first, data, changed)
		}
		gap = max(gap, series[changed].at.Sub(series[data].at))
		pass := -1
		for k := range series {
			if fnStreamingDownComplete(series[:k+1], steady) == nil {
				pass = k
				break
			}
		}
		if pass < 0 {
			t.Errorf("%s: C's child never passes: %v", key, fnStreamingDownComplete(series, steady))
			continue
		}
		passed[key] = pass
		if span := series[pass].at.Sub(series[first].at); span < steady || span >= steady+time.Second ||
			series[pass].state.db != final.db {
			t.Errorf("%s: C's child passes %v after its first complete reading with %v, want %v after with %v", key,
				span, series[pass].state.db, steady, final.db)
		}
		// the window's edge: held exactly its span it passes, 1 ms more it does not
		span := series[pass].at.Sub(series[first].at)
		if err := fnStreamingDownComplete(series[:pass+1], span); err != nil {
			t.Errorf("%s: complete for exactly %v: %v", key, span, err)
		}
		if err := fnStreamingDownComplete(series[:pass+1], span+time.Millisecond); err == nil {
			t.Errorf("%s: complete for %v passes over %v", key, span, span+time.Millisecond)
		}
	}
	if 3*gap > steady {
		t.Errorf("fnStreamingDownSteady %v is under three times the longest gap C's children recorded between their data's "+
			"first bytes and their final counts, %v", steady, gap)
	}
	// named wrong series, each made from c1/0's: refused
	series, pass := fcFnDownSeries["c1/0"], passed["c1/0"]
	ok := series[:pass+1]
	first := slices.IndexFunc(series, func(r fnStreamingDownRead) bool { return r.state.complete() == nil })
	youngAt := slices.IndexFunc(series, func(r fnStreamingDownRead) bool {
		return r.at.UnixMilli() == fcFnDownBodies["young"].at
	})
	planted := func(st func(state) state) []fnStreamingDownRead {
		out := slices.Clone(ok)
		k := len(out) - 5 // 2 s before the last reading
		out[k].state = st(out[k].state)
		return out
	}
	for name, x := range map[string]struct {
		reads []fnStreamingDownRead
		want  string
	}{
		"no reading":                {nil, "no reading"},
		"cut at the young reading":  {series[:youngAt+1], "not every count stored is collected"},
		"cut at the first complete": {series[:first+1], "not every count stored is collected"},
		"complete for under steady": {series[first:pass], "complete for"},
		"apart counts in the window": {planted(func(s state) state {
			s.db, s.collected = [3]int64{67, 13, 12}, [3]int64{67, 13, 12}
			return s
		}), "counts [67 13 12] apart from the last reading's [73 14 13]"},
		"replicating in the window": {planted(func(s state) state { s.status = "replicating"; return s }),
			"the sender is replicating"},
		"the data gone in the window": {planted(func(s state) state { s.data = 0; return s }),
			"no collected data sent yet"},
	} {
		if err := fnStreamingDownComplete(x.reads, steady); err == nil || !strings.Contains(err.Error(), x.want) {
			t.Errorf("fnStreamingDownComplete, %s: %v, want %q", name, err, x.want)
		}
	}
	// the children alike, and apart (H41's pass c1: the oracle's 73/14/13, the candidate's 67/13/12; one count each)
	for _, run := range []string{"c1", "c2"} {
		o, c := fcFnDownSeries[run+"/0"], fcFnDownSeries[run+"/1"]
		if err := fnStreamingDownApart(o[len(o)-1].state, c[len(c)-1].state); err != nil {
			t.Errorf("%s: C's two children: %v", run, err)
		}
	}
	whole := recorded("c1/0", fcFnDownBodies["complete"].at)
	for name, db := range map[string][3]int64{"H41's pass c1": {67, 13, 12}, "a metric more": {74, 14, 13},
		"an instance more": {73, 15, 13}, "a context more": {73, 14, 14}} {
		c := whole
		c.db, c.collected = db, db
		if err := fnStreamingDownApart(whole, c); err == nil || !strings.Contains(err.Error(), "apart") {
			t.Errorf("fnStreamingDownApart, %s: %v", name, err)
		}
	}
	// on stub agents answering C's bodies: the request, the settle and the two children together
	answer := func(body string) string { return stubAnswer{body: func(int64) string { return body }}.raw() }
	served := func(body string) string { return dashNormStub(t, func() string { return answer(body) }).Addr }
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	asked := make(chan string, 1)
	go func() {
		c, err := l.Accept()
		if err != nil {
			return
		}
		defer c.Close()
		var req []byte
		buf := make([]byte, 4096)
		for !strings.Contains(string(req), "\r\n\r\n") {
			n, err := c.Read(buf)
			if err != nil {
				return
			}
			req = append(req, buf[:n]...)
		}
		line, _, _ := strings.Cut(string(req), "\r\n")
		asked <- line
		_, _ = c.Write([]byte(answer(complete)))
	}()
	s, err := fnStreamingDownAsk(l.Addr().String())
	line := ""
	select {
	case line = <-asked:
	case <-time.After(5 * time.Second):
	}
	if err != nil || s != whole || line != "GET /api/v3/node_instances?harness=wait HTTP/1.1" {
		t.Errorf("fnStreamingDownAsk: %+v %v, asked %q", s, err, line)
	}
	start := time.Now()
	if err := fnStreamingDownSettled(served(complete), 3*time.Second, 600*time.Millisecond); err != nil ||
		time.Since(start) < 600*time.Millisecond {
		t.Errorf("fnStreamingDownSettled on C's complete answer: %v after %v", err, time.Since(start))
	}
	if err := fnStreamingDownSettled(served(young), 800*time.Millisecond, 300*time.Millisecond); err == nil ||
		!strings.Contains(err.Error(), "not every count stored is collected") {
		t.Errorf("fnStreamingDownSettled on C's young answer: %v", err)
	}
	// young answers first, then complete ones: passes only steady after the first complete answer
	var n atomic.Int64
	var completeFrom atomic.Int64
	growing := dashNormStub(t, func() string {
		if n.Add(1) <= 4 {
			return answer(young)
		}
		completeFrom.CompareAndSwap(0, time.Now().UnixNano())
		return answer(complete)
	}).Addr
	if err := fnStreamingDownSettled(growing, 5*time.Second, 600*time.Millisecond); err != nil ||
		completeFrom.Load() == 0 || time.Since(time.Unix(0, completeFrom.Load())) < 600*time.Millisecond {
		t.Errorf("fnStreamingDownSettled on a child that grows: %v, %v after its first complete answer", err,
			time.Since(time.Unix(0, completeFrom.Load())))
	}
	sixtySeven := complete
	for _, count := range []string{"metrics", "instances", "contexts"} {
		v := niStreamValue(t, young, at("db", count)...)
		sixtySeven = edit(edit(sixtySeven, at("db", count), v), at("ingest", count), v)
	}
	gone, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	absent := gone.Addr().String()
	_ = gone.Close()
	for name, x := range map[string]struct {
		addr              [2]string
		oracle, candidate string
	}{
		"both complete and alike":      {[2]string{served(complete), served(complete)}, "", ""},
		"the candidate's child young":  {[2]string{served(complete), served(young)}, "", "not every count stored"},
		"the oracle's child young":     {[2]string{served(young), served(complete)}, "not every count stored", ""},
		"the candidate's child apart":  {[2]string{served(complete), served(sixtySeven)}, "", "apart"},
		"the candidate's child absent": {[2]string{served(complete), absent}, "", absent},
	} {
		o, c := fnStreamingDownTogether(x.addr, 600*time.Millisecond)
		for _, got := range []struct {
			side string
			err  error
			want string
		}{{"oracle", o, x.oracle}, {"candidate", c, x.candidate}} {
			if (got.err == nil) != (got.want == "") || (got.want != "" && !strings.Contains(fmt.Sprint(got.err), got.want)) {
				t.Errorf("fnStreamingDownTogether, %s: the %s's %v, want %q", name, got.side, got.err, got.want)
			}
		}
	}
}

// testDashNormClosersFTLS: a child received over TLS on C's recorded answers: TestNodeInstancesAPI/tls's v3-ni
// (fcNITLS: its guard holds C's oracle, C's pair shows nothing, the child's ends without `:SSL` (T5's m30) and its own
// port swapped are reported, on the oracle by its guard) and the table of TestFnNetdataStreaming/tls-child (fcFnTLS:
// the guard, the facts, the comparison; InSSL PLAIN and another InRemotePort refused).
func testDashNormClosersFTLS(t *testing.T) {
	r := fcNITLS
	fam := nodeInstancesFamily(r.sides)
	if got := niJudged(t, fam, niTLSChildRow.guard, r, r.body[0], r.body[1]); got != nil {
		t.Errorf("C's TLS pair: %q", got)
	}
	plain := func(i int) string { return strings.ReplaceAll(r.body[i], ":SSL\"", "\"") }
	swapped := func(i int) string {
		return strings.Replace(r.body[i], `]:`+r.sides[i].peer+`:SSL"`, `]:`+fnsSwapped(t, r.sides[i].peer)+`:SSL"`, 1)
	}
	for name, body := range map[string]func(int) string{"the ends without :SSL (m30)": plain,
		"the child's port swapped": swapped} {
		if body(1) == r.body[1] {
			t.Fatalf("harness: %s changed nothing", name)
		}
		if got := niJudged(t, fam, niTLSChildRow.guard, r, r.body[0], body(1)); len(got) == 0 {
			t.Errorf("%s: no difference", name)
		}
		got := niJudged(t, fam, niTLSChildRow.guard, r, body(0), r.body[1])
		if len(got) == 0 || !strings.HasPrefix(got[0], "guard: ") {
			t.Errorf("%s on the oracle: %q, want its guard's refusal", name, got)
		}
	}
	// the table
	if err := fnStreamingTLSGuard(fnsDoc(t, fcFnTLS[0].resp)); err != nil {
		t.Errorf("C's TLS table: the guard: %v", err)
	}
	for i, rec := range fcFnTLS {
		v := fnsDoc(t, rec.resp)
		for what, err := range map[string]error{"fnStreamingFacts": fnStreamingFacts(v, rec.port, rec.flight),
			"fnStreamingTLSFacts": fnStreamingTLSFacts(v, fcFnTLSPeers[i])} {
			if err != nil {
				t.Errorf("C's TLS table %d: %s: %v", i, what, err)
			}
		}
	}
	if d := fcFnDiffs(t, fcFnTLS[0].resp, fcFnTLS[1].resp, nil); d != nil {
		t.Errorf("C's TLS tables differ: %q", d)
	}
	plainTable := fnsWith(t, fcFnTLS[1].resp, fnsCell{1, "InSSL", `"PLAIN"`})
	if d := fcFnDiffs(t, fcFnTLS[0].resp, plainTable, nil); len(d) == 0 {
		t.Errorf("the TLS child's InSSL PLAIN (m30): no difference")
	}
	if err := fnStreamingTLSGuard(fnsDoc(t, fnsWith(t, fcFnTLS[0].resp, fnsCell{1, "InSSL", `"PLAIN"`}))); err == nil {
		t.Errorf("the oracle's TLS child PLAIN: the guard takes it")
	}
	if err := fnStreamingTLSFacts(fnsDoc(t, fcFnTLS[1].resp), fnsSwapped(t, fcFnTLSPeers[1])); err == nil {
		t.Errorf("fnStreamingTLSFacts takes another port than the child's")
	}
}

// testDashNormClosersFEph: gone/ephemeral on C's recorded tables (fcFnEph): the guard holds C's oracle, both hold the
// facts and the windows (the children's apart, each held to its own), C's pair is equal; a child left permanent (with
// or without its severity), localhost marked, an ephemeral child critical are refused; and the guard of `gone` itself
// refuses the ephemeral table.
func testDashNormClosersFEph(t *testing.T) {
	g := fnStreamingEphemeralGuard(fcFnEphBase)
	if err := g(fnsDoc(t, fcFnEph[0].resp)); err != nil {
		t.Errorf("C's ephemeral table: the guard: %v", err)
	}
	for i, rec := range fcFnEph {
		v := fnsDoc(t, rec.resp)
		if err := fnStreamingFacts(v, rec.port, rec.flight); err != nil {
			t.Errorf("C's ephemeral table %d: fnStreamingFacts: %v", i, err)
		}
		if err := fnStreamingSince(v, [2]int64{rec.launch, rec.launch + niStartSlack}, rec.left); err != nil {
			t.Errorf("C's ephemeral table %d: fnStreamingSince: %v", i, err)
		}
	}
	if d := fcFnDiffs(t, fcFnEph[0].resp, fcFnEph[1].resp, nil); d != nil {
		t.Errorf("C's ephemeral tables differ: %q", d)
	}
	// the children left a second apart on each side (R111 G-N1: the windows are disjoint) and each child's InSince is
	// held to its own window: the second child read as leaving in the first one's second is refused
	for i, rec := range fcFnEph {
		one, two, three := rec.left[childHost.Hostname], rec.left[child2Host.Hostname],
			rec.left[fnStreamingChild3.Hostname]
		if len(rec.left) != 3 || one[0] == 0 || one[1] >= two[0] || two[1] >= three[0] {
			t.Errorf("C's ephemeral table %d: the children's windows are not apart: %v", i, rec.left)
		}
		found := false
		for r := range 4 {
			if fnsCellOf(t, rec.resp, r, "Node") != strconv.Quote(child2Host.Hostname) {
				continue
			}
			found = true
			early := fnsWith(t, rec.resp, fnsCell{r, "InSince", strconv.FormatInt(one[0]*1000, 10)})
			if err := fnStreamingSince(fnsDoc(t, early), [2]int64{rec.launch, rec.launch + niStartSlack},
				rec.left); err == nil || !strings.Contains(err.Error(), "its disconnection") {
				t.Errorf("C's ephemeral table %d, the second child leaving in the first one's second: %v", i, err)
			}
		}
		if !found {
			t.Errorf("harness: C's ephemeral table %d has no row of %s", i, child2Host.Hostname)
		}
	}
	if err := fnStreamingGoneGuard(fcFnEphBase)(fnsDoc(t, fcFnEph[0].resp)); err == nil {
		t.Errorf("the gone guard takes the ephemeral table")
	}
	for name, cells := range map[string][]fnsCell{
		"a child left permanent":      {{2, "Ephemerality", `"permanent"`}, {2, "rowOptions", `{"severity":"critical"}`}},
		"a child unmarked, normal":    {{2, "Ephemerality", `"permanent"`}},
		"localhost marked":            {{0, "Ephemerality", `"ephemeral"`}},
		"an ephemeral child critical": {{3, "rowOptions", `{"severity":"critical"}`}},
	} {
		if err := g(fnsDoc(t, fnsWith(t, fcFnEph[0].resp, cells...))); err == nil {
			t.Errorf("%s: the guard takes it", name)
		}
		if d := fcFnDiffs(t, fcFnEph[0].resp, fnsWith(t, fcFnEph[1].resp, cells...), nil); len(d) == 0 {
			t.Errorf("%s: no difference", name)
		}
	}
}

// testDashNormClosersFLoad: the load check's judges: niStreamLoadAnswer on C's recorded answers (a node-instance
// answer with localhost's stream, a table of one row) and on wrong ones; niStreamLoadJudge's three bounds.
func testDashNormClosersFLoad(t *testing.T) {
	ni := "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n" + fcNIStream["reset/reset/v3-ni"].body[0]
	table := fcFnOut["reset/reset"].rec[0].resp
	ni503 := strings.Replace(ni, "200 OK", "503 Service Unavailable", 1)
	for name, c := range map[string]struct {
		target, answer string
		ok             bool
	}{
		"C's node instances":   {niStreamLoadTargets[0], ni, true},
		"C's table":            {niStreamLoadTargets[2], table, true},
		"a 503":                {niStreamLoadTargets[0], "HTTP/1.1 503 Service Unavailable\r\n\r\n{}", false},
		"a 503 with C's body":  {niStreamLoadTargets[0], ni503, false},
		"no stream":            {niStreamLoadTargets[0], "HTTP/1.1 200 OK\r\n\r\n" + fcNITLS.body[0], false},
		"a table of two rows":  {niStreamLoadTargets[2], fcFnTLS[0].resp, false},
		"not JSON":             {niStreamLoadTargets[2], "HTTP/1.1 200 OK\r\n\r\n{", false},
		"a table that is none": {niStreamLoadTargets[2], ni, false},
	} {
		if err := niStreamLoadAnswer(c.target, []byte(c.answer)); (err == nil) != c.ok {
			t.Errorf("niStreamLoadAnswer, %s: %v, want ok %t", name, err, c.ok)
		}
	}
	ok := niStreamLoadStat{answers: niStreamLoadAnswers, slowest: niStreamLoadLimit}
	for name, c := range map[string]struct {
		st   niStreamLoadStat
		want string
	}{
		"at the bounds": {ok, ""},
		"one bad":       {niStreamLoadStat{answers: 1000, bad: 1, first: "x"}, "1 of 1000 answers"},
		"one too slow":  {niStreamLoadStat{answers: 1000, slowest: niStreamLoadLimit + time.Millisecond}, "an answer took"},
		"too few":       {niStreamLoadStat{answers: niStreamLoadAnswers - 1}, "answers in"},
	} {
		err := niStreamLoadJudge("x", c.st)
		if (c.want == "") != (err == nil) || (err != nil && !strings.Contains(err.Error(), c.want)) {
			t.Errorf("niStreamLoadJudge, %s: %v, want %q", name, err, c.want)
		}
	}
	if got := niStreamLoadReplay(stream.ReplayEvent{Chart: "a.b"}); len(got) != 1 ||
		!strings.HasPrefix(got[0], `REPLAY_CHART "a.b" "false" `) {
		t.Errorf("niStreamLoadReplay: %q", got)
	}
}

// testDashNormClosersFPost: the POST of streaming-info: a POST of netdata-streaming by the admin with its JSON body,
// and C's table to it (SA-F's probe p1, fcPost) is its GET's: the same view (fnStreamingRender) on each side.
func testDashNormClosersFPost(t *testing.T) {
	req := string(fnBuiltinsStreamingPost().post())
	for _, want := range []string{"POST /api/v1/function?function=netdata-streaming HTTP/1.1\r\n",
		fnBuiltinsUsers[2].header, "Content-Type: application/json\r\n", "\r\n\r\n" + fnBuiltinsStreamingBody} {
		if !strings.Contains(req, want) {
			t.Errorf("the POST %q has no %q", req, want)
		}
	}
	for i := range 2 {
		get, post := fcPost["get"][i], fcPost["json"][i]
		gv, gp := fnStreamingRender([]byte(get.resp), fnsSideOf(get), fnStreamingStandalone)
		pv, pp := fnStreamingRender([]byte(post.resp), fnsSideOf(post), fnStreamingStandalone)
		if gp != nil || pp != nil {
			t.Errorf("side %d: C's GET and POST views: %q %q", i, gp, pp)
		}
		if gv != pv {
			t.Errorf("side %d: C's POST view is not its GET's:\n%s\n%s", i, gv, pv)
		}
	}
}
