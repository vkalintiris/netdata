// SPDX-License-Identifier: GPL-3.0-or-later

// Command notifystub is the harness's recording notifier (package notify): an agent runs it in place of
// alarm-notify.sh. It records the call in its own directory, does what the control file's first matching rule says,
// and writes nothing to stdout or stderr unless a rule asks for a line on stdout (the agent closes the call's stdout
// at its first wait).
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/notify"
)

// a stray call (a harness that died while a rule slept) gives up after this
const maxLife = 5 * time.Minute

func main() {
	// before anything is opened: what the agent handed over
	fds, flags := openFds(), stdioFlags()

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
		ParentComm: strings.TrimSpace(string(comm)), StartUt: time.Now().UnixMicro(), Rule: rule, Fds: fds, StdioFlags: flags})
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

	if action.StdoutAfterMs > 0 {
		time.Sleep(min(time.Duration(action.StdoutAfterMs)*time.Millisecond, maxLife))
		stdout := func(state string) {
			_ = enc.Encode(struct {
				Stdout string `json:"stdout"`
			}{state})
			_ = f.Sync()
		}
		// the record says `before` first: a write to a pipe nobody reads any more does not return (os.Stdout's
		// write dies of SIGPIPE, as a script's would)
		stdout("before")
		if _, err := os.Stdout.WriteString("a line nobody reads\n"); err != nil {
			stdout("error: " + err.Error())
		} else {
			stdout("written")
		}
	}
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

// openFds are the descriptors the process started with, in number order, each with its target. They are read with
// system calls of their own: the os package's first open starts the runtime's poller, whose descriptors would be
// taken for the agent's.
func openFds() []string {
	dir, err := syscall.Open("/proc/self/fd", syscall.O_RDONLY|syscall.O_DIRECTORY|syscall.O_CLOEXEC, 0)
	if err != nil {
		return nil
	}
	defer syscall.Close(dir)
	var names []string
	buf := make([]byte, 8192)
	for {
		n, err := syscall.ReadDirent(dir, buf)
		if err != nil || n <= 0 {
			break
		}
		_, _, names = syscall.ParseDirent(buf[:n], -1, names)
	}
	var numbers []int
	for _, name := range names {
		// the directory's own descriptor is not one the process started with
		if fd, err := strconv.Atoi(name); err == nil && fd != dir {
			numbers = append(numbers, fd)
		}
	}
	slices.Sort(numbers)
	out := []string{}
	target := make([]byte, 4096)
	for _, fd := range numbers {
		n, err := syscall.Readlink(fmt.Sprintf("/proc/self/fd/%d", fd), target)
		if err != nil {
			continue
		}
		out = append(out, fmt.Sprintf("%d %s", fd, target[:n]))
	}
	return out
}

// stdioFlags are the open flags of descriptors 0, 1 and 2 as fdinfo prints them (octal); a closed one has none.
func stdioFlags() []string {
	out := []string{}
	buf := make([]byte, 4096)
	for fd := range 3 {
		flags := ""
		if info, err := syscall.Open(fmt.Sprintf("/proc/self/fdinfo/%d", fd), syscall.O_RDONLY|syscall.O_CLOEXEC, 0); err == nil {
			n, _ := syscall.Read(info, buf)
			syscall.Close(info)
			for _, line := range strings.Split(string(buf[:max(n, 0)]), "\n") {
				if v, ok := strings.CutPrefix(line, "flags:"); ok {
					flags = strings.TrimSpace(v)
				}
			}
		}
		out = append(out, fmt.Sprintf("%d %s", fd, flags))
	}
	return out
}
