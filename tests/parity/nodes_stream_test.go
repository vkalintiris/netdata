// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"crypto/tls"
	"fmt"
	"net"
	"os"
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

// The node instances of agents that stream out (check `api.v2-node-instances-stream`, milestone 10 commit 11, D241 F3):
// `/api/v2|v3/node_instances` of an agent whose localhost has a sender, so that its instance prints `stream`
// (rrdhost_sender_to_json(), database/contexts/api_v2_contexts.c:381-433) with the sender's parents
// (rrdhost_stream_parents_to_json(), streaming/stream-parents.c:174-225). The pairs (runNIStream) are an agent whose
// sender never started (`never`, and `never-tz` in another time zone; `never2` with two parents), two agents streaming
// to one scripted parent, first connected, then refused (`out`; `tls` the same over TLS on [::1]), streaming to one
// that takes compression (`zip`, one stage: `compressed`) and with their second connection reset (`reset`), and two
// agents whose two parents are both banned at their probes (`ban`). The `load` subtest asks the rows back to back while
// a scripted parent resets every session (niStreamLoad).

// jsonRewrite is the JSON text b with each value visit handles replaced: visit gets every value's path (object keys
// and `[i]`) and bytes, outermost first, and hands back what to write instead; a value it does not handle is walked
// into when it is an object or an array (jsonItems), and kept as it is otherwise. The bytes between the values (the
// layout) are kept.
func jsonRewrite(b []byte, path []string, visit func(path []string, value []byte) ([]byte, bool)) []byte {
	if out, ok := visit(path, b); ok {
		return out
	}
	trimmed := bytes.TrimLeft(b, " \t\r\n")
	if len(trimmed) == 0 || (trimmed[0] != '{' && trimmed[0] != '[') {
		return b
	}
	items, err := jsonItems(b)
	if err != nil {
		return b
	}
	var out []byte
	last := 0
	for i, it := range items {
		step := it.key
		if trimmed[0] == '[' {
			step = "[" + strconv.Itoa(i) + "]"
		}
		out = append(out, b[last:it.start]...)
		out = append(out, jsonRewrite(b[it.start:it.end], append(slices.Clone(path), step), visit)...)
		last = it.end
	}
	return append(out, b[last:]...)
}

// niIsStream tells whether path names an instance's `stream` in a node-instance answer.
func niIsStream(path []string) bool {
	return len(path) == 5 && path[0] == "nodes" && path[2] == "instances" && path[4] == "stream"
}

// niStreamToken is the text that stands for the n-th `stream` object of a body while niOutsideStream's render runs:
// a string of no digit, which no render pattern matches.
func niStreamToken(n int) []byte {
	return []byte(`"~stream ` + strings.Repeat("|", n+1) + `~"`)
}

// niOutsideStream applies render to a node-instance answer outside its instances' `stream` objects, and stream to
// each of those (its text, with the whole body it came from and its path there): render's patterns are not
// path-aware (R108 "The harness as it is now" point 1: niIngestRender's port and age patterns would match a sender's
// ends and times). Each object is cut out for a token (niStreamToken) while render runs, then put back as stream
// wrote it.
func niOutsideStream(render func(i int, flight [2]int64, body []byte) []byte,
	stream func(i int, flight [2]int64, body []byte, path []string, obj []byte) []byte) func(i int, flight [2]int64,
	body []byte) []byte {
	return func(i int, flight [2]int64, body []byte) []byte {
		type object struct {
			path []string
			text []byte
		}
		var cut []object
		whole := body
		body = jsonRewrite(body, nil, func(path []string, value []byte) ([]byte, bool) {
			if !niIsStream(path) {
				return nil, false
			}
			cut = append(cut, object{path, value})
			return niStreamToken(len(cut) - 1), true
		})
		if render != nil {
			body = render(i, flight, body)
		}
		for n, obj := range cut {
			body = bytes.Replace(body, niStreamToken(n), stream(i, flight, whole, obj.path, obj.text), 1)
		}
		return body
	}
}

// niDurationRe is one part of C's duration text (duration_snprintf(..., "us", true),
// libnetdata/parsers/duration.c:337-400, as buffer_json_member_add_duration_ut writes it, buffer.c:418-422): a count
// and one of the units it writes from microseconds up to days.
var niDurationRe = regexp.MustCompile(`^([1-9][0-9]*)(us|ms|s|m|h|d)$`)

// niDurationUnits are the microseconds of each unit niDurationRe names.
var niDurationUnits = map[string]int64{"us": 1, "ms": 1e3, "s": 1e6, "m": 60e6, "h": 3600e6, "d": 86400e6}

// niDuration reads C's duration text in microseconds: `off` for 0, else the parts from the largest unit down, each
// smaller than the one before it, separated by one space (duration.c:337-400).
func niDuration(text string) (int64, bool) {
	if text == "off" {
		return 0, true
	}
	total, last := int64(0), int64(0)
	for i, part := range strings.Split(text, " ") {
		g := niDurationRe.FindStringSubmatch(part)
		if g == nil {
			return 0, false
		}
		n, err := strconv.ParseInt(g[1], 10, 64)
		unit := niDurationUnits[g[2]]
		if err != nil || (i > 0 && unit >= last) {
			return 0, false
		}
		total, last = total+n*unit, unit
	}
	return total, true
}

// niLocalRe is a parent's time as C writes it (buffer_json_member_add_datetime_rfc3339(..., false),
// rfc3339_datetime_ut(.., 2, false), libnetdata/datetime/rfc3339.c:76-190): the local date and time, two fraction
// digits (truncated) only when the microseconds are not 0, then `Z` for an offset under a minute, else `+hh:mm` or
// `-hh:mm`.
var niLocalRe = regexp.MustCompile(`^([0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{2})?)` +
	`(Z|[+-][0-9]{2}:[0-9]{2})$`)

// niUTCRe is a time of the status as `options=rfc3339` prints it (buffer_json_member_add_time_t_formatted,
// libnetdata/buffer/buffer.h:1119-1128): UTC, whole seconds.
var niUTCRe = regexp.MustCompile(`^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$`)

// niDigitRe is one digit.
var niDigitRe = regexp.MustCompile(`[0-9]`)

// niTimeShape is a time's text with each digit written 9 and its zone as it is: what of its form the render keeps.
func niTimeShape(text string) string {
	g := niLocalRe.FindStringSubmatch(text)
	if g == nil {
		return text
	}
	return niDigitRe.ReplaceAllString(g[1], "9") + g[2]
}

// niLocalTime reads a parent's time (niLocalRe) in microseconds of the epoch.
func niLocalTime(text string) (int64, bool) {
	if !niLocalRe.MatchString(text) {
		return 0, false
	}
	t, err := time.Parse(time.RFC3339Nano, text)
	return t.UnixMicro(), err == nil
}

// niStreamSide is what a node-instance answer of TestNodeInstancesStream's pairs takes from one side's own run, for
// niStreamRender: niSide's start window and listening port, the scripted parent's port, and the seconds the side's
// stream times may hold in each stage.
type niStreamSide struct {
	niSide
	stub string
	// ready is the second by which the agent answered its first request: its localhost, and the list of its
	// parents, made (stream-parents.c:940: a parent's since is the moment its list was made).
	ready int64
	// connected are the seconds the side's sender connected in: from the second before the scripted parent's first
	// probe to the second the side was first seen online and counted (out).
	connected [2]int64
	// closed are the seconds since the scripted parent closed the sessions: from the close to the second the side
	// was first seen refused (out, denied).
	closed [2]int64
	// probed are the seconds the sender probed its parents in: from the second before the scripted parents' first
	// probe to the second the side was first seen with both banned (ban).
	probed [2]int64
	// socket is the port of the sender's own end of its session with the scripted parent, once connected: the
	// session's remote port that the side's agent holds (niSessionPort); empty where the harness has none.
	socket string
	// young says that the stage reads a connection reset within its first seconds (reset): its chart definitions'
	// bytes are how far it got by then, which niStreamWords reads by form, and its reason and its parent's last
	// handshake are one of C's reset texts (RESET, niStreamResetReasons)
	young bool
	// compressed says that the side's connection compresses (zip): the bytes counted are the compressed ones
	// (stream-sender-commit.c:171-173), whose count for the chart definitions varied C against C
	compressed bool
}

// niStreamRun is one of TestNodeInstancesStream's pairs in one of its stages, as runNIStream hands it to a
// comparison: the pair, the scripted parent both sides stream to, the stage's name and each side's own values (0 the
// oracle's).
type niStreamRun struct {
	Pair  *Pair
	Stub  *stream.Parent
	Stage string
	Sides [2]niStreamSide
	// pair is the pair's name (niStreamPairs), tz the agents' TZ (empty: the harness's own)
	pair, tz string
	// second is the port of the second scripted parent (ban: the one that says it is this agent's child)
	second string
}

