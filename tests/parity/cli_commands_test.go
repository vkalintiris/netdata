// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// netdatacli is the C client installed next to the oracle: the surface users see (GOAL I2).
func netdatacli(t *testing.T) string {
	t.Helper()
	bin := filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "netdatacli")
	if _, err := os.Stat(bin); err != nil {
		t.Fatalf("parity: netdatacli: %v", err)
	}
	return bin
}

type cliResult struct {
	Stdout, Stderr string
	Exit           int
}

// runCLI runs netdatacli against a daemon's pipe; stdin is /dev/null.
func runCLI(t *testing.T, d *daemon.Daemon, args ...string) cliResult {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, netdatacli(t), args...)
	cmd.Env = append(os.Environ(), "NETDATA_PIPENAME="+d.PipeName)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	err := cmd.Run()
	res := cliResult{Stdout: stdout.String(), Stderr: stderr.String()}
	var exitErr *exec.ExitError
	switch {
	case err == nil:
	case errors.As(err, &exitErr):
		res.Exit = exitErr.ExitCode()
	default:
		t.Fatalf("netdatacli %v: %v", args, err)
	}
	return res
}

// rawCommand speaks the protocol directly, for frames the client cannot send: it writes payload, waits pause,
// half-closes unless told not to, and returns what came back within wait.
func rawCommand(t *testing.T, pipe string, payload []byte, pause time.Duration, halfClose bool, wait time.Duration) []byte {
	t.Helper()
	c, err := net.DialUnix("unix", nil, &net.UnixAddr{Name: pipe, Net: "unix"})
	if err != nil {
		t.Fatalf("dial %s: %v", pipe, err)
	}
	defer c.Close()
	if _, err := c.Write(payload); err != nil {
		t.Fatalf("write: %v", err)
	}
	time.Sleep(pause)
	if halfClose {
		_ = c.CloseWrite()
	}
	_ = c.SetReadDeadline(time.Now().Add(wait))
	b, _ := io.ReadAll(c)
	return b
}

// commandRecords are the records the commands write: their own, and what `reload-health` says of a rule directory it
// cannot open (libnetdata/paths/paths.c:232-235, :274-277: the run directory has no `etc/health.d`; the record carries
// the failed open's errno, on the command's worker thread).
var commandRecords = regexp.MustCompile(`msg="(COMMAND: |write-config |Cannot execute read-config|Reopening all log files\.|Log files re-opened\.|pipe_read_cb: |RRDLABEL: Cannot reload|CONFIG cannot open [a-z -]+ directory '[^']*/health\.d'\.)`)

// labelLines keeps reload-labels' lines, sorted (C lists them in pointer order).
func labelLines(stdout string) string {
	lines := strings.Split(strings.TrimSuffix(stdout, "\n"), "\n")
	sort.Strings(lines)
	return strings.Join(lines, "\n")
}

