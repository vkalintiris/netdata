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
	if !strings.HasPrefix(string(b), "#!/bin/sh\nd="+l.Dir+"\n") || !strings.HasSuffix(string(b), "exec \"$d/engine\" \"$@\"\n") {
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
	// the wrapper wrote pre-$$ and the engine read pre-<its pid>: the exec kept the pid
	pre := starts[0][0].Pre
	if !strings.HasPrefix(pre, "argv0 "+filepath.Join(l.PluginsDir, "difftest.plugin")+"\n") || !strings.Contains(pre, "SigIgn:") ||
		!strings.Contains(pre, "Max open files") {
		t.Errorf("pre:\n%s", pre)
	}
}