// niStreamStage polls each side's localhost `stream` (`/api/v3/node_instances`, tagged harness=wait: how many polls
// there are is each side's timing) until done holds of it, at most limit, and hands back the second each side was
// first seen so. The oracle's failure ends the case, which did not reach its stage; a candidate's is reported, and the
// rows that follow show how it differs.
func niStreamStage(t *testing.T, p *Pair, what string, limit time.Duration, done func(Value) bool) [2]int64 {
	t.Helper()
	var at [2]int64
	for i, side := range p.Each() {
		got := ""
		ok := pollUntil(limit, func() bool {
			b, err := v2Exchange(side.Daemon.Addr, v2Req{target: "/api/v3/node_instances?harness=wait"})
			if err != nil {
				got = err.Error()
				return false
			}
			v, err := ParseJSON(httpBody(b))
			if err != nil {
				got = fmt.Sprintf("%v: %s", err, truncateBytes(b))
				return false
			}
			s, err := dashAt(v, "nodes", "[0]", "instances", "[0]", "stream")
			if err != nil {
				got = err.Error()
				return false
			}
			got = s.String()
			return done(s)
		})
		at[i] = time.Now().Unix()
		switch {
		case ok:
		case side.Role == Oracle:
			t.Fatalf("oracle: localhost's stream is not %s after %v: %s", what, limit, got)
		default:
			t.Errorf("candidate: localhost's stream is not %s after %v: %s", what, limit, got)
		}
	}
	return at
}

// niStreamIs holds of a `stream` whose status is status and, unless reason is empty, whose reason is reason.
func niStreamIs(status, reason string) func(Value) bool {
	return func(s Value) bool {
		st, err := dashMember(s, "status")
		if err != nil || st.Text != status {
			return false
		}
		r, err := dashMember(s, "reason")
		return reason == "" || (err == nil && r.Text == reason)
	}
}

// niStreamCounted holds of an online `stream` whose connection is counted: C sets the CONNECTED flag before the
// stream thread counts the connection and stamps its second (R108 correction 11: stream-sender.c:149, :364-365), so
// a `stream` online with id 0 has not reached its dispatch yet.
func niStreamCounted(s Value) bool {
	id, err := dashMember(s, "id")
	return niStreamIs("online", "")(s) && err == nil && id.Kind == KindNumber && id.Text != "0"
}

// niStreamSides are the two sides' own values (niStreamSide) of a pair streaming to stub, started (niStreamPair)
// by the second up: the oracle was ready when the candidate's launch began, the candidate by then.
func niStreamSides(p *Pair, stub *stream.Parent, up int64) [2]niStreamSide {
	_, port, _ := net.SplitHostPort(stub.Addr())
	var sides [2]niStreamSide
	ready := [2]int64{p.Candidate.LaunchStartedAt.Unix(), up}
	for i, s := range niSides(p, [2][2]int64{}) {
		sides[i] = niStreamSide{niSide: s, stub: port, ready: ready[i]}
	}
	return sides
}

// niNeverOptions are the `never` pair's: dashPairOptions (no pulse: nothing is collected) as a child of stub only
// (no stream key: no parent), five seconds between attempts, as pulseChildOptions.
func niNeverOptions(stub *stream.Parent, env []string) daemon.Options {
	return dashPairOptions(daemon.Options{NoStreamKey: true, Env: env, StreamTo: &daemon.StreamTo{
		Destination: stub.Addr(), APIKey: parentIdentity.StreamKey, Extra: "    reconnect delay = 5\n"}})
}

// niStreamPair starts a pair with opts and the bearer tokens (fnWriteTokens), so that a Function's comparison can call
// on it.
func niStreamPair(t *testing.T, opts daemon.Options) *Pair {
	t.Helper()
	return startPairWith(t, opts, parentIdentity, binaries(t), [2]string{opts.SeedCache, opts.SeedCache},
		[2]Role{Oracle, Candidate}, fnWriteTokens)
}

// niStreamPairs are runNIStream's pairs, in the order TestNodeInstancesStream runs them.
var niStreamPairs = []string{"never", "never-tz", "out", "ban", "tls", "zip", "reset", "never2"}

// niStreamStub is the scripted parent of the pair named name: over TLS on [::1] for `tls` (a self-signed certificate,
// which the agents are told not to verify), one that accepts every capability offered, compression with them, and
// reads nothing for `zip`, else PlaintextAnswer's on 127.0.0.1 (stream.StartParent(nil)).
func niStreamStub(t *testing.T, name string) *stream.Parent {
	t.Helper()
	var stub *stream.Parent
	var err error
	switch name {
	case "tls":
		key, cert := selfSigned(t)
		pair, kerr := tls.X509KeyPair(cert, key)
		if kerr != nil {
			t.Fatal(kerr)
		}
		stub, err = stream.StartParentTLSOn("[::1]:0", nil, &tls.Config{Certificates: []tls.Certificate{pair}})
	case "zip":
		stub, err = stream.StartParent(func(r stream.Request) stream.Answer {
			return stream.Answer{Reply: stream.VCaps(r.Caps())}
		})
	default:
		stub, err = stream.StartParent(nil)
	}
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stub.Close() })
	return stub
}

// niStreamOutOptions are the options of a pair named name that streams to stub (pulseChildOptions): `tls` names the
// stub's address with `:SSL` and skips the certificate's verification (stream.conf `ssl skip certificate
// verification`), `zip` enables compression (both agents offer it by default, stream-conf.c; the harness turns it off
// elsewhere).
func niStreamOutOptions(name string, stub *stream.Parent) daemon.Options {
	opts := pulseChildOptions(stub, true)
	switch name {
	case "tls":
		opts.StreamTo.Destination = stub.Addr() + ":SSL"
		opts.StreamTo.Extra += "    ssl skip certificate verification = yes\n"
	case "zip":
		opts.StreamTo.Compression = true
	}
	return opts
}

// niStreamResetAll resets every open session of stub (stream.Session.Reset: SO_LINGER 0, the sender's socket loses
// its peer at once).
func niStreamResetAll(stub *stream.Parent) {
	for _, s := range stub.Sessions() {
		if !s.Closed() {
			_ = s.Reset()
		}
	}
}

// niStreamCountedAs holds of a `stream` of status whose connection is counted (niStreamCounted's id above 0).
func niStreamCountedAs(status string) func(Value) bool {
	return func(s Value) bool {
		id, err := dashMember(s, "id")
		return niStreamIs(status, "")(s) && err == nil && id.Kind == KindNumber && id.Text != "0"
	}
}

// niStreamOnlineAs holds of an online `stream` whose connection count is id.
func niStreamOnlineAs(id string) func(Value) bool {
	return func(s Value) bool {
		n, err := dashMember(s, "id")
		return niStreamIs("online", "")(s) && err == nil && n.Text == id
	}
}

