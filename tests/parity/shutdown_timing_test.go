// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

var (
	endedInRe    = regexp.MustCompile(`Shutdown process ended in ([^"]+)"`)
	durationPart = regexp.MustCompile(`(\d+)(ms|us|s)`)
)

// shutdownTimes are when a child's shutdown records came, relative to its SIGTERM record.
type shutdownTimes struct {
	disconnect, step6 time.Duration
	total             time.Duration
	// each record was found (SIGTERM first)
	found, disconnected, stepped bool
}

// recordTime is a record's time= field.
func recordTime(line string) (time.Time, bool) {
	m := recordTimeRe.FindStringSubmatch(line)
	if m == nil {
		return time.Time{}, false
	}
	at, err := time.Parse(time.RFC3339Nano, m[1])
	return at, err == nil
}

// parseEndedIn reads the watcher's "1s 305ms 902us".
func parseEndedIn(text string) time.Duration {
	var d time.Duration
	for _, m := range durationPart.FindAllStringSubmatch(text, -1) {
		var n time.Duration
		for _, c := range m[1] {
			n = n*10 + time.Duration(c-'0')
		}
		switch m[2] {
		case "s":
			d += n * time.Second
		case "ms":
			d += n * time.Millisecond
		case "us":
			d += n * time.Microsecond
		}
	}
	return d
}

func childShutdownTimes(t *testing.T, d *daemon.Daemon) shutdownTimes {
	t.Helper()
	var s shutdownTimes
	var sigterm time.Time
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		at, ok := recordTime(l)
		if !ok {
			continue
		}
		switch {
		case strings.Contains(l, `msg="SIGNAL: Received SIGTERM`):
			sigterm, s.found = at, true
		case !s.found:
		case strings.Contains(l, "sender disconnected from parent, reason: DISCONNECTED SHUTDOWN REQUESTED"):
			s.disconnect, s.disconnected = at.Sub(sigterm), true
		case strings.Contains(l, "shutdown step: [6/22]") && strings.Contains(l, "started"):
			s.step6, s.stepped = at.Sub(sigterm), true
		case endedInRe.MatchString(l):
			s.total = parseEndedIn(endedInRe.FindStringSubmatch(l)[1])
		}
	}
	return s
}

// TestShutdownTiming (check `daemon.shutdown-timing`, D110): a C child and a Rust child, each streaming to a
// recording parent, get SIGTERM. As in C, the child's streaming ends on its own once the exit starts, before the
// shutdown reaches the step that stops the streaming threads (C's service_running() is false for every service from
// the exit's start), and the steps only wait, so the shutdown lasts about as long as C's: its threads leave at C's
// cadences (the connector's last passes, PULSE's and RRDCONTEXT's seconds, the web workers' 100 ms). The Rust
// agent has no systemd bus watcher, whose wait of up to a second at `[3/22]` C's shutdown includes.
func TestShutdownTiming(t *testing.T) {
	bins := binaries(t)
	var times [2]shutdownTimes
	for i, role := range []Role{"shutdown-oracle", "shutdown-candidate"} {
		parent, err := stream.StartParent(func(r stream.Request) stream.Answer {
			a := stream.PlaintextAnswer(r)
			a.Reply = stream.VCaps(r.Caps() &^ (stream.CapsCompression | stream.CapReplication))
			return a
		})
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { parent.Close() })
		d := senderChild(t, bins[i], role, parent, "")
		if parent.WaitSession(1, 60*time.Second) == nil {
			t.Fatalf("%s: no STREAM connection within 60 s", role)
		}
		time.Sleep(5 * time.Second)
		if err := d.Stop(); err != nil {
			t.Fatalf("stop %s: %v", role, err)
		}
		times[i] = childShutdownTimes(t, d)
		t.Logf("%s: disconnect +%v, [6/22] +%v, shutdown %v", role, times[i].disconnect, times[i].step6, times[i].total)
	}
	for i, s := range times {
		if !s.found || !s.disconnected || !s.stepped || s.total == 0 {
			t.Fatalf("side %d: records missing: %+v", i, s)
		}
		if s.disconnect > s.step6 {
			t.Errorf("side %d: the sender disconnected at +%v, not before [6/22] at +%v", i, s.disconnect, s.step6)
		}
		if s.disconnect > 500*time.Millisecond {
			t.Errorf("side %d: the sender disconnected %v after SIGTERM", i, s.disconnect)
		}
	}
	oracle, candidate := times[0].total, times[1].total
	if candidate < oracle/2 || candidate > oracle+time.Second {
		t.Errorf("shutdown lasted %v, the oracle's %v", candidate, oracle)
	}
}
