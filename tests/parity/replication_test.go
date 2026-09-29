// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"regexp"
	"slices"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// replayEdges are the charts the edges check asks for itself, each with its request (`now` the wall clock when it
// asks, once the child has two minutes of data), and what C answers for it.
var replayEdges = map[string]func(now int64) string{
	// reversed: swapped, false, points
	"netdata.uptime": func(now int64) string { return fmt.Sprintf(`"false" %d %d`, now-110, now-115) },
	// no window: every exposed dimension's state, true with 0 0
	"netdata.clients": func(int64) string { return `"false" 0 0` },
	// no after
	"netdata.requests": func(now int64) string { return fmt.Sprintf(`"false" 0 %d`, now-110) },
	// an after that is no number: both ends 0
	"netdata.response_time": func(now int64) string { return fmt.Sprintf(`"false" 12a %d`, now-110) },
	// in the future: as no window
	"netdata.memory": func(now int64) string { return fmt.Sprintf(`"false" %d %d`, now+100, now+200) },
	// a start flag with spaces in its quotes: malformed (an ERR), false
	"netdata.server_cpu": func(now int64) string { return fmt.Sprintf(`" true  " %d %d`, now-120, now-115) },
}

// replayRequestLines are the edges check's requests: its charts', a chart the child does not have, and one with an
// empty id, which is never answered.
func replayRequestLines(now int64) []string {
	var lines []string
	for _, chart := range slices.Sorted(maps.Keys(replayEdges)) {
		lines = append(lines, `REPLAY_CHART "`+chart+`" `+replayEdges[chart](now))
	}
	return append(lines, `REPLAY_CHART "no.such.chart" "true" 1 2`, `REPLAY_CHART "" "true" 1 2`)
}

var (
	replayStepRe  = regexp.MustCompile(`^RBEGIN (?:SLOT:\S+ )?'' `)
	replayDimRe   = regexp.MustCompile(`^(RSET|RDSTATE) (?:SLOT:\S+ )?["']([^"']*)["']`)
	replayEndRe   = regexp.MustCompile(`^REND (\S+) (\S+) (\S+) (true  |false )(\S+) (\S+) (\S+)$`)
	replayStartRe = regexp.MustCompile(`^RBEGIN (?:SLOT:\S+ )?'([^']+)'$`)
)

// isZero is 0 in either of the answers' encodings.
func isZero(v string) string {
	if v == "0" || v == "#A" {
		return "0"
	}
	return "N"
}

// replayAnswers are each chart's replication answers, in order: the steps as one line with their count, the RSET
// dimensions as a set, the RDSTATE dimensions in order, RSSTATE, and REND with its verdict's bytes and whether each
// number is 0.
func replayAnswers(data []byte) map[string][]string {
	answers := map[string][]string{}
	var chart string
	var steps int
	var sets map[string]bool
	var lines []string
	for _, line := range strings.Split(string(data), "\n") {
		switch {
		case replayStartRe.MatchString(line):
			chart, steps, sets, lines = replayStartRe.FindStringSubmatch(line)[1], 0, map[string]bool{}, nil
		case chart == "":
		case replayStepRe.MatchString(line):
			steps++
		case replayDimRe.MatchString(line):
			m := replayDimRe.FindStringSubmatch(line)
			if m[1] == "RSET" {
				sets[m[2]] = true
			} else {
				lines = append(lines, "RDSTATE "+m[2])
			}
		case strings.HasPrefix(line, "RSSTATE "):
			lines = append(lines, "RSSTATE")
		case replayEndRe.MatchString(line):
			m := replayEndRe.FindStringSubmatch(line)
			answer := []string{fmt.Sprintf("steps %d", steps), "RSET " + strings.Join(slices.Sorted(maps.Keys(sets)), ",")}
			answer = append(answer, lines...)
			answer = append(answer, fmt.Sprintf("REND %s %s %s %q %s %s %s", isZero(m[1]), isZero(m[2]), isZero(m[3]),
				m[4], isZero(m[5]), isZero(m[6]), isZero(m[7])))
			answers[chart] = append(answers[chart], strings.Join(answer, " | "))
			chart = ""
		}
	}
	return answers
}

// replayRecordRe are a child's records about the requests it got.
var replayRecordRe = regexp.MustCompile(`msg="(STREAM SND REPLAY|REPLAY: malformed)`)

