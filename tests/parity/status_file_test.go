// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// statusVolatile hides what differs between two runs of one binary: clocks, the process, the live memory and disk,
// the peak RSS, the files' sizes, the metric counts and the steps' durations; `metrics.nodes` compares (S6b).
var statusVolatile = []Mask{
	{"@timestamp", "clock"},
	{"agent.since", "the GUID file's modification time (checked against each side's file)"},
	{"agent.ephemeral_id", "invocation id"},
	{"agent.uptime", "clock"},
	{"agent.pid", "process"},
	{"agent.timings", "durations"},
	{"host.uptime", "boot clock"},
	{"host.memory.free", "live"},
	{"host.memory.netdata", "peak RSS"},
	{"host.disk.db.free", "live"},
	{"host.disk.db.inodes_free", "live"},
	{"host.disk.netdata", "file sizes"},
	{"metrics.metrics", "the counts at the last save, which move while pulse runs"},
	{"metrics.instances", "the counts at the last save, which move while pulse runs"},
	{"metrics.contexts", "the counts at the last save, which move while pulse runs"},
}

// statusDifferences are what C and the Rust agent write differently, each with its reason.
var statusDifferences = []Mask{
	{"agent.stack_traces", "no libbacktrace (D87 F3)"},
}

// stepDurations are the shutdown steps' durations in the stack trace's timings.
var stepDurations = regexp.MustCompile(`(\\n#\d+ '[^']*'): [^\\"]*`)

// fatalDifferences are what a fatal error's record says differently (D90): the code location is each agent's own
// source, C's stack trace comes from libbacktrace (D87 F3), and the Rust agent has no worker registry yet.
var fatalDifferences = []Mask{
	{"fatal.filename", "each agent's source (D90.1)"},
	{"fatal.function", "each agent's source (D90.1)"},
	{"fatal.line", "each agent's source (D90.1)"},
	{"fatal.stack_trace", "libbacktrace (D87 F3)"},
	{"fatal.worker_job_id", "no worker registry (D90.3)"},
	{"fatal.thread_id", "the thread's id"},
}

// deadlyDifferences are what a deadly signal's record says differently: C names the function from its stack trace
// (libbacktrace, D87 F3; the Rust agent writes C's `thread:<name>:<job>`), and the stack trace itself.
var deadlyDifferences = []Mask{
	{"fatal.function", "C's from libbacktrace (D87 F3, D91.4)"},
	{"fatal.stack_trace", "libbacktrace (D87 F3)"},
	{"fatal.thread_id", "the thread's id"},
	// a signal sent to the process goes to any thread that does not block it: the main thread or another
	{"fatal.thread", "the thread the kernel picked"},
}

// statusOpts: crash reports off, so neither agent posts a report when a carried id enables them by default (D88.10).
var statusOpts = daemon.Options{DBMode: "alloc", StorageTiers: 1, GlobalExtra: "    crash reports = off\n"}

// statusFile reads a daemon's status file, its step durations masked.
func statusFile(t *testing.T, d *daemon.Daemon) Value {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(d.Opts.RunDir, "lib", "status-netdata.json"))
	if err != nil {
		t.Fatalf("parity: %v", err)
	}
	v, err := ParseJSON(stepDurations.ReplaceAll(b, []byte("$1: D")))
	if err != nil {
		t.Fatalf("parity: %s: %v", d.Opts.RunDir, err)
	}
	return v
}

// statusMember is the text of a dotted member of a status file.
func statusMember(v Value, path string) string {
	for _, key := range strings.Split(path, ".") {
		found := false
		for _, m := range v.Members {
			if m.Key == key {
				v, found = m.Value, true
				break
			}
		}
		if !found {
			return "<missing>"
		}
	}
	return v.String()
}

