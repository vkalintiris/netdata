// SPDX-License-Identifier: GPL-3.0-or-later

// Package notify is the harness's recording notifier: a static Go program (cmd/notifystub) an agent runs in place of
// alarm-notify.sh (`[health] script to execute on alarm`). Each call writes one record file beside the program: the
// arguments, the environment, the directory, the parent, and how the call ended. What a call does (its exit code, a
// sleep, ignoring SIGTERM) comes from rules in a control file, matched by the alert's name and new status, so a check
// can drive a failing notification and the agent's timeout. The program writes nothing to stdout or stderr.
package notify

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/netdata/netdata/tests/query-corpus/gobuild"
)

// Positions of the arguments a rule matches, in the call's argv (argv[0] is the program): the alert's name and its
// new status (health/health_notifications.c:107-150).
const (
	ArgName   = 7
	ArgStatus = 9
)

// Control is the control file: the first rule matching a call decides what it does; a call no rule matches exits 0.
type Control struct {
	Rules []Rule `json:"rules"`
}

// Rule is what the calls for an alert's transition to a status do. An empty Alert or Status matches any.
type Rule struct {
	Alert  string `json:"alert,omitempty"`
	Status string `json:"status,omitempty"`
	// Exit is the exit code
	Exit int `json:"exit,omitempty"`
	// SleepMs pauses before the exit
	SleepMs int `json:"sleepMs,omitempty"`
	// IgnoreTerm ignores SIGTERM (the agent's kill must escalate to SIGKILL)
	IgnoreTerm bool `json:"ignoreTerm,omitempty"`
}

// Matches reports whether the rule applies to a call's arguments.
func (r Rule) Matches(argv []string) bool {
	arg := func(i int) string {
		if i < len(argv) {
			return argv[i]
		}
		return ""
	}
	return (r.Alert == "" || r.Alert == arg(ArgName)) && (r.Status == "" || r.Status == arg(ArgStatus))
}

// Call is one call's record. The program writes it in two parts: what it was given, when it starts; End, when it
// ends by itself or by SIGTERM. A call without End was killed (SIGKILL), or still runs.
type Call struct {
	// Seq is the call's number on its side, from 1, in the order the calls started
	Seq  int      `json:"seq"`
	Argv []string `json:"argv"`
	Env  []string `json:"env"` // sorted
	Cwd  string   `json:"cwd"`
	Pid  int      `json:"pid"`
	Ppid int      `json:"ppid"`
	// ParentComm is the parent's comm (the agent's spawn server)
	ParentComm string `json:"parentComm"`
	StartUt    int64  `json:"start_ut"`
	// Rule is the index of the rule that matched, -1 for none
	Rule int `json:"rule"`
	// End is `exit N` or `signal SIGTERM`; EndUt when
	End   string `json:"end,omitempty"`
	EndUt int64  `json:"end_ut,omitempty"`
}

// Dir is where a side's notifier lives: the program, the control file and the records.
func Dir(runDir string) string { return filepath.Join(runDir, "notify") }

// Path is the program's path under a run directory: the value of `script to execute on alarm`.
func Path(runDir string) string { return filepath.Join(Dir(runDir), "stub") }

// Stub builds the program once per process (stdlib only, offline) and returns its path.
func Stub() (string, error) {
	return gobuild.Build("./notify/cmd/notifystub")
}

// Install puts the program under runDir (a hard link of `stub`, a copy across devices) with its control file.
func Install(runDir, stub string, ctl Control) error {
	dir := Dir(runDir)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	b, err := json.MarshalIndent(ctl, "", "  ")
	if err != nil {
		return err
	}
	if err := os.WriteFile(filepath.Join(dir, "control.json"), b, 0o644); err != nil {
		return err
	}
	to := Path(runDir)
	if err := os.Link(stub, to); err == nil {
		return nil
	}
	if b, err = os.ReadFile(stub); err != nil {
		return err
	}
	return os.WriteFile(to, b, 0o755)
}

// ReadControl reads the control file of the program's directory (the program's side).
func ReadControl(dir string) (Control, error) {
	var ctl Control
	b, err := os.ReadFile(filepath.Join(dir, "control.json"))
	if err != nil {
		return ctl, err
	}
	return ctl, json.Unmarshal(b, &ctl)
}

// Calls reads a side's records in call order. A record whose first part is still being written is left out.
func Calls(runDir string) ([]Call, error) {
	files, err := filepath.Glob(filepath.Join(Dir(runDir), "call-*.json"))
	if err != nil {
		return nil, err
	}
	slices.Sort(files)
	var out []Call
	for _, f := range files {
		fh, err := os.Open(f)
		if err != nil {
			return nil, err
		}
		var c Call
		dec := json.NewDecoder(fh)
		parts := 0
		for {
			// the second part sets only End and EndUt
			if err := dec.Decode(&c); err != nil {
				if !errors.Is(err, io.EOF) && !errors.Is(err, io.ErrUnexpectedEOF) {
					fh.Close()
					return nil, fmt.Errorf("notify: %s: %w", f, err)
				}
				break
			}
			parts++
		}
		fh.Close()
		if parts > 0 {
			out = append(out, c)
		}
	}
	return out, nil
}

// Summary is a call in one line for a failure text: its number, the alert, the transition and how it ended.
func (c Call) Summary() string {
	arg := func(i int) string {
		if i < len(c.Argv) {
			return c.Argv[i]
		}
		return "?"
	}
	end := c.End
	if end == "" {
		end = "no end"
	}
	return fmt.Sprintf("#%d %s %s->%s (%s)", c.Seq, arg(ArgName), arg(ArgStatus+1), arg(ArgStatus), strings.TrimSpace(end))
}