// runNIStream starts TestNodeInstancesStream's pair named name, with the bearer tokens, reaches each of its stages by
// waits of its own (never by a row's comparison) and runs compare in a subtest named after the stage (never-tz is
// never with the agents' TZ Asia/Kolkata: the parents' times are local, stream-parents.c:186):
//   - `never`: an agent alone with a destination (the scripted parent stub, which nothing ever reaches) and no pulse:
//     C queues a host for its parents only at its first collection (streaming/protocol/command-begin-set-end-init.c:
//     22-26), so its sender never starts. One stage, `never`, once both agents' start windows are over (niReady);
//     afterwards the stub must have seen no probe and no session;
//   - `out`: both agents streaming to stub (stream.StartParent(nil): PlaintextAnswer, each chart's streaming started
//     at once) as pulseChildOptions makes them (the pulse on: a sender starts at its host's first collection). Stage
//     `connected` once each side reads online and the parent's postponement after the handshake is over; stage
//     `denied` once the stub's script refuses every request (stream.RejectNotPermitted) and its sessions are closed,
//     and each side reads the refusal's reason;
//   - `ban`: as out, to two scripted parents (stub, then a second), whose probes answer with the agent's own machine
//     GUID, as this host and as a parent receiving it: both are banned at the first probe, and nothing connects.
//     Stage `banned` once each side has no parent left (NO PARENT TO SEND TO); afterwards neither parent may have
//     had a session;
//   - (H41) `tls`: as out, to a stub over TLS on [::1] (niStreamStub, niStreamOutOptions): stages `connected` and
//     `denied`, the ends and the parent's destination with `:SSL` while connected, none once refused;
//   - `zip`: as out, compression on, to a stub that takes every capability offered and asks no chart's
//     replication: stage `compressed` once each side reads replicating and counted, 6 s after the later session;
//   - `reset`: as out until connected; then a reset of every session once the parent's postponement is over, so that
//     both senders connect again at once; then, in the second after both were seen connected again and inside the
//     postponement that handshake set, a reset again: stage `reset` once each side reads offline (the disconnect's
//     reason rests until the postponement ends, 5 s after the handshake; which of a reset's two texts it is, the
//     path that saw the reset says: niStreamResetReasons);
//   - `never2`: as never, the destination naming two stubs.
//
// A Function's comparison (commit 12) calls it with its own compare: runNIStream(t, "out", func(t *testing.T, r
// *niStreamRun) { ... r.Stage, r.Pair ... }).
func runNIStream(t *testing.T, name string, compare func(t *testing.T, r *niStreamRun)) {
	t.Helper()
	stub := niStreamStub(t, name)
	var env []string
	if name == "never-tz" {
		env = []string{"TZ=Asia/Kolkata"}
	}
	switch name {
	case "never", "never-tz", "never2":
		opts := niNeverOptions(stub, env)
		var second *stream.Parent
		if name == "never2" {
			second = niStreamStub(t, "")
			opts.StreamTo.Destination = stub.Addr() + " " + second.Addr()
		}
		p := niStreamPair(t, opts)
		r := &niStreamRun{Pair: p, Stub: stub, Stage: "never", Sides: niStreamSides(p, stub, time.Now().Unix()),
			pair: name}
		if len(env) > 0 {
			r.tz = strings.TrimPrefix(env[0], "TZ=")
		}
		if second != nil {
			_, r.second, _ = net.SplitHostPort(second.Addr())
		}
		niReady(p)
		t.Run(r.Stage, func(t *testing.T) { compare(t, r) })
		for _, s := range []*stream.Parent{stub, second} {
			if s == nil {
				continue
			}
			if n, m := len(s.Probes()), len(s.Sessions()); n+m != 0 {
				t.Errorf("a scripted parent had %d probes and %d sessions from senders that never started", n, m)
			}
		}
	case "reset":
		p := niStreamPair(t, pulseChildOptions(stub, true))
		r := &niStreamRun{Pair: p, Stub: stub, Sides: niStreamSides(p, stub, time.Now().Unix()), pair: name}
		niStreamStage(t, p, "online and counted", 60*time.Second, niStreamCounted)
		last := time.Now()
		for _, s := range stub.Sessions() {
			last = s.At
		}
		time.Sleep(time.Until(last.Add(6 * time.Second)))
		// a reset of every session past the parent's postponement makes both sides connect again at once, each at
		// its connector's next pass (stream-sender.c:441-495, stream_connector_requeue(): no new postponement)
		niStreamResetAll(stub)
		from := time.Now().Unix()
		again := niStreamStage(t, p, "online again", 30*time.Second, niStreamOnlineAs("2"))
		for i := range r.Sides {
			r.Sides[i].connected = [2]int64{from, again[i]}
		}
		// a reset inside the postponement the second handshake set (5 s, stream-connector.c:236-238,
		// stream-parents.c:132-136): the disconnect's reason rests until it ends (stream-parents.c:633-642). It
		// comes in the second after the one each side was seen connected again, so that the disconnect's whole
		// second (the parent's `since`, stream-parents.c:103-108) is told apart from the connection's
		time.Sleep(time.Until(time.Unix(max(again[0], again[1])+1, 0)))
		niStreamResetAll(stub)
		closed := time.Now().Unix()
		gone := niStreamStage(t, p, "reset", 4*time.Second, niStreamIs("offline", ""))
		for i := range r.Sides {
			r.Sides[i].closed, r.Sides[i].young = [2]int64{closed, gone[i]}, true
		}
		r.Stage = "reset"
		t.Run(r.Stage, func(t *testing.T) { compare(t, r) })
	case "zip":
		p := niStreamPair(t, niStreamOutOptions(name, stub))
		r := &niStreamRun{Pair: p, Stub: stub, Sides: niStreamSides(p, stub, time.Now().Unix()), pair: name}
		// online for under a second (the definitions being sent), then replicating for good: the stub answers no
		// chart (SA-F's probe p1)
		seen := niStreamStage(t, p, "replicating and counted", 60*time.Second, niStreamCountedAs("replicating"))
		from := int64(0)
		if probes := stub.ProbeTimes(); len(probes) > 0 {
			from = probes[0].Unix() - 1
		}
		for i := range r.Sides {
			r.Sides[i].connected, r.Sides[i].compressed = [2]int64{from, seen[i]}, true
		}
		last := time.Now()
		for _, s := range stub.Sessions() {
			last = s.At
		}
		time.Sleep(time.Until(last.Add(6 * time.Second)))
		niStreamSockets(t, r)
		r.Stage = "compressed"
		t.Run(r.Stage, func(t *testing.T) { compare(t, r) })
	case "out", "tls":
		p := niStreamPair(t, niStreamOutOptions(name, stub))
		r := &niStreamRun{Pair: p, Stub: stub, Sides: niStreamSides(p, stub, time.Now().Unix()), pair: name}
		online := niStreamStage(t, p, "online and counted", 60*time.Second, niStreamCounted)
		// the pass that connects a parent reads its clock before it probes it (stream-parents.c:608, :854)
		probes := stub.ProbeTimes()
		from := int64(0)
		if len(probes) > 0 {
			from = probes[0].Unix() - 1
		}
		for i := range r.Sides {
			r.Sides[i].connected = [2]int64{from, online[i]}
		}
		// the parent stays postponed for 5 s after a handshake (stream-connector.c:236-238: reconnect delay 5,
		// stream-parents.c:110-117: randomize_wait_ut(5, 5))
		last := time.Now()
		for _, s := range stub.Sessions() {
			last = s.At
		}
		time.Sleep(time.Until(last.Add(6 * time.Second)))
		niStreamSockets(t, r)
		r.Stage = "connected"
		t.Run(r.Stage, func(t *testing.T) { compare(t, r) })
		stub.SetScript(func(stream.Request) stream.Answer {
			return stream.Answer{Reply: stream.RejectNotPermitted, Close: true}
		})
		closed := time.Now().Unix()
		for _, s := range stub.Sessions() {
			_ = s.Close()
		}
		denied := niStreamStage(t, p, "refused", 30*time.Second, niStreamIs("offline", "DENIED"))
		for i := range r.Sides {
			r.Sides[i].closed = [2]int64{closed, denied[i]}
		}
		// the connector's next pass, a second later, resets the parent's place (stream-parents.c:593-597). C tries
		// the parent again 5 to 60 s after the refusal (randomize_wait_ut(5, 60): stream-connector.c:75-83,
		// :244-250; stream-parents.c:110-117, :132-136), and that attempt changes `attempts` and the parent's
		// `since`: the rows ask at once, with a short settle (niStreamDeniedSettle) that ends before the earliest one
		time.Sleep(2 * time.Second)
		r.Stage = "denied"
		t.Run(r.Stage, func(t *testing.T) { compare(t, r) })
	case "ban":
		second, err := stream.StartParent(nil)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { second.Close() })
		// each probe answers with this agent's own machine GUID: a parent that is this host (ingest type localhost)
		// is banned for good, one that receives it (a child, online) for the session (stream-parents.c:647-713;
		// stream-path.c:162-164)
		stub.SetProbe(func(string) []byte { return streamInfo(parentIdentity.MachineGUID, "localhost", "online") })
		second.SetProbe(func(string) []byte { return streamInfo(parentIdentity.MachineGUID, "child", "online") })
		opts := pulseChildOptions(stub, true)
		opts.StreamTo.Destination = stub.Addr() + " " + second.Addr()
		p := niStreamPair(t, opts)
		r := &niStreamRun{Pair: p, Stub: stub, Sides: niStreamSides(p, stub, time.Now().Unix()), pair: name}
		_, r.second, _ = net.SplitHostPort(second.Addr())
		banned := niStreamStage(t, p, "banned", 60*time.Second, niStreamIs("offline", "NO PARENT TO SEND TO"))
		from := int64(0)
		if probes := stub.ProbeTimes(); len(probes) > 0 {
			from = probes[0].Unix() - 1
		}
		for i := range r.Sides {
			r.Sides[i].probed = [2]int64{from, banned[i]}
		}
		r.Stage = "banned"
		t.Run(r.Stage, func(t *testing.T) { compare(t, r) })
		if n := len(stub.Sessions()) + len(second.Sessions()); n != 0 {
			t.Errorf("%d sessions with parents that are banned", n)
		}
	default:
		t.Fatalf("harness: no stream pair %q", name)
	}
}

