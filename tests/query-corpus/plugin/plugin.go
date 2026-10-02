// SPDX-License-Identifier: GPL-3.0-or-later

// Package plugin is the harness's fake external plugin: `difftest.plugin`, a dash wrapper that records the process
// state an agent started it with and then execs a static Go engine (cmd/difftest) under the same pid. The engine
// plays a scenario (lines to emit, collections, a release to wait for, an exit) and writes one JSONL record file per
// start: its arguments, everything it read on stdin, how it ended. A check installs it in each side's run directory
// and compares what the two agents did with it.
package plugin

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"time"
)

// Name is the plugin's name: the file `difftest.plugin` in the run directory's `plugins.d`.
const Name = "difftest"

// Scenario is what the engine plays: start i plays Starts[i], the last one again for later starts.
type Scenario struct {
	Starts []Start `json:"starts"`
}

// Start is one run of the plugin: its steps in order; at stdin's end (the agent's stop) the engine exits 0.
type Start struct {
	Steps []Step `json:"steps"`
}

// Step is one action; exactly one field is set.
type Step struct {
	// Emit is written to stdout in one write.
	Emit string `json:"emit,omitempty"`
	// Stderr is one line written to stderr (the agent's collectors log).
	Stderr string `json:"stderr,omitempty"`
	// SleepMs pauses.
	SleepMs int `json:"sleepMs,omitempty"`
	// Collect defines a chart and collects it on whole seconds.
	Collect *Collect `json:"collect,omitempty"`
	// WaitFile waits until the check creates this file in the engine's directory (see Release).
	WaitFile string `json:"waitFile,omitempty"`
	// Exit ends the process with this code.
	Exit *int `json:"exit,omitempty"`
	// Hang waits for stdin's end or a signal.
	Hang bool `json:"hang,omitempty"`
	// Raise kills the process with this signal (its default action), e.g. "SIGUSR1".
	Raise string `json:"raise,omitempty"`
	// IgnoreTerm ignores SIGTERM from here on (the agent's kill must escalate to SIGKILL).
	IgnoreTerm bool `json:"ignoreTerm,omitempty"`
	// StayOnEOF keeps the process after stdin's end (recorded) instead of exiting.
	StayOnEOF bool `json:"stayOnEOF,omitempty"`
	// Expect waits until stdin, after the previous match, matches a pattern (see Expect).
	Expect *Expect `json:"expect,omitempty"`
}

// Expect is a step that waits for the agent to write something on the plugin's stdin: Re (RE2) is matched against
// what stdin carried after the end of the previous Expect's match, again at each read, so a call's line is matched
// once and in order. The match is recorded (`matched`, its text and named groups); the groups' values replace
// `{{name}}` in the Emit and Stderr steps that follow (a later capture of a name wins). Without TimeoutMs the step
// waits until stdin's end (`expect-eof`, then the next step); with it, `expect-timeout` after that many ms.
type Expect struct {
	Name      string `json:"name"`
	Re        string `json:"re"`
	TimeoutMs int    `json:"timeoutMs,omitempty"`
}

// Collect is CHART and DIMENSION lines for Chart (type.id, plugin left empty: the agent names it after the file),
// then N blocks `BEGIN`, a `SET` per dimension and `END <sec> 0`, one per whole wall-clock second, values counting from 1.
type Collect struct {
	Chart string   `json:"chart"`
	Dims  []string `json:"dims"`
	N     int      `json:"n"`
	// InBlock, when set, is a line written after each BEGIN, before the SETs (e.g. `HOST <guid>`: C keeps the scope
	// chart across HOST, so the block still completes the chart BEGIN named)
	InBlock string `json:"inBlock,omitempty"`
	// AfterBlock, when set, is a line written after each END (e.g. `HOST localhost`, so the next BEGIN finds the chart)
	AfterBlock string `json:"afterBlock,omitempty"`
	// Background returns at once: the definition and the blocks go out while the next steps play (each write whole,
	// so a block never lands inside another step's emit, e.g. a FUNCTION_RESULT span)
	Background bool `json:"background,omitempty"`
}

// ExitCode is a step's exit code.
func ExitCode(code int) *int { return &code }

