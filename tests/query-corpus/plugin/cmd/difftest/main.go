// SPDX-License-Identifier: GPL-3.0-or-later

// Command difftest is the fake plugin's engine: the wrapper `difftest.plugin` execs it under its own pid. It plays
// the scenario in its directory and records what it was given and what it saw (package plugin).
package main

import (
	"bytes"
	"cmp"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// a stray engine (a harness that died) gives up after this
const maxLife = 10 * time.Minute

type recorder struct {
	mu  sync.Mutex
	enc *json.Encoder
}

// stdout is held for each write to stdout: a background collection's blocks and the steps' emits never interleave.
var stdout sync.Mutex

func emit(s string) {
	stdout.Lock()
	defer stdout.Unlock()
	_, _ = os.Stdout.WriteString(s)
}

// input is what stdin carried so far, which Expect steps match in order: each from the end of the previous match.
type input struct {
	mu      sync.Mutex
	data    []byte
	pos     int
	eof     bool
	changed chan struct{} // closed and replaced at each read and at stdin's end
}

func newInput() *input { return &input{changed: make(chan struct{})} }

func (in *input) add(b []byte, eof bool) {
	in.mu.Lock()
	defer in.mu.Unlock()
	in.data = append(in.data, b...)
	in.eof = in.eof || eof
	close(in.changed)
	in.changed = make(chan struct{})
}

// expect waits until re matches after the previous match: the matched text and named groups, or how it failed
// ("expect-timeout" at the deadline, "expect-eof" at stdin's end without a match).
func (in *input) expect(re *regexp.Regexp, deadline <-chan time.Time) (string, map[string]string, string) {
	for {
		in.mu.Lock()
		data, pos, eof, changed := in.data, in.pos, in.eof, in.changed
		loc := re.FindSubmatchIndex(data[pos:])
		if loc != nil {
			in.pos = pos + loc[1]
		}
		in.mu.Unlock()
		if loc != nil {
			groups := map[string]string{}
			for i, name := range re.SubexpNames() {
				if name != "" && loc[2*i] >= 0 {
					groups[name] = string(data[pos+loc[2*i] : pos+loc[2*i+1]])
				}
			}
			return string(data[pos+loc[0] : pos+loc[1]]), groups, "matched"
		}
		if eof {
			return "", nil, "expect-eof"
		}
		select {
		case <-changed:
		case <-deadline:
			return "", nil, "expect-timeout"
		}
	}
}

// expand replaces each `{{name}}` of a captured group with its value; anything else stays as written.
func expand(s string, vars map[string]string) string {
	if len(vars) == 0 || !strings.Contains(s, "{{") {
		return s
	}
	pairs := make([]string, 0, 2*len(vars))
	for k, v := range vars {
		pairs = append(pairs, "{{"+k+"}}", v)
	}
	return strings.NewReplacer(pairs...).Replace(s)
}

func (r *recorder) write(rec plugin.Record) {
	r.mu.Lock()
	defer r.mu.Unlock()
	rec.T = time.Now()
	_ = r.enc.Encode(rec)
}

func main() {
	fds := openFds()
	exe, err := os.Executable()
	if err != nil {
		os.Exit(90)
	}
	dir := filepath.Dir(exe)
	var sc plugin.Scenario
	if b, err := os.ReadFile(filepath.Join(dir, "scenario.json")); err != nil || json.Unmarshal(b, &sc) != nil ||
		len(sc.Starts) == 0 {
		os.Exit(91)
	}
	f, n := nextStart(dir)
	if f == nil {
		os.Exit(92)
	}
	rec := &recorder{enc: json.NewEncoder(f)}
	pre, _ := os.ReadFile(filepath.Join(dir, fmt.Sprintf("pre-%d", os.Getpid())))
	cwd, _ := os.Getwd()
	pgid, _ := syscall.Getpgid(0)
	sid, _ := getsid()
	nice, policy := statFields()
	rec.write(plugin.Record{Kind: "start", Args: os.Args[1:], Env: os.Environ(), Cwd: cwd, Pid: os.Getpid(),
		Ppid: os.Getppid(), Pgid: pgid, Sid: sid, Fds: fds, Pre: string(pre), ParentComm: readTrim(fmt.Sprintf("/proc/%d/comm", os.Getppid())),
		StdioFlags: stdioFlags(), OomScoreAdj: readTrim("/proc/self/oom_score_adj"), Nice: nice, SchedPolicy: policy})
	// one end: stdin's end, an exit step and a signal may race
	var once sync.Once
	end := func(how string, code int) {
		once.Do(func() {
			rec.write(plugin.Record{Kind: "end", How: how})
			_ = f.Sync()
			os.Exit(code)
		})
	}

	// SIGTERM is recorded, then kills as it would have (the agent reads death by SIGTERM as a clean exit)
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM)
	go func() {
		s := <-signals
		once.Do(func() {
			rec.write(plugin.Record{Kind: "signal", Signal: s.String()})
			rec.write(plugin.Record{Kind: "end", How: "signal SIGTERM"})
			_ = f.Sync()
			signal.Reset(syscall.SIGTERM)
			_ = syscall.Kill(os.Getpid(), syscall.SIGTERM)
		})
	}()
	time.AfterFunc(maxLife, func() { end("cap", 93) })

	var stayOnEOF atomic.Bool
	eof := make(chan struct{})
	in := newInput()
	go func() {
		buf := make([]byte, 64*1024)
		for {
			k, err := os.Stdin.Read(buf)
			if k > 0 {
				rec.write(plugin.Record{Kind: "stdin", Data: string(buf[:k])})
				in.add(buf[:k], false)
			}
			if err != nil {
				rec.write(plugin.Record{Kind: "eof"})
				in.add(nil, true)
				close(eof)
				return
			}
		}
	}()
	// stdin's end (the agent stopping) ends any step, unless a step said to stay
	go func() {
		<-eof
		if !stayOnEOF.Load() {
			end("eof-exit 0", 0)
		}
	}()

	// the groups the Expect steps captured, for `{{name}}` in later emits
	vars := map[string]string{}
	start := sc.Starts[min(n-1, len(sc.Starts)-1)]
	for _, step := range start.Steps {
		switch {
		case step.Emit != "":
			rec.write(plugin.Record{Kind: "step", Step: "emit"})
			emit(expand(step.Emit, vars))
		case step.Stderr != "":
			rec.write(plugin.Record{Kind: "step", Step: "stderr"})
			_, _ = os.Stderr.WriteString(expand(step.Stderr, vars) + "\n")
		case step.SleepMs > 0:
			rec.write(plugin.Record{Kind: "step", Step: "sleep"})
			time.Sleep(time.Duration(step.SleepMs) * time.Millisecond)
		case step.Collect != nil:
			rec.write(plugin.Record{Kind: "step", Step: "collect"})
			if step.Collect.Background {
				go collect(rec, step.Collect)
			} else {
				collect(rec, step.Collect)
			}
		case step.Values != nil:
			rec.write(plugin.Record{Kind: "step", Step: "values"})
			values(rec, dir, step.Values)
		case step.Expect != nil:
			e := step.Expect
			re, err := regexp.Compile(e.Re)
			if err != nil {
				end("bad expect "+e.Name, 95)
			}
			rec.write(plugin.Record{Kind: "expect", Step: e.Name})
			var deadline <-chan time.Time
			if e.TimeoutMs > 0 {
				deadline = time.After(time.Duration(e.TimeoutMs) * time.Millisecond)
			}
			text, groups, how := in.expect(re, deadline)
			rec.write(plugin.Record{Kind: how, Step: e.Name, Data: text, Groups: groups})
			for k, v := range groups {
				vars[k] = v
			}
		case step.WaitFile != "":
			rec.write(plugin.Record{Kind: "waiting", Step: "wait", File: step.WaitFile})
			for {
				if _, err := os.Stat(filepath.Join(dir, step.WaitFile)); err == nil {
					break
				}
				time.Sleep(50 * time.Millisecond)
			}
		case step.Exit != nil:
			end(fmt.Sprintf("exit %d", *step.Exit), *step.Exit)
		case step.Hang:
			rec.write(plugin.Record{Kind: "step", Step: "hang"})
			select {}
		case step.Raise != "":
			rec.write(plugin.Record{Kind: "step", Step: "raise"})
			sig, ok := raisable[step.Raise]
			if !ok {
				end("unknown signal "+step.Raise, 94)
			}
			once.Do(func() {
				rec.write(plugin.Record{Kind: "end", How: "raise " + step.Raise})
				_ = f.Sync()
				// Go catches SIGUSR1 and its like and ignores them, signal.Reset included: a shell exec'd in this
				// process's place takes the signal with its default action, so the plugin's pid dies by it
				_ = syscall.Exec("/bin/sh", []string{"sh", "-c", "kill -s " + strings.TrimPrefix(step.Raise, "SIG") + " $$"}, os.Environ())
				signal.Reset(sig)
				_ = syscall.Kill(os.Getpid(), sig)
			})
			select {}
		case step.IgnoreTerm:
			rec.write(plugin.Record{Kind: "step", Step: "ignore-term"})
			signal.Ignore(syscall.SIGTERM)
		case step.StayOnEOF:
			rec.write(plugin.Record{Kind: "step", Step: "stay-on-eof"})
			stayOnEOF.Store(true)
		case step.Serve != nil:
			rec.write(plugin.Record{Kind: "step", Step: "serve"})
			var rules []rule
			for _, r := range step.Serve.Rules {
				re, err := regexp.Compile(r.Re)
				if err != nil {
					end("bad serve rule "+r.Name, 95)
				}
				rules = append(rules, rule{ServeRule: r, re: re})
			}
			go serve(rec, in, rules)
		}
	}
	// the steps ran out: wait for the agent to stop us
	select {}
}

