// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// spawnProbeScript records the context the spawn server gave the shell running a script, in the agent's cache
// directory, then execs the oracle's stock script. The script runs under the server's `/bin/sh -c`, so the server is
// its grandparent; dash moves its stdout to fd 11 while a redirected command runs, and holds the script on fd 10.
const spawnProbeScript = `#!/bin/sh
f="$NETDATA_CACHE_DIR/spawn-probe.${0##*/}.$$"
echo "== fds" >> "$f"
ls -l /proc/$$/fd/ >> "$f"
echo "== status" >> "$f"
grep -E '^(Umask|SigBlk|SigIgn|SigCgt|CapInh|CapPrm|CapEff|CapBnd|CapAmb|NoNewPrivs|Seccomp):' /proc/$$/status >> "$f"
echo "== cwd" >> "$f"
readlink /proc/$$/cwd >> "$f"
echo "== parent" >> "$f"
gp=$(cut -d' ' -f4 /proc/$PPID/stat)
cat /proc/$gp/comm >> "$f"
echo "== ids" >> "$f"
cut -d' ' -f5,6 /proc/$$/stat >> "$f"
echo "== env" >> "$f"
tr '\0' '\n' < /proc/$$/environ >> "$f"
exec '%s' "$@"
`

// spawnProbeDir is a plugins directory whose system-info.sh and get-kubernetes-labels.sh are probes.
func spawnProbeDir(t *testing.T) string {
	t.Helper()
	stock := filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", "libexec", "netdata", "plugins.d")
	dir := filepath.Join(t.TempDir(), "plugins.d")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, script := range []string{"system-info.sh", "get-kubernetes-labels.sh"} {
		target := filepath.Join(stock, script)
		if _, err := os.Stat(target); err != nil {
			t.Fatalf("parity: the oracle's %s: %v", script, err)
		}
		probe := fmt.Sprintf(spawnProbeScript, target)
		if err := os.WriteFile(filepath.Join(dir, script), []byte(probe), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

// spawnSide is one agent of a spawn check: its daemon, the run directory its spawn server's socket is in, and the
// directory its probes write to.
type spawnSide struct {
	role    Role
	d       *daemon.Daemon
	runtime string
}

// startSpawnAgents boots both agents with the probes as their plugins, each with a run directory the test owns (the
// launcher deletes its own when it reaps a daemon, before a server could be watched unlinking its socket).
func startSpawnAgents(t *testing.T, probes, logs string) [2]spawnSide {
	t.Helper()
	bins := binaries(t)
	var sides [2]spawnSide
	for i, role := range []Role{Oracle, Candidate} {
		runtime, err := os.MkdirTemp("", "spawn-rt-")
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = os.RemoveAll(runtime) })
		id := parentIdentity
		o := daemon.Options{Binary: bins[i], RunDir: runDir(t, role), Identity: &id, DBMode: "ram",
			StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true, PluginsDir: probes, LogsExtra: logs,
			Env: []string{"NETDATA_RUN_DIR=" + runtime}}
		d, err := daemon.Start(o)
		if err != nil {
			t.Fatalf("parity: start %s: %v", role, err)
		}
		t.Cleanup(func() { _ = d.Stop() })
		sides[i] = spawnSide{role: role, d: d, runtime: runtime}
	}
	return sides
}

// procStat is a process's ppid, comm and state ("" when it is gone).
func procStat(pid int) (ppid int, comm, state string) {
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return 0, "", ""
	}
	open, end := bytes.IndexByte(b, '('), bytes.LastIndexByte(b, ')')
	if open < 0 || end < open {
		return 0, "", ""
	}
	fields := strings.Fields(string(b[end+1:]))
	if len(fields) < 2 {
		return 0, "", ""
	}
	ppid, _ = strconv.Atoi(fields[1])
	return ppid, string(b[open+1 : end]), fields[0]
}

// spawnServerOf is the daemon's child named spawn-plugins, waited for up to 10 s; 0 when none came.
func spawnServerOf(daemonPID int) int {
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		entries, _ := os.ReadDir("/proc")
		for _, e := range entries {
			pid, err := strconv.Atoi(e.Name())
			if err != nil {
				continue
			}
			if ppid, comm, _ := procStat(pid); ppid == daemonPID && comm == "spawn-plugins" {
				return pid
			}
		}
		time.Sleep(100 * time.Millisecond)
	}
	return 0
}

// alivePID: the process exists and is no zombie.
func alivePID(pid int) bool {
	_, _, state := procStat(pid)
	return state != "" && state != "Z"
}

