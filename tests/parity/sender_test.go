// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// capture is a child's stream as a recording parent received it, normalized for comparison (brief
// `knowledge/brief-stream-sender.md` §6): the request's parameters and headers, the session's start (host labels
// as a set: C prints them in heap order), each chart's definition keyed by id (its times masked) and how many times
// it came, and each chart's data as the dimensions and flags of its blocks (values and times masked, the number of
// blocks left out; v1 blocks with their time reduced to zero or not); slot numbers masked, each chart's blocks
// checked against its definition's slot; chart labels as a set.
type capture struct {
	request []string
	start   []string
	charts  map[string][]string
	// defs counts each chart's definitions: a version that moves at every collection would redefine it each time; a
	// chart's entry in charts holds every definition, in order
	defs map[string]int
	data map[string][]string
	// v1zero counts each chart's v1 blocks sent with no time since the last update (the resync horizon)
	v1zero map[string]int
	// dimSlots are each chart's dimensions' slots in its last definition, which its SET2 lines must use
	dimSlots map[string]map[string]string
	// replays are each chart's replication answers: their lines' kinds and dimensions
	replays map[string][]string
	other   []string
	// slots are each chart's slot in its definition, which its blocks must use (the numbers follow the charts'
	// creation order, which varies)
	slots map[string]string
}

var (
	slotRe      = regexp.MustCompile(`SLOT:\S+ `)
	localPortRe = regexp.MustCompile(`127\.0\.0\.1:\d+`)
	replayRe    = regexp.MustCompile(`^(RBEGIN|RSET|RDSTATE|RSSTATE|REND)(?: SLOT:\S+)?(?: ('[^']*'|"[^"]*"))?`)
	// C's REND writes two spaces after true
	replayVerdictRe = regexp.MustCompile(` (true  |false )`)
	chartLineRe     = regexp.MustCompile(`^CHART (?:SLOT:\S+ )?"([^"]*)"`)
	begin2Re        = regexp.MustCompile(`^BEGIN2 (?:SLOT:\S+ )?'([^']*)' `)
	set2Re          = regexp.MustCompile(`^SET2 (?:(SLOT:\S+) )?'([^']*)' \S+ \S+ (\S*)$`)
	dimensionRe     = regexp.MustCompile(`^DIMENSION (?:(SLOT:\S+) )?"([^"]*)"`)
	definitionEndRe = regexp.MustCompile(`^CHART_DEFINITION_END .*`)
	v1BeginRe       = regexp.MustCompile(`^BEGIN "([^"]*)" (\d+)$`)
	v1SetRe         = regexp.MustCompile(`^SET "([^"]*)" = \S+$`)
)

