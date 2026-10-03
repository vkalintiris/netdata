// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"maps"
	"net"
	"os"
	"os/exec"
	"os/user"
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

// spawnProbeRecord records the context the spawn server gave the shell running a script, in the agent's cache
// directory; the probe then execs the oracle's stock script (spawnProbeDirWith). The script runs under the server's
// `/bin/sh -c`, so the server is its grandparent; dash moves its stdout to fd 11 while a redirected command runs, and
// holds the script on fd 10.
const spawnProbeRecord = `#!/bin/sh
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
`

// spawnProbeParent records the environment the spawn server executed the script's shell with (`/bin/sh -c`, the
// probe's parent) in its order, and that shell's command line: dash hands the script an environment rebuilt from its
// own table (its hash order, PWD added), so only the parent's shows the order of the spawn's.
const spawnProbeParent = `echo "== penv" >> "$f"
tr '\0' '\n' < /proc/$PPID/environ >> "$f"
echo "== pcmd" >> "$f"
tr '\0' ' ' < /proc/$PPID/cmdline >> "$f"
echo >> "$f"
`

// spawnProbeDir is a plugins directory whose system-info.sh and get-kubernetes-labels.sh are probes.
func spawnProbeDir(t *testing.T) string {
	t.Helper()
	return spawnProbeDirWith(t, "", "")
}

// spawnProbeDirWith is spawnProbeDir with more recording after spawnProbeRecord's and, with a tail, a system-info.sh
// that runs the stock script, then prints the tail (its exit code kept).
func spawnProbeDirWith(t *testing.T, more, tail string) string {
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
		run := fmt.Sprintf("exec '%s' \"$@\"\n", target)
		if script == "system-info.sh" && tail != "" {
			file := filepath.Join(dir, "system-info.tail")
			if err := os.WriteFile(file, []byte(tail), 0o644); err != nil {
				t.Fatal(err)
			}
			run = fmt.Sprintf("'%s' \"$@\"\nrc=$?\ncat '%s'\nexit $rc\n", target, file)
		}
		if err := os.WriteFile(filepath.Join(dir, script), []byte(spawnProbeRecord+more+run), 0o755); err != nil {
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
	return startSpawnAgentsWith(t, func(t *testing.T, o *daemon.Options) { o.PluginsDir, o.LogsExtra = probes, logs })
}

// startSpawnAgentsWith is startSpawnAgents with adjust setting each side's inputs (its options, files in its run
// directory) before its start: RunDir, the identity and the run directory's NETDATA_RUN_DIR are set when it runs.
func startSpawnAgentsWith(t *testing.T, adjust func(t *testing.T, o *daemon.Options)) [2]spawnSide {
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
			StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true, Env: []string{"NETDATA_RUN_DIR=" + runtime}}
		adjust(t, &o)
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
			case "env", "cwd", "penv", "pcmd":
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
// records; its answers to a client on its socket; its end with the daemon's; and the environment the scripts get from
// non-default inputs (env, env-edge: M8 commit 11, D182.3).
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

	// M8 commit 11 (D182.3): every input the agent exports to its children away from the defaults the subtests above
	// share (env), each conditional export's other arm with every exported name preset (env-edge), and the arms
	// neither takes (env-path)
	t.Run("env", func(t *testing.T) { runSpawnEnv(t, spawnEnvMain()) })
	t.Run("env-edge", func(t *testing.T) { runSpawnEnv(t, spawnEnvEdge(t)) })
	t.Run("env-path", func(t *testing.T) { runSpawnEnv(t, spawnEnvPath()) })
}