// Record is one line the engine writes; Kind says which fields are set.
type Record struct {
	T    time.Time `json:"t"`
	Kind string    `json:"kind"` // start, stdin, eof, step, collected, waiting, expect, matched, expect-timeout, expect-eof, signal, end
	// start
	Args []string `json:"args,omitempty"`
	Env  []string `json:"env,omitempty"`
	Cwd  string   `json:"cwd,omitempty"`
	Pid  int      `json:"pid,omitempty"`
	Ppid int      `json:"ppid,omitempty"`
	Pgid int      `json:"pgid,omitempty"`
	Sid  int      `json:"sid,omitempty"`
	Fds  []string `json:"fds,omitempty"`
	Pre  string   `json:"pre,omitempty"` // the wrapper's snapshot: argv0, /proc/self/status lines, limits
	// the parent's comm (the spawn server's or the daemon's), the open flags of fds 0-2 (`/proc/self/fdinfo`),
	// `oom_score_adj`, nice and the scheduling policy (`/proc/self/stat` fields 19 and 41)
	ParentComm  string   `json:"parentComm,omitempty"`
	StdioFlags  []string `json:"stdioFlags,omitempty"`
	OomScoreAdj string   `json:"oomScoreAdj,omitempty"`
	Nice        string   `json:"nice,omitempty"`
	SchedPolicy string   `json:"schedPolicy,omitempty"`
	// stdin; matched (the matched text)
	Data string `json:"data,omitempty"`
	// matched: the named groups
	Groups map[string]string `json:"groups,omitempty"`
	// step, waiting; expect, matched, expect-timeout, expect-eof (the Expect's name)
	Step string `json:"step,omitempty"`
	File string `json:"file,omitempty"`
	// collected
	Sec int64 `json:"sec,omitempty"`
	// signal, end
	Signal string `json:"signal,omitempty"`
	How    string `json:"how,omitempty"` // exit N, eof-exit 0, signal SIGTERM, cap
}

// Layout is where a side's plugin lives.
type Layout struct {
	PluginsDir string // <run>/plugins.d
	Dir        string // <run>/difftest: the engine, the scenario, the records
}

// LayoutOf is the layout under a run directory.
func LayoutOf(runDir string) Layout {
	return Layout{PluginsDir: filepath.Join(runDir, "plugins.d"), Dir: filepath.Join(runDir, Name)}
}

// wrapper is the plugin file: it records the state the agent gave the process before Go's runtime changes it
// (inherited ignored signals, the soft open-files limit), then execs the engine under the same pid. dash builtins
// only, so `$$` stays the plugin's pid.
const wrapper = `#!/bin/sh
d='%s'
{
  echo "argv0 $0"
  while read -r k v; do
    case "$k" in Umask:|Sig*|Shd*|Cap*|NoNewPrivs:|Seccomp:) echo "$k $v" ;; esac
  done < /proc/self/status
  while read -r l; do echo "$l"; done < /proc/self/limits
} > "$d/pre-$$"
exec "$d/engine" "$@"
`

// Install writes the plugin under runDir: the wrapper, the engine (a hard link of `engine`, a copy across devices)
// and the scenario.
func Install(runDir, engine string, sc Scenario) (Layout, error) {
	return install(LayoutOf(runDir), Name+".plugin", engine, sc)
}

// InstallAs writes another plugin: the wrapper as `file` in pluginsDir, its engine, scenario and records in
// MoreDir(runDir, pluginsDir, file).
func InstallAs(runDir, pluginsDir, file, engine string, sc Scenario) (Layout, error) {
	return install(Layout{PluginsDir: pluginsDir, Dir: MoreDir(runDir, pluginsDir, file)}, file, engine, sc)
}

// MoreDir is where InstallAs keeps a plugin's engine and records: `<runDir>/engines/<plugins dir's name>-<file>`.
func MoreDir(runDir, pluginsDir, file string) string {
	return filepath.Join(runDir, "engines", filepath.Base(pluginsDir)+"-"+file)
}

