// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The load check of the node instances' stream (R110's run-only item 10; D220 fork 6: a reader never waits for a
// connect): `TestNodeInstancesStream/load`, a flapping parent with a replication backlog while `node_instances` and
// `netdata-streaming` are asked hard. Nothing of the answers is compared (each moment differs); each side is held to
// answering every request, each within niStreamLoadLimit.

const (
	// niStreamLoadSpan is how long the parent flaps while the agents are asked.
	niStreamLoadSpan = 40 * time.Second
	// niStreamLoadFlap is the time between two resets of every session: past the parent's postponement after a
	// handshake (5 s, stream-connector.c:236-238), so each sender connects again at its connector's next pass.
	niStreamLoadFlap = 6 * time.Second
	// niStreamLoadLimit is the longest an answer may take: C's slowest was 8 ms in one run and 519 ms (on both sides
	// at once: the box) in another (SA-F's probe p1); a reader that waited for the stream thread or a connect would
	// wait seconds, a deadlock until the client's timeout (30 s).
	niStreamLoadLimit = 5 * time.Second
	// niStreamLoadAnswers is the fewest answers each client must have had: C gave about 60000 per client in the span.
	niStreamLoadAnswers = 200
)

// niStreamLoadTargets are the requests each side is asked back to back, each by a client of its own: the node
// instances twice, the admin's table once.
var niStreamLoadTargets = []string{"/api/v3/node_instances", "/api/v3/node_instances",
	"/api/v1/function?function=netdata-streaming"}

// niStreamLoadStat is what one client saw: how many answers, how many were not as niStreamLoadAnswer holds (the first
// of them), and the slowest.
type niStreamLoadStat struct {
	answers, bad int
	first        string
	slowest      time.Duration
}

// niStreamLoadAnswer tells whether b answers target: a 200 whose body is the JSON of localhost's instance with its
// `stream` (node_instances: rrdhost_sender_to_json(), api_v2_contexts.c:381-433, for a host with a sender), or a
// table of one row (netdata-streaming: localhost alone).
func niStreamLoadAnswer(target string, b []byte) error {
	if s := fnStatusLine(b); s != "HTTP/1.1 200 OK" {
		return fmt.Errorf("answered %q", truncateBytes(b))
	}
	v, err := ParseJSON(httpBody(b))
	if err != nil {
		return err
	}
	if strings.Contains(target, "node_instances") {
		_, err = dashAt(v, "nodes", "[0]", "instances", "[0]", "stream", "status")
		return err
	}
	if err := dashIs(`"table"`, "type")(v); err != nil {
		return err
	}
	if _, err := dashAt(v, "data", "[0]"); err != nil {
		return err
	}
	_, err = dashAt(v, "data", "[1]")
	if err == nil {
		return fmt.Errorf("a table of more than one row")
	}
	return nil
}

// niStreamLoadJudge holds one client's niStreamLoadStat: every answer as niStreamLoadAnswer holds, none slower than
// niStreamLoadLimit, at least niStreamLoadAnswers of them.
func niStreamLoadJudge(target string, st niStreamLoadStat) error {
	switch {
	case st.bad > 0:
		return fmt.Errorf("%s: %d of %d answers are not its own; the first: %s", target, st.bad, st.answers, st.first)
	case st.slowest > niStreamLoadLimit:
		return fmt.Errorf("%s: an answer took %v, more than %v", target, st.slowest, niStreamLoadLimit)
	case st.answers < niStreamLoadAnswers:
		return fmt.Errorf("%s: %d answers in %v, fewer than %d", target, st.answers, niStreamLoadSpan,
			niStreamLoadAnswers)
	}
	return nil
}

// niStreamLoadReplay is the load's parent's answer at each chart's definition end and at each replication answer's
// end: the chart's last 10 minutes again, so its replication never ends (a backlog that does not drain).
func niStreamLoadReplay(ev stream.ReplayEvent) []string {
	now := time.Now().Unix()
	return []string{fmt.Sprintf(`REPLAY_CHART "%s" "false" %d %d`, ev.Chart, now-600, now)}
}

// niStreamLoad (TestNodeInstancesStream/load): both agents (pulseChildOptions, the bearer tokens) stream to one
// scripted parent that drops compression and asks every chart's replication again at each answer's end
// (niStreamLoadReplay); once each side is online and counted, for niStreamLoadSpan the parent resets every session each
// niStreamLoadFlap while each side is asked niStreamLoadTargets back to back, then each client is judged
// (niStreamLoadJudge: the oracle's failure ends the case, a candidate's is reported), and each sender must connect
// again within 30 s.
func niStreamLoad(t *testing.T) {
	stub, err := stream.StartParent(func(r stream.Request) stream.Answer {
		return stream.Answer{Reply: stream.VCaps(r.Caps() &^ stream.CapsCompression), Replay: niStreamLoadReplay}
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stub.Close() })
	p := niStreamPair(t, pulseChildOptions(stub, true))
	niStreamStage(t, p, "online and counted", 60*time.Second, niStreamCounted)
	stop := make(chan struct{})
	stats := make([][]niStreamLoadStat, 2)
	var wg sync.WaitGroup
	for i, side := range p.Each() {
		stats[i] = make([]niStreamLoadStat, len(niStreamLoadTargets))
		for k, target := range niStreamLoadTargets {
			req := v2Request(v2Req{target: target, headers: []string{fnBuiltinsUsers[2].header}})
			wg.Add(1)
			go func() {
				defer wg.Done()
				st := &stats[i][k]
				for {
					select {
					case <-stop:
						return
					default:
					}
					from := time.Now()
					b, err := rawExchange(side.Daemon.Addr, req, 30*time.Second)
					st.slowest = max(st.slowest, time.Since(from))
					st.answers++
					if err == nil {
						err = niStreamLoadAnswer(target, b)
					}
					if err != nil {
						if st.bad == 0 {
							st.first = err.Error()
						}
						st.bad++
					}
				}
			}()
		}
	}
	t0, flaps := time.Now(), 0
	for time.Since(t0)+niStreamLoadFlap <= niStreamLoadSpan {
		time.Sleep(niStreamLoadFlap)
		for _, s := range stub.Sessions() {
			if !s.Closed() {
				_ = s.Reset()
				flaps++
			}
		}
	}
	close(stop)
	wg.Wait()
	t.Logf("%d resets, %d sessions; oracle %+v; candidate %+v", flaps, len(stub.Sessions()), stats[0], stats[1])
	for i, side := range p.Each() {
		for k, target := range niStreamLoadTargets {
			err := niStreamLoadJudge(target, stats[i][k])
			switch {
			case err == nil:
			case side.Role == Oracle:
				t.Fatalf("oracle: %v", err)
			default:
				t.Errorf("candidate: %v", err)
			}
		}
	}
	niStreamStage(t, p, "online again after the resets", 30*time.Second, niStreamCounted)
}