// rule is a Serve step's rule with its pattern compiled.
type rule struct {
	plugin.ServeRule
	re   *regexp.Regexp
	used int
}

// call is one call the agent wrote: its header's words and, for a FUNCTION_PAYLOAD block, the payload and its type.
type call struct {
	tx, cmd, access, source, ctype, payload string
}

// serve answers the calls on stdin from its first byte, in order, with a cursor of its own (plugin.Serve).
func serve(rec *recorder, in *input, rules []rule) {
	pos := 0
	for {
		in.mu.Lock()
		// in.add only appends: the bytes up to this length stay as they are
		data, eof, changed := in.data, in.eof, in.changed
		in.mu.Unlock()
		for {
			c, next, ok := nextCall(data, pos)
			if !ok {
				break
			}
			pos = next
			if c != nil {
				answer(rec, rules, *c)
			}
		}
		if eof {
			return
		}
		<-changed
	}
}

// nextCall reads the line (or a FUNCTION_PAYLOAD block) at pos: the call it holds (nil for any other line) and where
// the next one starts; ok is false while it is not whole yet.
func nextCall(data []byte, pos int) (*call, int, bool) {
	nl := bytes.IndexByte(data[pos:], '\n')
	if nl < 0 {
		return nil, pos, false
	}
	line, next := string(data[pos:pos+nl]), pos+nl+1
	w := quotedWords(line)
	switch {
	case strings.HasPrefix(line, "FUNCTION_PAYLOAD ") && len(w) >= 7:
		// the payload, then the agent's newline and the end line (pluginsd_functions.c:24-38)
		const term = "\nFUNCTION_PAYLOAD_END\n"
		k := bytes.Index(data[next:], []byte(term))
		if k < 0 {
			return nil, pos, false
		}
		return &call{tx: w[1], cmd: w[3], access: w[4], source: w[5], ctype: w[6], payload: string(data[next : next+k])},
			next + k + len(term), true
	case strings.HasPrefix(line, "FUNCTION ") && len(w) >= 6:
		return &call{tx: w[1], cmd: w[3], access: w[4], source: w[5]}, next, true
	}
	return nil, next, true
}

