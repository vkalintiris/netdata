// SPDX-License-Identifier: GPL-3.0-or-later

package notify

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/gobuild"
)

func TestMain(m *testing.M) {
	code := m.Run()
	gobuild.Cleanup()
	os.Exit(code)
}

// command is the line an agent runs a notification with (health_notifications.c:107-150): `exec` and 34 single-quoted
// words, under /bin/sh -c; the alert's name is word 7 and its new status word 9.
func command(stub, name, status string) *exec.Cmd {
	words := []string{stub}
	for i := 1; i <= 33; i++ {
		switch i {
		case ArgName:
			words = append(words, name)
		case ArgStatus:
			words = append(words, status)
		default:
			words = append(words, fmt.Sprintf("a%d", i))
		}
	}
	line := "exec"
	for _, w := range words {
		line += " '" + w + "'"
	}
	return exec.Command("/bin/sh", "-c", line)
}

func install(t *testing.T, ctl Control) string {
	t.Helper()
	stub, err := Stub()
	if err != nil {
		t.Fatal(err)
	}
	run := t.TempDir()
	if err := Install(run, stub, ctl); err != nil {
		t.Fatal(err)
	}
	return run
}

// A call records what it was given and how it ended, numbered in call order; it writes nothing to stdout or stderr.
func TestTheStubRecordsACall(t *testing.T) {
	run := install(t, Control{})
	for i := range 3 {
		cmd := command(Path(run), "an alert", "WARNING")
		cmd.Dir = run
		cmd.Env = []string{"B=2", "A=1 x", fmt.Sprintf("CALL=%d", i)}
		var out, errOut bytes.Buffer
		cmd.Stdout, cmd.Stderr = &out, &errOut
		if err := cmd.Run(); err != nil {
			t.Fatalf("call %d: %v", i, err)
		}
		if out.Len() != 0 || errOut.Len() != 0 {
			t.Errorf("call %d wrote stdout %q, stderr %q", i, out.String(), errOut.String())
		}
	}
	calls, err := Calls(run)
	if err != nil || len(calls) != 3 {
		t.Fatalf("calls: %d %v", len(calls), err)
	}
	for i, c := range calls {
		if c.Seq != i+1 || len(c.Argv) != 34 || c.Argv[0] != Path(run) || c.Argv[1] != "a1" || c.Argv[ArgName] != "an alert" ||
			c.Argv[ArgStatus] != "WARNING" || c.Argv[33] != "a33" {
			t.Errorf("call %d: seq %d, argv %q", i, c.Seq, c.Argv)
		}
		// dash adds PWD to an environment without it; the rest is as given, sorted
		env := slices.DeleteFunc(slices.Clone(c.Env), func(e string) bool { return strings.HasPrefix(e, "PWD=") })
		if want := []string{"A=1 x", "B=2", fmt.Sprintf("CALL=%d", i)}; !slices.Equal(env, want) {
			t.Errorf("call %d: env %q, want %q", i, c.Env, want)
		}
		if c.Cwd != run || c.Ppid != os.Getpid() || c.Pid == 0 || c.ParentComm == "" || c.StartUt == 0 || c.Rule != -1 ||
			c.End != "exit 0" || c.EndUt < c.StartUt {
			t.Errorf("call %d: %+v", i, c)
		}
	}
}

// The first rule matching the alert and the status decides the call: its exit code and its sleep.
func TestTheStubFollowsItsRules(t *testing.T) {
	run := install(t, Control{Rules: []Rule{
		{Alert: "a", Status: "CRITICAL", Exit: 3},
		{Alert: "a", SleepMs: 300, Exit: 4},
		{Status: "CLEAR", Exit: 5},
		{Alert: "a", Status: "CRITICAL", Exit: 6},
	}})
	cases := []struct {
		name, status string
		rule, exit   int
	}{
		{"a", "CRITICAL", 0, 3}, {"a", "WARNING", 1, 4}, {"b", "CLEAR", 2, 5}, {"a", "CLEAR", 1, 4}, {"b", "WARNING", -1, 0},
	}
	for _, c := range cases {
		started := time.Now()
		err := command(Path(run), c.name, c.status).Run()
		code := 0
		if ee, ok := err.(*exec.ExitError); ok {
			code = ee.ExitCode()
		} else if err != nil {
			t.Fatal(err)
		}
		if code != c.exit {
			t.Errorf("%s %s: exit %d, want %d", c.name, c.status, code, c.exit)
		}
		if slept := time.Since(started) >= 300*time.Millisecond; slept != (c.rule == 1) {
			t.Errorf("%s %s: took %v", c.name, c.status, time.Since(started))
		}
	}
	calls, err := Calls(run)
	if err != nil || len(calls) != len(cases) {
		t.Fatalf("calls: %d %v", len(calls), err)
	}
	for i, c := range cases {
		if calls[i].Rule != c.rule || calls[i].End != fmt.Sprintf("exit %d", c.exit) {
			t.Errorf("%s %s: rule %d, end %q", c.name, c.status, calls[i].Rule, calls[i].End)
		}
	}
}