// TestCLICommands runs every netdatacli command (shutdown-agent and fatal-agent aside, see TestCLILifecycle) against
// both daemons, with a C child streaming to them and after it left, and compares what the client prints and its exit
// status, the raw replies of frames the client cannot send, the commands' records and the socket file.
func TestCLICommands(t *testing.T) {
	opts := daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, LogsExtra: "    level = debug\n"}
	p := StartPair(t, opts, parentIdentity)
	compare := func(name string, get func(d *daemon.Daemon) string) {
		t.Helper()
		oracle, candidate := get(p.Oracle), get(p.Candidate)
		if oracle != candidate {
			t.Errorf("%s:\noracle:    %q\ncandidate: %q", name, oracle, candidate)
		}
	}
	cli := func(name string, args ...string) {
		t.Helper()
		compare(name, func(d *daemon.Daemon) string { return fmt.Sprintf("%+v", runCLI(t, d, args...)) })
	}

	for _, c := range []struct {
		name string
		args []string
	}{
		{"help", []string{"help"}},
		{"reload-health", []string{"reload-health"}},
		{"reopen-logs", []string{"reopen-logs"}},
		{"reload-claiming-state", []string{"reload-claiming-state"}},
		{"ping", []string{"ping"}},
		{"aclk-state", []string{"aclk-state"}},
		{"aclk-state json", []string{"aclk-state", "json"}},
		{"version", []string{"version"}},
		{"update-node-info", []string{"update-node-info"}},
		{"mark without node", []string{"mark-stale-nodes-ephemeral"}},
		{"remove without node", []string{"remove-stale-node"}},
		{"mark all", []string{"mark-stale-nodes-ephemeral", "ALL_NODES"}},
		{"remove all", []string{"remove-stale-node", "ALL_NODES"}},
		{"mark localhost", []string{"mark-stale-nodes-ephemeral", parentIdentity.Hostname}},
		{"remove localhost guid", []string{"remove-stale-node", parentIdentity.MachineGUID}},
		{"mark unknown", []string{"mark-stale-nodes-ephemeral", "nosuchnode"}},
		{"read-config without args", []string{"read-config"}},
		{"read-config unknown", []string{"read-config", "netdata|nosuch|key"}},
		{"read-config one separator", []string{"read-config", "netdata|global"}},
		{"read-config hostname", []string{"read-config", "netdata|global|hostname"}},
		{"read-config cloud url", []string{"read-config", "cloud|global|url"}},
		{"write-config without args", []string{"write-config"}},
		{"write-config", []string{"write-config", "netdata|global|x-test|v1"}},
		{"read-config written", []string{"read-config", "netdata|global|x-test"}},
		{"write-config cloud", []string{"write-config", "cloud|global|x-test|a|b"}},
		{"read-config cloud written", []string{"read-config", "cloud|global|x-test"}},
		{"unknown command", []string{"nosuchcommand"}},
		{"upper case", []string{"PING"}},
		{"no argument", nil},
		{"prefix", []string{"helpme"}},
		{"spaces", []string{"  ping  "}},
	} {
		cli(c.name, c.args...)
	}
	// What `reload-health` left (M9 commit 9, D214 F6). Health is off here and the installed stock rules are on: C's
	// reload reads the rule files all the same (health/health.c:215-218 tests no switch), registers health's DynCfg
	// template with a job per alert name, and reads the two directories' keys, which `/netdata.conf` lists from then
	// on; no host's health is on, so no alert is linked. The tree is read as anybody may read it (no token is laid
	// out here), each health node as its id and status, in the tree's order.
	reloaded := func(name string, get func(d *daemon.Daemon) string, guard func(oracle string) error) {
		t.Helper()
		oracle, candidate := get(p.Oracle), get(p.Candidate)
		if err := guard(oracle); err != nil {
			t.Errorf("oracle: %s: %v", name, err)
		}
		// a thousand nodes are not printed whole
		if oracle != candidate {
			t.Errorf("%s differs: %s", name, healthFirstDifference(oracle, candidate))
			return
		}
		lines := strings.Split(oracle, "\n")
		t.Logf("%s, both sides: %d lines, the first %q", name, len(lines), lines[:min(3, len(lines))])
	}
	reloaded("reload-health: health's DynCfg nodes", func(d *daemon.Daemon) string {
		r := healthGet(d, "/api/v1/config?action=tree&path=/health")
		return healthView(r, strings.Join(healthNodeStatuses(healthView(r, string(r.Body))), "\n"))
	}, func(oracle string) error {
		if nodes := strings.Split(healthBody(oracle), "\n"); !strings.HasPrefix(oracle, "HTTP 200, ") ||
			!slices.Contains(nodes, healthJobPrefix+" accepted") || len(nodes) < 100+1 {
			return fmt.Errorf("%d health nodes, want the template, accepted, and at least 100 jobs: %.300s", len(nodes), oracle)
		}
		return nil
	})
	reloaded("reload-health: /api/v1/alarms?all", func(d *daemon.Daemon) string {
		r := healthGet(d, "/api/v1/alarms?all")
		return healthView(r, healthClock(string(r.Body)))
	}, healthHostOff)
	reloaded("reload-health: the health directories' keys", func(d *daemon.Daemon) string {
		r := healthGet(d, "/netdata.conf")
		return healthView(r, strings.Join(healthDirLines(string(r.Body), d.Opts.RunDir), "\n"))
	}, func(oracle string) error {
		// both keys, at their defaults, which the dump prints as comments (their order is the agent's, and is compared)
		lines := strings.Split(healthBody(oracle), "\n")
		slices.Sort(lines)
		if len(lines) != 2 || lines[0] != "# health config = {run}/etc/health.d" || !strings.HasPrefix(lines[1], "# stock health config = /") {
			return fmt.Errorf("the keys are %q", lines)
		}
		return nil
	})
	compare("reload-labels", func(d *daemon.Daemon) string {
		r := runCLI(t, d, "reload-labels")
		return fmt.Sprintf("%d %q %s", r.Exit, r.Stderr, labelLines(r.Stdout))
	})
	// dumpconfig prints /netdata.conf, which TestNetdataConf compares across the daemons
	for _, side := range p.Each() {
		r := runCLI(t, side.Daemon, "dumpconfig")
		b, err := rawExchange(side.Daemon.Addr, []byte("GET /netdata.conf HTTP/1.1\r\n\r\n"), 5*time.Second)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		if _, body, _ := bytes.Cut(b, []byte("\r\n\r\n")); r.Exit != 0 || r.Stdout != string(body)+"\n" {
			t.Errorf("%s: dumpconfig (exit %d) is not /netdata.conf", side.Role, r.Exit)
		}
	}

	pad := func(prefix string, n int) []byte {
		return append([]byte(prefix), bytes.Repeat([]byte(" "), n-len(prefix))...)
	}
	for _, c := range []struct {
		name      string
		payload   []byte
		pause     time.Duration
		halfClose bool
	}{
		{"no half-close", []byte("ping"), 0, false},
		{"empty", nil, 0, true},
		{"nul", []byte("ping\x00junk"), 0, true},
		{"long tail", append([]byte("ping"), append(bytes.Repeat([]byte(" "), 20000), "junk"...)...), 0, true},
		{"cut at 8191", append(bytes.Repeat([]byte(" "), 8188), "ping"...), 0, true},
		{"fits 8191", append(bytes.Repeat([]byte(" "), 8187), "ping"...), 0, true},
		{"64 KiB then a pause", pad("version", 65536), 300 * time.Millisecond, true},
		{"64 KiB less one then a pause", pad("version", 65535), 300 * time.Millisecond, true},
		{"128 KiB then a pause", pad("version", 131072), 300 * time.Millisecond, true},
	} {
		compare("raw "+c.name, func(d *daemon.Daemon) string {
			return fmt.Sprintf("%q", rawCommand(t, d.PipeName, c.payload, c.pause, c.halfClose, 2*time.Second))
		})
	}

	// a C child: node instances while it streams, then the stale-node commands once it left
	const childGUID = "5a1e0000-0000-4000-8000-00000000cc01"
	child := startCChild(t, Role("child"), "parity-cli-child", childGUID, startTee(t, p, &teeReplies{}), false)
	reachable := func(d *daemon.Daemon) (bool, bool) {
		r, err := Get(d, "/api/v1/info", nil)
		var info map[string]any
		if err == nil {
			err = json.Unmarshal(r.Body, &info)
		}
		if err != nil {
			t.Fatalf("info: %v", err)
		}
		hosts, _ := info["mirrored_hosts_status"].([]any)
		for _, h := range hosts {
			if m, _ := h.(map[string]any); m["guid"] == childGUID {
				r, _ := m["reachable"].(bool)
				return true, r
			}
		}
		return false, false
	}
	waitChild := func(want bool) {
		t.Helper()
		deadline := time.Now().Add(60 * time.Second)
		for {
			ok := true
			for _, side := range p.Each() {
				if known, r := reachable(side.Daemon); !known || r != want {
					ok = false
				}
			}
			if ok {
				return
			}
			if time.Now().After(deadline) {
				t.Fatalf("the child did not become reachable=%v on both parents", want)
			}
			time.Sleep(200 * time.Millisecond)
		}
	}
	waitChild(true)
	// C finds hostnames in its metadata database, which it writes every 5 s
	time.Sleep(7 * time.Second)
	cli("child: aclk-state json", "aclk-state", "json")
	cli("child: mark online", "mark-stale-nodes-ephemeral", "parity-cli-child")
	cli("child: remove online", "remove-stale-node", childGUID)
	if err := child.Stop(); err != nil {
		t.Fatalf("stop child: %v", err)
	}
	waitChild(false)
	cli("child left: aclk-state json", "aclk-state", "json")
	cli("child left: mark", "mark-stale-nodes-ephemeral", "parity-cli-child")
	cli("child left: mark again", "mark-stale-nodes-ephemeral", childGUID)
	cli("child left: mark all", "mark-stale-nodes-ephemeral", "ALL_NODES")
	cli("child left: remove", "remove-stale-node", childGUID)
	cli("child removed: remove again", "remove-stale-node", childGUID)
	cli("child removed: remove all", "remove-stale-node", "ALL_NODES")
	compare("child removed: mirrored hosts", func(d *daemon.Daemon) string {
		known, _ := reachable(d)
		return fmt.Sprint(known)
	})
	// the next store job deletes the removed child's dimensions
	time.Sleep(7 * time.Second)

	for _, side := range p.Each() {
		st, err := os.Lstat(side.Daemon.PipeName)
		if err != nil || st.Mode()&os.ModeSocket == 0 || st.Mode().Perm() != 0o770 {
			t.Errorf("%s: socket file %v, %v", side.Role, st, err)
		}
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
		if _, err := os.Lstat(side.Daemon.PipeName); !os.IsNotExist(err) {
			t.Errorf("%s: the socket file remains after the exit: %v", side.Role, err)
		}
	}
	// How many zero-byte reads a large command meets depends on how the kernel hands its chunks to the reader, on
	// both agents (libuv reads again after a full buffer and records the EAGAIN): those records compare as a set.
	var records [2][]string
	var zeroReads [2][]string
	for i, side := range p.Each() {
		seen := map[string]bool{}
		for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
			if !commandRecords.MatchString(l) {
				continue
			}
			n := normalizeLog(l, side.Daemon.Opts.RunDir, "")
			switch {
			case !strings.Contains(l, `msg="pipe_read_cb: `):
				records[i] = append(records[i], n)
			case !seen[n]:
				seen[n] = true
				zeroReads[i] = append(zeroReads[i], n)
			}
		}
	}
	if d := diffMultisets(records[0], records[1]); d != "" {
		t.Errorf("command records differ:\n%s", d)
	}
	if d := diffMultisets(zeroReads[0], zeroReads[1]); d != "" {
		t.Errorf("zero-byte read records differ:\n%s", d)
	}
	if len(records[0]) == 0 {
		t.Errorf("no command records")
	}

	// what the stale-node commands stored (D61.2): each parent restarted on its own cache knows the same hosts
	restarted := &Pair{
		Oracle: startOn(t, os.Getenv("PARITY_ORACLE"), Role("restart-oracle"),
			filepath.Join(p.Oracle.Opts.RunDir, "cache"), opts),
		Candidate: startOn(t, os.Getenv("PARITY_CANDIDATE"), Role("restart-candidate"),
			filepath.Join(p.Candidate.Opts.RunDir, "cache"), opts),
	}
	compareArchived(t, restarted)
}