// quotedWords splits a line the agent wrote on spaces, a double-quoted word whole (the agent never writes a `"`
// inside one).
func quotedWords(line string) []string {
	var out []string
	for i := 0; i < len(line); {
		if line[i] == ' ' {
			i++
			continue
		}
		if line[i] == '"' {
			j := strings.IndexByte(line[i+1:], '"')
			if j < 0 {
				return append(out, line[i+1:])
			}
			out = append(out, line[i+1:i+1+j])
			i += j + 2
			continue
		}
		j := strings.IndexByte(line[i:], ' ')
		if j < 0 {
			return append(out, line[i:])
		}
		out = append(out, line[i:i+j])
		i += j
	}
	return out
}

// answer records a call with the first rule matching its command and writes that rule's answer, or records "-" and
// leaves it unanswered.
func answer(rec *recorder, rules []rule, c call) {
	vars := map[string]string{"tx": c.tx, "cmd": c.cmd, "access": c.access, "source": c.source, "payload": c.payload,
		"type": c.ctype}
	f := strings.Fields(c.cmd)
	for i, k := range []string{"id", "action", "name"} {
		vars[k] = ""
		if len(f) > i+1 {
			vars[k] = f[i+1]
		}
	}
	for i := range rules {
		r := &rules[i]
		if !r.re.MatchString(c.cmd) || r.Times > 0 && r.used >= r.Times {
			continue
		}
		r.used++
		rec.write(plugin.Record{Kind: "served", Step: r.Name, Data: c.cmd})
		if !r.Silent {
			emit(plugin.Result(c.tx, r.Code, cmp.Or(r.Type, "application/json"), cmp.Or(r.Expires, "0"),
				expand(r.Body, vars)) + expand(r.Then, vars))
		}
		return
	}
	rec.write(plugin.Record{Kind: "served", Step: "-", Data: c.cmd})
}