// spawnEnvVariant is an environment variant of plugins.spawn: inputs away from the defaults both agents share
// elsewhere, so an agent exporting a default where C exports its input fails. Each child's environment is compared
// sorted and, as the spawn server executed it, in order (spawn.env.order).
type spawnEnvVariant struct {
	// adjust sets a side's inputs: its options and files in its run directory (probes: the variant's primary plugins
	// directory, holding the probes)
	adjust func(t *testing.T, o *daemon.Options, probes string)
	// tail, when set, follows the stock system-info.sh's output: lines for the parser's other branches
	tail string
	// want are lines every child's environment holds on the oracle (spawnEnvView's masks); late are lines the children
	// started after system-info.sh's output was read hold (get-kubernetes-labels.sh's runs); wantNot are texts no
	// child's environment holds
	want, late, wantNot []string
	// records, when set, picks daemon.log records compared in file order; wantRecords are texts the oracle's records
	// or its collector.log and stdout.log hold
	records     *regexp.Regexp
	wantRecords []string
	// created are directories under the run directory the agent creates (verify_required_directory, environment.c
	// :17-22): each one's mode is compared, and the oracle's must be wantModes'
	created   []string
	wantModes []string
	// charts are the oracle's /api/v1/charts `timezone` and `update_every` (the daemon's own use of TZ and [db]
	// update every, charts2json.c:71-76), compared on both; the timezone masked while PULSE runs
	charts string
}

// spawnExported are the names the agent exports to its children (environment.c, nd_log-config.c, netdata-conf-*.c,
// registry_init.c, registry.c, machine-guid.c, main.c, analytics.c, nd_log-init.c, run_dir.c): spawnEnvView shows
// their values, env-edge presets those that are no input (spawnInputs).
var spawnExported = map[string]bool{}

func init() {
	for _, k := range strings.Fields(`NETDATA_UPDATE_EVERY NETDATA_VERSION NETDATA_HOSTNAME NETDATA_HOST_PREFIX
		NETDATA_CONFIG_DIR NETDATA_USER_CONFIG_DIR NETDATA_STOCK_CONFIG_DIR NETDATA_STOCK_DATA_DIR NETDATA_PLUGINS_DIR
		NETDATA_WEB_DIR NETDATA_CACHE_DIR NETDATA_LIB_DIR NETDATA_LOG_DIR CLAIMING_DIR NETDATA_USER_PLUGINS_DIRS
		NETDATA_LISTEN_PORT PATH PYTHONPATH PYTHONUNBUFFERED LC_ALL NETDATA_CONF_CPUS MALLOC_ARENA_MAX
		UV_THREADPOOL_SIZE TZ HOME NETDATA_INTERNALS_MONITORING NETDATA_REGISTRY_HOSTNAME NETDATA_REGISTRY_URL
		NETDATA_REGISTRY_CLOUD_BASE_URL NETDATA_REGISTRY_UNIQUE_ID NETDATA_LOG_METHOD NETDATA_LOG_FORMAT
		NETDATA_LOG_LEVEL NETDATA_SYSLOG_FACILITY NETDATA_ERRORS_THROTTLE_PERIOD NETDATA_ERRORS_PER_PERIOD
		NETDATA_DEBUG_FLAGS NETDATA_INVOCATION_ID NETDATA_RUN_DIR`) {
		spawnExported[k] = true
	}
}

// spawnEnvView is a child's environment as compared: probeRuns' masks, the variant's probe directory as <PROBES>,
// and a variable the harness passed down unchanged shown as `<inherited>` (the box's environment holds secrets)
// unless the agent exports it.
func spawnEnvView(env []string, probes string) []string {
	out := make([]string, 0, len(env))
	for _, kv := range env {
		k, v, _ := strings.Cut(kv, "=")
		if own, ok := os.LookupEnv(k); ok && own == v && !spawnExported[k] {
			kv = k + "=<inherited>"
		}
		out = append(out, strings.ReplaceAll(kv, filepath.Dir(probes), "<PROBES>"))
	}
	return out
}

// spawnEnvRecordRe is a logfmt record's level and message.
var spawnEnvRecordRe = regexp.MustCompile(`level=(\S+) .*msg="((?:[^"\\]|\\.)*)"`)