// niStreamFirstSlack is how many seconds before a sender's first probe its host's first stored point lies at least,
// from two facts that hold only together (the window has no spare second in the worst case):
//   - the first collection F queues the sender (command-begin-set-end-init.c:22-26), whose queueing postpones every
//     parent by more than 5 s with `reconnect delay = 5` (stream-connector.c:491-516, stream-parents.c:110-129; the
//     minimum, stream-conf.h:12) and wakes the connector, whose passes then run every 1000 ms from F
//     (stream-connector.c:484-488, :555): the first pass that may probe is F + 6 s at the earliest;
//   - a chart's first collection is not stored (rrdset-collection.c:597-605, :656-671: the pulse charts have no
//     STORE_FIRST), so the database's first time is floor(F) + 1.
//
// Together: the second the side is first seen connected or banned, less 5, is at least the first time. Changing the
// slack or the stage from either fact alone brings a one-second flake.
const niStreamFirstSlack = 5

// collected are the seconds the side's database first time may be in: from its launch (niSide.started) to
// niStreamFirstSlack before the second it was first seen connected (out) or with its parents banned (ban), each
// after its own first probe (niStreamFirstSlack says why); zeros for a pair without a collection (never). In every
// recorded run, C's and the Rust build's first time lay 2 to 5 s inside it.
func (s niStreamSide) collected() [2]int64 {
	end := s.connected[1]
	if end == 0 {
		end = s.probed[1]
	}
	if end == 0 {
		return [2]int64{}
	}
	return [2]int64{s.started[0], end - niStreamFirstSlack}
}

// niFirstRe is a database's first time written as a number.
var niFirstRe = regexp.MustCompile(`"first_time":(\s*)([0-9]+)`)

// niFirstRender writes localhost's database first time FIRST where it is a second of the side's first collection
// (niStreamSide.collected): with the pulse on, the first point the agent stored (rrdhost_retention(),
// rrdhost-status.c:121-122), each side's own second. Any other value is left as written.
func niFirstRender(side niStreamSide, body []byte) []byte {
	w := side.collected()
	if w[1] == 0 {
		return body
	}
	return niFirstRe.ReplaceAllFunc(body, func(m []byte) []byte {
		g := niFirstRe.FindSubmatch(m)
		if n, err := strconv.ParseInt(string(g[2]), 10, 64); err == nil && n >= w[0] && n <= w[1] {
			return []byte(`"first_time":` + string(g[1]) + `"FIRST"`)
		}
		return m
	})
}

// niStreamWord is the word a time of the side's stream reads as (sec, a second), by the window it lies in: START the
// agent's start (niSide.started; with boot, up to the second it was ready: a time taken while it booted), CONNECTED
// the sender's connection (connected), CLOSED after the parent closed the sessions (closed), PROBED the sender's
// probes of its parents (probed); empty in none.
func (s niStreamSide) niStreamWord(sec int64, boot bool) string {
	in := func(w [2]int64) bool { return w[1] != 0 && sec >= w[0] && sec <= w[1] }
	started := s.started
	if boot {
		started[1] = max(started[1], s.ready)
	}
	switch {
	case in(started):
		return "START"
	case in(s.connected):
		return "CONNECTED"
	case in(s.closed):
		return "CLOSED"
	case in(s.probed):
		return "PROBED"
	}
	return ""
}

// niInFlight tells whether ut (microseconds) is a moment the request was in flight (flight, seconds), less the 10 ms a
// parent's time loses to its two fraction digits.
func niInFlight(ut int64, flight [2]int64) bool {
	return ut >= flight[0]*1e6-10_000 && ut < (flight[1]+1)*1e6
}

// niPortOf is an end `[address]:port` (with `:SSL` on TLS) cut at its port: the text before the port, the port, the
// text after it.
func niPortOf(end string) (before, port, after string, ok bool) {
	at := strings.LastIndex(end, "]:")
	if at < 0 {
		return "", "", "", false
	}
	before, rest := end[:at+2], end[at+2:]
	port, after, _ = strings.Cut(rest, ":")
	if after != "" {
		after = ":" + after
	}
	return before, port, after, true
}

// niStreamWords are the replacements niStreamRender writes in one `stream` object (v) of side's answer whose
// walk's clock is now (clocked: the body has one) and whose request was in flight for the seconds flight, each a JSON
// text by its path in the object (joined by dots):
//   - `since` reads its window's word (niStreamWord; C: rrdhost-status.c:252, :289-290, the sender's connection
//     second or the agent's start), with `options=rfc3339` its word and its shape (niTimeShape); `age` reads
//     NOW-SINCE where since + age is the walk's clock (api_v2_contexts.c:392, :495);
//   - the destination's `local` port reads SOCKET where it is the sender's own end, which the kernel chose
//     (rrdhost-status.c:253, socket-peers.c): the port of its session that the scripted parent saw (niIsSocket); the
//     address, `remote` and the rest are compared as written;
//   - the traffic's counters niSentRendered names read SENT when above 0, and so do the chart definitions' bytes of a
//     side whose connection compresses (niStreamSide.compressed); on a young connection (niStreamSide.young) the chart
//     definitions' bytes read COUNT whatever they hold (how far it got: they varied C against C), and data and
//     replication keep the SENT rule (C read 0 for both on every side recorded, niStreamFacts); the functions' bytes
//     are compared as written (niSentRendered does not name them);
//   - on a young connection the stream's `reason` reads RESET where it is a text C gives a reset
//     (niStreamResetReasons), and so does each parent's `last_handshake` that is the stream's own reason (one call
//     writes both, stream-parents.c:103-108);
//   - each parent's `since` and `next_check` (local time, niLocalRe) read their window's word and shape, `next_check`
//     LATER for a moment after the request began, at most 61 s after it ended (a refusal postpones a parent up to
//     60 s, stream-connector.c:75-83, stream-parents.c:110-117); `age` reads NOW-SINCE and `next_in` NOW-NEXT where
//     they are the distance from that time to a moment of the flight (stream-parents.c:186-200: C takes the parents'
//     clock as it writes them).
func niStreamWords(side niStreamSide, v Value, now int64, clocked bool, flight [2]int64, first string) map[string]string {
	words := map[string]string{}
	quote := func(s string) string { return strconv.Quote(s) }
	since, err := dashMember(v, "since")
	sec, dated := int64(0), false
	switch {
	case err != nil:
	case since.Kind == KindNumber:
		if n, err := strconv.ParseInt(since.Text, 10, 64); err == nil {
			sec, dated = n, true
			if w := side.niStreamWord(n, false); w != "" {
				words["since"] = quote(w)
			}
		}
	case since.Kind == KindString && niUTCRe.MatchString(since.Text):
		if t, err := time.Parse(time.RFC3339, since.Text); err == nil {
			sec, dated = t.Unix(), true
			if w := side.niStreamWord(sec, false); w != "" {
				words["since"] = quote(w + " " + niDigitRe.ReplaceAllString(since.Text, "9"))
			}
		}
	}
	if age, err := dashMember(v, "age"); err == nil && dated && clocked && age.Kind == KindNumber {
		if n, err := strconv.ParseInt(age.Text, 10, 64); err == nil && sec+n == now {
			words["age"] = quote("NOW-SINCE")
		}
	}
	if local, err := dashAt(v, "destination", "local"); err == nil && local.Kind == KindString {
		if before, port, after, ok := niPortOf(local.Text); ok && side.niIsSocket(port) {
			words["destination.local"] = quote(before + "SOCKET" + after)
		}
	}
	traffic, _ := dashAt(v, "destination", "traffic")
	for _, m := range traffic.Members {
		n, err := strconv.ParseInt(m.Value.Text, 10, 64)
		if err != nil || m.Value.Kind != KindNumber {
			continue
		}
		switch {
		case side.young && m.Key == "metadata":
			words["destination.traffic."+m.Key] = quote("COUNT")
		case n > 0 && (slices.Contains(niSentRendered, m.Key) || (m.Key == "metadata" && side.compressed)):
			words["destination.traffic."+m.Key] = quote("SENT")
		}
	}
	reason, _ := dashMember(v, "reason")
	reset := side.young && reason.Kind == KindString && slices.Contains(niStreamResetReasons, reason.Text)
	if reset {
		words["reason"] = quote("RESET")
	}
	path, _ := dashAt(v, "destination", "streaming_path")
	for k, e := range path.Items {
		at := "destination.streaming_path.[" + strconv.Itoa(k) + "]."
		if since, err := dashMember(e, "since"); err == nil && since.Kind == KindNumber {
			if n, err := strconv.ParseInt(since.Text, 10, 64); err == nil {
				if w := side.niStreamWord(n, false); w != "" {
					words[at+"since"] = quote(w)
				}
			}
		}
		if f, err := dashMember(e, "first_time_t"); err == nil && f.Kind == KindNumber && f.Text != "0" &&
			f.String() == first {
			words[at+"first_time_t"] = quote("DB-FIRST")
		}
	}
	parents, _ := dashAt(v, "destination", "parents")
	for k, d := range parents.Items {
		at := "destination.parents.[" + strconv.Itoa(k) + "]."
		if h, err := dashMember(d, "last_handshake"); err == nil && reset && h.Kind == KindString &&
			h.Text == reason.Text {
			words[at+"last_handshake"] = quote("RESET")
		}
		for _, pair := range [][3]string{{"since", "age", "NOW-SINCE"}, {"next_check", "next_in", "NOW-NEXT"}} {
			t, err := dashMember(d, pair[0])
			if err != nil || t.Kind != KindString {
				continue
			}
			ut, ok := niLocalTime(t.Text)
			if !ok {
				continue
			}
			word := side.niStreamWord(ut/1e6, true)
			if pair[0] == "next_check" && ut > flight[0]*1e6 && ut <= (flight[1]+61)*1e6 {
				word = "LATER"
			}
			if word != "" {
				words[at+pair[0]] = quote(word + " " + niTimeShape(t.Text))
			}
			span, err := dashMember(d, pair[1])
			if err != nil || span.Kind != KindString {
				continue
			}
			us, ok := niDuration(span.Text)
			moment := ut + us
			if pair[0] == "next_check" {
				moment = ut - us
			}
			if ok && niInFlight(moment, flight) {
				words[at+pair[1]] = quote(pair[2])
			}
		}
	}
	return words
}

