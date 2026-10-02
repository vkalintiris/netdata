// SPDX-License-Identifier: GPL-3.0-or-later

package plugin

import (
	"bufio"
	"encoding/json"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestMain(m *testing.M) {
	code := m.Run()
	Cleanup()
	os.Exit(code)
}

func TestScenarioRoundTrips(t *testing.T) {
	sc := Scenario{Starts: []Start{
		{Steps: []Step{{Emit: "X\n"}, {Stderr: "e"}, {SleepMs: 5}, {Collect: &Collect{Chart: "a.b", Dims: []string{"x"}, N: 2}},
			{WaitFile: "release-1"}, {Exit: ExitCode(0)}}},
		{Steps: []Step{{Hang: true}}},
	}}
	b, err := json.Marshal(sc)
	if err != nil {
		t.Fatal(err)
	}
	var got Scenario
	if err := json.Unmarshal(b, &got); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, sc) {
		t.Errorf("got %+v, want %+v", got, sc)
	}
}

func TestInstallLaysOutThePlugin(t *testing.T) {
	run := t.TempDir()
	fake := filepath.Join(t.TempDir(), "engine")
	if err := os.WriteFile(fake, []byte("engine"), 0o755); err != nil {
		t.Fatal(err)
	}
	l, err := Install(run, fake, Scenario{Starts: []Start{{Steps: []Step{{Hang: true}}}}})
	if err != nil {
		t.Fatal(err)
	}
	if l != (Layout{PluginsDir: filepath.Join(run, "plugins.d"), Dir: filepath.Join(run, "difftest")}) {
		t.Errorf("layout %+v", l)
	}
	st, err := os.Stat(filepath.Join(l.PluginsDir, "difftest.plugin"))
	if err != nil || st.Mode().Perm() != 0o755 {
		t.Fatalf("wrapper: %v %v", st, err)
	}
	b, _ := os.ReadFile(filepath.Join(l.PluginsDir, "difftest.plugin"))
	if !strings.HasPrefix(string(b), "#!/bin/sh\nd='"+l.Dir+"'\n") || !strings.HasSuffix(string(b), "exec \"$d/engine\" \"$@\"\n") {
		t.Errorf("wrapper:\n%s", b)
	}
	if b, _ := os.ReadFile(filepath.Join(l.Dir, "engine")); string(b) != "engine" {
		t.Errorf("engine %q", b)
	}
	var sc Scenario
	if b, _ := os.ReadFile(filepath.Join(l.Dir, "scenario.json")); json.Unmarshal(b, &sc) != nil || len(sc.Starts) != 1 {
		t.Errorf("scenario %+v", sc)
	}
}