// runSpawnEnv starts both agents with the variant's inputs, has each run get-kubernetes-labels.sh again after the
// freeze (reload-labels, as `children`), and compares per script and run: the environment sorted and in the spawn's
// order, and the shell's command line; then /api/v1/charts' timezone and update_every, the created directories'
// modes, collector.log but the daemon's own records and stdout.log whole (C's temporary "init" server left out,
// D134.3), and the variant's daemon.log records. The oracle's guards: every want, late and wantNot, the shell
// `/bin/sh -c`, charts, wantModes, every wantRecords text (in the records or the two files).
func runSpawnEnv(t *testing.T, v spawnEnvVariant) {
	probes := spawnProbeDirWith(t, spawnProbeParent, v.tail)
	sides := startSpawnAgentsWith(t, func(t *testing.T, o *daemon.Options) { v.adjust(t, o, probes) })
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
		}
		if len(runs[0]) != len(runs[1]) || len(runs[0]) == 0 {
			t.Errorf("%s runs: oracle %d, candidate %d", script, len(runs[0]), len(runs[1]))
			continue
		}
		for j := range runs[0] {
			var env, order [2][]string
			for i := range sides {
				env[i] = spawnEnvView(runs[i][j]["env"], probes)
				order[i] = spawnEnvView(runs[i][j]["penv"], probes)
			}
			what := fmt.Sprintf("%s run %d", script, j+1)
			want := v.want
			if script == "get-kubernetes-labels.sh" {
				want = slices.Concat(want, v.late)
			}
			for _, w := range want {
				if !slices.Contains(env[0], w) {
					t.Errorf("oracle: %s: no %q in its environment", what, w)
				}
			}
			for _, w := range v.wantNot {
				if i := slices.IndexFunc(env[0], func(l string) bool { return strings.Contains(l, w) }); i >= 0 {
					t.Errorf("oracle: %s: %q holds %q", what, env[0][i], w)
				}
			}
			if pcmd := strings.Join(runs[0][j]["pcmd"], ""); !strings.HasPrefix(pcmd, "/bin/sh -c ") {
				t.Errorf("oracle: %s: its shell is %q", what, pcmd)
			}
			diffLines(t, what+" environment", env[0], env[1])
			diffLines(t, what+" environment in the spawn's order", order[0], order[1])
			diffLines(t, what+" shell", runs[0][j]["pcmd"], runs[1][j]["pcmd"])
			if j == 0 {
				t.Logf("%s of %d, the oracle's spawn environment in order (its shell %q):\n%s", what, len(runs[0]),
					runs[0][j]["pcmd"], strings.Join(order[0], "\n"))
			}
		}
	}
	var recs, outputs [2][]string
	for i, s := range sides {
		mask := func(m string) string {
			for _, re := range spawnMasks {
				m = re.ReplaceAllStringFunc(m, func(w string) string { return regexp.MustCompile(`\d+`).ReplaceAllString(w, "N") })
			}
			for _, d := range []string{s.d.Opts.RunDir, s.runtime} {
				m = strings.ReplaceAll(m, d, "<RUN>")
			}
			return strings.ReplaceAll(m, filepath.Dir(probes), "<PROBES>")
		}
		// collector.log but the daemon's own records (its threads' are other checks', statsd's unported: log.parity),
		// so the spawn server's records and what the children and the loader write; stdout.log whole (the daemon's
		// stderr before its logs open, and the loader's): a logfmt or json record as its comm, level and message, any
		// other line whole
		for _, name := range []string{"collector.log", "stdout.log"} {
			var lines []string
			for _, l := range logLines(t, s.d.Opts.RunDir, name) {
				var rec struct {
					Comm  string          `json:"comm"`
					Level json.RawMessage `json:"level"`
					Msg   string          `json:"msg"`
				}
				if m := spawnEnvRecordRe.FindStringSubmatch(l); m != nil {
					l = logField(l, "comm") + " " + m[1] + " " + m[2]
				} else if strings.HasPrefix(l, "{") && json.Unmarshal([]byte(l), &rec) == nil {
					l = "comm=" + rec.Comm + " " + string(rec.Level) + " " + rec.Msg
				}
				if name == "collector.log" && strings.HasPrefix(l, "comm=netdata ") {
					continue
				}
				lines = append(lines, mask(l))
			}
			for _, l := range withoutInitServer(lines) {
				outputs[i] = append(outputs[i], name+": "+l)
			}
		}
		if v.records == nil {
			continue
		}
		for _, l := range logLines(t, s.d.Opts.RunDir, "daemon.log") {
			if m := spawnEnvRecordRe.FindStringSubmatch(l); m != nil && v.records.MatchString(m[2]) {
				recs[i] = append(recs[i], threadOf(l)+" "+m[1]+" "+mask(m[2]))
			}
		}
	}
	var modes [2][]string
	for i, s := range sides {
		for _, rel := range v.created {
			mode := "missing"
			if fi, err := os.Stat(filepath.Join(s.d.Opts.RunDir, rel)); err == nil {
				mode = fi.Mode().String()
			}
			modes[i] = append(modes[i], rel+" "+mode)
		}
	}
	var charts [2]string
	for i, s := range sides {
		r, err := Get(s.d, "/api/v1/charts", nil)
		var doc struct {
			Timezone    string `json:"timezone"`
			UpdateEvery int    `json:"update_every"`
		}
		if err == nil {
			err = json.Unmarshal(r.Body, &doc)
		}
		if err != nil {
			t.Errorf("%s: /api/v1/charts: %v", s.role, err)
		}
		// with PULSE on, its first pass re-detects the system's timezone over one TZ named (pulse-daemon.c:75-120):
		// whether it ran before this read varies C against C (h14 e4)
		if !s.d.Opts.PulseOff {
			doc.Timezone = "<PULSE>"
		}
		charts[i] = fmt.Sprintf("timezone %s, update_every %d", doc.Timezone, doc.UpdateEvery)
	}
	if charts[0] != charts[1] {
		t.Errorf("/api/v1/charts: oracle %s, candidate %s", charts[0], charts[1])
	}
	if v.charts != "" && charts[0] != v.charts {
		t.Errorf("oracle: /api/v1/charts %s, want %s", charts[0], v.charts)
	}
	diffLines(t, "the created directories' modes", modes[0], modes[1])
	if !slices.Equal(modes[0], v.wantModes) {
		t.Errorf("oracle: the created directories' modes %q, want %q", modes[0], v.wantModes)
	}
	diffLines(t, "collector.log and stdout.log", outputs[0], outputs[1])
	diffLines(t, "the variant's daemon.log records", recs[0], recs[1])
	for _, w := range v.wantRecords {
		if !slices.ContainsFunc(slices.Concat(recs[0], outputs[0]), func(l string) bool { return strings.Contains(l, w) }) {
			t.Errorf("oracle: no record holds %q", w)
		}
	}
	t.Logf("collector.log and stdout.log:\n%s\ndaemon.log records:\n%s", strings.Join(outputs[0], "\n"),
		strings.Join(recs[0], "\n"))
}