// TestReplicationEdges (check `stream.rchild-replication/edges`, milestone 7 commit 6, D105; map
// `knowledge/map-m7-commit6-replication.md` §14.2): a C child and a Rust child in turn stream to a scripted parent
// that starts every chart's streaming at its definition except the check's own, which it asks for itself once the
// child has two minutes of data: a reversed window, no window, no after, an after that is no number, a window in
// the future, a malformed start flag, a chart the child does not have and an empty id. Compared: each of those
// charts' answers (their lines' kinds, the steps counted, the dimensions, REND's verdict bytes and which numbers are
// 0), that the empty id got no answer, and the children's records about the requests, as sets.
func TestReplicationEdges(t *testing.T) {
	bins := binaries(t)
	var answers [2]map[string][]string
	var records, summaries [2][]string
	for i, role := range []Role{"replay-edges-oracle", "replay-edges-candidate"} {
		var mu sync.Mutex
		defined := map[string]bool{}
		parent, err := stream.StartParent(func(r stream.Request) stream.Answer {
			a := stream.PlaintextAnswer(r)
			a.Replay = func(ev stream.ReplayEvent) []string {
				if ev.Answer {
					return nil
				}
				mu.Lock()
				defined[ev.Chart] = true
				mu.Unlock()
				if _, own := replayEdges[ev.Chart]; own {
					return nil
				}
				return []string{`REPLAY_CHART "` + ev.Chart + `" "true" 0 0`}
			}
			return a
		})
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { parent.Close() })
		d := senderChild(t, bins[i], role, parent, "")
		session := parent.WaitSession(1, 60*time.Second)
		if session == nil {
			t.Fatalf("%s: no STREAM connection within 60 s", role)
		}
		// two minutes of data before the requests, so a window older than 100 intervals stays false
		time.Sleep(125 * time.Second)
		mu.Lock()
		for chart := range replayEdges {
			if !defined[chart] {
				t.Errorf("%s: the child never defined %s", role, chart)
			}
		}
		mu.Unlock()
		if err := session.Send(replayRequestLines(time.Now().Unix())...); err != nil {
			t.Fatal(err)
		}
		deadline := time.Now().Add(20 * time.Second)
		for {
			got := replayAnswers(session.Data())
			if len(got) >= len(replayEdges)+1 || time.Now().After(deadline) {
				break
			}
			time.Sleep(200 * time.Millisecond)
		}
		// the empty id's answer, if any, would have come by now
		time.Sleep(3 * time.Second)
		// the summary after these answers, about 30 s after the last (the first came after the start's answers)
		deadline = time.Now().Add(70 * time.Second)
		for len(replaySummaries(t, d)) < 2 && time.Now().Before(deadline) {
			time.Sleep(time.Second)
		}
		_ = d.Stop()
		answers[i] = replayAnswers(session.Data())
		records[i] = childReplayRecords(t, d)
		summaries[i] = replaySummaryTotals(replaySummaries(t, d))
	}
	for _, chart := range append(slices.Sorted(maps.Keys(replayEdges)), "no.such.chart") {
		if len(answers[0][chart]) != 1 {
			t.Errorf("the oracle answered %s %d times: %v", chart, len(answers[0][chart]), answers[0][chart])
		}
		diffLines(t, "answers of "+chart, answers[0][chart], answers[1][chart])
	}
	for i, a := range answers {
		if _, ok := a[""]; ok {
			t.Errorf("side %d answered the empty chart id", i)
		}
	}
	diffLines(t, "the children's replication records", records[0], records[1])
	diffLines(t, "the children's replication summaries", summaries[0], summaries[1])
	if len(summaries[0]) == 0 || !strings.Contains(strings.Join(summaries[0], "\n"), "ignored-not-found") {
		t.Errorf("the oracle's summaries lack the not-found request: %v", summaries[0])
	}
	var lines []string
	for chart, a := range answers[0] {
		lines = append(lines, chart+": "+strings.Join(a, " || "))
	}
	sort.Strings(lines)
	t.Logf("oracle answers:\n%s", strings.Join(lines, "\n"))
	t.Logf("oracle records:\n%s", strings.Join(records[0], "\n"))
	t.Logf("oracle summaries:\n%s", strings.Join(summaries[0], "\n"))
}

var summaryRe = regexp.MustCompile(`msg="REPLICATION SEND SUMMARY: all senders finished replication\. Received (\d+), ` +
	`executed (\d+) and replied to (\d+) requests\. ([^"]*)"`)

// replaySummaries are a child's REPLICATION SEND SUMMARY records, in order.
func replaySummaries(t *testing.T, d *daemon.Daemon) [][]string {
	t.Helper()
	var out [][]string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := summaryRe.FindStringSubmatch(l); m != nil {
			out = append(out, m[1:])
		}
	}
	return out
}

// replaySummaryTotals are the summaries' counts added up, and each summary's remainders: how the requests divide
// between two summaries depends on when the lazy charts came.
func replaySummaryTotals(summaries [][]string) []string {
	var received, executed, replied int
	out := []string{fmt.Sprintf("summaries %d", len(summaries))}
	for _, s := range summaries {
		var r, e, p int
		fmt.Sscan(s[0], &r)
		fmt.Sscan(s[1], &e)
		fmt.Sscan(s[2], &p)
		received, executed, replied = received+r, executed+e, replied+p
		out = append(out, "remainders: "+s[3])
	}
	return append(out, fmt.Sprintf("received %d, executed %d, replied %d", received, executed, replied))
}

// childReplayRecords are a child's records about the replication requests it got, each once, normalized.
func childReplayRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	seen := map[string]bool{}
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if !replayRecordRe.MatchString(l) {
			continue
		}
		n := normalizeLog(l, d.Opts.RunDir, "")
		n = anyLocalPortRe.ReplaceAllString(n, "127.0.0.1${1}P")
		n = anyDstPortRe.ReplaceAllString(n, " dst_port=P")
		n = threadNRe.ReplaceAllString(n, "${1}[n]")
		n = replayThreadRe.ReplaceAllString(n, "thread=REPLAY[n]")
		// the request's window, which each run asks for at its own time
		n = epochSecondsRe.ReplaceAllString(n, "S")
		if !seen[n] {
			seen[n] = true
			out = append(out, n)
		}
	}
	sort.Strings(out)
	return out
}

var (
	// replayThreadRe is the REPLAY thread that answered, whichever of them it was.
	replayThreadRe = regexp.MustCompile(`thread=REPLAY\[\d+\]`)
	epochSecondsRe = regexp.MustCompile(`\b1\d{9}\b`)
)