// The engine, through the wrapper as an agent runs it (`exec <plugin> <update every> <options>` under /bin/sh),
// plays its scenario and records it; the second start plays the second scenario entry.
func TestTheEnginePlaysAScenario(t *testing.T) {
	engine, err := Engine()
	if err != nil {
		t.Fatal(err)
	}
	run := t.TempDir()
	sc := Scenario{Starts: []Start{
		{Steps: []Step{{Emit: "HELLO\n"}, {Collect: &Collect{Chart: "difftest.a", Dims: []string{"x"}, N: 1}},
			{WaitFile: "release-1"}, {Exit: ExitCode(3)}}},
		{Steps: []Step{{Hang: true}}},
	}}
	l, err := Install(run, engine, sc)
	if err != nil {
		t.Fatal(err)
	}
	start := func() (*exec.Cmd, io.WriteCloser, *bufio.Reader) {
		cmd := exec.Command("/bin/sh", "-c", "exec "+filepath.Join(l.PluginsDir, "difftest.plugin")+" 1 alpha 'b c'")
		in, _ := cmd.StdinPipe()
		out, _ := cmd.StdoutPipe()
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		return cmd, in, bufio.NewReader(out)
	}

	cmd, in, out := start()
	var lines []string
	for range 6 {
		line, err := out.ReadString('\n')
		if err != nil {
			t.Fatalf("after %q: %v", lines, err)
		}
		lines = append(lines, line)
	}
	if lines[0] != "HELLO\n" || !strings.HasPrefix(lines[1], "CHART difftest.a ") || lines[2] != "DIMENSION x '' absolute 1 1\n" ||
		lines[3] != "BEGIN difftest.a\n" || lines[4] != "SET x = 1\n" || !strings.HasPrefix(lines[5], "END ") {
		t.Errorf("lines %q", lines)
	}
	if _, ok := l.WaitFor(5*time.Second, func(s [][]Record) bool { return len(s) == 1 && Has(s[0], "waiting", "release-1") }); !ok {
		t.Fatal("no waiting record")
	}
	_, _ = in.Write([]byte("QUIT\n"))
	if _, ok := l.WaitFor(5*time.Second, func(s [][]Record) bool { return strings.Contains(ViewOf(s[0]).Stdin, "QUIT") }); !ok {
		t.Fatal("QUIT not recorded")
	}
	if err := l.Release("release-1"); err != nil {
		t.Fatal(err)
	}
	err = cmd.Wait()
	if ee, ok := err.(*exec.ExitError); !ok || ee.ExitCode() != 3 {
		t.Fatalf("first start ended %v", err)
	}

	cmd, in, _ = start()
	if _, ok := l.WaitFor(5*time.Second, func(s [][]Record) bool { return len(s) == 2 && Has(s[1], "step", "hang") }); !ok {
		t.Fatal("the second start did not hang")
	}
	_ = in.Close()
	if err := cmd.Wait(); err != nil {
		t.Fatalf("second start ended %v", err)
	}

	starts, err := l.Starts()
	if err != nil {
		t.Fatal(err)
	}
	views := []View{ViewOf(starts[0]), ViewOf(starts[1])}
	want := []View{
		{Args: []string{"1", "alpha", "b c"}, Stdin: "QUIT\n", Steps: []string{"emit", "collect", "wait"}, End: "exit 3"},
		{Args: []string{"1", "alpha", "b c"}, EOF: true, Steps: []string{"hang"}, End: "eof-exit 0"},
	}
	if !reflect.DeepEqual(views, want) {
		t.Errorf("views %+v, want %+v", views, want)
	}
	// the spawn context the spawn commit compares: the parent (this test), stdio's open flags, the scheduling
	first := starts[0][0]
	if first.ParentComm == "" || len(first.StdioFlags) != 3 || first.OomScoreAdj == "" || first.Nice == "" || first.SchedPolicy == "" {
		t.Errorf("spawn context: %+v", first)
	}
	// the wrapper wrote pre-$$ and the engine read pre-<its pid>: the exec kept the pid
	pre := starts[0][0].Pre
	if !strings.HasPrefix(pre, "argv0 "+filepath.Join(l.PluginsDir, "difftest.plugin")+"\n") || !strings.Contains(pre, "SigIgn:") ||
		!strings.Contains(pre, "Max open files") {
		t.Errorf("pre:\n%s", pre)
	}
}

