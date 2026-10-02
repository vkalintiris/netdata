// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// TestDynCfgHandback (check `dyncfg.handback`, M8 commit 8, D169; plan §5.3): DynCfg's saved state across plugin and
// agent restarts and between the two agents. Compared per case as dyncfg.api's (runDynCfgCases): the exchanges and
// the files at each phase (every line, `created=`/`modified=` through dcClock), every plugin start's stdin (the echoes
// a registration draws from saved state, plan §3.5), the plugin threads', DynCfg, access and call records.
func TestDynCfgHandback(t *testing.T) {
	runDynCfgCases(t, dcHandbackCases())
}

// dcChanges are the user's changes the restart cases start from: the single updated and disabled, j2 added and
// updated, the template disabled (its file saved, the jobs' echoes), then enabled again (j2's user-disabled
// state stays false).
func dcChanges(t *testing.T, x *dcSide, base int) []string {
	t.Helper()
	out := []string{
		x.send(t, "update s", "/api/v1/config?action=update&id=difftest:s", base+1, `{"s":1}`),
		x.get(t, "disable s", "action=disable&id=difftest:s", base+2),
		x.send(t, "add j2", "/api/v1/config?action=add&id=difftest:t&name=j2", base+3, `{"j":2}`),
	}
	x.waitTree(t, map[string]string{"difftest:t:j2": "running"})
	out = append(out,
		x.send(t, "update j2", "/api/v1/config?action=update&id=difftest:t:j2", base+4, "{\"j\":22}\n"),
		x.get(t, "disable the template", "action=disable&id=difftest:t", base+5))
	// the fan-out's echoes (j1, j2) and the single's disable answered
	x.served(t, 1, 3, "disable")
	x.waitTree(t, map[string]string{dcJ1: "disabled", "difftest:t:j2": "disabled"})
	return out
}

// dcHeld is a start that serves, waits for the check's release `go<n>`, registers the usual nodes, then exits on the
// release `exit<n>` (release files stay in the engine's directory across starts: each start needs names of its own).
func dcHeld(n string) plugin.Start {
	return plugin.Start{Steps: []plugin.Step{dcServe(), {WaitFile: "go" + n}, {Emit: dcCreates}, {WaitFile: "exit" + n},
		{Exit: plugin.ExitCode(0)}}}
}

// dcExits is a start that serves, registers the usual nodes, then exits on the release `exit`.
var dcExits = plugin.Start{Steps: []plugin.Step{dcServe(), {Emit: dcCreates}, {WaitFile: "exit"}, {Exit: plugin.ExitCode(0)}}}

// exitPlugin releases start n's exit and waits until it ended; with `update every = 10` C starts it again only
// 10 s later (plugins_d.c), so an agent restart right after never races a new start.
func (x *dcSide) exitPlugin(t *testing.T, n int, release string) bool {
	t.Helper()
	x.release(t, x.l, release)
	return x.step(t, x.l, "the plugin did not exit", startEnded(n))
}

// held waits until start n waits for its release.
func (x *dcSide) held(t *testing.T, n int, release string) bool {
	t.Helper()
	return x.step(t, x.l, "the plugin did not wait for "+release, func(s [][]plugin.Record) bool {
		return len(s) >= n && plugin.Has(s[n-1], "waiting", release)
	})
}

// restart restarts a side's agent, with binary when set.
func (x *dcSide) restart(t *testing.T, binary string) bool {
	t.Helper()
	if binary != "" {
		x.d.Opts.Binary = binary
	}
	if err := x.d.Restart(); err != nil {
		t.Errorf("%s: restart: %v", x.role, err)
		return false
	}
	return true
}

// dcLoaded are the nodes dcChanges saved, as orphans loaded from their files.
var dcLoaded = map[string]string{dcS: "orphan", dcT: "orphan", "difftest:t:j2": "orphan"}

// dcAfterRegistration waits for start n's echoes after the release: the single's disable and update (the update's
// answer leaves it running), the template's `add j2`, j1's and j2's enable, or disable under a user-disabled template
// (jobs, the statuses they reach only by their echoes' answers).
func dcAfterRegistration(t *testing.T, x *dcSide, n int, jobs string) bool {
	t.Helper()
	return x.served(t, n, 1, "update") && x.served(t, n, 1, "add") &&
		x.waitTree(t, map[string]string{dcS: "running", dcJ1: jobs, "difftest:t:j2": jobs})
}

