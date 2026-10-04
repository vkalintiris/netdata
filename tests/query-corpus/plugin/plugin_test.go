// SPDX-License-Identifier: GPL-3.0-or-later

package plugin

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"slices"
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

// The engine's Serve step: every call from the start's first byte gets the first matching rule's answer, with the
// call's parts in its body and a line after it; a payload block is taken whole; a silent rule and an unmatched call
// are recorded and left unanswered; an Expect step still matches the same bytes.
func TestTheEngineServes(t *testing.T) {
	engine, err := Engine()
	if err != nil {
		t.Fatal(err)
	}
	const a, p, s, u = "5a1e00000000400080000000000000b1", "5a1e00000000400080000000000000b2",
		"5a1e00000000400080000000000000b3", "5a1e00000000400080000000000000b4"
	sc := Scenario{Starts: []Start{{Steps: []Step{
		{Serve: &Serve{Rules: []ServeRule{
			{Name: "quiet", Re: `^config x restart$`, Silent: true},
			{Name: "update", Re: `^config \S+ update$`, Code: "202", Body: "{{id}} {{type}} [{{payload}}] {{source}}\n",
				Then: "CONFIG {{id}} status accepted\n"},
			{Name: "any", Re: `^config `, Code: "200", Type: "text/plain", Expires: "7",
				Body: "{{tx}} {{action}} {{name}} {{access}}\n"},
		}}},
		{Emit: ConfigCreate("x", "running", "single", "/p", "internal", "internal", "get update", 0x8, 0)},
		{Expect: &Expect{Name: "last", Re: `(?m)^FUNCTION (?P<last>\S+) \d+ "config x add j"[^\n]*\n`}},
		{Emit: "LAST {{last}}\n"},
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
	r := bufio.NewReader(stdout)
	next := func() string {
		t.Helper()
		line, err := r.ReadString('\n')
		if err != nil {
			t.Fatalf("stdout: %v", err)
		}
		return line
	}
	if got := next(); got != "CONFIG 'x' create 'running' 'single' '/p' 'internal' 'internal' 'get update' 0x8 0x0\n" {
		t.Errorf("create %q", got)
	}
	// a call in two writes, a payload block, a silent call, other lines, an unmatched call, then one Expect matches
	_, _ = in.Write([]byte("FUNCTION " + a + " 10 \"config x get\" \"0x7ff\" \"\""))
	time.Sleep(100 * time.Millisecond)
	_, _ = in.Write([]byte("\nFUNCTION_PAYLOAD " + p + " 120 \"config x update\" \"0x8\" \"ip=localhost\" \"application/json\"\n" +
		"{\"a\":1}\nb\nFUNCTION_PAYLOAD_END\n"))
	_, _ = in.Write([]byte("FUNCTION " + s + " 10 \"config x restart\" \"0x8\" \"\"\nFUNCTION_CANCEL " + s + "\nQUIT\n" +
		"FUNCTION " + u + " 10 \"other y\" \"0x8\" \"\"\nFUNCTION " + u + " 10 \"config x add j\" \"0x8\" \"\"\n"))
	want := []string{
		"FUNCTION_RESULT_BEGIN " + a + " 200 text/plain 7\n", a + " get  0x7ff\n", "FUNCTION_RESULT_END\n",
		"FUNCTION_RESULT_BEGIN " + p + " 202 application/json 0\n", "x application/json [{\"a\":1}\n", "b] ip=localhost\n",
		"FUNCTION_RESULT_END\n", "CONFIG x status accepted\n",
		"FUNCTION_RESULT_BEGIN " + u + " 200 text/plain 7\n", u + " add j 0x8\n", "FUNCTION_RESULT_END\n",
	}
	// the Expect's line and the Serve's answer to the same call come in either order
	var got []string
	for range len(want) + 1 {
		got = append(got, next())
	}
	if i := slices.Index(got, "LAST "+u+"\n"); i < 0 {
		t.Errorf("no Expect line in %q", got)
	} else {
		got = slices.Delete(got, i, i+1)
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("answers %q, want %q", got, want)
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
	if w := []string{"any", "update", "quiet", "-", "any"}; !reflect.DeepEqual(v.Served, w) {
		t.Errorf("served %q, want %q", v.Served, w)
	}
	if w := []string{"serve", "emit", "matched:last", "emit"}; !reflect.DeepEqual(v.Steps, w) {
		t.Errorf("steps %q, want %q", v.Steps, w)
	}
}

// The engine's Values step: the chart with its context, one block per whole second with the current
// phase's values (a dimension the phase leaves out gets no SET), a phase switched by its Until file at the next whole
// second, the last phase held, and each phase's first second recorded as the second its first block names.
func TestTheEngineCollectsValues(t *testing.T) {
	engine, err := Engine()
	if err != nil {
		t.Fatal(err)
	}
	sc := Scenario{Starts: []Start{{Steps: []Step{{WaitFile: "create"}, {Values: &Values{
		Chart: "difftest.v", Context: "difftest.ctx", Dims: []string{"a", "b"},
		Phases: []Phase{
			{Set: map[string]int64{"a": 10, "b": 1}, Until: "p1"},
			{Set: map[string]int64{"a": 70}, Until: "p2"},
			{Set: map[string]int64{"a": -95, "b": 3}, Until: "never-read"},
		},
	}}}}}}
	l, err := Install(t.TempDir(), engine, sc)
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("/bin/sh", "-c", "exec "+filepath.Join(l.PluginsDir, "difftest.plugin")+" 1")
	in, _ := cmd.StdinPipe()
	pipe, _ := cmd.StdoutPipe()
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	out := bufio.NewReader(pipe)
	line := func() string {
		s, err := out.ReadString('\n')
		if err != nil {
			t.Fatalf("stdout: %v", err)
		}
		return s
	}
	// block reads one block: its lines between BEGIN and END, and the second END names
	block := func() (string, int64) {
		if got := line(); got != "BEGIN difftest.v\n" {
			t.Fatalf("a block starts %q", got)
		}
		var sets strings.Builder
		for {
			l := line()
			if sec, ok := strings.CutPrefix(l, "END "); ok {
				var s int64
				if _, err := fmt.Sscanf(sec, "%d 0\n", &s); err != nil {
					t.Fatalf("END line %q", l)
				}
				return sets.String(), s
			}
			sets.WriteString(l)
		}
	}
	// midSecond releases a file at the middle of a second, as a check does, and returns that second
	midSecond := func(name string) int64 {
		now := time.Now()
		time.Sleep(now.Truncate(time.Second).Add(1500 * time.Millisecond).Sub(now))
		if err := l.Release(name); err != nil {
			t.Fatal(err)
		}
		return time.Now().Unix()
	}

	if _, ok := l.WaitFor(5*time.Second, func(s [][]Record) bool { return len(s) == 1 && Has(s[0], "waiting", "create") }); !ok {
		t.Fatal("the start did not wait for the chart's release")
	}
	if err := l.Release("create"); err != nil {
		t.Fatal(err)
	}
	def := []string{line(), line(), line()}
	wantDef := []string{"CHART difftest.v '' 'title' 'units' 'family' 'difftest.ctx' line 1000 1 '' '' ''\n",
		"DIMENSION a '' absolute 1 1\n", "DIMENSION b '' absolute 1 1\n"}
	if !slices.Equal(def, wantDef) {
		t.Errorf("definition %q, want %q", def, wantDef)
	}

	// phase 0 until p1 is released, then phase 1 from the next whole second, then phase 2, which holds though its
	// Until file exists
	want := []string{"SET a = 10\nSET b = 1\n", "SET a = 70\n", "SET a = -95\nSET b = 3\n"}
	firstSec := map[string]int64{}
	var last int64
	var released [2]int64
	phase := 0
	for i := 0; phase < 3; i++ {
		sets, sec := block()
		if last != 0 && sec != last+1 {
			t.Errorf("block %d names second %d after %d", i, sec, last)
		}
		last = sec
		if phase+1 < len(want) && sets == want[phase+1] {
			phase++
		}
		if sets != want[phase] {
			t.Fatalf("block %d: %q, in phase %d", i, sets, phase)
		}
		if _, seen := firstSec[fmt.Sprint(phase)]; !seen {
			firstSec[fmt.Sprint(phase)] = sec
			switch phase {
			case 0:
				released[0] = midSecond("p1")
			case 1:
				released[1] = midSecond("p2")
			case 2:
				if err := l.Release("never-read"); err != nil {
					t.Fatal(err)
				}
				for range 2 {
					if sets, _ := block(); sets != want[2] {
						t.Errorf("the last phase did not hold: %q", sets)
					}
				}
				phase++
			}
		}
	}
	// a release at mid-second switches at the very next whole second
	if firstSec["1"] != released[0]+1 || firstSec["2"] != released[1]+1 {
		t.Errorf("phases began at %v, released during %v: not the next whole second", firstSec, released)
	}
	_ = in.Close()
	if err := cmd.Wait(); err != nil {
		t.Fatalf("the start ended %v", err)
	}
	starts, err := l.Starts()
	if err != nil || len(starts) != 1 {
		t.Fatalf("starts: %d %v", len(starts), err)
	}
	if got := PhaseSeconds(starts[0]); !reflect.DeepEqual(got, firstSec) {
		t.Errorf("phase records %v, the blocks' first seconds %v", got, firstSec)
	}
	if v := ViewOf(starts[0]); !slices.Equal(v.Steps, []string{"wait", "values"}) || v.End != "eof-exit 0" {
		t.Errorf("view %+v", v)
	}
}