// raisable are the signals a Raise step may name.
var raisable = map[string]syscall.Signal{"SIGUSR1": syscall.SIGUSR1, "SIGUSR2": syscall.SIGUSR2, "SIGSEGV": syscall.SIGSEGV}

// collect writes the chart's definition, then one block per whole wall-clock second.
func collect(rec *recorder, c *plugin.Collect) {
	var def strings.Builder
	fmt.Fprintf(&def, "CHART %s '' 'difftest' 'units' 'family' '' line 1000 1 '' '' ''\n", c.Chart)
	for _, d := range c.Dims {
		fmt.Fprintf(&def, "DIMENSION %s '' absolute 1 1\n", d)
	}
	emit(def.String())
	for i := 1; i <= c.N; i++ {
		next := time.Now().Truncate(time.Second).Add(time.Second)
		time.Sleep(time.Until(next))
		var b strings.Builder
		fmt.Fprintf(&b, "BEGIN %s\n", c.Chart)
		if c.InBlock != "" {
			b.WriteString(c.InBlock + "\n")
		}
		for _, d := range c.Dims {
			fmt.Fprintf(&b, "SET %s = %d\n", d, i)
		}
		fmt.Fprintf(&b, "END %d 0\n", next.Unix())
		if c.AfterBlock != "" {
			b.WriteString(c.AfterBlock + "\n")
		}
		emit(b.String())
		rec.write(plugin.Record{Kind: "collected", Sec: next.Unix()})
	}
}