// spawnEnvMain is `env`: the exported values from netdata.conf ([db], [global], [web], [logs], [registry],
// [environment variables], a second and third plugins directory, the stock config, stock data and web directories by
// other names, PULSE on), cloud.conf's url, an upper-case machine GUID in its file; in the environment, an upper-case
// dashed NETDATA_INVOCATION_ID (C exports its compact lower-case form), an empty TZ (C sets [environment variables]
// TZ, analytics.c:891-895), and LC_ALL and PYTHONPATH that C replaces. The collector log is a file in json with a
// level C does not know: C exports stderr, logfmt and that level read as info (nd_log-config.c:146-162,
// nd_log-internals.c:186-194), after [logs] level's notice.
func spawnEnvMain() spawnEnvVariant {
	return spawnEnvVariant{
		adjust: func(t *testing.T, o *daemon.Options, probes string) {
			users := []string{filepath.Join(filepath.Dir(probes), "user-a.d"), filepath.Join(filepath.Dir(probes), "user-b.d")}
			for _, d := range users {
				if err := os.MkdirAll(d, 0o755); err != nil {
					t.Fatal(err)
				}
			}
			o.PluginsDir = fmt.Sprintf("%q %q %q", probes, users[0], users[1])
			// the stock directories by other names: links to the oracle's (each must exist, environment.c:77-80)
			usr := filepath.Dir(filepath.Dir(os.Getenv("PARITY_ORACLE")))
			for link, target := range map[*string]string{
				&o.StockConfigDir: filepath.Join(usr, "lib", "netdata", "conf.d"),
				&o.StockDataDir:   filepath.Join(usr, "share", "netdata"),
				&o.WebDir:         filepath.Join(usr, "share", "netdata", "web"),
			} {
				*link = filepath.Join(filepath.Dir(probes), "stock-"+filepath.Base(target))
				if err := os.Symlink(target, *link); err != nil && !os.IsExist(err) {
					t.Fatal(err)
				}
			}
			o.UpdateEvery = "2"
			o.PulseOff = false
			o.GlobalExtra = "    host access prefix = /\n    cpu cores = 3\n    glibc malloc arena max for plugins = 3\n"
			o.WebExtra = "    default port = 29999\n"
			o.LogsExtra = "    level = notice\n    facility = local3\n    logs flood protection period = 2m\n" +
				"    logs to trigger flood protection = 333\n    debug flags = 0x100\n" +
				"    collector = json,level=parity-level@" + filepath.Join(o.RunDir, "log", "collector.log") + "\n"
			o.ConfExtra = "[registry]\n    registry hostname = parity-registry\n" +
				"    registry to announce = https://registry.parity.invalid\n\n" +
				"[environment variables]\n    PATH = /usr/bin:/bin:/usr/sbin:/sbin:/parity/bin\n" +
				"    PYTHONPATH = /parity/python\n    TZ = Europe/Athens\n"
			id := *o.Identity
			id.MachineGUID = strings.ToUpper(id.MachineGUID)
			o.Identity = &id
			o.Env = append(o.Env, "NETDATA_INVOCATION_ID=5A1E0000-0000-4000-8000-0000000000BB", "LC_ALL=C.UTF-8", "TZ=",
				"PYTHONPATH=/parity/inherited")
			cloud := filepath.Join(o.RunDir, "lib", "cloud.d")
			if err := os.MkdirAll(cloud, 0o770); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(cloud, "cloud.conf"), []byte("[global]\n    url = https://cloud.parity.invalid\n"), 0o640); err != nil {
				t.Fatal(err)
			}
		},
		want: []string{
			"NETDATA_UPDATE_EVERY=2", "NETDATA_HOSTNAME=parity-parent", "NETDATA_HOST_PREFIX=/",
			"NETDATA_CONFIG_DIR=<RUN>/etc", "NETDATA_USER_CONFIG_DIR=<RUN>/etc", "NETDATA_PLUGINS_DIR=<PROBES>/plugins.d",
			"NETDATA_STOCK_CONFIG_DIR=<PROBES>/stock-conf.d", "NETDATA_STOCK_DATA_DIR=<PROBES>/stock-netdata",
			"NETDATA_WEB_DIR=<PROBES>/stock-web",
			"NETDATA_CACHE_DIR=<RUN>/cache", "NETDATA_LIB_DIR=<RUN>/lib", "NETDATA_LOG_DIR=<RUN>/log",
			"CLAIMING_DIR=<RUN>/lib/cloud.d", "NETDATA_USER_PLUGINS_DIRS=<PROBES>/user-a.d <PROBES>/user-b.d",
			"NETDATA_LISTEN_PORT=29999", "PATH=/usr/bin:/bin:/usr/sbin:/sbin:/parity/bin", "PYTHONPATH=/parity/python",
			"PYTHONUNBUFFERED=1", "LC_ALL=C", "TZ=Europe/Athens", "NETDATA_CONF_CPUS=3", "MALLOC_ARENA_MAX=3",
			"UV_THREADPOOL_SIZE=18", "HOME=<RUN>/lib", "NETDATA_INTERNALS_MONITORING=YES",
			"NETDATA_REGISTRY_HOSTNAME=parity-registry", "NETDATA_REGISTRY_URL=https://registry.parity.invalid",
			"NETDATA_REGISTRY_CLOUD_BASE_URL=https://cloud.parity.invalid",
			"NETDATA_REGISTRY_UNIQUE_ID=5a1e0000-0000-4000-8000-0000000000aa",
			"NETDATA_LOG_METHOD=stderr", "NETDATA_LOG_FORMAT=logfmt", "NETDATA_LOG_LEVEL=info",
			"NETDATA_SYSLOG_FACILITY=local3", "NETDATA_ERRORS_THROTTLE_PERIOD=120", "NETDATA_ERRORS_PER_PERIOD=333",
			"NETDATA_DEBUG_FLAGS=0x100", "NETDATA_INVOCATION_ID=5a1e00000000400080000000000000bb",
		},
		wantNot: []string{"parity/inherited", "C.UTF-8"},
		charts:  "timezone <PULSE>, update_every 2",
	}
}