// niIsSocket tells whether port is the side's sender's own: its session's port with the scripted parent where the
// harness found it (socket: the kernel's choice, read back from both ends), else any port (1 to 65535) other than the
// agent's listening port and the parent's (a stage whose session the harness did not map).
func (s niStreamSide) niIsSocket(port string) bool {
	if s.socket != "" {
		return port == s.socket
	}
	n, err := strconv.Atoi(port)
	return err == nil && niPeerRe.MatchString(port) && n <= 65535 && port != s.listen && port != s.stub
}

// niTCPSockets reads the text of /proc/net/tcp or /proc/net/tcp6 into into: each socket's inode (the tenth field)
// keyed to its ports, `<local port>><remote port>` (the hex port after the last colon of `local_address` and of
// `rem_address`); a line without both, as the header, is skipped.
func niTCPSockets(text string, into map[string]string) {
	for _, l := range strings.Split(text, "\n") {
		f := strings.Fields(l)
		if len(f) < 10 {
			continue
		}
		var ports []string
		for _, end := range f[1:3] {
			at := strings.LastIndex(end, ":")
			if at < 0 {
				break
			}
			n, err := strconv.ParseUint(end[at+1:], 16, 16)
			if err != nil {
				break
			}
			ports = append(ports, strconv.FormatUint(n, 10))
		}
		if len(ports) == 2 {
			into[f[9]] = ports[0] + ">" + ports[1]
		}
	}
}

// niSocketInode is the inode a file descriptor's link names when it is a socket (`socket:[inode]`).
func niSocketInode(link string) (string, bool) {
	inode, ok := strings.CutPrefix(link, "socket:[")
	inode, closed := strings.CutSuffix(inode, "]")
	return inode, ok && closed && inode != ""
}

// niHeldSockets are the TCP sockets the process pid holds, each as `<local port>><remote port>` (niTCPSockets): its
// descriptors' links (/proc/<pid>/fd) matched to each socket's ports by inode (/proc/net/tcp and tcp6).
func niHeldSockets(pid int) (map[string]bool, error) {
	sockets := map[string]string{}
	for _, f := range []string{"/proc/net/tcp", "/proc/net/tcp6"} {
		if b, err := os.ReadFile(f); err == nil {
			niTCPSockets(string(b), sockets)
		}
	}
	dir := fmt.Sprintf("/proc/%d/fd", pid)
	fds, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("harness: the agent's descriptors: %v", err)
	}
	held := map[string]bool{}
	for _, fd := range fds {
		if link, err := os.Readlink(filepath.Join(dir, fd.Name())); err == nil {
			if inode, ok := niSocketInode(link); ok && sockets[inode] != "" {
				held[sockets[inode]] = true
			}
		}
	}
	return held, nil
}

// niSessionPort is the port of the sender's own end of the open session the agent pid holds with stub: the remote
// port of the one session whose socket is, at the agent's end, a socket the agent holds from that port to the stub's
// (niHeldSockets). Both agents of a pair stream to one stub with one identity, so only the kernel tells their
// sessions apart.
func niSessionPort(stub *stream.Parent, pid int) (string, error) {
	held, err := niHeldSockets(pid)
	if err != nil {
		return "", err
	}
	_, stubPort, err := net.SplitHostPort(stub.Addr())
	if err != nil {
		return "", fmt.Errorf("harness: the scripted parent's address: %v", err)
	}
	var found []string
	for _, s := range stub.Sessions() {
		if a, ok := s.RemoteAddr().(*net.TCPAddr); ok && !s.Closed() && held[strconv.Itoa(a.Port)+">"+stubPort] {
			found = append(found, strconv.Itoa(a.Port))
		}
	}
	if len(found) != 1 {
		return "", fmt.Errorf("the agent (PID %d) holds %d open sessions with the scripted parent: %v", pid, len(found),
			found)
	}
	return found[0], nil
}

// niStreamSockets sets each side's socket (niSessionPort) once its sender connected to r's scripted parent: the
// oracle's failure ends the case, a candidate's is reported (its rows then judge its port by the general rule).
func niStreamSockets(t *testing.T, r *niStreamRun) {
	t.Helper()
	for i, side := range r.Pair.Each() {
		port, err := niSessionPort(r.Stub, side.Daemon.PID())
		switch {
		case err == nil:
			r.Sides[i].socket = port
		case side.Role == Oracle:
			t.Fatalf("oracle: %v", err)
		default:
			t.Errorf("candidate: %v", err)
		}
	}
}

// niStreamResetReasons are the texts C's sender gives a connection its parent reset (SO_LINGER 0), by the path that
// sees the reset first, which a run does not fix (C against C in SA-F2's pass c1: one side each): the poll's error or
// hang-up (stream-sender.c:870-893, DISCONNECT SOCKET ERROR), or a send() or recv() failing with ECONNRESET
// (:726-727, :748-750; :815, :835-837: DISCONNECTED SOCKET CLOSED BY REMOTE END). The port has the same three paths
// (streaming/src/sender/dispatch.rs:260, :299-312, :357-380 at 553b778338).
var niStreamResetReasons = []string{"DISCONNECT SOCKET ERROR", "DISCONNECTED SOCKET CLOSED BY REMOTE END"}

// niSentRendered are the traffic counters niStreamWords reads by form (SENT for a count above 0, a 0 as written):
// the bytes of each side's own collection (data) and of the replication answers to the parent's starts (5209 against
// 5210 bytes C against C in probe p1: the digits of their times, stream-replication-sender.c:680-694), counted as
// they are queued and kept after a disconnect (stream-circular-buffer.c:118, stream-sender.c:369). The chart
// definitions' bytes (metadata) were equal C against C and are compared.
var niSentRendered = []string{"data", "replication"}

// niStreamRender renders the `stream` of side i's answer at path by niStreamWords (body is the whole answer: its
// walk's clock, niNow; the instance's db.first_time).
func niStreamRender(sides [2]niStreamSide) func(i int, flight [2]int64, body []byte, path []string, obj []byte) []byte {
	return func(i int, flight [2]int64, body []byte, path []string, obj []byte) []byte {
		v, err := ParseJSON(obj)
		if err != nil {
			return obj
		}
		n, clocked := niNow(body)
		first := ""
		if doc, err := ParseJSON(body); err == nil {
			if f, err := dashAt(doc, append(slices.Clone(path[:len(path)-1]), "db", "first_time")...); err == nil {
				first = f.String()
			}
		}
		words := niStreamWords(sides[i], v, n, clocked, flight, first)
		return jsonRewrite(obj, nil, func(path []string, value []byte) ([]byte, bool) {
			w, ok := words[strings.Join(path, ".")]
			return []byte(w), ok
		})
	}
}

// niStreamFamily compares a node-instance answer of r's pair: nodeInstancesFamily outside the `stream` objects
// (niIngestRender), with localhost's first time FIRST where it is a second of the side's first collection
// (niFirstRender: the pulse pairs), each `stream` by niStreamRender (niOutsideStream). Nothing of the pulse is masked:
// in four C-against-C runs (probes p1, p2, runs c1, c2) the counts of the instance and of the agent, and the contexts'
// hash, were equal at the stages' moments, and on the Rust build too.
func niStreamFamily(r *niStreamRun) v2Family {
	var sides [2]niSide
	for i := range r.Sides {
		sides[i] = r.Sides[i].niSide
	}
	fam := nodeInstancesFamily(sides)
	outside := fam.render
	fam.render = niOutsideStream(func(i int, flight [2]int64, body []byte) []byte {
		return niFirstRender(r.Sides[i], outside(i, flight, body))
	}, niStreamRender(r.Sides))
	return fam
}