// procStatusOf are a process's /proc status lines by key.
func procStatusOf(t *testing.T, pid int) map[string]string {
	t.Helper()
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/status", pid))
	if err != nil {
		t.Fatalf("parity: status of %d: %v", pid, err)
	}
	out := map[string]string{}
	for _, l := range strings.Split(string(b), "\n") {
		if k, v, ok := strings.Cut(l, ":"); ok {
			out[k] = strings.TrimSpace(v)
		}
	}
	return out
}

// retitled is C's os_setproctitle() of `argv`: the first argument becomes `title` cut or blank-padded to its length,
// every other one blanks; as /proc/<pid>/cmdline shows it.
func retitled(title string, argv []string) string {
	var b strings.Builder
	for i, a := range argv {
		w := []byte(strings.Repeat(" ", len(a)))
		if i == 0 {
			copy(w, title)
		}
		b.Write(w)
		b.WriteByte(0)
	}
	return b.String()
}

// fdKinds are a process's descriptors: number to its target's kind (`pipe`, `socket`, `anon_inode:...`) or path, run
// directories masked.
func fdKinds(pid int, masks ...string) map[string]string {
	out := map[string]string{}
	entries, _ := os.ReadDir(fmt.Sprintf("/proc/%d/fd", pid))
	for _, e := range entries {
		target, err := os.Readlink(fmt.Sprintf("/proc/%d/fd/%s", pid, e.Name()))
		if err != nil {
			continue
		}
		out[e.Name()] = fdKind(target, masks...)
	}
	return out
}

var (
	fdInodeRe   = regexp.MustCompile(`^(pipe|socket):\[\d+\]$`)
	probeFileRe = regexp.MustCompile(`(spawn-probe\.[^/]+\.sh)\.\d+$`)
)

func fdKind(target string, masks ...string) string {
	if m := fdInodeRe.FindStringSubmatch(target); m != nil {
		return m[1]
	}
	for _, m := range masks {
		target = strings.ReplaceAll(target, m, "<RUN>")
	}
	return probeFileRe.ReplaceAllString(target, "$1.N")
}

// envOf is a process's environment as it was executed.
func envOf(pid int) []string {
	b, _ := os.ReadFile(fmt.Sprintf("/proc/%d/environ", pid))
	return strings.FieldsFunc(string(b), func(r rune) bool { return r == 0 })
}

// spawnRecordRe is a record of the spawn server or its client.
var spawnRecordRe = regexp.MustCompile(`msg="(SPAWN [^"]*)"`)

var spawnMasks = []*regexp.Regexp{
	regexp.MustCompile(`\bpid \d+`),
	regexp.MustCompile(`\brequest (No )?\d+`),
	regexp.MustCompile(`\bon pid \d+`),
}

// spawnRecords are the spawn records of a log, in order, pids and request ids masked, with `comm` and level.
func spawnRecords(t *testing.T, s spawnSide, name string, masks ...string) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, s.d.Opts.RunDir, name) {
		m := spawnRecordRe.FindStringSubmatch(l)
		if m == nil {
			continue
		}
		msg := m[1]
		for _, re := range spawnMasks {
			msg = re.ReplaceAllStringFunc(msg, func(w string) string {
				return regexp.MustCompile(`\d+`).ReplaceAllString(w, "N")
			})
		}
		for _, m := range masks {
			msg = strings.ReplaceAll(msg, m, "<RUN>")
		}
		out = append(out, fmt.Sprintf("%s %s %s", logField(l, "comm"), logField(l, "level"), msg))
	}
	return withoutInitServer(out)
}

// withoutInitServer drops what C's temporary "init" server logs (D134.3: the agent has none): its own records, and
// the daemon's creation record of it, the first one, which comes before it.
func withoutInitServer(records []string) []string {
	if !slices.ContainsFunc(records, func(r string) bool { return strings.HasPrefix(r, "comm=spawn-init ") }) {
		return records
	}
	out := make([]string, 0, len(records))
	dropped := false
	for _, r := range records {
		if strings.HasPrefix(r, "comm=spawn-init ") {
			continue
		}
		if !dropped && strings.HasSuffix(r, "SPAWN SERVER: server created on pid N") {
			dropped = true
			continue
		}
		out = append(out, r)
	}
	return out
}