func dcHandbackCases() map[string]dcCase {
	cases := map[string]dcCase{}

	// the files (dyncfg-files.c:19-68): one per saved node under its escaped id; `saves` counts every save, `created`
	// stays, `modified` moves; a user enable/disable saves only when user_disabled changes; a template's disable
	// saves the template; a removed job's file is deleted; the directory and file modes (C's 0755 and fopen's 0666,
	// under the daemon's umask 0007)
	cases["files"] = dcCase{
		sc:    dcScenario(dcCreates),
		ready: dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			var out []string
			step := func(label, obs string) {
				out = append(out, obs)
				out = append(out, x.files(t, "files after "+label)...)
			}
			step("update s", x.send(t, "update s", "/api/v1/config?action=update&id=difftest:s", 301, `{"s":1}`))
			step("disable s", x.get(t, "disable s", "action=disable&id=difftest:s", 302))
			step("disable s again", x.get(t, "disable s again", "action=disable&id=difftest:s", 303))
			step("add j2", x.send(t, "add j2", "/api/v1/config?action=add&id=difftest:t&name=j2", 304, `{"j":2}`))
			x.waitTree(t, map[string]string{"difftest:t:j2": "running"})
			step("update j2", x.send(t, "update j2", "/api/v1/config?action=update&id=difftest:t:j2", 305, `{"j":22}`))
			step("enable s", x.get(t, "enable s", "action=enable&id=difftest:s", 306))
			step("disable t", x.get(t, "disable t", "action=disable&id=difftest:t", 307))
			// s's two forwarded disables, the fan-out's two
			x.served(t, 1, 4, "disable")
			step("remove j2", x.get(t, "remove j2", "action=remove&id=difftest:t:j2", 308))
			return append(out, x.get(t, "tree", "action=tree", 309))
		},
		want: []string{"saves=3", "difftest%3At%3Aj2.dyncfg", "dir mode 0750"},
	}

	// a plugin restart (dyncfg.c:423-464, :286-321): start 2 registers the same nodes and C echoes the saved state
	// back in its dictionary's order (P1): the single's disable (user-disabled) and its update with the saved payload,
	// the template's `add j2` with j2's payload (the plugin registers j2 after answering), j1's disable (its template
	// was disabled and enabled: a job's own user_disabled decides)
	cases["plugin-restart"] = dcCase{
		ue:     2,
		starts: 2,
		sc:     plugin.Scenario{Starts: []plugin.Start{dcExits, {Steps: []plugin.Step{dcServe(), {Emit: dcCreates}}}}},
		ready:  dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			out := dcChanges(t, x, 320)
			out = append(out, x.get(t, "enable the template", "action=enable&id=difftest:t", 326))
			// the echoes so far: s and j1 registered, j2 registered after its add, then the template's two
			x.served(t, 1, 5, "enable")
			x.waitTree(t, map[string]string{dcJ1: "running", "difftest:t:j2": "running"})
			out = append(out, x.get(t, "tree before the restart", "action=tree", 327))
			if !x.exitPlugin(t, 1, "exit") || !dcAfterRegistration(t, x, 2, "running") {
				return out
			}
			return append(out, x.get(t, "tree after the restart", "action=tree", 328))
		},
		want: []string{`FUNCTION_PAYLOAD RANDOM 10 "config difftest:t add j2" "0x7ff"`,
			`FUNCTION_PAYLOAD RANDOM 10 "config difftest:s update" "0x7ff"`},
	}

	// an agent restart: the saved nodes load as orphans (dyncfg-files.c:81-293: `created_ut` the load's time,
	// `modified_ut` the file's), the plugin's start 2 waits; once it registers, the echoes as in plugin-restart
	cases["agent-restart"] = dcCase{
		ue:     10,
		starts: 2,
		sc:     plugin.Scenario{Starts: []plugin.Start{dcExits, dcHeld("2")}},
		ready:  dcReady,
		pair: func(t *testing.T, p *Pair, xs [2]*dcSide) [2][]string {
			return dcBoth(xs, func(_ int, x *dcSide) []string {
				out := dcChanges(t, x, 340)
				if !x.exitPlugin(t, 1, "exit") || !x.restart(t, "") || !x.held(t, 2, "go2") || !x.waitTree(t, dcLoaded) {
					return out
				}
				out = append(out, x.get(t, "tree after the agent restart", "action=tree", 341))
				out = append(out, x.files(t, "files after the agent restart")...)
				x.release(t, x.l, "go2")
				if dcAfterRegistration(t, x, 2, "disabled") {
					out = append(out, x.get(t, "tree after the registration", "action=tree", 342))
				}
				return append(out, x.files(t, "files at the end")...)
			})
		},
	}

	// hand-made files (dyncfg-files.c:81-293, P2): loaded in readdir order (their records' order), a newer version
	// (NOTICE), no id (ERR, ignored), lines without `=`, empty values, unknown keys and garbage numbers, an empty
	// payload, a length mismatch (WARNING), a raw-named file (renamed), another host (out of the tree; NOTICE at its
	// registration), stale cmds (healed), a symlink (loaded, renamed, written through on save), a non-.dyncfg file and
	// a directory (skipped); constant times far in the past stay raw
	{
		dir := func(runDir string) string { return dcConfigDir(runDir) }
		cases["seeded"] = dcCase{
			ue:     10,
			starts: 1,
			prepare: func(t *testing.T, runDir string) {
				d := dir(runDir)
				w := func(name string, b []byte) { dcWriteFile(t, filepath.Join(d, name), b) }
				const c1, m1 = 1600000000000001, 1600000000000002
				w("difftest%3Aa.dyncfg", dcFile("difftest:a", "", "/difftest", "single", "dyncfg", "seeded", c1, m1, false, 3,
					"get schema update enable disable ", `{"a":1}`))
				w("difftest%3At.dyncfg", dcFile("difftest:t", "", "/difftest/tmpl", "template", "internal", "internal", c1, m1,
					true, 1, "schema add enable disable test userconfig ", ""))
				w("difftest%3At%3Aj7.dyncfg", dcFile("difftest:t:j7", "difftest:t", "/difftest/tmpl", "job", "dyncfg", "seeded",
					c1, m1, false, 2, "get schema update enable disable restart test userconfig remove ", "{\"j\":7}\n"))
				w("v99.dyncfg", []byte("version=99\nid=difftest:v99\nhost="+dcHost()+"\npath=/difftest\ntype=single\ncmds=get \n"))
				w("noid.dyncfg", []byte("version=1\nhost="+dcHost()+"\npath=/difftest\ntype=single\n"))
				w("malformed.dyncfg", []byte("version=1\nid = difftest:m \nno equals here\nhost="+dcHost()+
					"\npath=/difftest\ntype=\nbogus_key=1\nsaves=abc\ncreated=xyz\nuser_disabled=yes\ncmds=get nope update\n"))
				w("empty.dyncfg", []byte("version=1\nid=difftest:e\nhost="+dcHost()+"\npath=/difftest\ntype=single\n"+
					"content_type=application/json\ncontent_length=0\n---\n"))
				w("mismatch.dyncfg", []byte("version=1\nid=difftest:mm\nhost="+dcHost()+"\npath=/difftest\ntype=single\n"+
					"content_type=application/json\ncontent_length=99\n---\n{\"x\":1}"))
				w("difftest:r.dyncfg", dcFile("difftest:r", "", "/difftest", "single", "dyncfg", "seeded", c1, m1, false, 1, "get update ", `{"r":1}`))
				w("otherhost.dyncfg", []byte("version=1\nid=difftest:h\nhost=5a1e00000000400080000000000000ff\npath=/elsewhere\n"+
					"type=single\nsource_type=dyncfg\nsource=seeded\nsaves=1\ncmds=get update \ncontent_type=application/json\n"+
					"content_length=7\n---\n{\"h\":1}"))
				w("difftest%3Ac.dyncfg", dcFile("difftest:c", "", "/difftest", "single", "user", "seeded", c1, m1, false, 1,
					"get add remove ", ""))
				w("target.txt", dcFile("difftest:l", "", "/difftest", "single", "dyncfg", "seeded", c1, m1, false, 1,
					"get update ", `{"l":1}`))
				if err := os.Symlink("target.txt", filepath.Join(d, "link.dyncfg")); err != nil {
					t.Fatal(err)
				}
				w("x.txt", dcFile("difftest:x", "", "/difftest", "single", "dyncfg", "seeded", c1, m1, false, 1, "get ", ""))
				if err := os.MkdirAll(filepath.Join(d, "d.dyncfg"), 0o755); err != nil {
					t.Fatal(err)
				}
			},
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{dcServe(), {WaitFile: "go"}, {Emit: dcCreateT +
				plugin.ConfigCreate("difftest:a", "accepted", "single", "/difftest", "internal", "internal", "get schema update enable disable", 0x8, 0x8) +
				plugin.ConfigCreate("difftest:h", "running", "single", "/difftest/h", "internal", "internal", "get update", 0x8, 0x8) +
				plugin.ConfigCreate("difftest:l", "running", "single", "/difftest", "internal", "internal", "get update", 0x8, 0x8)}}}}},
			ready: map[string]string{"difftest:a": "orphan", "difftest:t:j7": "orphan", "difftest:l": "orphan"},
			play: func(t *testing.T, x *dcSide) []string {
				out := []string{x.get(t, "tree of the loaded files", "action=tree", 361)}
				out = append(out, x.files(t, "files after the load")...)
				x.release(t, x.l, "go")
				// the saved updates of difftest:a, :h and :l, the template's `add j7`; j7, registered after it, is disabled
				// (its template is user-disabled)
				x.served(t, 1, 3, "update")
				x.served(t, 1, 1, "add")
				x.waitTree(t, map[string]string{"difftest:a": "running", "difftest:h": "running", "difftest:l": "running",
					"difftest:t:j7": "disabled"})
				out = append(out,
					x.get(t, "tree after the registrations", "action=tree", 362),
					x.send(t, "update the symlinked node", "/api/v1/config?action=update&id=difftest:l", 363, `{"l":2}`))
				return out
			},
			want: []string{
				"DYNCFG: configuration file '<RUN>/lib/config/v99.dyncfg' has version 99, which is newer than our version 1",
				"DYNCFG: configuration file '<RUN>/lib/config/noid.dyncfg' does not include a unique id. Ignoring it.",
				"DYNCFG: content_length 99 does not match actual payload size 7 for file '<RUN>/lib/config/mismatch.dyncfg'",
				"DYNCFG: configuration 'difftest:h' changed host id from '5a1e0000-0000-4000-8000-0000000000ff'",
			},
		}
	}

	// the two agents' files across binaries (status.file's swap, SF:219-247): after the user's changes each run
	// directory restarts with C (c-after-rust: C reads the candidate's files as its own, then saves once more); then the
	// oracle's directory is copied over the candidate's and each restarts with its own binary (rust-after-c: the
	// candidate reads C's files)
	cases["swap"] = dcCase{
		ue:     10,
		starts: 3,
		sc:     plugin.Scenario{Starts: []plugin.Start{dcExits, dcHeld("2"), dcHeld("3")}},
		ready:  dcReady,
		pair: func(t *testing.T, p *Pair, xs [2]*dcSide) [2][]string {
			bins := binaries(t)
			obs := dcBoth(xs, func(_ int, x *dcSide) []string {
				out := dcChanges(t, x, 380)
				if !x.exitPlugin(t, 1, "exit") || !x.restart(t, bins[0]) || !x.held(t, 2, "go2") || !x.waitTree(t, dcLoaded) {
					return out
				}
				out = append(out, x.get(t, "c-after-rust: tree", "action=tree", 381))
				x.release(t, x.l, "go2")
				if !dcAfterRegistration(t, x, 2, "disabled") {
					return out
				}
				out = append(out,
					x.get(t, "c-after-rust: tree after the registration", "action=tree", 382),
					x.send(t, "c-after-rust: update s", "/api/v1/config?action=update&id=difftest:s", 383, `{"s":3}`))
				out = append(out, x.files(t, "c-after-rust: files")...)
				if x.exitPlugin(t, 2, "exit2") {
					if err := x.d.Stop(); err != nil {
						t.Errorf("%s: stop: %v", x.role, err)
					}
				}
				return out
			})
			// the oracle's files replace the candidate's
			from, to := dcConfigDir(xs[0].d.Opts.RunDir), dcConfigDir(xs[1].d.Opts.RunDir)
			if err := os.RemoveAll(to); err != nil {
				t.Fatal(err)
			}
			if b, err := exec.Command("cp", "-a", from, to).CombinedOutput(); err != nil {
				t.Fatalf("copy %s: %v: %s", from, err, b)
			}
			more := dcBoth(xs, func(i int, x *dcSide) []string {
				if !x.restart(t, bins[i]) || !x.held(t, 3, "go3") || !x.waitTree(t, dcLoaded) {
					return nil
				}
				out := []string{x.get(t, "rust-after-c: tree", "action=tree", 391)}
				out = append(out, x.files(t, "rust-after-c: files")...)
				x.release(t, x.l, "go3")
				if dcAfterRegistration(t, x, 3, "disabled") {
					out = append(out, x.get(t, "rust-after-c: tree after the registration", "action=tree", 392))
				}
				return append(out, x.files(t, "rust-after-c: files at the end")...)
			})
			// the copied files carry the oracle's times on both sides, next to each side's own new ones: this phase is
			// ranked on its own (`R<k>`)
			now := time.Now()
			for i := range more {
				more[i] = dcClockAs("R", more[i], now.Add(-10*time.Minute), now.Add(time.Minute))
			}
			return [2][]string{append(obs[0], more[0]...), append(obs[1], more[1]...)}
		},
	}
	return cases
}