// parseCapture splits a plaintext stream into its parts.
func parseCapture(req stream.Request, data []byte) capture {
	c := capture{charts: map[string][]string{}, defs: map[string]int{}, data: map[string][]string{},
		slots: map[string]string{}, replays: map[string][]string{}, v1zero: map[string]int{},
		dimSlots: map[string]map[string]string{}}
	// data must follow their chart's definition in the session
	defined := func(chart, what string) {
		if c.defs[chart] == 0 {
			c.other = append(c.other, what+" of "+chart+" before its definition")
		}
	}
	params := make([]string, 0, len(req.Params))
	for k, v := range req.Params {
		if k == "ml_capable" {
			// no ML here (D101.5)
			v = []string{"M"}
		}
		params = append(params, k+"="+strings.Join(v, ","))
	}
	sort.Strings(params)
	c.request = append(params, req.Headers...)
	var chart string // the chart whose definition or block is being read
	var labels, clabels []string
	var block []string
	v1Time := "" // the open v1 block's time: 0, or N for any other
	inPath := false
	for _, line := range strings.Split(string(data), "\n") {
		switch {
		case line == "":
		case inPath:
			if line == "JSON_PAYLOAD_END" {
				inPath = false
				c.start = append(c.start, "JSON STREAM_PATH <payload>")
			}
		case line == "JSON STREAM_PATH":
			inPath = true
		case strings.HasPrefix(line, "LABEL "):
			// each child's parent listens on its own port
			labels = append(labels, localPortRe.ReplaceAllString(line, "127.0.0.1:P"))
		case line == "OVERWRITE labels":
			sort.Strings(labels)
			c.start = append(append(c.start, labels...), line)
			labels = nil
		case strings.HasPrefix(line, "FUNCTION "), strings.HasPrefix(line, "FUNCTION_DEL "):
			// the functions' catalogue comes with the functions milestone, M8 (D100.9)
		case strings.HasPrefix(line, "CLAIMED_ID "), strings.HasPrefix(line, "VARIABLE HOST "):
			c.start = append(c.start, line)
		case chartLineRe.MatchString(line):
			chart = chartLineRe.FindStringSubmatch(line)[1]
			c.slots[chart] = slotRe.FindString(line)
			if c.defs[chart] > 0 {
				c.charts[chart] = append(c.charts[chart], "--- defined again")
			}
			c.charts[chart] = append(c.charts[chart], slotRe.ReplaceAllString(line, "SLOT:N "))
			c.defs[chart]++
			c.dimSlots[chart] = map[string]string{}
			clabels = nil
		case strings.HasPrefix(line, "CLABEL "):
			clabels = append(clabels, line)
		case line == "CLABEL_COMMIT":
			// C prints a chart's labels in heap order
			sort.Strings(clabels)
			c.charts[chart] = append(append(c.charts[chart], clabels...), line)
			clabels = nil
		case strings.HasPrefix(line, "DIMENSION "), strings.HasPrefix(line, "VARIABLE CHART "):
			if m := dimensionRe.FindStringSubmatch(line); m != nil && c.dimSlots[chart] != nil {
				c.dimSlots[chart][m[2]] = m[1]
			}
			c.charts[chart] = append(c.charts[chart], slotRe.ReplaceAllString(line, "SLOT:N "))
		case definitionEndRe.MatchString(line):
			c.charts[chart] = append(c.charts[chart], "CHART_DEFINITION_END <times>")
		case begin2Re.MatchString(line):
			chart = begin2Re.FindStringSubmatch(line)[1]
			defined(chart, "BEGIN2")
			if slot := slotRe.FindString(line); slot != c.slots[chart] {
				c.other = append(c.other, "BEGIN2 of "+chart+" in "+slot+", its definition's "+c.slots[chart])
			}
			block = nil
		case set2Re.MatchString(line):
			m := set2Re.FindStringSubmatch(line)
			if want := c.dimSlots[chart][m[2]]; m[1] != want {
				c.other = append(c.other, "SET2 of "+chart+"/"+m[2]+" in "+m[1]+", its definition's "+want)
			}
			block = append(block, m[2]+" "+m[3])
		case replayRe.MatchString(line):
			m := replayRe.FindStringSubmatch(line)
			if m[1] == "RBEGIN" && m[2] != "''" {
				chart = strings.Trim(m[2], "'")
				c.replays[chart] = nil
				continue
			}
			// the line's kind and dimension; points, values and times masked, REND's verdict kept
			shape := m[1]
			if m[1] == "RSET" || m[1] == "RDSTATE" {
				shape += " " + m[2]
			}
			if m[1] == "REND" {
				shape += " " + replayVerdictRe.FindString(line)
			}
			if m[1] != "RBEGIN" && m[1] != "RSET" || !slices.Contains(c.replays[chart], shape) {
				c.replays[chart] = append(c.replays[chart], shape)
			}
		case line == "END2":
			shape := strings.Join(block, ",")
			if !slices.Contains(c.data[chart], shape) {
				c.data[chart] = append(c.data[chart], shape)
			}
		case v1BeginRe.MatchString(line):
			m := v1BeginRe.FindStringSubmatch(line)
			chart, block, v1Time = m[1], nil, "N"
			defined(chart, "BEGIN")
			if m[2] == "0" {
				v1Time = "0"
				c.v1zero[chart]++
			}
		case v1SetRe.MatchString(line):
			block = append(block, v1SetRe.FindStringSubmatch(line)[1])
		case line == "END":
			shape := "v1 " + v1Time + " " + strings.Join(block, ",")
			if !slices.Contains(c.data[chart], shape) {
				c.data[chart] = append(c.data[chart], shape)
			}
		default:
			c.other = append(c.other, line)
		}
	}
	for id := range c.data {
		sort.Strings(c.data[id])
	}
	return c
}

