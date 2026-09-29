// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"net"
	"os"
	"regexp"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// relaySession is one STREAM connection through a relay: what the child sent up and what the parent sent down.
type relaySession struct {
	mu       sync.Mutex
	up, down bytes.Buffer
}

func (s *relaySession) data() ([]byte, []byte) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return bytes.Clone(s.up.Bytes()), bytes.Clone(s.down.Bytes())
}

type relayWriter struct {
	s    *relaySession
	down bool
}

func (w relayWriter) Write(b []byte) (int, error) {
	w.s.mu.Lock()
	defer w.s.mu.Unlock()
	if w.down {
		return w.s.down.Write(b)
	}
	return w.s.up.Write(b)
}

// relay passes a child's connections to its parent, recording its STREAM sessions both ways (probes pass through
// unrecorded).
type relay struct {
	addr     string
	mu       sync.Mutex
	sessions []*relaySession
}

func startRelay(t *testing.T, target string) *relay {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	r := &relay{addr: ln.Addr().String()}
	go func() {
		for {
			child, err := ln.Accept()
			if err != nil {
				return
			}
			go r.pass(child, target)
		}
	}()
	return r
}

func (r *relay) pass(child net.Conn, target string) {
	defer child.Close()
	parent, err := net.Dial("tcp", target)
	if err != nil {
		return
	}
	defer parent.Close()
	br := bufio.NewReader(child)
	first, err := br.Peek(7)
	var s *relaySession
	if err == nil && string(first) == "STREAM " {
		s = &relaySession{}
		r.mu.Lock()
		r.sessions = append(r.sessions, s)
		r.mu.Unlock()
	}
	go func() {
		if s != nil {
			_, _ = io.Copy(child, io.TeeReader(parent, relayWriter{s, true}))
		} else {
			_, _ = io.Copy(child, parent)
		}
		_ = child.Close()
	}()
	if s != nil {
		_, _ = io.Copy(parent, io.TeeReader(br, relayWriter{s, false}))
	} else {
		_, _ = io.Copy(parent, br)
	}
}

func (r *relay) session(n int) *relaySession {
	r.mu.Lock()
	defer r.mu.Unlock()
	if n < len(r.sessions) {
		return r.sessions[n]
	}
	return nil
}

var definedChartRe = regexp.MustCompile(`(?m)^CHART (?:SLOT:\S+ )?"([^"]*)"`)

// replayVerdicts are each chart's answers' verdicts in a session, the falses run together: "false+ true" for a
// replication in rounds.
func replayVerdicts(up []byte) map[string]string {
	out := map[string]string{}
	for chart, answers := range replayAnswers(up) {
		var verdicts []string
		for _, a := range answers {
			v := "false"
			if strings.Contains(a, `"true  "`) {
				v = "true"
			}
			if n := len(verdicts); n > 0 && verdicts[n-1] == "false+" && v == "false" {
				continue
			}
			if v == "false" {
				v = "false+"
			}
			verdicts = append(verdicts, v)
		}
		out[chart] = strings.Join(verdicts, " ")
	}
	return out
}

// allStreaming tells whether every chart the child defined in the session got an answer that starts streaming.
func allStreaming(up []byte) bool {
	verdicts := replayVerdicts(up)
	defined := definedChartRe.FindAllSubmatch(up, -1)
	if len(defined) == 0 {
		return false
	}
	for _, m := range defined {
		if !strings.HasSuffix(verdicts[string(m[1])], "true") {
			return false
		}
	}
	return true
}

// gapCharts are the charts whose replicated data the gap check compares with the child's own.
var gapCharts = []string{"netdata.server_cpu", "netdata.uptime", "netdata.db_samples_collected"}

func seriesOf(t *testing.T, d *daemon.Daemon, prefix, chart string, after, before int64) string {
	t.Helper()
	req := fmt.Sprintf("GET %s/api/v1/data?chart=%s&after=%d&before=%d&points=%d&group=average&format=csv "+
		"HTTP/1.1\r\nConnection: close\r\n\r\n", prefix, chart, after, before, before-after+1)
	b, err := rawExchange(d.Addr, []byte(req), 10*time.Second)
	if err != nil {
		t.Errorf("%s%s %s: %v", d.Addr, prefix, chart, err)
		return ""
	}
	return string(httpBody(b))
}

// gapRun is what one child's run of the gap check found.
type gapRun struct {
	verdicts map[string]string
	records  []string
	summary  string
}