func install(l Layout, file, engine string, sc Scenario) (Layout, error) {
	for _, dir := range []string{l.PluginsDir, l.Dir} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return l, err
		}
	}
	// written aside and renamed in: a scan never sees a partial wrapper
	tmp := filepath.Join(l.Dir, "wrapper.tmp")
	if err := os.WriteFile(tmp, []byte(fmt.Sprintf(wrapper, l.Dir)), 0o755); err != nil {
		return l, err
	}
	if err := linkOrCopy(engine, filepath.Join(l.Dir, "engine")); err != nil {
		return l, err
	}
	b, err := json.MarshalIndent(sc, "", "  ")
	if err != nil {
		return l, err
	}
	if err := os.WriteFile(filepath.Join(l.Dir, "scenario.json"), b, 0o644); err != nil {
		return l, err
	}
	return l, os.Rename(tmp, filepath.Join(l.PluginsDir, file))
}

func linkOrCopy(from, to string) error {
	if err := os.Link(from, to); err == nil {
		return nil
	}
	b, err := os.ReadFile(from)
	if err != nil {
		return err
	}
	return os.WriteFile(to, b, 0o755)
}

// Release creates the file a WaitFile step waits for.
func (l Layout) Release(name string) error {
	return os.WriteFile(filepath.Join(l.Dir, name), nil, 0o644)
}

var engine struct {
	once sync.Once
	path string
	err  error
}

// Engine builds the engine once per process (stdlib only, offline) and returns its path; Cleanup removes it.
func Engine() (string, error) {
	engine.once.Do(func() {
		goBin, err := exec.LookPath("go")
		if err != nil {
			engine.err = fmt.Errorf("plugin: the engine needs go on PATH: %w", err)
			return
		}
		_, self, _, _ := runtime.Caller(0)
		module := filepath.Dir(filepath.Dir(self))
		dir, err := os.MkdirTemp("", "difftest-engine-")
		if err != nil {
			engine.err = err
			return
		}
		out := filepath.Join(dir, "engine")
		cmd := exec.Command(goBin, "build", "-trimpath", "-o", out, "./plugin/cmd/difftest")
		cmd.Dir = module
		cmd.Env = append(os.Environ(), "CGO_ENABLED=0", "GOPROXY=off", "GOTOOLCHAIN=local", "GOWORK=off")
		if b, err := cmd.CombinedOutput(); err != nil {
			engine.err = fmt.Errorf("plugin: building the engine: %v: %s", err, b)
			return
		}
		engine.path = out
	})
	return engine.path, engine.err
}

// Cleanup removes the built engine (the run directories keep their links).
func Cleanup() {
	if engine.path != "" {
		_ = os.RemoveAll(filepath.Dir(engine.path))
	}
}

// Starts reads every start's records, in start order.
func (l Layout) Starts() ([][]Record, error) {
	files, err := filepath.Glob(filepath.Join(l.Dir, "start-*.jsonl"))
	if err != nil {
		return nil, err
	}
	slices.Sort(files)
	var out [][]Record
	for _, f := range files {
		recs, err := readRecords(f)
		if err != nil {
			return nil, err
		}
		out = append(out, recs)
	}
	return out, nil
}

func readRecords(path string) ([]Record, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	// only whole lines: the last one may be in the writing, or cut by a kill
	if i := bytes.LastIndexByte(b, '\n'); i >= 0 {
		b = b[:i+1]
	} else {
		b = nil
	}
	var out []Record
	sc := bufio.NewScanner(bytes.NewReader(b))
	sc.Buffer(make([]byte, 0, 1<<16), 1<<24)
	for sc.Scan() {
		var r Record
		if err := json.Unmarshal(sc.Bytes(), &r); err != nil {
			return nil, fmt.Errorf("plugin: %s: %w", path, err)
		}
		out = append(out, r)
	}
	return out, sc.Err()
}

// WaitFor polls the starts until ok holds or the timeout passes, returning the last read.
func (l Layout) WaitFor(timeout time.Duration, ok func([][]Record) bool) ([][]Record, bool) {
	deadline := time.Now().Add(timeout)
	for {
		starts, err := l.Starts()
		if err == nil && ok(starts) {
			return starts, true
		}
		if time.Now().After(deadline) {
			return starts, false
		}
		time.Sleep(100 * time.Millisecond)
	}
}

// Has reports whether a start holds a record of this kind (and, when given, step or file).
func Has(start []Record, kind, name string) bool {
	return slices.ContainsFunc(start, func(r Record) bool {
		return r.Kind == kind && (name == "" || r.Step == name || r.File == name)
	})
}