// niStreamInst is the path of localhost's instance, the one node of these pairs.
var niStreamInst = []string{"nodes", "[0]", "instances", "[0]"}

// niStreamAt is the path of a member of localhost's instance.
func niStreamAt(keys ...string) []string { return append(slices.Clone(niStreamInst), keys...) }

// niStreamOurs are the capabilities a sender offers with compression off (`enable compression = no`) and no ML, as
// C lists them in its own stream path entry (stream_our_capabilities(), streaming/stream-capabilities.c:101-147:
// the compressions and ML_MODELS are disabled; stream-path.c:140), in C's order of names
// (stream-capabilities.c:15-41).
const niStreamOurs = `["V1","V2","VN","VCAPS","HLABELS","CLAIM","CLABELS","FUNCTIONS","FUNCDEL","REPLICATION",` +
	`"BINARY","INTERPOLATED","IEEE754","DYNCFG","SLOTS","PROGRESS","NODEID","PATHS","FLOATBASELINE"]`

// niStreamOursZipped are niStreamOurs with compression on (the `zip` pair): the four compressions in C's order of
// names (stream-capabilities.c:15-41; stream_our_capabilities(), :101-147).
const niStreamOursZipped = `["V1","V2","VN","VCAPS","HLABELS","CLAIM","CLABELS","LZ4","FUNCTIONS","FUNCDEL",` +
	`"REPLICATION","BINARY","INTERPOLATED","IEEE754","DYNCFG","SLOTS","ZSTD","GZIP","BROTLI","PROGRESS","NODEID",` +
	`"PATHS","FLOATBASELINE"]`

// niStreamZipped are the capabilities a sender holds once the `zip` pair's stub answered with every one it offered:
// niStreamOursZipped less V1, V2 and VN (convert_stream_version_to_capabilities(), stream-capabilities.c:149-175), the
// four compressions kept (the stub's answer names them all).
const niStreamZipped = `["VCAPS","HLABELS","CLAIM","CLABELS","LZ4","FUNCTIONS","FUNCDEL","REPLICATION","BINARY",` +
	`"INTERPOLATED","IEEE754","DYNCFG","SLOTS","ZSTD","GZIP","BROTLI","PROGRESS","NODEID","PATHS","FLOATBASELINE"]`

// niStreamNegotiated are the capabilities a sender holds once the scripted parent answered with what it offered
// (stream.PlaintextAnswer): niStreamOurs less V1, V2 and VN, which VCAPS replaces
// (convert_stream_version_to_capabilities(), stream-capabilities.c:149-175).
const niStreamNegotiated = `["VCAPS","HLABELS","CLAIM","CLABELS","FUNCTIONS","FUNCDEL","REPLICATION","BINARY",` +
	`"INTERPOLATED","IEEE754","DYNCFG","SLOTS","PROGRESS","NODEID","PATHS","FLOATBASELINE"]`

// niStreamPath is localhost's streaming path as these agents print it in their `stream`: their own entry alone
// (rrdhost_stream_path_to_json(), stream-path.c:179-225; the scripted parent sends no path back), unclaimed (node and
// claim ids null, no flag), hops 0, `since` its start (stream-path.c:128-136, :135: localhost has no receiver), START,
// its first time the database's (first: `0` with nothing stored, else DB-FIRST, niStreamWords), start and
// shutdown times 0 (a run directory without earlier starts), and the capabilities it offers (ours: niStreamOurs, or
// niStreamOursZipped with compression on).
func niStreamPath(first, ours string) string {
	return `[{"version":1,"hostname":"` + parentIdentity.Hostname + `","host_id":"` + parentIdentity.MachineGUID +
		`","node_id":null,"claim_id":null,"hops":0,"since":"START","first_time_t":` + first + `,"start_time":0,` +
		`"shutdown_time":0,"capabilities":` + ours + `,"flags":[]}]`
}

// niStreamZone is the zone suffix C writes after a parent's local time on an agent whose TZ is tz (empty: the
// harness's own, which the agents inherit): `Z` for UTC, else the offset (rfc3339.c:153-185).
func niStreamZone(tz string) string {
	loc := time.Local
	if tz != "" {
		if l, err := time.LoadLocation(tz); err == nil {
			loc = l
		}
	}
	return time.Now().In(loc).Format("Z07:00")
}

// niStreamParent is a scripted parent's item in `parents` as niStreamWords renders it: the destination (the stub's
// address at port as the agents' `destination` names it, niStreamWire, with `:SSL` over TLS: stream-parents.c:
// 183-190), its attempts printed + 1 (stream-parents.c:183), `since` as word and shape (fraction digits and zone),
// `age` NOW-SINCE, then the members its state prints (rest).
func niStreamParent(r *niStreamRun, port string, attempts int, since, rest string) string {
	return niStreamParentShaped(r, port, attempts, since, "9999-99-99T99:99:99.99", rest)
}

// niStreamParentShaped is niStreamParent with its `since` of the shape given before the zone: a whole second, which
// a disconnect writes (stream-parents.c:103-108), has no fraction (rfc3339.c:141-150).
func niStreamParentShaped(r *niStreamRun, port string, attempts int, since, shape, rest string) string {
	_, host, ssl := niStreamWire(r)
	return fmt.Sprintf(`{"attempts":%d,"destination":"%s:%s%s","since":"%s %s%s",`+
		`"age":"NOW-SINCE",%s}`, attempts, host, port, ssl, since, shape, niStreamZone(r.tz), rest)
}

// niStreamWire is how r's agents reach their scripted parent: the address of a socket's end as the stream's
// destination prints it (`[%s]`, rrdhost_sender_to_json(), api_v2_contexts.c:405-410: 127.0.0.1, or ::1 for `tls`,
// whose stub listens on IPv6), the host a parent's `destination` names (the configured text: `[::1]` keeps its
// brackets) and the suffix TLS adds to both (`:SSL`).
func niStreamWire(r *niStreamRun) (end, host, ssl string) {
	if r.pair == "tls" {
		return "[::1]", "[::1]", ":SSL"
	}
	return "[127.0.0.1]", "127.0.0.1", ""
}