// spawnJunk is env-edge's preset of every name the agent exports and does not read as an input (spawnInputs): C
// overwrites each in place. MALLOC_ARENA_MAX gets a valid value instead (spawnValidPresets).
const spawnJunk = "parity-junk"

// spawnValidPresets are env-edge's presets that must stay valid: glibc's loader reads MALLOC_ARENA_MAX at every exec
// (the glibc.malloc.arena_max tunable) and warns on stderr about an invalid one, which the Rust spawn server's own exec
// writes to collector.log where C's forked server writes nothing (D184, a recorded deviation, not compared here).
var spawnValidPresets = map[string]string{"MALLOC_ARENA_MAX": "7"}

// spawnInputs are the exported names C also reads (environment.c:113-121, analytics.c:891, netdata-conf-logs.c:40,
// nd_log-init.c:9, run_dir.c:45): env-edge sets them as inputs, not presets.
var spawnInputs = []string{"PATH", "PYTHONPATH", "TZ", "NETDATA_LOG_LEVEL", "NETDATA_INVOCATION_ID", "NETDATA_RUN_DIR"}

// spawnEnvEdge is `env-edge`: each conditional export's other arm. No `[directories] home` (HOME from the password
// database, main.c:1287-1295), an invalid host prefix (exported empty, paths.c:91-145), `cpu cores = 0` (1),
// `libuv worker threads` set, a malloc arena count past the CPUs (clamped, with C's NOTICE), `facility = security`
// (exported as its first name, auth); in the environment, no PATH (C's fallback, environment.c:113-115), a TZ C
// keeps over [environment variables] TZ, an invalid NETDATA_INVOCATION_ID with a compact upper-case INVOCATION_ID
// (its lower-case form exported, nd_log-init.c:7-20), NETDATA_LOG_LEVEL as [logs] level's default
// (netdata-conf-logs.c:40-43), an inherited PYTHONPATH, and every other exported name preset (spawnJunk;
// MALLOC_ARENA_MAX valid, spawnValidPresets).
// system-info.sh's output ends with lines for the parser's other branches (rrdhost-system-info.c:417-462): no `=`,
// an empty name, an empty value, an unknown name, a later value for a name, names this host's script never prints, a
// value ending in CR, a line past fgets()'s 1,022 bytes (read as two, the rest skipped for its missing `=`). The cloud
// directory is left for the agent to create (0770 under the inherited umask, environment.c:84).
func spawnEnvEdge(t *testing.T) spawnEnvVariant {
	u, err := user.Current()
	if err != nil {
		t.Fatal(err)
	}
	var env []string
	for _, k := range slices.Sorted(maps.Keys(spawnExported)) {
		if v, ok := spawnValidPresets[k]; ok {
			env = append(env, k+"="+v)
		} else if !slices.Contains(spawnInputs, k) {
			env = append(env, k+"="+spawnJunk)
		}
	}
	// the umask the agents inherit (cloud.d is created 0770 under it)
	umask := syscall.Umask(0)
	syscall.Umask(umask)
	env = append(env, "NETDATA_LOG_LEVEL=notice", "TZ=Europe/Paris", "NETDATA_INVOCATION_ID=parity-not-a-uuid",
		"INVOCATION_ID=5A1E00000000400080000000000000CC", "PYTHONPATH=/parity/inherited")
	return spawnEnvVariant{
		adjust: func(t *testing.T, o *daemon.Options, probes string) {
			if _, err := os.Stat("/usr/bin/env"); err != nil {
				t.Fatalf("parity: env-edge unsets PATH with /usr/bin/env: %v", err)
			}
			o.PluginsDir = fmt.Sprintf("%q", probes)
			o.NoHomeDir = true
			o.GlobalExtra = "    host access prefix = /nonexistent-parity\n    cpu cores = 0\n    libuv worker threads = 20\n" +
				"    glibc malloc arena max for plugins = 999\n"
			o.LogsExtra = "    facility = security\n"
			o.ConfExtra = "[environment variables]\n    TZ = Europe/Athens\n"
			o.Wrap = []string{"/usr/bin/env", "-u", "PATH"}
			o.Env = append(o.Env, env...)
		},
		tail: "parity line without an equals sign\n=parity-empty-name\nNETDATA_SYSTEM_CPU_VENDOR=\nNETDATA_PARITY_UNKNOWN=1\n" +
			"NETDATA_SYSTEM_CPU_MODEL=Parity CPU\nNETDATA_HOST_OS_LABEL_EDITION=parity-edition\n" +
			"NETDATA_HOST_OS_LABEL_BUILD=parity-build\nNETDATA_PROTOCOL_VERSION=parity-protocol\n" +
			"NETDATA_CONTAINER_IS_OFFICIAL_IMAGE=parity-official\nNETDATA_HOST_IS_K8S_NODE=parity-k8s\n" +
			"NETDATA_SYSTEM_CONTAINER=parity-container\r\n" +
			"NETDATA_SYSTEM_DISK_DETECTION=" + strings.Repeat("d", 1100) + "\n",
		want: []string{
			"NETDATA_UPDATE_EVERY=1", "NETDATA_HOSTNAME=parity-parent", "NETDATA_HOST_PREFIX=",
			"NETDATA_CONFIG_DIR=<RUN>/etc", "NETDATA_USER_CONFIG_DIR=<RUN>/etc", "NETDATA_PLUGINS_DIR=<PROBES>/plugins.d",
			"NETDATA_CACHE_DIR=<RUN>/cache", "NETDATA_LIB_DIR=<RUN>/lib", "NETDATA_LOG_DIR=<RUN>/log",
			"CLAIMING_DIR=<RUN>/lib/cloud.d", "NETDATA_USER_PLUGINS_DIRS=", "NETDATA_LISTEN_PORT=19999",
			"PATH=/bin:/usr/bin:/sbin:/usr/sbin:/usr/local/bin:/usr/local/sbin", "PYTHONPATH=/parity/inherited",
			"PYTHONUNBUFFERED=1", "LC_ALL=C", "TZ=Europe/Paris", "NETDATA_CONF_CPUS=1",
			"MALLOC_ARENA_MAX=" + strconv.Itoa(onlineCPUs(t)), "UV_THREADPOOL_SIZE=20", "HOME=" + u.HomeDir,
			"NETDATA_INTERNALS_MONITORING=NO", "NETDATA_REGISTRY_HOSTNAME=parity-parent",
			"NETDATA_REGISTRY_URL=https://registry.my-netdata.io", "NETDATA_REGISTRY_CLOUD_BASE_URL=https://app.netdata.cloud",
			"NETDATA_REGISTRY_UNIQUE_ID=5a1e0000-0000-4000-8000-0000000000aa",
			"NETDATA_LOG_METHOD=stderr", "NETDATA_LOG_FORMAT=logfmt", "NETDATA_LOG_LEVEL=notice",
			"NETDATA_SYSLOG_FACILITY=auth", "NETDATA_ERRORS_THROTTLE_PERIOD=60", "NETDATA_ERRORS_PER_PERIOD=1000",
			"NETDATA_DEBUG_FLAGS=0x0000000000000000", "NETDATA_INVOCATION_ID=5a1e00000000400080000000000000cc",
			"INVOCATION_ID=5A1E00000000400080000000000000CC",
		},
		late: []string{
			"NETDATA_SYSTEM_CPU_MODEL=Parity CPU", "NETDATA_HOST_OS_LABEL_EDITION=parity-edition",
			"NETDATA_HOST_OS_LABEL_BUILD=parity-build", "NETDATA_PROTOCOL_VERSION=parity-protocol",
			"NETDATA_CONTAINER_IS_OFFICIAL_IMAGE=parity-official", "NETDATA_HOST_IS_K8S_NODE=parity-k8s",
			"NETDATA_SYSTEM_CONTAINER=parity-container",
			"NETDATA_SYSTEM_DISK_DETECTION=" + strings.Repeat("d", 992),
		},
		wantNot:   []string{spawnJunk, "NETDATA_PARITY_UNKNOWN", "parity-empty-name", "Europe/Athens"},
		created:   []string{"lib/cloud.d"},
		wantModes: []string{"lib/cloud.d " + (os.ModeDir | os.FileMode(0o770&^umask)).String()},
		records:   regexp.MustCompile(`^SYSTEM INFO: |host prefix|malloc arenas`),
		charts:    "timezone Europe/Paris, update_every 1",
		wantRecords: []string{
			"SYSTEM INFO: Skipping malformed line from system-info.sh (no '=' found)",
			"SYSTEM INFO: Skipping empty name or value from system-info.sh: '=parity-empty-name'",
			"SYSTEM INFO: Skipping empty name or value from system-info.sh: 'NETDATA_SYSTEM_CPU_VENDOR='",
			"SYSTEM INFO: Unexpected variable 'NETDATA_PARITY_UNKNOWN=1'",
			"SYSTEM INFO: Skipping malformed line from system-info.sh (no '=' found): '" + strings.Repeat("d", 108) + `\n'`,
			"Ignoring host prefix '/nonexistent-parity'",
			"malloc arenas can be from 1 to " + strconv.Itoa(onlineCPUs(t)),
		},
	}
}

