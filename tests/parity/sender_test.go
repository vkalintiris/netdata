// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// capture is a child's stream as a recording parent received it, normalized for comparison (brief
// `knowledge/brief-stream-sender.md` §6): the request's parameters and headers, the session's start (host labels
// as a set: C prints them in heap order), each chart's definition keyed by id (its times masked), and each chart's
// data as the dimensions and flags of its blocks (values and times masked, the number of blocks left out); slot
// numbers masked, each chart's blocks checked against its definition's slot; chart labels as a set.
type capture struct {
	request []string
	start   []string
	charts  map[string][]string
	data    map[string][]string
	// replays are each chart's replication answers: their lines' kinds and dimensions
	replays map[string][]string
	other   []string
	// slots are each chart's slot in its definition, which its blocks must use (the numbers follow the charts'
	// creation order, which varies)
	slots map[string]string
}

var (
	slotRe          = regexp.MustCompile(`SLOT:\S+ `)
	localPortRe     = regexp.MustCompile(`127\.0\.0\.1:\d+`)
	replayRe        = regexp.MustCompile(`^(RBEGIN|RSET|RDSTATE|RSSTATE|REND)(?: SLOT:\S+)?(?: ('[^']*'|"[^"]*"))?`)
	replayVerdictRe = regexp.MustCompile(` (true|false) `)
	chartLineRe     = regexp.MustCompile(`^CHART (?:SLOT:\S+ )?"([^"]*)"`)
	begin2Re        = regexp.MustCompile(`^BEGIN2 (?:SLOT:\S+ )?'([^']*)' `)
	set2Re          = regexp.MustCompile(`^SET2 (?:SLOT:\S+ )?'([^']*)' \S+ \S+ (\S*)$`)
	definitionEndRe = regexp.MustCompile(`^CHART_DEFINITION_END .*`)
)

// parseCapture splits a plaintext stream into its parts.
func parseCapture(req stream.Request, data []byte) capture {
	c := capture{charts: map[string][]string{}, data: map[string][]string{}, slots: map[string]string{},
		replays: map[string][]string{}}
	params := make([]string, 0, len(req.Params))
	for k, v := range req.Params {
		params = append(params, k+"="+strings.Join(v, ","))
	}
	sort.Strings(params)
	c.request = append(params, req.Headers...)
	var chart string // the chart whose definition or block is being read
	var labels, clabels []string
	var block []string
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
		case strings.HasPrefix(line, "CLAIMED_ID "), strings.HasPrefix(line, "VARIABLE HOST "),
			strings.HasPrefix(line, "FUNCTION "), strings.HasPrefix(line, "FUNCTION_DEL "):
			c.start = append(c.start, line)
		case chartLineRe.MatchString(line):
			chart = chartLineRe.FindStringSubmatch(line)[1]
			c.slots[chart] = slotRe.FindString(line)
			c.charts[chart] = []string{slotRe.ReplaceAllString(line, "SLOT:N ")}
			clabels = nil
		case strings.HasPrefix(line, "CLABEL "):
			clabels = append(clabels, line)
		case line == "CLABEL_COMMIT":
			// C prints a chart's labels in heap order
			sort.Strings(clabels)
			c.charts[chart] = append(append(c.charts[chart], clabels...), line)
			clabels = nil
		case strings.HasPrefix(line, "DIMENSION "), strings.HasPrefix(line, "VARIABLE CHART "):
			c.charts[chart] = append(c.charts[chart], slotRe.ReplaceAllString(line, "SLOT:N "))
		case definitionEndRe.MatchString(line):
			c.charts[chart] = append(c.charts[chart], "CHART_DEFINITION_END <times>")
		case begin2Re.MatchString(line):
			chart = begin2Re.FindStringSubmatch(line)[1]
			if slot := slotRe.FindString(line); slot != c.slots[chart] {
				c.other = append(c.other, "BEGIN2 of "+chart+" in "+slot+", its definition's "+c.slots[chart])
			}
			block = nil
		case set2Re.MatchString(line):
			m := set2Re.FindStringSubmatch(line)
			block = append(block, m[1]+" "+m[2])
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

// senderChild boots a daemon streaming to `parent` as a child with its own identity.
func senderChild(t *testing.T, bin string, role Role, parent *stream.Parent) *daemon.Daemon {
	t.Helper()
	id := daemon.Identity{Hostname: "sender-child", StreamKey: "5a1e0000-0000-4000-8000-0000000000c1",
		MachineGUID: "5a1e0000-0000-4000-8000-0000000000cc"}
	o := daemon.Options{Binary: bin, RunDir: runDir(t, role), Identity: &id, DBMode: "alloc", StorageTiers: 1,
		NoStreamKey: true, StreamTo: &daemon.StreamTo{Destination: parent.Addr(), APIKey: parentIdentity.StreamKey,
			Extra: "    reconnect delay = 5\n"}}
	d, err := daemon.Start(o)
	if err != nil {
		t.Fatalf("parity: start %s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	return d
}

// TestSenderCapture (check `stream.sender-capture`, milestone 7 commit 0, D100): a child streams to a recording
// parent that accepts it in plaintext; after the session's start and a few seconds of data the two children's
// captures are compared. C against C proves the capture and its normalization stable; the Rust sender comes later.
func TestSenderCapture(t *testing.T) {
	bins := binaries(t)
	if bins[0] != bins[1] && os.Getenv("PARITY_SENDER") != "1" {
		t.Skip("the Rust sender comes with milestone 7 commit 4 (D100); PARITY_SENDER=1 runs it anyway")
	}
	var caps [2]capture
	for i, role := range []Role{"sender-oracle", "sender-candidate"} {
		parent, err := stream.StartParent(nil)
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { parent.Close() })
		d := senderChild(t, bins[i], role, parent)
		s := parent.WaitSession(1, 60*time.Second)
		if s == nil {
			t.Fatalf("%s: no STREAM connection within 60 s (probes %q)", role, parent.Probes())
		}
		time.Sleep(15 * time.Second)
		if err := d.Stop(); err != nil {
			t.Fatalf("stop %s: %v", role, err)
		}
		caps[i] = parseCapture(s.Request, s.Data())
		if os.Getenv("PARITY_KEEP") == "1" {
			_ = os.WriteFile(filepath.Join(d.Opts.RunDir, "capture.txt"), s.Data(), 0o644)
		}
		t.Logf("%s: %d bytes, %d charts, %d with data, %d other lines, probes %q", role, len(s.Data()),
			len(caps[i].charts), len(caps[i].data), len(caps[i].other), parent.Probes())
	}
	compareCaptures(t, "capture", caps[0], caps[1])
	if len(caps[0].charts) == 0 || len(caps[0].data) == 0 {
		t.Errorf("the oracle's capture has %d charts and %d with data", len(caps[0].charts), len(caps[0].data))
	}
}
