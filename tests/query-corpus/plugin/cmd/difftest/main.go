// SPDX-License-Identifier: GPL-3.0-or-later

// Command difftest is the fake plugin's engine: the wrapper `difftest.plugin` execs it under its own pid. It plays
// the scenario in its directory and records what it was given and what it saw (package plugin).
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"sync"
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
	rec.write(plugin.Record{Kind: "start", Args: os.Args[1:], Env: os.Environ(), Cwd: cwd, Pid: os.Getpid(),
		Ppid: os.Getppid(), Pgid: pgid, Sid: sid, Fds: fds, Pre: string(pre)})
	end := func(how string, code int) {
		rec.write(plugin.Record{Kind: "end", How: how})
		_ = f.Sync()
		os.Exit(code)
	}

	// SIGTERM is recorded, then kills as it would have (the agent reads death by SIGTERM as a clean exit)
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM)
	go func() {
		s := <-signals
		rec.write(plugin.Record{Kind: "signal", Signal: s.String()})
		rec.write(plugin.Record{Kind: "end", How: "signal SIGTERM"})
		_ = f.Sync()
		signal.Reset(syscall.SIGTERM)
		_ = syscall.Kill(os.Getpid(), syscall.SIGTERM)
	}()
	time.AfterFunc(maxLife, func() { end("cap", 93) })

	eof := make(chan struct{})
	go func() {
		buf := make([]byte, 64*1024)
		for {
			k, err := os.Stdin.Read(buf)
			if k > 0 {
				rec.write(plugin.Record{Kind: "stdin", Data: string(buf[:k])})
			}
			if err != nil {
				rec.write(plugin.Record{Kind: "eof"})
				close(eof)
				return
			}
		}
	}()
	// stdin's end (the agent stopping) ends any step
	go func() {
		<-eof
		end("eof-exit 0", 0)
	}()

	start := sc.Starts[min(n-1, len(sc.Starts)-1)]
	for _, step := range start.Steps {
		switch {
		case step.Emit != "":
			rec.write(plugin.Record{Kind: "step", Step: "emit"})
			_, _ = os.Stdout.WriteString(step.Emit)
		case step.Stderr != "":
			rec.write(plugin.Record{Kind: "step", Step: "stderr"})
			_, _ = os.Stderr.WriteString(step.Stderr + "\n")
		case step.SleepMs > 0:
			rec.write(plugin.Record{Kind: "step", Step: "sleep"})
			time.Sleep(time.Duration(step.SleepMs) * time.Millisecond)
		case step.Collect != nil:
			rec.write(plugin.Record{Kind: "step", Step: "collect"})
			collect(rec, step.Collect)
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
		}
	}
	// the steps ran out: wait for the agent to stop us
	select {}
}

// collect writes the chart's definition, then one block per whole wall-clock second.
func collect(rec *recorder, c *plugin.Collect) {
	var def strings.Builder
	fmt.Fprintf(&def, "CHART %s '' 'difftest' 'units' 'family' '' line 1000 1 '' '' ''\n", c.Chart)
	for _, d := range c.Dims {
		fmt.Fprintf(&def, "DIMENSION %s '' absolute 1 1\n", d)
	}
	_, _ = os.Stdout.WriteString(def.String())
	for i := 1; i <= c.N; i++ {
		next := time.Now().Truncate(time.Second).Add(time.Second)
		time.Sleep(time.Until(next))
		var b strings.Builder
		fmt.Fprintf(&b, "BEGIN %s\n", c.Chart)
		for _, d := range c.Dims {
			fmt.Fprintf(&b, "SET %s = %d\n", d, i)
		}
		fmt.Fprintf(&b, "END %d 0\n", next.Unix())
		_, _ = os.Stdout.WriteString(b.String())
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

func getsid() (int, error) {
	sid, _, errno := syscall.RawSyscall(syscall.SYS_GETSID, 0, 0, 0)
	if errno != 0 {
		return 0, errno
	}
	return int(sid), nil
}