// compareCaptures reports how two captures differ.
func compareCaptures(t *testing.T, stage string, a, b capture) {
	t.Helper()
	diff := func(what string, x, y []string) {
		if !slices.Equal(x, y) {
			t.Errorf("%s: %s differ:\noracle:\n%s\ncandidate:\n%s", stage, what, strings.Join(x, "\n"),
				strings.Join(y, "\n"))
		}
	}
	diff("requests", a.request, b.request)
	diff("session starts", a.start, b.start)
	keys := func(m map[string][]string) []string {
		var k []string
		for id := range m {
			k = append(k, id)
		}
		sort.Strings(k)
		return k
	}
	diff("charts", keys(a.charts), keys(b.charts))
	for _, id := range keys(a.charts) {
		if y, ok := b.charts[id]; ok {
			diff("chart "+id, a.charts[id], y)
			if a.defs[id] != b.defs[id] {
				t.Errorf("%s: chart %s defined %d times by the oracle, %d by the candidate", stage, id, a.defs[id],
					b.defs[id])
			}
		}
	}
	diff("charts with data", keys(a.data), keys(b.data))
	for _, id := range keys(a.charts) {
		if a.v1zero[id] != b.v1zero[id] {
			t.Errorf("%s: chart %s sent %d v1 blocks with no time by the oracle, %d by the candidate", stage, id,
				a.v1zero[id], b.v1zero[id])
		}
	}
	for _, id := range keys(a.data) {
		if y, ok := b.data[id]; ok {
			diff("data of "+id, a.data[id], y)
		}
	}
	diff("replicated charts", keys(a.replays), keys(b.replays))
	for _, id := range keys(a.replays) {
		if y, ok := b.replays[id]; ok {
			diff("replication of "+id, a.replays[id], y)
		}
	}
	diff("other lines", a.other, b.other)
}

// senderChild boots a daemon streaming to `parent` as a child with its own identity, `extra` in its [stream]
// section.
func senderChild(t *testing.T, bin string, role Role, parent *stream.Parent, extra string) *daemon.Daemon {
	t.Helper()
	id := daemon.Identity{Hostname: "sender-child", StreamKey: "5a1e0000-0000-4000-8000-0000000000c1",
		MachineGUID: "5a1e0000-0000-4000-8000-0000000000cc"}
	o := daemon.Options{Binary: bin, RunDir: runDir(t, role), Identity: &id, DBMode: "alloc", StorageTiers: 1,
		NoStreamKey: true, StreamTo: &daemon.StreamTo{Destination: parent.Addr(), APIKey: parentIdentity.StreamKey,
			Extra: "    reconnect delay = 5\n" + extra}}
	d, err := daemon.Start(o)
	if err != nil {
		t.Fatalf("parity: start %s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	return d
}

// senderRecordRe are the records of the per-collection gate and of the connection's reset, in the order the
// collector thread writes them.
var senderRecordRe = regexp.MustCompile(`msg="(STREAM SND '[^']*': streaming is (not )?ready|STREAM REPLAY: sender replicating-charts counter)`)

// senderRecords are a child's gate and reset records, in order.
func senderRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := senderRecordRe.FindString(l); m != "" {
			out = append(out, threadOf(l)+" "+m)
		}
	}
	return out
}

// senderVariant is what a recording parent negotiates with a sender child, and what happens during the session.
type senderVariant struct {
	name string
	// refused are the capabilities the parent does not answer, besides compression
	refused uint32
	// extra is the child's [stream] section lines
	extra string
	// during runs while the first session streams
	during func(t *testing.T, d *daemon.Daemon, s *stream.Session)
	// sessions are the sessions captured and compared
	sessions int
}