// spawnEnvPath is `env-path`: the arms the other two leave. An inherited PATH past 4,095 bytes and no [environment
// variables] PATH (C's snprintfz cuts the default at 4,095 bytes, environment.c:112-116); `[logs] level` alone, an
// alias (exported as its first name, warning); an unknown facility (daemon); malloc arenas below 1 (1) and `libuv
// worker threads` below the minimum (16, with C's ERR, inicfg_get_number_range).
func spawnEnvPath() spawnEnvVariant {
	path := "/usr/bin:/bin" + strings.Repeat(":/parity/long", 400)
	return spawnEnvVariant{
		adjust: func(t *testing.T, o *daemon.Options, probes string) {
			o.PluginsDir = fmt.Sprintf("%q", probes)
			o.GlobalExtra = "    glibc malloc arena max for plugins = 0\n    libuv worker threads = 5\n"
			o.LogsExtra = "    level = warn\n    facility = parity-facility\n"
			o.Env = append(o.Env, "PATH="+path)
		},
		want: []string{"PATH=" + (path + ":/sbin:/usr/sbin:/usr/local/bin:/usr/local/sbin")[:4095],
			"NETDATA_LOG_LEVEL=warning", "NETDATA_SYSLOG_FACILITY=daemon", "MALLOC_ARENA_MAX=1", "UV_THREADPOOL_SIZE=16"},
		records: regexp.MustCompile(`out of range|malloc arenas`),
		wantRecords: []string{
			"CONFIG: out of range [global].libuv worker threads = 5. Acceptable values: 16 to 1024 inclusive. Setting it to 16",
		},
	}
}

// onlineCPUs counts /sys/devices/system/cpu/online's ranges: sysconf(_SC_NPROCESSORS_ONLN), C's count of the system's
// CPUs (get_system_cpus.c).
func onlineCPUs(t *testing.T) int {
	t.Helper()
	b, err := os.ReadFile("/sys/devices/system/cpu/online")
	if err != nil {
		t.Fatal(err)
	}
	n := 0
	for _, r := range strings.Split(strings.TrimSpace(string(b)), ",") {
		lo, hi, ok := strings.Cut(r, "-")
		a, err1 := strconv.Atoi(lo)
		z := a
		var err2 error
		if ok {
			z, err2 = strconv.Atoi(hi)
		}
		if err1 != nil || err2 != nil {
			t.Fatalf("parity: /sys/devices/system/cpu/online %q", b)
		}
		n += z - a + 1
	}
	return n
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