// niStreamFacts are the facts of the `stream` of localhost's instance in r's stage, as C printed them (probe p1) and
// niStreamWords renders them; rfc3339 when the request asked `options=rfc3339`:
//   - never: the sender exists (a destination and an API key, stream-sender-api.c:18-74) and was never queued (no
//     collection): offline (rrdhost-status.c:279-281), connection 0, hops the ingestion's + 1, `since` the agent's
//     start (:289-290), NEVER CONNECTED (the reason's 0, stream-handshake.c:9-12), nothing replicated, both ends
//     `[not connected]:0` (socket-peers.c: no socket), no capability, compression off and four zeros; one parent
//     never tried: attempts 0 + 1, `since` the moment its list was made, its reason 0, never postponed (no
//     `next_check`) and never ranked (no `batch`), info and skipped false (calloc, stream-parents.c:927-950);
//   - connected: online (the CONNECTED flag, :262-275), the first connection counted at the dispatch
//     (stream-sender.c:364-365) and its second, hops 1, everything replicated (completion 100, rrdhost-status.c:81-97),
//     the socket's two ends (the agent's own port, SOCKET; the parent's address), the negotiated names, compression
//     off (the parent dropped it), the traffic by form; the parent tried once more (attempts 1 + 1), since the pass
//     that connected it, its reason SOCKET CONNECTED (stream-connector.c:236-238 writes it at the handshake), no
//     longer postponed (5 s after it), ranked alone (batch 1, order 1, not random: stream-parents.c:815-819), its probe
//     unanswered (the stub's 404 has a length of 0: info false, :496-516) and not skipped;
//   - compressed (zip): as connected, but the stub took every capability offered, the compressions with them
//     (niStreamZipped), and asks no chart's replication: each chart waits for its parent's answer (replicating,
//     completion 100 and 13 instances, stream-replication-sender.c), and only the chart definitions went, compressed
//     (compression true; their bytes varied C against C: SENT), no data, function or replication bytes;
//   - denied: offline with the refusal's reason (DENIED, stream-connector.c:75-83, :244-250), the connection count
//     and `since` the dispatch's (a refusal is no dispatch), hops 1, nothing replicated (completion 0), both ends
//     `[not connected]:0` (the socket closed, stream-sender.c:483), no capability, the last connection's traffic
//     (zeroed only at a connect or a removal); the parent tried again after the close (attempts 2 + 1), since that
//     pass, its reason DENIED, postponed 5 to 60 s (LATER, NOW-NEXT; stream-parents.c:110-117), its place reset by
//     the next pass (no batch, info false, skipped true: :593-597);
//   - reset: the second connection (id 2) reset in the second after both sides were seen connected again
//     (runNIStream): about 1 s after the later side's handshake, up to a connector pass (1 s) more after an earlier
//     side's (SA-F's probe p2: the handshake at about 14:44:09.00 by its parent's next_check 14:44:14.00, the reset in
//     second 14:44:10). Offline with a reset's reason (RESET, niStreamResetReasons), `since` the connection's second,
//     both ends `[not connected]:0`, no capability, nothing replicated (completion 0); the chart definitions' bytes how
//     far the connection got (COUNT: 7437 and 8734 on p2's C sides), the data and the replication answers none yet (0
//     on every C side recorded, p2's and, at an earlier timing (the reset 0.27 s after the handshake), p1's; how long
//     the 0 lasts: niStreamResetSettle; a chart's data waits for its replication to finish,
//     command-begin-set-end-init.c:71-77, stream-replication-sender.c:711, and C's replication thread had answered no
//     request of the parent's by then; its idle waits are up to 1 s, :1719, :1879-1910); the parent tried twice
//     (attempts 2 + 1), `since` the disconnect's whole second (CLOSED, no fraction: stream-parents.c:103-108,
//     rfc3339.c:141-150), its reason the host's, postponed until 5 s after the handshake (LATER, NOW-NEXT) and its
//     place reset (no batch, info false, skipped true);
//   - banned: never connected (as never, the traffic zeros) and no parent left to try: the host's reason NO PARENT TO
//     SEND TO (stream-parents.c:735-744); each parent probed once, never connected (attempts 0 + 1), since the
//     probing pass (:653, :698), and only its ban printed (:212-218): the one that says it is this host banned for
//     good (`it is the localhost`, :647-656), the one that receives this host for the session (`it is our parent`,
//     :694-701; stream-path.c:162-164: this agent's own GUID).
func niStreamFacts(r *niStreamRun, rfc3339 bool) []dashFact {
	st := niStreamAt("stream")
	dst := append(slices.Clone(st), "destination")
	since := func(word string) string {
		if rfc3339 {
			return `"` + word + ` 9999-99-99T99:99:99Z"`
		}
		return `"` + word + `"`
	}
	port := r.Sides[0].stub
	end, _, ssl := niStreamWire(r)
	switch r.Stage {
	case "never":
		parents := niStreamParent(r, port, 1, "START", `"last_handshake":"NEVER CONNECTED","info":false,"skipped":false`)
		if r.pair == "never2" {
			// the second parent of the list, made in the same moment (stream-parents.c:927-950, :964)
			parents += "," + niStreamParent(r, r.second, 1, "START", `"last_handshake":"NEVER CONNECTED","info":false,`+
				`"skipped":false`)
		}
		return slices.Concat(
			[]dashFact{dashKeys("id hops status since age reason replication destination", st...),
				dashKeys("local remote capabilities traffic parents streaming_path", dst...)},
			dashMembers(st, "id", "0", "hops", "1", "status", `"offline"`, "since", since("START"),
				"age", `"NOW-SINCE"`, "reason", `"NEVER CONNECTED"`,
				"replication", `{"in_progress":false,"completion":0,"instances":0}`),
			dashMembers(dst, "local", `"[not connected]:0"`, "remote", `"[not connected]:0"`, "capabilities", "[]",
				"traffic", `{"compression":false,"data":0,"metadata":0,"functions":0,"replication":0}`,
				"parents", "["+parents+"]", "streaming_path", niStreamPath("0", niStreamOurs)))
	case "connected":
		return slices.Concat(
			[]dashFact{dashKeys("id hops status since age replication destination", st...),
				dashKeys("local remote capabilities traffic parents streaming_path", dst...)},
			dashMembers(st, "id", "1", "hops", "1", "status", `"online"`, "since", since("CONNECTED"),
				"age", `"NOW-SINCE"`, "replication", `{"in_progress":false,"completion":100,"instances":0}`),
			dashMembers(dst, "local", `"`+end+`:SOCKET`+ssl+`"`, "remote", `"`+end+`:`+port+ssl+`"`,
				"capabilities", niStreamNegotiated,
				"parents", "["+niStreamParent(r, port, 2, "CONNECTED", `"last_handshake":"SOCKET CONNECTED","batch":1,`+
					`"order":1,"random":false,"info":false,"skipped":false`)+"]",
				"streaming_path", niStreamPath(`"DB-FIRST"`, niStreamOurs)),
			niStreamTraffic(dst))
	case "compressed":
		return slices.Concat(
			[]dashFact{dashKeys("id hops status since age replication destination", st...),
				dashKeys("local remote capabilities traffic parents streaming_path", dst...)},
			dashMembers(st, "id", "1", "hops", "1", "status", `"replicating"`, "since", since("CONNECTED"),
				"age", `"NOW-SINCE"`, "replication", `{"in_progress":true,"completion":100,"instances":13}`),
			dashMembers(dst, "local", `"`+end+`:SOCKET`+ssl+`"`, "remote", `"`+end+`:`+port+ssl+`"`,
				"capabilities", niStreamZipped,
				"traffic", `{"compression":true,"data":0,"metadata":"SENT","functions":0,"replication":0}`,
				"parents", "["+niStreamParent(r, port, 2, "CONNECTED", `"last_handshake":"SOCKET CONNECTED","batch":1,`+
					`"order":1,"random":false,"info":false,"skipped":false`)+"]",
				"streaming_path", niStreamPath(`"DB-FIRST"`, niStreamOursZipped)))
	case "reset":
		return slices.Concat(
			[]dashFact{dashKeys("id hops status since age reason replication destination", st...),
				dashKeys("local remote capabilities traffic parents streaming_path", dst...)},
			dashMembers(st, "id", "2", "hops", "1", "status", `"offline"`, "since", since("CONNECTED"),
				"age", `"NOW-SINCE"`, "reason", `"RESET"`,
				"replication", `{"in_progress":false,"completion":0,"instances":0}`),
			dashMembers(dst, "local", `"[not connected]:0"`, "remote", `"[not connected]:0"`, "capabilities", "[]",
				"traffic", `{"compression":false,"data":0,"metadata":"COUNT","functions":0,"replication":0}`,
				"parents", "["+niStreamParentShaped(r, port, 3, "CLOSED", "9999-99-99T99:99:99",
					`"last_handshake":"RESET","next_check":"LATER 9999-99-99T99:99:99.99`+
						niStreamZone(r.tz)+`","next_in":"NOW-NEXT","info":false,"skipped":true`)+"]",
				"streaming_path", niStreamPath(`"DB-FIRST"`, niStreamOurs)))
	case "denied":
		return slices.Concat(
			[]dashFact{dashKeys("id hops status since age reason replication destination", st...),
				dashKeys("local remote capabilities traffic parents streaming_path", dst...)},
			dashMembers(st, "id", "1", "hops", "1", "status", `"offline"`, "since", since("CONNECTED"),
				"age", `"NOW-SINCE"`, "reason", `"DENIED"`,
				"replication", `{"in_progress":false,"completion":0,"instances":0}`),
			dashMembers(dst, "local", `"[not connected]:0"`, "remote", `"[not connected]:0"`, "capabilities", "[]",
				"parents", "["+niStreamParent(r, port, 3, "CLOSED", `"last_handshake":"DENIED","next_check":"LATER `+
					`9999-99-99T99:99:99.99`+niStreamZone(r.tz)+`","next_in":"NOW-NEXT","info":false,"skipped":true`)+"]",
				"streaming_path", niStreamPath(`"DB-FIRST"`, niStreamOurs)),
			niStreamTraffic(dst))
	case "banned":
		return slices.Concat(
			[]dashFact{dashKeys("id hops status since age reason replication destination", st...),
				dashKeys("local remote capabilities traffic parents streaming_path", dst...)},
			dashMembers(st, "id", "0", "hops", "1", "status", `"offline"`, "since", since("START"),
				"age", `"NOW-SINCE"`, "reason", `"NO PARENT TO SEND TO"`,
				"replication", `{"in_progress":false,"completion":0,"instances":0}`),
			dashMembers(dst, "local", `"[not connected]:0"`, "remote", `"[not connected]:0"`, "capabilities", "[]",
				"traffic", `{"compression":false,"data":0,"metadata":0,"functions":0,"replication":0}`,
				"parents", "["+niStreamParent(r, port, 1, "PROBED", `"ban":"it is the localhost"`)+","+
					niStreamParent(r, r.second, 1, "PROBED", `"ban":"it is our parent"`)+"]",
				"streaming_path", niStreamPath(`"DB-FIRST"`, niStreamOurs)))
	}
	return []dashFact{func(Value) error { return fmt.Errorf("harness: no facts for the stage %q", r.Stage) }}
}