var senderVariants = []senderVariant{
	// each chart's replication answered (commit 6), then its live data
	{name: "replication"},
	{name: "norepl", refused: stream.CapReplication},
	{name: "hex", refused: stream.CapReplication | stream.CapSlots | stream.CapIEEE754 | stream.CapFloatBaseline},
	{name: "v1", refused: stream.CapReplication | stream.CapInterpolated,
		extra: "    initial clock resync iterations = 3\n"},
	// a definition after a reconnect waits for the resync horizon: v1 blocks with no time until then
	{name: "v1-reconnect", refused: stream.CapReplication | stream.CapInterpolated, sessions: 2,
		extra: "    initial clock resync iterations = 3\n",
		during: func(t *testing.T, _ *daemon.Daemon, s *stream.Session) {
			time.Sleep(8 * time.Second)
			_ = s.Close()
		}},
	{name: "nolabels",
		refused: stream.CapReplication | stream.CapCLabels | stream.CapHLabels | stream.CapClaim | stream.CapPaths},
	{name: "pattern", refused: stream.CapReplication,
		extra: "    send charts matching = !netdata.http_api_* !netdata.network_streaming *\n"},
	{name: "reconnect", refused: stream.CapReplication, sessions: 2,
		during: func(t *testing.T, _ *daemon.Daemon, s *stream.Session) {
			time.Sleep(8 * time.Second)
			_ = s.Close()
		}},
	{name: "reload-labels", refused: stream.CapReplication,
		during: func(t *testing.T, d *daemon.Daemon, _ *stream.Session) {
			time.Sleep(6 * time.Second)
			if r := runCLI(t, d, "reload-labels"); r.Exit != 0 {
				t.Errorf("reload-labels: exit %d: %s", r.Exit, r.Stderr)
			}
		}},
}

// TestSenderCapture (checks `stream.sender-capture` and `stream.rchild-transcript`, milestone 7 commits 0, 4 and
// 5, D100, D104.8): a child streams to a recording parent that accepts it in plaintext with the variant's
// capabilities; after the session's start and a few seconds of data the two children's captures are compared, with
// the children's gate and reset records. C against C proves the capture and its normalization stable.
func TestSenderCapture(t *testing.T) {
	bins := binaries(t)
	for _, v := range senderVariants {
		t.Run(v.name, func(t *testing.T) {
			sessions := max(v.sessions, 1)
			var caps [2][]capture
			var records [2][]string
			// one at a time: a C child connects before its system-info script ends on a busy host, and sends the
			// fields empty
			for i, role := range []Role{"sender-oracle", "sender-candidate"} {
				caps[i], records[i] = senderRun(t, bins[i], Role(string(role)+"-"+v.name), v, sessions)
			}
			if t.Failed() {
				return
			}
			for n := range sessions {
				compareCaptures(t, "session "+strconv.Itoa(n+1), caps[0][n], caps[1][n])
				if len(caps[0][n].charts) == 0 || len(caps[0][n].data) == 0 {
					t.Errorf("session %d: the oracle's capture has %d charts and %d with data", n+1,
						len(caps[0][n].charts), len(caps[0][n].data))
				}
			}
			diffLines(t, "gate and reset records", records[0], records[1])
		})
	}
}

// senderRun runs one child against a recording parent for the variant and returns its sessions' captures and its
// gate and reset records.
func senderRun(t *testing.T, bin string, role Role, v senderVariant, sessions int) ([]capture, []string) {
	script := func(r stream.Request) stream.Answer {
		a := stream.PlaintextAnswer(r)
		a.Reply = stream.VCaps(r.Caps() &^ (stream.CapsCompression | v.refused))
		return a
	}
	parent, err := stream.StartParent(script)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { parent.Close() })
	d := senderChild(t, bin, role, parent, v.extra)
	var out []capture
	for n := 1; n <= sessions; n++ {
		s := parent.WaitSession(n, 60*time.Second)
		if s == nil {
			t.Fatalf("%s: no STREAM connection %d within 60 s (probes %q)", role, n, parent.Probes())
		}
		if n == 1 && v.during != nil {
			v.during(t, d, s)
		}
		if n == sessions {
			time.Sleep(15 * time.Second)
		}
	}
	if err := d.Stop(); err != nil {
		t.Errorf("stop %s: %v", role, err)
	}
	for n, s := range parent.Sessions()[:sessions] {
		c := parseCapture(s.Request, s.Data())
		if os.Getenv("PARITY_KEEP") == "1" {
			_ = os.WriteFile(filepath.Join(d.Opts.RunDir, "capture-"+strconv.Itoa(n+1)+".txt"), s.Data(), 0o644)
		}
		t.Logf("%s session %d: %d bytes, %d charts, %d with data, %d other lines", role, n+1, len(s.Data()),
			len(c.charts), len(c.data), len(c.other))
		out = append(out, c)
	}
	return out, senderRecords(t, d)
}