// compareStatusFiles compares two daemons' status files; `both` when C wrote both; `extra` masks more.
func compareStatusFiles(t *testing.T, stage string, p *Pair, both bool, extra ...Mask) [2]Value {
	t.Helper()
	masks := append(append([]Mask{}, statusVolatile...), extra...)
	if !both {
		masks = append(masks, statusDifferences...)
	}
	var files [2]Value
	for i, side := range p.Each() {
		f := statusFile(t, side.Daemon)
		checkSince(t, stage, side.Role, side.Daemon, f)
		files[i] = ApplyMasks(f, masks)
	}
	for _, d := range Compare(files[0], files[1]) {
		t.Errorf("%s: %s", stage, d)
	}
	return files
}

// checkSince checks `agent.since` against the daemon's GUID file: its modification time, centiseconds truncated.
func checkSince(t *testing.T, stage string, role Role, d *daemon.Daemon, f Value) {
	t.Helper()
	st, err := os.Stat(filepath.Join(d.Opts.RunDir, "lib", "registry", "netdata.public.unique.id"))
	if err != nil {
		t.Fatalf("parity: %v", err)
	}
	want := `"` + st.ModTime().UTC().Format("2006-01-02T15:04:05.00") + `Z"`
	if got := statusMember(f, "agent.since"); got != want {
		t.Errorf("%s: %s: agent.since %s, want the GUID file's %s", stage, role, got, want)
	}
}

// lastExit is the "Last exit status" line of the daemon's latest start.
var lastExitRe = regexp.MustCompile(`Last exit status: ([^\\]*):`)

func lastExit(t *testing.T, d *daemon.Daemon) string {
	t.Helper()
	found := ""
	for _, line := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := lastExitRe.FindStringSubmatch(line); m != nil {
			found = m[1]
		}
	}
	return found
}

// expectStatus checks what both files say, and that the latest starts named the same last exit.
func expectStatus(t *testing.T, stage string, p *Pair, want map[string]string, exit string) {
	t.Helper()
	for _, side := range p.Each() {
		f := statusFile(t, side.Daemon)
		for path, value := range want {
			if got := statusMember(f, path); got != value {
				t.Errorf("%s: %s: %s = %s, want %s", stage, side.Role, path, got, value)
			}
		}
		if got := lastExit(t, side.Daemon); got != exit {
			t.Errorf("%s: %s: last exit %q, want %q", stage, side.Role, got, exit)
		}
	}
}

// waitRunning waits until a daemon's status file says it runs.
func waitRunning(t *testing.T, d *daemon.Daemon) {
	t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) {
		b, _ := os.ReadFile(filepath.Join(d.Opts.RunDir, "lib", "status-netdata.json"))
		if strings.Contains(string(b), `"status":"running"`) {
			return
		}
		time.Sleep(200 * time.Millisecond)
	}
	t.Fatalf("parity: %s: the status file never said running", d.Opts.RunDir)
}

// noStrayStatusFiles fails when a fallback directory holds a status file either agent would load (map §6).
func noStrayStatusFiles(t *testing.T) {
	t.Helper()
	wd, _ := os.Getwd()
	for _, dir := range []string{"/tmp", "/run", "/var/run", wd} {
		for _, name := range []string{"status-netdata.json", "dedup-netdata.dat"} {
			if _, err := os.Stat(filepath.Join(dir, name)); err == nil {
				t.Fatalf("parity: %s/%s exists: both agents would read it", dir, name)
			}
		}
	}
}

