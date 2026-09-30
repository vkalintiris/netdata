package parity

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// sigIgnWrap execs the daemon with the signals C catches (and SIGCHLD) ignored, as `nohup`, a non-interactive shell's
// `&` or a supervisor can leave them.
var sigIgnWrap = []string{"env", "--ignore-signal=HUP,INT,QUIT,TERM,USR2,CHLD"}

func sigBit(sig syscall.Signal) uint64 { return 1 << (uint(sig) - 1) }

// sigIgnOf is the SigIgn mask of a process.
func sigIgnOf(t *testing.T, pid int) uint64 {
	t.Helper()
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/status", pid))
	if err != nil {
		t.Fatal(err)
	}
	for _, l := range strings.Split(string(b), "\n") {
		if v, ok := strings.CutPrefix(l, "SigIgn:"); ok {
			mask, err := strconv.ParseUint(strings.TrimSpace(v), 16, 64)
			if err != nil {
				t.Fatal(err)
			}
			return mask
		}
	}
	t.Fatalf("no SigIgn for pid %d", pid)
	return 0
}

// sigProbeDir is a plugins directory whose system-info.sh records the SigIgn of the shell running it, in the running
// agent's cache directory, then runs the oracle's stock script.
func sigProbeDir(t *testing.T) string {
	t.Helper()
	stock := filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", "libexec", "netdata", "plugins.d", "system-info.sh")
	if _, err := os.Stat(stock); err != nil {
		t.Fatalf("parity: the oracle's system-info.sh: %v", err)
	}
	dir := filepath.Join(t.TempDir(), "plugins.d")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	probe := "#!/bin/sh\ngrep '^SigIgn:' /proc/$$/status >> \"$NETDATA_CACHE_DIR/sig-probe.txt\"\nexec '" + stock + "' \"$@\"\n"
	if err := os.WriteFile(filepath.Join(dir, "system-info.sh"), []byte(probe), 0o755); err != nil {
		t.Fatal(err)
	}
	return dir
}

// probeRecords are the distinct SigIgn lines the probe wrote (C runs the script more than once).
func probeRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(d.Opts.RunDir, "cache", "sig-probe.txt"))
	if err != nil {
		t.Fatalf("%s: the probe never ran: %v", d.Opts.Binary, err)
	}
	lines := strings.Fields(strings.ReplaceAll(string(b), "SigIgn:", ""))
	slices.Sort(lines)
	return slices.Compact(lines)
}

// TestInheritedSigIgn (`daemon.inherited-sig-ign`, D123): both agents start with HUP, INT, QUIT, TERM, USR2 and CHLD
// ignored. C's handlers replace the ignore of the signals it catches, so the scripts it runs start with the default
// action for each; the Rust agent resets them after blocking them. The daemons still act on HUP, USR2 and INT or QUIT.
// The daemon's own SigIgn is compared without SIGCHLD, which C leaves ignored and Rust resets (D123.2, DEFECTS).
func TestInheritedSigIgn(t *testing.T) {
	caught := sigBit(syscall.SIGHUP) | sigBit(syscall.SIGINT) | sigBit(syscall.SIGQUIT) | sigBit(syscall.SIGTERM) |
		sigBit(syscall.SIGUSR2)
	chld := sigBit(syscall.SIGCHLD)
	for _, exit := range []syscall.Signal{syscall.SIGINT, syscall.SIGQUIT} {
		t.Run(strings.ToLower(strings.TrimPrefix(signalName(exit), "SIG")), func(t *testing.T) {
			p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, PluginsDir: sigProbeDir(t),
				Wrap: sigIgnWrap}, parentIdentity)
			for _, side := range p.Each() {
				waitRunning(t, side.Daemon)
			}
			var masks [2]uint64
			for i, side := range p.Each() {
				masks[i] = sigIgnOf(t, side.Daemon.PID())
			}
			// the launcher's ignore took effect (C keeps SIGCHLD's), and C replaced the ones it catches
			if masks[0]&chld == 0 || masks[0]&caught != 0 {
				t.Fatalf("oracle SigIgn %016x: the wrapper did not apply", masks[0])
			}
			if masks[0]&^chld != masks[1]&^chld {
				t.Errorf("daemon SigIgn (SIGCHLD aside): oracle %016x, candidate %016x", masks[0], masks[1])
			}
			if bins := binaries(t); bins[0] != bins[1] && masks[1]&chld != 0 {
				t.Errorf("candidate SigIgn %016x: SIGCHLD still ignored (D123.2)", masks[1])
			}
			var probes [2][]string
			for i, side := range p.Each() {
				probes[i] = probeRecords(t, side.Daemon)
			}
			if !slices.Equal(probes[0], probes[1]) {
				t.Errorf("system-info.sh SigIgn: oracle %q, candidate %q", probes[0], probes[1])
			}
			for _, side := range p.Each() {
				for _, sig := range []syscall.Signal{syscall.SIGHUP, syscall.SIGUSR2} {
					if err := syscall.Kill(side.Daemon.PID(), sig); err != nil {
						t.Fatalf("%s: %v", side.Role, err)
					}
				}
				waitRecord(t, side.Daemon, "SIGNAL: Received SIGHUP. Reopening all log files...", 20*time.Second)
				waitRecord(t, side.Daemon, "SIGNAL: Received SIGUSR2. Reloading HEALTH configuration...", 20*time.Second)
			}
			var codes [2]int
			for i, side := range p.Each() {
				if err := syscall.Kill(side.Daemon.PID(), exit); err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				code, err := side.Daemon.WaitExit(30 * time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				codes[i] = code
				want := "SIGNAL: Received " + signalName(exit) + ". Cleaning up to exit..."
				if !slices.ContainsFunc(logLines(t, side.Daemon.Opts.RunDir, "daemon.log"),
					func(l string) bool { return strings.Contains(l, want) }) {
					t.Errorf("%s: no %q record", side.Role, want)
				}
			}
			if codes[0] != codes[1] {
				t.Errorf("exit codes: oracle %d, candidate %d", codes[0], codes[1])
			}
		})
	}
}

// signalName is C's name of a signal (SIGINT, SIGQUIT).
func signalName(sig syscall.Signal) string {
	switch sig {
	case syscall.SIGINT:
		return "SIGINT"
	case syscall.SIGQUIT:
		return "SIGQUIT"
	}
	return sig.String()
}