// logField is a logfmt record's `key=value`.
func logField(l, key string) string {
	i := strings.Index(l, " "+key+"=")
	if i < 0 {
		return key + "=?"
	}
	v := l[i+1:]
	if j := strings.IndexByte(v, ' '); j >= 0 {
		v = v[:j]
	}
	return v
}

// probeRuns are a side's probe records, per script, each a map of section to lines (descriptors as kinds).
func probeRuns(t *testing.T, s spawnSide, script string) []map[string][]string {
	t.Helper()
	cache := filepath.Join(s.d.Opts.RunDir, "cache")
	files, _ := filepath.Glob(filepath.Join(cache, "spawn-probe."+script+".*"))
	slices.Sort(files)
	masks := []string{s.d.Opts.RunDir, s.runtime}
	var out []map[string][]string
	for _, f := range files {
		b, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		run := map[string][]string{}
		section := ""
		for _, l := range strings.Split(strings.TrimRight(string(b), "\n"), "\n") {
			if name, ok := strings.CutPrefix(l, "== "); ok {
				section = name
				continue
			}
			switch section {
			case "fds":
				// `lrwx------ 1 u g 64 <date> 3 -> pipe:[12]`
				num, target, ok := strings.Cut(l, " -> ")
				if !ok {
					continue
				}
				fields := strings.Fields(num)
				l = fields[len(fields)-1] + " " + fdKind(target, masks...)
			case "env", "cwd":
				for _, m := range masks {
					l = strings.ReplaceAll(l, m, "<RUN>")
				}
			}
			run[section] = append(run[section], l)
		}
		slices.Sort(run["env"])
		out = append(out, run)
	}
	return out
}

// spawnEnvMasks are the environment variables whose values differ by side by design: each daemon's own identity and
// timing.
var spawnEnvMasks = map[string]bool{
	"NETDATA_INVOCATION_ID": true,
	"INVOCATION_ID":         true,
}

func maskEnv(env []string) []string {
	out := make([]string, 0, len(env))
	for _, e := range env {
		if k, _, ok := strings.Cut(e, "="); ok && spawnEnvMasks[k] {
			e = k + "=V"
		}
		out = append(out, e)
	}
	slices.Sort(out)
	return out
}