// SIGTERM is recorded and kills the call as it would have; a rule that ignores it leaves SIGKILL, and no end.
func TestTheStubRecordsItsSignals(t *testing.T) {
	run := install(t, Control{Rules: []Rule{
		{Alert: "polite", SleepMs: 30000},
		{Alert: "stubborn", SleepMs: 30000, IgnoreTerm: true},
	}})
	started := func(n int) {
		t.Helper()
		for end := time.Now().Add(5 * time.Second); ; time.Sleep(20 * time.Millisecond) {
			if calls, err := Calls(run); err == nil && len(calls) == n {
				return
			}
			if time.Now().After(end) {
				t.Fatalf("call %d did not start", n)
			}
		}
	}
	signaled := func(err error) syscall.Signal {
		if ee, ok := err.(*exec.ExitError); ok {
			if ws, ok := ee.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
				return ws.Signal()
			}
		}
		t.Fatalf("the call ended %v, want a signal", err)
		return 0
	}

	polite := command(Path(run), "polite", "WARNING")
	if err := polite.Start(); err != nil {
		t.Fatal(err)
	}
	started(1)
	_ = polite.Process.Signal(syscall.SIGTERM)
	if sig := signaled(polite.Wait()); sig != syscall.SIGTERM {
		t.Errorf("polite died by %v", sig)
	}

	stubborn := command(Path(run), "stubborn", "WARNING")
	if err := stubborn.Start(); err != nil {
		t.Fatal(err)
	}
	started(2)
	_ = stubborn.Process.Signal(syscall.SIGTERM)
	time.Sleep(300 * time.Millisecond)
	if err := stubborn.Process.Signal(syscall.Signal(0)); err != nil {
		t.Fatalf("stubborn did not survive SIGTERM: %v", err)
	}
	_ = stubborn.Process.Kill()
	if sig := signaled(stubborn.Wait()); sig != syscall.SIGKILL {
		t.Errorf("stubborn died by %v", sig)
	}

	calls, err := Calls(run)
	if err != nil || len(calls) != 2 {
		t.Fatalf("calls: %d %v", len(calls), err)
	}
	if calls[0].End != "signal SIGTERM" || calls[0].Rule != 0 || calls[1].End != "" || calls[1].Rule != 1 {
		t.Errorf("ends %q and %q, rules %d and %d", calls[0].End, calls[1].End, calls[0].Rule, calls[1].Rule)
	}
}

// Install lays out the program and its control file; a missing control file is recorded, not fatal.
func TestInstallAndAMissingControlFile(t *testing.T) {
	run := install(t, Control{Rules: []Rule{{Alert: "a", Exit: 2}}})
	if st, err := os.Stat(Path(run)); err != nil || st.Mode().Perm()&0o111 == 0 || Path(run) != filepath.Join(run, "notify", "stub") {
		t.Fatalf("stub: %v %v", st, err)
	}
	ctl, err := ReadControl(Dir(run))
	if err != nil || len(ctl.Rules) != 1 || ctl.Rules[0] != (Rule{Alert: "a", Exit: 2}) {
		t.Fatalf("control: %+v %v", ctl, err)
	}
	if err := os.Remove(filepath.Join(Dir(run), "control.json")); err != nil {
		t.Fatal(err)
	}
	if err := command(Path(run), "a", "WARNING").Run(); err != nil {
		t.Fatalf("without a control file: %v", err)
	}
	calls, err := Calls(run)
	if err != nil || len(calls) != 1 || calls[0].Rule != -2 || calls[0].End != "exit 0" {
		t.Errorf("calls %+v %v", calls, err)
	}
}