// TestStatusFile compares the daemon status file (check `daemon.status-file`): after a clean stop, a restart, a C
// start after a Rust run and a Rust start after a C run, the recovery of a lost GUID from it, a start after a
// SIGKILL, and a fatal error's record; each start's "Last exit status" is the same as C's.
func TestStatusFile(t *testing.T) {
	noStrayStatusFiles(t)
	bins := binaries(t)
	oracle, candidate := bins[0], bins[1]
	p := StartPair(t, statusOpts, parentIdentity)
	for _, side := range p.Each() {
		waitRunning(t, side.Daemon)
	}
	stopBoth(t, p)
	compareStatusFiles(t, "first", p, false)
	expectStatus(t, "first", p, map[string]string{
		"agent.status": `"exited"`, "agent.restarts": "1", "agent.crashes": "0", "agent.reliability": "1",
		"agent.exit_reason": `["signal-terminate"]`,
	}, "No status found for the previous Netdata session (new Netdata, or older version) (no last status)")

	t.Run("restart", func(t *testing.T) {
		for _, side := range p.Each() {
			if err := side.Daemon.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
		}
		stopBoth(t, p)
		compareStatusFiles(t, "restart", p, false)
		expectStatus(t, "restart", p, map[string]string{"agent.restarts": "2", "agent.reliability": "2"},
			"Netdata was last stopped gracefully (exit instructed)")
	})

	// C after the Rust agent reads its file as it reads its own: C, C, C next to C, Rust, C
	t.Run("c-after-rust", func(t *testing.T) {
		for _, side := range p.Each() {
			d := side.Daemon
			d.Opts.Binary = oracle
			if err := d.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
		}
		expectStatus(t, "c-after-rust", p, map[string]string{"agent.restarts": "3", "agent.reliability": "3"},
			"Netdata was last stopped gracefully (exit instructed)")
		stopBoth(t, p)
		compareStatusFiles(t, "c-after-rust", p, true)
	})

	// the Rust agent after C: the pair's candidate side restarts with the candidate
	t.Run("rust-after-c", func(t *testing.T) {
		p.Oracle.Opts.Binary, p.Candidate.Opts.Binary = oracle, candidate
		for _, side := range p.Each() {
			if err := side.Daemon.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
		}
		expectStatus(t, "rust-after-c", p, map[string]string{"agent.restarts": "4", "agent.reliability": "4"},
			"Netdata was last stopped gracefully (exit instructed)")
		stopBoth(t, p)
		compareStatusFiles(t, "rust-after-c", p, false)
	})

	// a lost GUID file: the GUID comes back from the status file
	t.Run("guid-recovery", func(t *testing.T) {
		for _, side := range p.Each() {
			d := side.Daemon
			if err := os.Remove(filepath.Join(d.Opts.RunDir, "lib", "registry", "netdata.public.unique.id")); err != nil {
				t.Fatalf("parity: %v", err)
			}
			if err := d.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
			recovered := false
			for _, line := range logLines(t, d.Opts.RunDir, "daemon.log") {
				recovered = recovered || strings.Contains(line, `msg="MACHINE_GUID: got previous GUID from daemon status file"`)
			}
			if !recovered {
				t.Errorf("%s: no GUID recovery record", side.Role)
			}
			if got := statusMember(statusFile(t, d), "agent.id"); got != `"`+parentIdentity.MachineGUID+`"` {
				t.Errorf("%s: agent.id %s, want %s", side.Role, got, parentIdentity.MachineGUID)
			}
		}
		stopBoth(t, p)
		compareStatusFiles(t, "guid-recovery", p, false)
	})

	// killed while running: a crash, reliability goes negative
	t.Run("killed-hard", func(t *testing.T) {
		for _, side := range p.Each() {
			d := side.Daemon
			if err := d.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
			waitRunning(t, d)
			if err := d.Kill(); err != nil {
				t.Fatalf("parity: kill %s: %v", side.Role, err)
			}
			if err := d.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
		}
		expectStatus(t, "killed-hard", p, map[string]string{"agent.crashes": "1", "agent.reliability": "-1"},
			"Netdata was last killed/crashed while operating normally (killed hard)")
		stopBoth(t, p)
		compareStatusFiles(t, "killed-hard", p, false)
	})

	// deadly signals while running: C's record from the handler over the last save, death by the signal; SIGSEGV is a
	// crash for the next start, SIGABRT not (it is not a deadly signal for the classification)
	for _, c := range []struct {
		name, reason, code, exit string
		sig                      syscall.Signal
	}{
		{"sigsegv", `["signal-segmentation-fault"]`, `"SIGSEGV/SI_USER"`,
			"Netdata was last crashed after receiving a deadly signal (deadly signal)", syscall.SIGSEGV},
		{"sigabrt", `["signal-abort"]`, `"SIGABRT/SI_USER"`,
			"Netdata was last crashed due to a fatal error (killed fatal)", syscall.SIGABRT},
	} {
		t.Run(c.name, func(t *testing.T) {
			for _, side := range p.Each() {
				d := side.Daemon
				if err := d.Restart(); err != nil {
					t.Fatalf("parity: restart %s: %v", side.Role, err)
				}
				waitRunning(t, d)
				if err := d.Signal(c.sig); err != nil {
					t.Fatalf("parity: signal %s: %v", side.Role, err)
				}
				if sig, err := d.WaitSignal(60 * time.Second); sig != c.sig || err != nil {
					t.Errorf("%s: ended by signal %d (%v), want %d", side.Role, sig, err, c.sig)
				}
			}
			expectStatus(t, c.name, p, map[string]string{
				"agent.status": `"running"`, "agent.exit_reason": c.reason, "fatal.signal_code": c.code,
			}, "Netdata was last stopped gracefully (exit instructed)")
			compareStatusFiles(t, c.name, p, false, deadlyDifferences...)
			// what the Rust agent writes in place of C's libbacktrace members (D91.4)
			rust := statusFile(t, p.Candidate)
			thread := strings.Trim(statusMember(rust, "fatal.thread"), `"`)
			if thread == "" || strings.Contains(thread, "[") {
				t.Errorf("%s: candidate: fatal.thread %q, want a name without its index", c.name, thread)
			}
			for path, want := range map[string]string{
				"fatal.function":    `"thread:` + thread + `:0"`,
				"fatal.stack_trace": `"info: no stack trace backend available"`,
			} {
				if got := statusMember(rust, path); got != want {
					t.Errorf("%s: candidate: %s = %s, want %s", c.name, path, got, want)
				}
			}
			for _, side := range p.Each() {
				if err := side.Daemon.Restart(); err != nil {
					t.Fatalf("parity: restart %s: %v", side.Role, err)
				}
			}
			for _, side := range p.Each() {
				if got := lastExit(t, side.Daemon); got != c.exit {
					t.Errorf("%s: %s: last exit %q, want %q", c.name, side.Role, got, c.exit)
				}
			}
			stopBoth(t, p)
		})
	}

	// a fatal error (netdatacli fatal-agent): its record, then a crash for the next start
	t.Run("fatal-agent", func(t *testing.T) {
		for _, side := range p.Each() {
			d := side.Daemon
			if err := d.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
			waitRunning(t, d)
			runCLI(t, d, "fatal-agent")
			if code, err := d.WaitExit(60 * time.Second); code != 1 || err != nil {
				t.Errorf("%s: exit %d (%v), want 1", side.Role, code, err)
			}
		}
		expectStatus(t, "fatal-agent", p, map[string]string{
			"agent.status": `"exited"`, "agent.exit_reason": `["fatal"]`, "fatal.thread": `"UV_WORKER"`,
			"fatal.message": `"COMMAND: netdata now exits."`, "fatal.errno": `""`,
		}, "Netdata was last stopped gracefully (exit instructed)")
		compareStatusFiles(t, "fatal-agent", p, false, fatalDifferences...)
		for _, side := range p.Each() {
			if err := side.Daemon.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
		}
		// the kill, the two deadly signals and this fatal error: four crashes
		expectStatus(t, "after fatal-agent", p, map[string]string{"agent.crashes": "4", "agent.reliability": "-1"},
			"Netdata was last stopped gracefully after it encountered a fatal error (fatal and exit)")
		stopBoth(t, p)
		compareStatusFiles(t, "after fatal-agent", p, false)
	})
}