// TestReplicationGap (check `stream.rchild-replication/gap`, PARITY_LONG; milestone 7 commit 6, D105; map
// `knowledge/map-m7-commit6-replication.md` §14.1): a C child and a Rust child in turn (dbengine, one tier, no
// compression) stream through a relay to a C parent (dbengine, one tier, 20 s replication steps). Once every chart
// streams and the child has 150 s of data, the parent stops for 180 s and starts again on its run directory; the
// child replicates the gap in rounds, false until a round ends within 100 s of now. Within a run: over the gap, the parent's series of the child equal the child's
// own. Between the runs: each chart's rounds in the second session (`false+ true`), the parent's receiver records,
// and the child's summary after the replication (received, executed and replied equal, no remainder).
func TestReplicationGap(t *testing.T) {
	if os.Getenv("PARITY_LONG") != "1" {
		t.Skip("PARITY_LONG=1 runs the replication gap check (about 20 minutes)")
	}
	const hostname, guid = "parity-gap-child", "5a1e0000-0000-4000-8000-00000000c0dd"
	bins := binaries(t)
	var runs [2]gapRun
	for i, role := range []Role{"gap-oracle", "gap-candidate"} {
		parent, err := daemon.Start(daemon.Options{Binary: bins[0], RunDir: runDir(t, Role(string(role)+"-parent")),
			Identity: &parentIdentity, StorageTiers: 1, ReplicationStepSeconds: 20})
		if err != nil {
			t.Fatalf("%s: start parent: %v", role, err)
		}
		t.Cleanup(func() { _ = parent.Stop() })
		r := startRelay(t, parent.Addr)
		child, err := daemon.Start(daemon.Options{Binary: bins[i], RunDir: runDir(t, Role(string(role)+"-child")),
			StorageTiers: 1, NoStreamKey: true,
			Identity: &daemon.Identity{Hostname: hostname, StreamKey: cChildKey, MachineGUID: guid},
			StreamTo: &daemon.StreamTo{Destination: r.addr, APIKey: parentIdentity.StreamKey,
				Extra: "    reconnect delay = 5\n"}})
		if err != nil {
			t.Fatalf("%s: start child: %v", role, err)
		}
		t.Cleanup(func() { _ = child.Stop() })
		waitSession := func(n int, limit time.Duration) *relaySession {
			deadline := time.Now().Add(limit)
			for time.Now().Before(deadline) {
				if s := r.session(n); s != nil {
					if up, _ := s.data(); allStreaming(up) {
						return s
					}
				}
				time.Sleep(time.Second)
			}
			return nil
		}
		if waitSession(0, 120*time.Second) == nil {
			t.Fatalf("%s: session 1: not every chart streams within 120 s", role)
		}
		time.Sleep(150 * time.Second)
		// the parent's last point of each compared chart before the gap
		last := map[string]int64{}
		for _, chart := range gapCharts {
			last[chart] = jsonNumber(t, parent, "/host/"+hostname+"/api/v1/chart?chart="+chart, "last_entry")
		}
		if err := parent.Stop(); err != nil {
			t.Fatalf("%s: stop parent: %v", role, err)
		}
		// longer than the 100 intervals before now that a child answers as streaming, so the gap comes in rounds
		time.Sleep(180 * time.Second)
		restarted := time.Now().Unix()
		if err := parent.Restart(); err != nil {
			t.Fatalf("%s: restart parent: %v", role, err)
		}
		s := waitSession(1, 240*time.Second)
		if s == nil {
			t.Fatalf("%s: session 2: not every chart streams within 240 s of the restart", role)
		}
		time.Sleep(10 * time.Second)
		for _, chart := range gapCharts {
			after, before := last[chart]+1, restarted-5
			own := seriesOf(t, child, "", chart, after, before)
			replicated := seriesOf(t, parent, "/host/"+hostname, chart, after, before)
			if rows := strings.Count(own, "\n"); rows < 50 {
				t.Errorf("%s: %s: the child's own series has %d rows over the gap", role, chart, rows)
			} else if own != replicated {
				t.Errorf("%s: %s: the parent's series over the gap [%d, %d] differ from the child's own:\n%s",
					role, chart, after, before, firstDifference([]byte(own), []byte(replicated)))
			}
		}
		up, _ := s.data()
		runs[i].verdicts = replayVerdicts(up)
		// the summary after the second session's answers, about 30 s after the last
		deadline := time.Now().Add(60 * time.Second)
		for time.Now().Before(deadline) {
			if n := len(replaySummaries(t, child)); n >= 2 {
				break
			}
			time.Sleep(time.Second)
		}
		summaries := replaySummaries(t, child)
		if len(summaries) < 2 {
			t.Errorf("%s: %d summaries", role, len(summaries))
		} else {
			last := summaries[len(summaries)-1]
			runs[i].summary = fmt.Sprintf("received = executed = replied: %v, remainders %q",
				last[0] == last[1] && last[1] == last[2], last[3])
		}
		_ = child.Stop()
		_ = parent.Stop()
		for _, r := range parentRecords(t, parent, "STREAM RCV", nil) {
			// a Rust child is not ML capable (D101.5)
			runs[i].records = append(runs[i].records, mlCapableRe.ReplaceAllString(r, "ml_capable=M"))
		}
	}
	var charts [2][]string
	for i, run := range runs {
		for chart, v := range run.verdicts {
			charts[i] = append(charts[i], chart+": "+v)
		}
		slices.Sort(charts[i])
	}
	diffLines(t, "the second session's rounds", charts[0], charts[1])
	if !slices.ContainsFunc(charts[0], func(l string) bool { return strings.HasSuffix(l, ": false+ true") }) {
		t.Errorf("the oracle replicated no chart in rounds: %v", charts[0])
	}
	diffLines(t, "the parent's receiver records", runs[0].records, runs[1].records)
	if runs[0].summary != runs[1].summary {
		t.Errorf("the summaries differ:\noracle:    %s\ncandidate: %s", runs[0].summary, runs[1].summary)
	}
	t.Logf("oracle rounds:\n%s", strings.Join(charts[0], "\n"))
	t.Logf("oracle summary: %s", runs[0].summary)
}