// niStreamTraffic are the facts of a sender's traffic after its connection, plaintext: compression off, the
// collected data and the replication answers above 0 (SENT), no function's bytes, the chart definitions above 0
// (12132 bytes on all four sides of probe p1; compared).
func niStreamTraffic(dst []string) []dashFact {
	tr := append(slices.Clone(dst), "traffic")
	return slices.Concat(dashMembers(tr, "compression", "false", "data", `"SENT"`, "functions", "0",
		"replication", `"SENT"`), []dashFact{dashKeys("compression data metadata functions replication", tr...),
		dashAbove(0, append(slices.Clone(tr), "metadata")...)})
}

// niStreamAgent are the facts of the agent's info (api_v2_contexts_agents.c:11-121): its members, its one host,
// sending while the sender is connected (rrd-metadata.c:33: the CONNECTED flag), and its cloud status of an agent
// that never connected (claim/cloud-status.c:38-44, :74-75), as niIngestRender names it in every form (the cloud
// status keeps its numbers with rfc3339; niNow reads the walk's clock as a date then).
func niStreamAgent(sending bool) []dashFact {
	nodes := `{"total":1,"receiving":0,"sending":0,"archived":0}`
	if sending {
		nodes = `{"total":1,"receiving":0,"sending":1,"archived":0}`
	}
	return slices.Concat(
		[]dashFact{dashKeys("mg nd nm now ai application cloud nodes metrics instances contexts capabilities api "+
			"db_size timings", "agents", "[0]")},
		dashMembers([]string{"agents", "[0]"}, "mg", strconv.Quote(parentIdentity.MachineGUID), "nodes", nodes),
		dashMembers([]string{"agents", "[0]", "cloud"}, "status", `"available"`, "since", `"START"`, "age",
			`"NOW-SINCE"`))
}

// niStreamHost are the facts of localhost, the one node of r's pair, and of its instance but for `stream`
// (niStreamFacts): no `st` on the node (the mode's last argument is false), C's members in order with `stream`
// between `ingest` and `ml` (api_v2_contexts.c:524-571), the agent's own functions, capabilities and dyncfg
// (aclk_capas.c:41-42; rrdhost-status.c:404-405), ML and health off. Without a collection (never) the database and
// the ingestion are initializing (rrdhost-status.c:124-130, :171-173) in the agent's dbengine, begun at its start
// (niLocalIngest); collecting (out, ban, tls, zip, reset), localhost is online and live (:176-177, :387-390) in
// memory mode alloc, its first time a second of its first collection (FIRST, niFirstRender) and counts above 0
// (pulse). rfc3339: the dates are UTC texts, which niIngestRender names as it names the numbers (niDated's words); the
// first time 0 is null.
func niStreamHost(r *niStreamRun, rfc3339 bool) []dashFact {
	node := []string{"nodes", "[0]"}
	db, ingest := niStreamAt("db"), niStreamAt("ingest")
	facts := slices.Concat(dashParent(0, 0),
		[]dashFact{dashKeys("api nodes versions agents timings"), dashAbsent("nodes", "[1]"),
			dashKeys("mg nm ni instances", node...), dashAbsent(append(slices.Clone(node), "instances", "[1]")...),
			dashKeys("st db ingest stream ml health functions capabilities dyncfg", niStreamInst...)},
		dashMembers(niStreamInst, "st", `{"ai":0,"code":200,"msg":""}`, "ml", `{"status":"disabled","type":"disabled"}`,
			"health", `{"status":"disabled"}`, "capabilities", nodeCaps(true), "dyncfg", `{"status":"online"}`),
		dashMembers(ingest, "id", "0", "hops", "0", "type", `"localhost"`))
	if r.Stage == "never" {
		f, first := niShort, "0"
		if rfc3339 {
			f, first = niDated, "null"
		}
		return slices.Concat(facts, niLocalIngestIn(f, niStreamInst),
			dashMembers(db, "status", `"initializing"`, "liveness", `"stale"`, "mode", `"dbengine"`,
				"first_time", first, "last_time", f.word("NOW"), "metrics", "0", "instances", "0", "contexts", "0"),
			dashMembers(ingest, "status", `"initializing"`, "metrics", "0", "instances", "0", "contexts", "0"),
			niStreamAgent(false))
	}
	return slices.Concat(facts,
		dashMembers(db, "status", `"online"`, "liveness", `"live"`, "mode", `"alloc"`, "last_time", `"NOW"`),
		dashMembers(ingest, "status", `"online"`, "since", `"START"`, "age", `"NOW-SINCE"`),
		dashMembers(db, "first_time", `"FIRST"`),
		[]dashFact{dashAbove(0, append(slices.Clone(db), "metrics")...), dashAbove(0, append(slices.Clone(ingest), "metrics")...)},
		niStreamAgent(r.Stage == "connected" || r.Stage == "compressed"))
}

// niStreamDeniedSettle is how long the `denied` stage's rows ask again while the sides differ: the stage starts 2 s
// after each side was seen refused, and C's next attempt comes 5 s after the refusal at the earliest (runNIStream),
// so a longer settle could compare a side that tried again (`attempts` 4, its parent's `since` past CLOSED). Nothing
// of the stage moves before that attempt (C against C equal at once in every run).
const niStreamDeniedSettle = 2 * time.Second

// niStreamResetSettle is how long the `reset` stage's rows ask again while the sides differ: the stage starts at most
// about 1.3 s after the later side's second handshake (the reset comes at the start of the second after that side was
// seen connected, each side seen within a poll of 250 ms) and up to a connector pass (1 s) more after an earlier
// side's (niStreamFacts' reset), and C tries the parent again 5 s after its handshake (SA-F's probe p1: the reason
// rested 4.77 s, both sides alike), so the rows' rounds end before it while the two sides reconnect within about a
// second of each other. A side that reconnects some 3 s after the other moves the reset past the earlier side's
// postponement (RV-2 S4): the stage then fails, on the oracle's guard when the oracle is the earlier side. The guard's
// 0 data and 0 replication bytes are a timing fact too: C read them about 1 s after its handshake (p2) and had sent
// 5231 replication and 5920 data bytes 3 s after one (SA-F's probe p1, sessions 4 and 5); between the two nothing is
// measured. So a candidate that reconnects a connector pass after the oracle can move the oracle's reset into that
// window (an oracle failure at `traffic`), and a candidate whose replication answers within about 1 s reads SENT
// against C's 0 (red, as the Function's reset row): either is a red run, never a false pass.
const niStreamResetSettle = time.Second

// niStreamRow is one request of TestNodeInstancesStream in r's stage, with its family.
type niStreamRow struct {
	req v2Req
	fam v2Family
}

// niStreamRows are TestNodeInstancesStream's requests in r's stage: `/api/v3/node_instances` in every stage and, on
// the `never` pair, with `options=rfc3339` (the stream's `since` a UTC date, api_v2_contexts.c:391,
// buffer.h:1119-1128; the parents' times stay local, stream-parents.c:186).
func niStreamRows(r *niStreamRun) []niStreamRow {
	fam := niStreamFamily(r)
	switch r.Stage {
	case "denied":
		fam.settle = niStreamDeniedSettle
	case "reset":
		fam.settle = niStreamResetSettle
	}
	rows := []niStreamRow{{fam: fam, req: v2Req{name: "v3-ni", target: "/api/v3/node_instances", status: "200",
		guard: dashGuard(niStreamHost(r, false), niStreamFacts(r, false))}}}
	if r.pair == "never" {
		rows = append(rows, niStreamRow{fam: fam, req: v2Req{name: "v3-ni-rfc3339",
			target: "/api/v3/node_instances?options=rfc3339", status: "200",
			guard: dashGuard(niStreamHost(r, true), niStreamFacts(r, true))}})
	}
	return rows
}

// TestNodeInstancesStream compares `/api/v3/node_instances` of agents that stream out (check
// `api.v2-node-instances-stream`, milestone 10 commit 11, D241 F3): the pairs of runNIStream, each stage's rows
// (niStreamRows). Red on Rust before commit 11's step 11c (no `stream` member).
func TestNodeInstancesStream(t *testing.T) {
	for _, name := range niStreamPairs {
		t.Run(name, func(t *testing.T) {
			runNIStream(t, name, func(t *testing.T, r *niStreamRun) {
				for _, row := range niStreamRows(r) {
					t.Run(row.req.name, func(t *testing.T) { compareV2(t, r.Pair, row.req, row.fam) })
				}
			})
		})
	}
	t.Run("load", niStreamLoad)
}