// values writes the chart's definition (with its labels), then one block per whole wall-clock second with the current
// phase's values, a phase's own lines before its first block, until the process ends (plugin.Values).
func values(rec *recorder, dir string, v *plugin.Values) {
	var def strings.Builder
	fmt.Fprintf(&def, "CHART %s '' 'title' 'units' '%s' '%s' line 1000 1 '' '' '%s'\n", v.Chart,
		cmp.Or(v.Family, "family"), v.Context, v.Module)
	for _, d := range v.Dims {
		fmt.Fprintf(&def, "DIMENSION %s '' absolute 1 1\n", d)
	}
	for _, l := range v.Labels {
		name, value, _ := strings.Cut(l, " ")
		fmt.Fprintf(&def, "CLABEL '%s' '%s' 1\n", name, value)
	}
	if len(v.Labels) > 0 {
		def.WriteString("CLABEL_COMMIT\n")
	}
	emit(def.String())
	if len(v.Phases) == 0 {
		return
	}
	phase, first := 0, true
	for {
		next := time.Now().Truncate(time.Second).Add(time.Second)
		time.Sleep(time.Until(next))
		for phase < len(v.Phases)-1 && v.Phases[phase].Until != "" {
			if _, err := os.Stat(filepath.Join(dir, v.Phases[phase].Until)); err != nil {
				break
			}
			phase, first = phase+1, true
		}
		var b strings.Builder
		if first {
			// the phase's own lines, whole, before its first block
			b.WriteString(strings.ReplaceAll(v.Phases[phase].Emit, "{{sec}}", strconv.FormatInt(next.Unix(), 10)))
		}
		fmt.Fprintf(&b, "BEGIN %s\n", v.Chart)
		for _, d := range v.Dims {
			if value, ok := v.Phases[phase].Set[d]; ok {
				fmt.Fprintf(&b, "SET %s = %d\n", d, value)
			}
		}
		fmt.Fprintf(&b, "END %d 0\n", next.Unix())
		emit(b.String())
		if first {
			rec.write(plugin.Record{Kind: "phase", Step: strconv.Itoa(phase), Sec: next.Unix()})
			first = false
		}
		rec.write(plugin.Record{Kind: "collected", Sec: next.Unix()})
	}
}

// nextStart creates the next start-NNN.jsonl (O_EXCL: a start never reuses one).
func nextStart(dir string) (*os.File, int) {
	for n := 1; n < 1000; n++ {
		f, err := os.OpenFile(filepath.Join(dir, fmt.Sprintf("start-%03d.jsonl", n)), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
		if err == nil {
			return f, n
		}
	}
	return nil, 0
}

// openFds are the descriptors the process started with, each with its target, before the engine opens any.
func openFds() []string {
	entries, err := os.ReadDir("/proc/self/fd")
	if err != nil {
		return nil
	}
	var out []string
	for _, e := range entries {
		target, err := os.Readlink(filepath.Join("/proc/self/fd", e.Name()))
		if err != nil {
			// the directory's own descriptor, closed already
			continue
		}
		out = append(out, e.Name()+" "+target)
	}
	return out
}

// readTrim is a small /proc file's content, trimmed.
func readTrim(path string) string {
	b, _ := os.ReadFile(path)
	return strings.TrimSpace(string(b))
}

// stdioFlags are the open flags of fds 0-2 as fdinfo prints them (octal).
func stdioFlags() []string {
	var out []string
	for fd := range 3 {
		flags := ""
		for _, line := range strings.Split(readTrim(fmt.Sprintf("/proc/self/fdinfo/%d", fd)), "\n") {
			if v, ok := strings.CutPrefix(line, "flags:"); ok {
				flags = strings.TrimSpace(v)
			}
		}
		out = append(out, fmt.Sprintf("%d %s", fd, flags))
	}
	return out
}

// statFields are nice and the scheduling policy from /proc/self/stat (fields 19 and 41, counted after the comm).
func statFields() (string, string) {
	stat := readTrim("/proc/self/stat")
	if i := strings.LastIndexByte(stat, ')'); i >= 0 {
		f := strings.Fields(stat[i+1:])
		// f[0] is field 3 (state)
		if len(f) > 38 {
			return f[16], f[38]
		}
	}
	return "", ""
}

func getsid() (int, error) {
	sid, _, errno := syscall.RawSyscall(syscall.SYS_GETSID, 0, 0, 0)
	if errno != 0 {
		return 0, errno
	}
	return int(sid), nil
}
