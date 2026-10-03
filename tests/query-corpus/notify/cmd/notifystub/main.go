// SPDX-License-Identifier: GPL-3.0-or-later

// Command notifystub is the harness's recording notifier (package notify): an agent runs it in place of
// alarm-notify.sh. It records the call in its own directory, does what the control file's first matching rule says,
// and never writes to stdout or stderr (the agent closes the call's stdout at its first wait).
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/notify"
)

// a stray call (a harness that died while a rule slept) gives up after this
const maxLife = 5 * time.Minute

func main() {
	exe, err := os.Executable()
	if err != nil {
		os.Exit(90)
	}
	dir := filepath.Dir(exe)
	f, seq := nextCall(dir)
	if f == nil {
		os.Exit(92)
	}
	enc := json.NewEncoder(f)

	// a control file that cannot be read is no rule at all: the record's rule -2 tells
	rule, action := -1, notify.Rule{}
	if ctl, err := notify.ReadControl(dir); err != nil {
		rule = -2
	} else {
		for i, r := range ctl.Rules {
			if r.Matches(os.Args) {
				rule, action = i, r
				break
			}
		}
	}

	// the signal's disposition is set before the record is written: a reader that saw the record may send SIGTERM
	signals := make(chan os.Signal, 1)
	if action.IgnoreTerm {
		signal.Ignore(syscall.SIGTERM)
	} else {
		signal.Notify(signals, syscall.SIGTERM)
	}

	env := os.Environ()
	slices.Sort(env)
	cwd, _ := os.Getwd()
	comm, _ := os.ReadFile(fmt.Sprintf("/proc/%d/comm", os.Getppid()))
	_ = enc.Encode(notify.Call{Seq: seq, Argv: os.Args, Env: env, Cwd: cwd, Pid: os.Getpid(), Ppid: os.Getppid(),
		ParentComm: strings.TrimSpace(string(comm)), StartUt: time.Now().UnixMicro(), Rule: rule})
	_ = f.Sync()

	// one end: the exit and a signal may race
	var once sync.Once
	end := func(how string, then func()) {
		once.Do(func() {
			_ = enc.Encode(struct {
				End   string `json:"end"`
				EndUt int64  `json:"end_ut"`
			}{how, time.Now().UnixMicro()})
			_ = f.Sync()
			then()
		})
	}
	// SIGTERM is recorded, then kills as it would have
	go func() {
		<-signals
		end("signal SIGTERM", func() {
			signal.Reset(syscall.SIGTERM)
			_ = syscall.Kill(os.Getpid(), syscall.SIGTERM)
		})
	}()

	if action.SleepMs > 0 {
		time.Sleep(min(time.Duration(action.SleepMs)*time.Millisecond, maxLife))
	}
	end(fmt.Sprintf("exit %d", action.Exit), func() { os.Exit(action.Exit) })
	// a signal's end is on its way
	select {}
}

// nextCall creates the next call-NNN.json (O_EXCL: two calls at once never share one).
func nextCall(dir string) (*os.File, int) {
	for n := 1; n < 100000; n++ {
		f, err := os.OpenFile(filepath.Join(dir, fmt.Sprintf("call-%05d.json", n)), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
		if err == nil {
			return f, n
		}
		if !os.IsExist(err) {
			return nil, 0
		}
	}
	return nil, 0
}