// The engine's Expect steps: a call's lines written in pieces are matched once and in order, the captured transaction
// answers in later emits, a payload block is matched whole, a timeout and stdin's end are recorded; a background
// collection goes on while the steps play, and none of its lines lands inside an answer's span.
func TestTheEngineAnswersCalls(t *testing.T) {
	engine, err := Engine()
	if err != nil {
		t.Fatal(err)
	}
	const a, p = "5a1e00000000400080000000000000a1", "5a1e00000000400080000000000000a2"
	sc := Scenario{Starts: []Start{{Steps: []Step{
		// stdin's end then ends only the last Expect, which the exit follows
		{StayOnEOF: true},
		{Emit: "FUNCTION GLOBAL 'difftest-fn' 10 'help' 'top' '0x13' 100 3\n"},
		{Collect: &Collect{Chart: "difftest.bg", Dims: []string{"x"}, N: 30, Background: true}},
		ExpectFunction("a"),
		{Emit: Result("{{a}}", "200", "application/json", "0", "{\"a\":1}\n")},
		ExpectPayload("p"),
		{Emit: Result("{{p}}", "200", "text/plain", "0", "") + "{{nope}} $1\n"},
		{Expect: &Expect{Name: "never", Re: `(?m)^NEVER\n`, TimeoutMs: 100}},
		ExpectCancel("x"),
		{Emit: Progress("{{x}}", 1, 2)},
		ExpectProgress("last"),
		{Exit: ExitCode(0)},
	}}}}
	l, err := Install(t.TempDir(), engine, sc)
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("/bin/sh", "-c", "exec "+filepath.Join(l.PluginsDir, "difftest.plugin")+" 1")
	in, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	lines := make(chan string, 1000)
	go func() {
		r := bufio.NewReader(stdout)
		for {
			line, err := r.ReadString('\n')
			if err != nil {
				close(lines)
				return
			}
			lines <- line
		}
	}()
	// next is the next line on stdout
	next := func(want string) string {
		t.Helper()
		select {
		case line, ok := <-lines:
			if !ok {
				t.Fatalf("stdout ended before %q", want)
			}
			return line
		case <-time.After(10 * time.Second):
			t.Fatalf("no %q within 10 s", want)
		}
		return ""
	}
	// waitLine waits for a line, skipping the background collection's
	waitLine := func(want string) {
		t.Helper()
		for next(want) != want {
		}
	}
	// waitSpan waits for a span's first line, then takes the rest as the very next lines: a background block's line
	// among them fails (each emit is one write under the engine's stdout lock)
	waitSpan := func(span ...string) {
		t.Helper()
		waitLine(span[0])
		for _, want := range span[1:] {
			if got := next(want); got != want {
				t.Errorf("inside the span %q: %q, want %q", span[0], got, want)
			}
		}
	}
	// the FUNCTION line in two writes: matched only once whole
	_, _ = in.Write([]byte("FUNCTION " + a + " 10 \"difftest-fn x\" \"0x13\" \"src\""))
	time.Sleep(200 * time.Millisecond)
	_, _ = in.Write([]byte("\n"))
	waitSpan("FUNCTION_RESULT_BEGIN "+a+" 200 application/json 0\n", "{\"a\":1}\n", "FUNCTION_RESULT_END\n")
	block := "FUNCTION_PAYLOAD " + p + " 10 \"difftest-fn y\" \"0x13\" \"src\" \"application/json\"\n{\"b\":2}\nFUNCTION_PAYLOAD_END x\n\nFUNCTION_PAYLOAD_END\n"
	_, _ = in.Write([]byte(block))
	// an unknown name and `$` stay as written, in the same write
	waitSpan("FUNCTION_RESULT_BEGIN "+p+" 200 text/plain 0\n", "FUNCTION_RESULT_END\n", "{{nope}} $1\n")
	_, _ = in.Write([]byte("FUNCTION_CANCEL " + a + "\n"))
	waitLine("FUNCTION_PROGRESS " + a + " 1 2\n")
	if _, ok := l.WaitFor(5*time.Second, func(s [][]Record) bool { return len(s) == 1 && Has(s[0], "collected", "") }); !ok {
		t.Error("the background collection wrote nothing while the steps played")
	}
	_ = in.Close()
	if err := cmd.Wait(); err != nil {
		t.Errorf("the engine ended %v", err)
	}

	starts, err := l.Starts()
	if err != nil || len(starts) != 1 {
		t.Fatalf("starts %d: %v", len(starts), err)
	}
	v := ViewOf(starts[0])
	want := []string{"stay-on-eof", "emit", "collect", "matched:a", "emit", "matched:p", "emit", "expect-timeout:never",
		"matched:x", "emit", "expect-eof:last"}
	if !reflect.DeepEqual(v.Steps, want) {
		t.Errorf("steps %q, want %q", v.Steps, want)
	}
	if r, ok := Matched(starts[0], "p"); !ok || r.Data != block || r.Groups["p"] != p {
		t.Errorf("the payload's match: %+v", r)
	}
	if r, ok := Matched(starts[0], "a"); !ok || r.Groups["a"] != a {
		t.Errorf("the call's match: %+v", r)
	}
}