// TestPluginsSpawn (`plugins.spawn`, M8 commit 2, D140): the spawn server each agent starts at "plugins spawn server",
// as /proc shows it; the context of the scripts it starts (probes recording themselves, then the stock scripts); its
// records; its answers to a client on its socket; and its end with the daemon's.
func TestPluginsSpawn(t *testing.T) {
	probes := spawnProbeDir(t)

	t.Run("server", func(t *testing.T) {
		sides := startSpawnAgents(t, probes, "")
		var views [2][]string
		for i, s := range sides {
			pid := spawnServerOf(s.d.PID())
			if pid == 0 {
				t.Fatalf("%s: no spawn-plugins child of the daemon", s.role)
			}
			st := procStatusOf(t, pid)
			daemonArgv := strings.FieldsFunc(string(must(os.ReadFile(fmt.Sprintf("/proc/%d/cmdline", s.d.PID())))),
				func(r rune) bool { return r == 0 })
			cmdline := string(must(os.ReadFile(fmt.Sprintf("/proc/%d/cmdline", pid))))
			if cmdline != retitled("spawn-plugins", daemonArgv) {
				t.Errorf("%s: the server's command line %q is not C's retitle of %q", s.role, cmdline, daemonArgv)
			}
			if st["Threads"] != "1" {
				t.Errorf("%s: the server has %s threads", s.role, st["Threads"])
			}
			cgt, _ := strconv.ParseUint(st["SigCgt"], 16, 64)
			if want := uint64(1)<<(syscall.SIGTERM-1) | uint64(1)<<(syscall.SIGCHLD-1); cgt&want != want {
				t.Errorf("%s: the server catches %016x, not TERM and CHLD", s.role, cgt)
			}
			sock := filepath.Join(s.runtime, "netdata-spawn-plugins.sock")
			fi, err := os.Stat(sock)
			if err != nil || fi.Mode()&os.ModeSocket == 0 || fi.Mode().Perm() != 0o770 {
				t.Errorf("%s: the server's socket %s: %v %v", s.role, sock, fi, err)
			}
			// the server's environment is the daemon's as it was executed (Rust's adds its marker, D136.2)
			env := envOf(pid)
			marker := slices.Index(env, "NETDATA_SPAWN_SERVER=1")
			if bins := binaries(t); s.role == Candidate && bins[0] != bins[1] && marker < 0 {
				t.Errorf("candidate: the server lacks its marker")
			}
			if marker >= 0 {
				env = slices.Delete(env, marker, marker+1)
			}
			if !slices.Equal(maskEnv(env), maskEnv(envOf(s.d.PID()))) {
				t.Errorf("%s: the server's environment is not the daemon's", s.role)
			}
			fds := fdKinds(pid, s.d.Opts.RunDir, s.runtime)
			var kinds []string
			for n, k := range fds {
				if fd, _ := strconv.Atoi(n); fd > 2 {
					kinds = append(kinds, k)
				}
			}
			slices.Sort(kinds)
			views[i] = []string{
				"comm spawn-plugins",
				"fd0 " + fds["0"], "fd1 " + fds["1"], "fd2 " + fds["2"], "other fds " + strings.Join(kinds, ","),
				"SigBlk " + st["SigBlk"], "SigIgn " + st["SigIgn"], "Umask " + st["Umask"],
				"Cap " + st["CapInh"] + " " + st["CapPrm"] + " " + st["CapEff"] + " " + st["CapBnd"] + " " + st["CapAmb"],
				"NoNewPrivs " + st["NoNewPrivs"], "Seccomp " + st["Seccomp"],
			}
		}
		diffLines(t, "the spawn servers", views[0], views[1])
		t.Logf("server:\n%s", strings.Join(views[0], "\n"))
	})

	t.Run("children", func(t *testing.T) {
		sides := startSpawnAgents(t, probes, "")
		// a spawn after the freeze: the labels reload runs the kubernetes script again
		for _, s := range sides {
			if r := runCLI(t, s.d, "reload-labels"); r.Exit != 0 {
				t.Fatalf("%s: reload-labels: exit %d: %s", s.role, r.Exit, r.Stderr)
			}
		}
		deadline := time.Now().Add(10 * time.Second)
		for _, s := range sides {
			for len(probeRuns(t, s, "get-kubernetes-labels.sh")) < 2 && time.Now().Before(deadline) {
				time.Sleep(100 * time.Millisecond)
			}
		}
		for _, script := range []string{"system-info.sh", "get-kubernetes-labels.sh"} {
			var runs [2][]map[string][]string
			for i, s := range sides {
				runs[i] = probeRuns(t, s, script)
				for _, r := range runs[i] {
					if got := strings.Join(r["parent"], ""); got != "spawn-plugins" {
						t.Errorf("%s: %s was started by %q", s.role, script, got)
					}
				}
			}
			if len(runs[0]) != len(runs[1]) || len(runs[0]) == 0 {
				t.Errorf("%s runs: oracle %d, candidate %d", script, len(runs[0]), len(runs[1]))
				continue
			}
			for j := range runs[0] {
				for _, section := range []string{"fds", "status", "cwd", "parent"} {
					diffLines(t, fmt.Sprintf("%s run %d %s", script, j+1, section), runs[0][j][section], runs[1][j][section])
				}
				diffLines(t, fmt.Sprintf("%s run %d environment", script, j+1), maskEnv(runs[0][j]["env"]),
					maskEnv(runs[1][j]["env"]))
				// the children share the daemon's process group and session (no setsid, no setpgid)
				for i, s := range sides {
					if got, want := strings.Join(runs[i][j]["ids"], ""), daemonIDs(s.d.PID()); got != want {
						t.Errorf("%s: %s pgid/sid %q, the daemon's %q", s.role, script, got, want)
					}
				}
			}
		}
	})

	t.Run("records", func(t *testing.T) {
		sides := startSpawnAgents(t, probes, "    level = debug\n")
		time.Sleep(time.Second)
		var recs [2][]string
		for i, s := range sides {
			recs[i] = spawnRecords(t, s, "collector.log", s.d.Opts.RunDir, s.runtime, probes)
		}
		if len(recs[0]) == 0 {
			t.Fatalf("the oracle logged no spawn record")
		}
		diffLines(t, "the spawn records of collector.log", recs[0], recs[1])
		t.Logf("records:\n%s", strings.Join(recs[0], "\n"))
	})

	t.Run("wire", func(t *testing.T) {
		sides := startSpawnAgents(t, probes, "")
		var answers [2][]string
		for i, s := range sides {
			sock := filepath.Join(s.runtime, "netdata-spawn-plugins.sock")
			before := len(spawnRecords(t, s, "collector.log"))
			for _, c := range spawnWireCases() {
				answers[i] = append(answers[i], c.name+": "+spawnExchange(t, sock, c.bytes, c.fds))
			}
			time.Sleep(500 * time.Millisecond)
			answers[i] = append(answers[i], spawnRecords(t, s, "collector.log")[before:]...)
		}
		if !strings.Contains(answers[0][0], "ee55daba0400000000000000") {
			t.Errorf("the oracle did not answer the PING: %s", answers[0][0])
		}
		diffLines(t, "the servers' answers and records", answers[0], answers[1])
		t.Logf("wire:\n%s", strings.Join(answers[0], "\n"))
	})

	t.Run("stop", func(t *testing.T) {
		sides := startSpawnAgents(t, probes, "")
		for _, s := range sides {
			pid := spawnServerOf(s.d.PID())
			if pid == 0 {
				t.Fatalf("%s: no spawn server", s.role)
			}
			if err := s.d.Stop(); err != nil {
				t.Fatalf("%s: stop: %v", s.role, err)
			}
			if !waitFor(5*time.Second, func() bool { return !alivePID(pid) }) {
				t.Errorf("%s: the spawn server outlived the daemon", s.role)
			}
			if _, err := os.Stat(filepath.Join(s.runtime, "netdata-spawn-plugins.sock")); !os.IsNotExist(err) {
				t.Errorf("%s: the socket stays after the stop: %v", s.role, err)
			}
		}
	})

	// the server's own deaths: SIGTERM ends its loop with C's ERR, SIGKILL leaves its socket dead; neither restarts,
	// and the next spawn (a labels reload) fails to connect
	for _, sig := range []syscall.Signal{syscall.SIGTERM, syscall.SIGKILL} {
		name := map[syscall.Signal]string{syscall.SIGTERM: "server-term", syscall.SIGKILL: "server-kill"}[sig]
		t.Run(name, func(t *testing.T) {
			sides := startSpawnAgents(t, probes, "")
			var recs [2][]string
			for i, s := range sides {
				pid := spawnServerOf(s.d.PID())
				if pid == 0 {
					t.Fatalf("%s: no spawn server", s.role)
				}
				before := len(spawnRecords(t, s, "collector.log"))
				if err := syscall.Kill(pid, sig); err != nil {
					t.Fatal(err)
				}
				if !waitFor(5*time.Second, func() bool { return !alivePID(pid) }) {
					t.Fatalf("%s: the spawn server survived %s", s.role, name)
				}
				_ = runCLI(t, s.d, "reload-labels")
				time.Sleep(500 * time.Millisecond)
				if again := spawnServerOf(s.d.PID()); again != 0 && again != pid {
					t.Errorf("%s: a new spawn server %d", s.role, again)
				}
				recs[i] = spawnRecords(t, s, "collector.log", s.d.Opts.RunDir, s.runtime, probes)[before:]
			}
			diffLines(t, "the records after the server's "+name, recs[0], recs[1])
			t.Logf("records:\n%s", strings.Join(recs[0], "\n"))
		})
	}

	// `-W buildinfojson` without a daemon runs the system info script through a temporary unnamed server, gone with
	// its socket afterwards (`buildinfo.c:1423-1434`)
	t.Run("buildinfo", func(t *testing.T) {
		bins := binaries(t)
		var recs [2][]string
		for i, bin := range bins {
			runtime := t.TempDir()
			cmd := exec.Command(bin, "-W", "buildinfojson")
			cmd.Env = append(os.Environ(), "NETDATA_RUN_DIR="+runtime, "NETDATA_LOG_LEVEL=debug")
			var stderr bytes.Buffer
			cmd.Stderr = &stderr
			if err := cmd.Run(); err != nil {
				t.Fatalf("%s: %v: %s", bin, err, stderr.String())
			}
			for _, l := range strings.Split(stderr.String(), "\n") {
				if m := spawnRecordRe.FindStringSubmatch(l); m != nil {
					msg := regexp.MustCompile(`\d+`).ReplaceAllString(m[1], "N")
					recs[i] = append(recs[i], logField(l, "comm")+" "+logField(l, "level")+" "+msg)
				}
			}
			left, _ := os.ReadDir(runtime)
			if len(left) != 0 {
				t.Errorf("%s: left in its run directory: %v", bin, left)
			}
		}
		diffLines(t, "-W buildinfojson's spawn records", recs[0], recs[1])
		t.Logf("records:\n%s", strings.Join(recs[0], "\n"))
	})

	t.Run("parent-death", func(t *testing.T) {
		sides := startSpawnAgents(t, probes, "    level = debug\n")
		var recs [2][]string
		for i, s := range sides {
			pid := spawnServerOf(s.d.PID())
			if pid == 0 {
				t.Fatalf("%s: no spawn server", s.role)
			}
			before := len(spawnRecords(t, s, "collector.log"))
			if err := s.d.Kill(); err != nil {
				t.Fatalf("%s: kill: %v", s.role, err)
			}
			if !waitFor(5*time.Second, func() bool { return !alivePID(pid) }) {
				t.Errorf("%s: the spawn server outlived its killed daemon", s.role)
			}
			if _, err := os.Stat(filepath.Join(s.runtime, "netdata-spawn-plugins.sock")); !os.IsNotExist(err) {
				t.Errorf("%s: the socket stays after the daemon's death: %v", s.role, err)
			}
			recs[i] = spawnRecords(t, s, "collector.log")[before:]
		}
		if len(recs[0]) == 0 {
			t.Errorf("the oracle's server logged nothing at its daemon's death")
		}
		diffLines(t, "the records after the daemon's death", recs[0], recs[1])
	})
}