// waitPulseStored waits until localhost's pulse charts stored a point (their second cycle): from then on an exit has
// dirty pages to flush on both sides, and the flush's records compare.
func waitPulseStored(t *testing.T, d *daemon.Daemon) {
	t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for {
		if _, charts, err := hostCharts(d, ""); err == nil && charts["netdata.clients"].LastEntry > 0 {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("%s: localhost's pulse charts stored nothing", d.Opts.Binary)
		}
		time.Sleep(200 * time.Millisecond)
	}
}

// TestCLILifecycle compares the command server's ways out: `shutdown-agent` (the exit runs on its thread),
// `fatal-agent` (an abnormal exit from a command) and a pipe that cannot be bound (no server; SIGHUP and SIGUSR2 then
// do nothing but log). Each compares what the client and the daemon exit with, whether the socket file is gone, and
// the whole logs.
func TestCLILifecycle(t *testing.T) {
	opts := daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, LogsExtra: "    level = debug\n"}
	exit := func(t *testing.T, p *Pair, command string) {
		for _, side := range p.Each() {
			waitPulseStored(t, side.Daemon)
		}
		var got [2]string
		for i, side := range p.Each() {
			r := runCLI(t, side.Daemon, command)
			code, err := side.Daemon.WaitExit(60 * time.Second)
			_, pipeErr := os.Lstat(side.Daemon.PipeName)
			got[i] = fmt.Sprintf("client %+v, daemon exit %d (%v), socket file gone %v", r, code, err,
				os.IsNotExist(pipeErr))
		}
		if got[0] != got[1] {
			t.Errorf("oracle:    %s\ncandidate: %s", got[0], got[1])
		}
		compareLogFiles(t, p)
	}
	t.Run("shutdown-agent", func(t *testing.T) {
		exit(t, StartPair(t, opts, parentIdentity), "shutdown-agent")
	})
	t.Run("fatal-agent", func(t *testing.T) {
		exit(t, StartPair(t, opts, parentIdentity), "fatal-agent")
	})
	t.Run("bind failure", func(t *testing.T) {
		o := opts
		o.PipeName = "{run}/missing/netdata.pipe"
		p := StartPair(t, o, parentIdentity)
		for _, side := range p.Each() {
			waitPulseStored(t, side.Daemon)
		}
		for _, side := range p.Each() {
			if r := runCLI(t, side.Daemon, "ping"); r.Exit != 255 {
				t.Errorf("%s: ping %+v", side.Role, r)
			}
			for _, sig := range []syscall.Signal{syscall.SIGHUP, syscall.SIGUSR2} {
				if err := syscall.Kill(side.Daemon.PID(), sig); err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				time.Sleep(300 * time.Millisecond)
			}
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFiles(t, p)
	})
}