// Before is a start's records written before t: what the agent did before its stop, whose kill races the plugin's
// last records.
func Before(start []Record, t time.Time) []Record {
	var out []Record
	for _, r := range start {
		if r.T.Before(t) {
			out = append(out, r)
		}
	}
	return out
}

// View is what two agents must agree on about one start: its arguments, all it read on stdin (chunking dropped),
// whether it saw stdin's end, its steps (an Expect's outcome as `matched:<name>`, `expect-timeout:<name>` or
// `expect-eof:<name>`), and how it ended.
type View struct {
	Args  []string
	Stdin string
	EOF   bool
	Steps []string
	End   string
}

// ViewOf is a start's view.
func ViewOf(start []Record) View {
	var v View
	var stdin strings.Builder
	for _, r := range start {
		switch r.Kind {
		case "start":
			v.Args = r.Args
		case "stdin":
			stdin.WriteString(r.Data)
		case "eof":
			v.EOF = true
		case "step", "waiting":
			v.Steps = append(v.Steps, r.Step)
		case "matched", "expect-timeout", "expect-eof":
			v.Steps = append(v.Steps, r.Kind+":"+r.Step)
		case "end":
			v.End = r.How
		}
	}
	v.Stdin = stdin.String()
	return v
}

// Time is when a start's first record of this kind was written (zero when none).
func Time(start []Record, kind string) time.Time {
	for _, r := range start {
		if r.Kind == kind {
			return r.T
		}
	}
	return time.Time{}
}

// Matched is the record of a start's Expect `name` that matched (ok false when it did not, or not yet).
func Matched(start []Record, name string) (Record, bool) {
	for _, r := range start {
		if r.Kind == "matched" && r.Step == name {
			return r, true
		}
	}
	return Record{}, false
}

// Expectations of the lines an agent writes to a plugin for a call (src/plugins.d/pluginsd_functions.c:10-50,
// :255-257, :323-327, :369-373): each captures the call's transaction (the 32 lowercase hex digits the agent writes,
// or whatever it writes instead) into the group `name`. They match any arguments: what the agent wrote is compared
// byte for byte through the start's view, so a candidate's wrong line still gets the plugin's answer.

// ExpectFunction waits for a `FUNCTION <tx> ...` line.
func ExpectFunction(name string) Step {
	return Step{Expect: &Expect{Name: name, Re: `(?m)^FUNCTION (?P<` + name + `>\S+) [^\n]*\n`}}
}

// ExpectPayload waits for a whole `FUNCTION_PAYLOAD <tx> ...` block, up to its `FUNCTION_PAYLOAD_END` line.
func ExpectPayload(name string) Step {
	return Step{Expect: &Expect{Name: name, Re: `(?ms)^FUNCTION_PAYLOAD (?P<` + name + `>\S+) .*?^FUNCTION_PAYLOAD_END\n`}}
}

// ExpectCancel waits for a `FUNCTION_CANCEL <tx>` line.
func ExpectCancel(name string) Step {
	return Step{Expect: &Expect{Name: name, Re: `(?m)^FUNCTION_CANCEL (?P<` + name + `>\S+)\n`}}
}

// ExpectProgress waits for a `FUNCTION_PROGRESS <tx>` line.
func ExpectProgress(name string) Step {
	return Step{Expect: &Expect{Name: name, Re: `(?m)^FUNCTION_PROGRESS (?P<` + name + `>\S+)\n`}}
}

// Result is a plugin's answer to a call: `FUNCTION_RESULT_BEGIN <tx> <code> <format> <expires>`, the body as given
// (its lines with their newlines; empty for none), then `FUNCTION_RESULT_END` (src/plugins.d/pluginsd_functions.c:669-713).
// The words are written as given, so a test can send a garbage code, a quoted empty format or an alias.
func Result(tx, code, format, expires, body string) string {
	return "FUNCTION_RESULT_BEGIN " + tx + " " + code + " " + format + " " + expires + "\n" + body + "FUNCTION_RESULT_END\n"
}

// Progress is a plugin's progress report for a call (src/plugins.d/pluginsd_functions.c:715-736).
func Progress(tx string, done, all int) string {
	return fmt.Sprintf("FUNCTION_PROGRESS %s %d %d\n", tx, done, all)
}