// daemonIDs is a process's `pgid sid`.
func daemonIDs(pid int) string {
	b, _ := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	end := bytes.LastIndexByte(b, ')')
	if end < 0 {
		return ""
	}
	f := strings.Fields(string(b[end+1:]))
	if len(f) < 4 {
		return ""
	}
	return f[2] + " " + f[3]
}

func waitFor(d time.Duration, ok func() bool) bool {
	deadline := time.Now().Add(d)
	for time.Now().Before(deadline) {
		if ok() {
			return true
		}
		time.Sleep(50 * time.Millisecond)
	}
	return ok()
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

// spawnWireCase is a client's message to a server: its bytes and how many descriptors go with them.
type spawnWireCase struct {
	name  string
	bytes []byte
	fds   int
}

// spawnHeader is a request header in the 64-bit layout (`spawn_server_nofork.c:873-892`), request 7.
func spawnHeader(msgType byte, magic [16]byte, envSize, argvSize uint64, instance byte) []byte {
	b := []byte{msgType}
	b = append(b, magic[:]...)
	for _, v := range []uint64{7, envSize, argvSize, 0} {
		b = binary.LittleEndian.AppendUint64(b, v)
	}
	return append(b, instance)
}

// spawnWireCases: a PING, then requests every server refuses before its key matters or because of it.
func spawnWireCases() []spawnWireCase {
	env, argv := []byte("A=1\x00\x00"), []byte("/bin/true\x00\x00")
	wrong := [16]byte{1, 2, 3}
	request := func(msgType byte) []byte {
		return append(append(spawnHeader(msgType, wrong, uint64(len(env)), uint64(len(argv)), 0), env...), argv...)
	}
	return []spawnWireCase{
		{"ping", spawnHeader(2, [16]byte{}, 0, 0, 0), 0},
		{"no descriptors", request(1), 0},
		{"three descriptors", request(1), 3},
		{"wrong key", request(1), 4},
		{"unknown message", request(7), 4},
		{"nothing", nil, 0},
	}
}

// spawnExchange sends one message (with copies of a pipe's write end as its descriptors) and reads to the end: the
// answer in hex, or how the server ended it.
func spawnExchange(t *testing.T, sock string, msg []byte, fds int) string {
	t.Helper()
	conn, err := net.DialUnix("unix", nil, &net.UnixAddr{Name: sock, Net: "unix"})
	if err != nil {
		return "connect: " + err.Error()
	}
	defer conn.Close()
	var p [2]int
	if err := syscall.Pipe(p[:]); err != nil {
		t.Fatal(err)
	}
	defer syscall.Close(p[0])
	defer syscall.Close(p[1])
	var oob []byte
	if fds > 0 {
		passed := make([]int, fds)
		for i := range passed {
			passed[i] = p[1]
		}
		oob = syscall.UnixRights(passed...)
	}
	if len(msg) > 0 || len(oob) > 0 {
		if _, _, err := conn.WriteMsgUnix(msg, oob, nil); err != nil {
			return "send: " + err.Error()
		}
	}
	_ = conn.CloseWrite()
	_ = conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	var got []byte
	buf := make([]byte, 64)
	for {
		n, err := conn.Read(buf)
		got = append(got, buf[:n]...)
		if err != nil {
			if len(got) == 0 {
				// a reset (the request left unread) and an end are both "no answer"
				return "no answer"
			}
			return fmt.Sprintf("%x", got)
		}
	}
}
