// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/notify"
)

// The reload of the health configuration (M9 commit 9, D214 F6; plan evidence/2026-10-05-plan-m9-commit9.md §3.6-3.10,
// §6.3): `netdatacli reload-health` and SIGUSR2 both run health_plugin_reload() (health/health.c:215-218), on a
// worker thread of the command server and on the main thread, which waits for the signals. It unregisters health's
// DynCfg nodes, empties the rule store, reads the rule files again (the directories' keys read at every reload),
// registers the nodes again (the DynCfg core replays the jobs a user saved), and then, on every host whose health is
// on and ran once, deletes every alert (a REMOVED entry each) and links the charts again
// (health_prototypes.c:502-521, :706-739). It tests no `[health] enabled`, reads nothing of `[health]` again, and
// neither reads nor writes the silencers.
//
// The entries a reload logs are stored by the metadata thread's own round, and HEALTH passes a host by until then
// (health_event_loop.c:396-401): an alert's first status after a reload comes one to seven seconds later, on each
// side at its own round. The cases give their pair the store's bound (healthStoreBound) and print that status's
// duration as `D` (healthSinceLink).

// The records of a reload (commands.c:182, signal-handler.c:249-252).
const (
	healthRecCommand = `msg="COMMAND: Reloading HEALTH configuration."`
	healthRecSignal  = `msg="SIGNAL: Received SIGUSR2. Reloading HEALTH configuration..."`
)

// healthCLIDone is what `netdatacli` prints and exits with for a command that answers nothing (cliResult).
const healthCLIDone = "{Stdout: Stderr: Exit:0}"

// reload has both agents reload their health configuration through `netdatacli reload-health`, the oracle first, and
// compares what the client printed and exited with, and what the command did to each side's silencers file
// (healthPair.watchFile). The oracle's client must print nothing and exit 0, and its file must not be written. The
// command answers when the reload ended (it runs inside the command, commands.c:176-187).
func (h *healthPair) reload(t *testing.T, label string) {
	t.Helper()
	h.command(t, label, "the silencers file, not written: ", "reload-health")
}

// command runs one `netdatacli` command on both sides, the oracle first (`{run}` in an argument is the side's run
// directory), and compares what the client printed and exited with and what the command did to the side's silencers
// file. The oracle's client must print nothing and exit 0, and the lines about its file begin with `file`.
func (h *healthPair) command(t *testing.T, label, file string, args ...string) {
	t.Helper()
	var got [2]string
	for i, s := range h.p.Each() {
		own := slices.Clone(args)
		for k := range own {
			own[k] = strings.ReplaceAll(own[k], "{run}", s.Daemon.Opts.RunDir)
		}
		after := h.watchFile(i)
		got[i] = fmt.Sprintf("%+v\n%s", runCLI(t, s.Daemon, own...), after())
	}
	if !strings.HasPrefix(got[0], healthCLIDone+"\n"+file) {
		t.Fatalf("oracle: %s: netdatacli %s:\n%s", label, strings.Join(args, " "), got[0])
	}
	if got[0] != got[1] {
		t.Fatalf("%s: netdatacli %s differs\noracle:\n%s\ncandidate:\n%s", label, strings.Join(args, " "), got[0], got[1])
	}
	t.Logf("%s: netdatacli %s, both sides:\n%s", label, strings.Join(args, " "), got[0])
}

// signalReload has both agents reload by SIGUSR2 and waits for each side's record of the signal: the reload runs
// right after it, on the thread that wrote it (signal-handler.c:247-255). What the reload did is waited for by the
// views compared next.
func (h *healthPair) signalReload(t *testing.T) {
	t.Helper()
	for _, s := range h.p.Each() {
		if err := syscall.Kill(s.Daemon.PID(), syscall.SIGUSR2); err != nil {
			t.Fatalf("%s: SIGUSR2: %v", s.Role, err)
		}
	}
	for _, s := range h.p.Each() {
		waitRecord(t, s.Daemon, strings.Trim(strings.TrimPrefix(healthRecSignal, "msg="), `"`), 20*time.Second)
	}
}

// rewrite replaces each side's rule file (healthConfFile) with `conf`, `{run}` in it the side's run directory, held
// to the rule rail as the file a case starts with is (healthPrepare; daemon.ValidateHealthRules).
func (h *healthPair) rewrite(t *testing.T, conf string) {
	t.Helper()
	for _, s := range h.p.Each() {
		text, err := healthRulesFor(s.Daemon.Opts.RunDir, conf)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(s.Daemon.Opts.RunDir, "etc", "health.d", healthConfFile), []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}

// healthRulesFor is a rule file's text as side `runDir` gets it: `{run}` replaced by the side's run directory, then
// held to the rail: a rule's own `exec` stays under the side's notifier directory or where nothing can be executed.
func healthRulesFor(runDir, conf string) (string, error) {
	text := strings.ReplaceAll(conf, "{run}", runDir)
	if err := daemon.ValidateHealthRules(text, notify.Dir(runDir)+"/", "/dev/null/"); err != nil {
		return "", fmt.Errorf("harness: the rule file is not rewritten: %w", err)
	}
	return text, nil
}

// healthDirLines are the lines of a `/netdata.conf` dump's [directories] section that name a health directory
// (`health config`, `stock health config`: health.c:148-158), trimmed, the run directory replaced. An agent lists a
// key once it read it: C reads `health config` at every load of the rule files, and `stock health config` too when
// the stock rules are on, so an agent whose health is off lists neither before its first reload. A key at its
// default is a comment line; one a command set (`write-config`) is printed as it is, after a marker line that names
// it (`#| >>> [directories].health config <<<`), which this keeps too.
func healthDirLines(conf, runDir string) []string {
	var out []string
	in := false
	for _, line := range strings.Split(conf, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]") {
			in = line == "[directories]"
			continue
		}
		if in && strings.Contains(line, "health config") {
			out = append(out, strings.ReplaceAll(line, runDir, "{run}"))
		}
	}
	return out
}

// dirKeys is side i's view of the health directories' keys in `/netdata.conf` (healthDirLines).
func (h *healthPair) dirKeys(i int) string {
	d := h.p.Each()[i].Daemon
	r := healthGet(d, "/netdata.conf")
	return healthView(r, strings.Join(healthDirLines(string(r.Body), d.Opts.RunDir), "\n"))
}

// healthDirsAre is a guard on a view of the directories' keys (healthPair.dirKeys): exactly these lines.
func healthDirsAre(lines ...string) func(string) error {
	return healthIsText("HTTP 200, text/plain; charset=utf-8\n" + strings.Join(lines, "\n"))
}

// a node's status in its object of a tree (dyncfg-tree.c:20-60)
var healthNodeStatusRe = regexp.MustCompile(`"status":"([a-z]+)"`)

// healthNodeStatuses are health's DynCfg nodes in a view of the tree, in the tree's order: `<id> <status>`.
func healthNodeStatuses(view string) []string {
	var out []string
	for _, id := range healthJobs(view) {
		node, _ := healthNodeOf(view, id)
		status := "?"
		if m := healthNodeStatusRe.FindStringSubmatch(node); m != nil {
			status = m[1]
		}
		out = append(out, id+" "+status)
	}
	return out
}

// healthNodesAre is a guard on a view of the tree: health's nodes are exactly these, each `<id> <status>`, in the
// tree's order.
func healthNodesAre(want ...string) func(string) error {
	return func(view string) error {
		if !strings.HasPrefix(view, "HTTP 200, ") {
			return fmt.Errorf("answered %q", strings.SplitN(view, "\n", 2)[0])
		}
		if got := healthNodeStatuses(view); !slices.Equal(got, want) {
			return fmt.Errorf("health's nodes are %q, want %q", got, want)
		}
		return nil
	}
}

// an alert's id and its name in a view of `/api/v1/alarms?all`, as the normalizer prints them (health_json.c:53-58)
var healthAlarmIDRe = regexp.MustCompile(`"id": (a[+-]\d+),\s*"config_hash_id": "[^"]*",\s*"name": "([^"]*)"`)

// healthAlarmIDs are the ids of the alerts a view of `/api/v1/alarms?all` lists, by name.
func healthAlarmIDs(view string) map[string]string {
	out := map[string]string{}
	for _, m := range healthAlarmIDRe.FindAllStringSubmatch(view, -1) {
		out[m[2]] = m[1]
	}
	return out
}

// healthIDsAre is a guard on a view of `/api/v1/alarms?all`: each alert of `want` is listed with its id (an alert
// linked again keeps the id it had: rrdcalc.c:130-169).
func healthIDsAre(want map[string]string) func(string) error {
	return func(view string) error {
		if len(want) == 0 {
			return fmt.Errorf("harness: no id to hold the view to")
		}
		got := healthAlarmIDs(view)
		for _, name := range slices.Sorted(maps.Keys(want)) {
			if want[name] == "" || got[name] != want[name] {
				return fmt.Errorf("the id of %s is %q, want %q", name, got[name], want[name])
			}
		}
		return nil
	}
}

// healthLastChanges is a guard on a view of the transitions (healthPair.transitions): the alert's last changes of
// status are these, in turn (`OLD->NEW`).
func healthLastChanges(name string, changes ...string) func(string) error {
	return func(view string) error {
		var got []string
		for _, l := range strings.Split(view, "\n") {
			if rest, ok := strings.CutPrefix(l, name+": "); ok {
				change, _, _ := strings.Cut(rest, " ")
				got = append(got, change)
			}
		}
		if len(got) < len(changes) || !slices.Equal(got[len(got)-len(changes):], changes) {
			return fmt.Errorf("the changes of %s are %v, want them to end with %v", name, got, changes)
		}
		return nil
	}
}

// healthChangesOf is a guard on a view of the transitions: the alert has exactly n entries.
func healthChangesOf(name string, n int) func(string) error {
	return func(view string) error {
		got := 0
		for _, l := range strings.Split(view, "\n") {
			if strings.HasPrefix(l, name+": ") {
				got++
			}
		}
		if got != n {
			return fmt.Errorf("%s has %d entries, want %d", name, got, n)
		}
		return nil
	}
}

// healthReloaded is the guard on the transitions after a reload for an alert that stood at `was` and whose rule is
// still there: it was removed, linked again, and reached `now`.
func healthReloaded(name, was, now string) func(string) error {
	return healthLastChanges(name, was+"->REMOVED", "REMOVED->UNINITIALIZED", "UNINITIALIZED->"+now)
}

// commandRecordsOf are a daemon.log's records of the commands a reload case sends and of SIGUSR2's reload
// (commands.c:182, :315, :340; signal-handler.c:249-252), in file order, normalized (the run directory, the clocks,
// the thread ids and a worker thread's number).
func commandRecordsOf(lines []string, runDir string) []string {
	var out []string
	for _, l := range lines {
		if strings.Contains(l, healthRecCommand) || strings.Contains(l, `msg="write-config `) || strings.Contains(l, `msg="SIGNAL: Received SIGUSR2.`) {
			out = append(out, normalizeLog(l, runDir, ""))
		}
	}
	return out
}

// compareCommandRecords compares both sides' records of the commands and of SIGUSR2, once both stopped.
func (h *healthPair) compareCommandRecords(t *testing.T, total int, want map[string]int) {
	t.Helper()
	h.compareLines(t, "the commands' records", func(i int) []string {
		d := h.p.Each()[i].Daemon
		return commandRecordsOf(logLines(t, d.Opts.RunDir, "daemon.log"), d.Opts.RunDir)
	}, healthRecordsCount(total, want))
}

// compareHealthLog compares both sides' health.log, once both stopped, in file order: every transition's record with
// its thread (a reload's are the command's worker's, or the signal thread's), a first status's duration as `D`
// (healthLogSinceLink), a worker's number and a web record's connection count masked. The oracle's must hold `least`
// records or more.
func (h *healthPair) compareHealthLog(t *testing.T, least int) {
	t.Helper()
	h.compareLines(t, "health.log", func(i int) []string {
		lines := h.n[i].healthLog(t, h.p.Each()[i].Daemon)
		for k, l := range lines {
			lines[k] = healthConnRe.ReplaceAllString(healthLogSinceLink(l), " conn=N")
		}
		return lines
	}, func(oracle []string) error {
		if len(oracle) < least {
			return fmt.Errorf("%d records, want %d or more", len(oracle), least)
		}
		return nil
	})
}

// TestHealthReload (check `health.reload`, M9 commit 9, D214 F6): what a reload of the health configuration does to a
// running agent, behind the health runner's recording notifier. Cases:
//   - `cli`: `netdatacli reload-health` four times with an alert raised, at debug level: over the same file (every
//     alert is removed and linked again with its id, and nothing is notified again); over a rewritten file (a rule
//     changed, one gone, one new: the DynCfg tree follows); after `write-config` of a `[health]` key (a reload reads
//     none of them); and after `write-config` of `[directories] health config` (the reload reads that directory);
//     health.log's records of the reloads carry the command's worker thread;
//   - `signal`: the first reload by SIGUSR2, at debug level: the two records, and health.log with the reload's thread
//     (whether HEALTH logged a pass it skipped while the reload's entries waited is not compared: a store that comes
//     before HEALTH's next pass leaves no such record);
//   - `off`: health off: the start loads nothing; a reload reads the rule file, registers the template and the jobs,
//     replays a saved DynCfg job, lists the directory's key in `/netdata.conf` and serves the rule's `alert_config`,
//     and links nothing on localhost; an `add` is accepted, and a second reload replays it;
//   - `dyncfg`: a job added, a rule file's job updated then removed, another disabled; then a reload: the removed
//     rule is back from its file, the added job is replayed, the disabled job stays disabled, and the added alert is
//     unlinked and linked twice (once by its replay, once by the reload's walk);
//   - `silenced`: a reload with every alert silenced, then with every alert disabled: the silencers stay, their file
//     is not written, the new alerts take their flags at HEALTH's next pass, and nothing is notified;
//   - `child`: a reload with a streaming child connected (both hosts' alerts are removed and linked again) and after
//     it disconnected (the reload passes the child's host by);
//   - `off-child`: the parent's health off and a child whose section turns its own on: no rule before the reload, and
//     after it the child's alert linked, raised and notified.
func TestHealthReload(t *testing.T) {
	runHealthCases(t, map[string]healthCase{
		"cli":      healthReloadCLICase(),
		"signal":   healthReloadSignalCase(),
		"off":      healthReloadOffCase(),
		"dyncfg":   healthReloadDynCfgCase(),
		"silenced": healthReloadSilencedCase(),
		"child":    healthReloadChildCase(),
		// beyond the plan's list: what the rail on stream.conf is for (D214 F7)
		"off-child": healthReloadOffChildCase(),
	})
}

// healthReloadOther is the `cli` case's second rule directory, under the run directory.
const healthReloadOther = "etc/other.d"

// healthReloadCLICase is `cli`.
func healthReloadCLICase() healthCase {
	a, b, c, d := healthJob("hr_a"), healthJob("hr_b"), healthJob("hr_c"), healthJob("hr_d")
	running := func(id string) func(string) error {
		return healthNode(id, `"type":"job","template":"`+healthJobPrefix+`","status":"running",`+healthCmdsFile, `"source_type":"user"`, `"saves":0`)
	}
	return healthCase{
		conf:  healthDcAlert("hr_a", 50) + healthDcAlert("hr_b", 90),
		files: map[string]string{healthReloadOther + "/other.conf": healthDcAlert("hr_d", 60)},
		// at debug level health.log has a record for every entry, the reloads' unlinks and links with their thread
		logs: healthLogsDebug,
		sc:   healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			transcript := h.calls(t)
			start := map[string]string{"hr_a": "WARNING", "hr_b": "CLEAR"}
			ids := healthAlarmIDs(h.compareNow(t, "at the start: /api/v1/alarms?all", h.all, healthWant(start)))
			h.processed(t, "at the start", "hr_a", "WARNING")
			h.compareNow(t, "at the start: the notifier's calls", transcript, healthCallsWant(1, healthCallFor("hr_a", "WARNING", 1, nil), healthCallsEnded(0)))
			h.compareNow(t, "at the start: the tree", h.tree, healthBoth(healthTemplateNode, running(a), running(b),
				healthNodesAre(healthJobPrefix+" accepted", a+" running", b+" running")))
			h.compareNow(t, "at the start: the directories' keys", h.dirKeys, healthDirsAre("# health config = {run}/etc/health.d"))

			// the same file: every alert is removed and linked again with its id; WARNING is the status hr_a was last
			// notified for, so nothing is notified
			time.Sleep(healthUnlinkHold)
			h.reload(t, "the first reload")
			h.compareNow(t, "reloaded: the transitions", h.changes, healthBoth(healthReloaded("hr_a", "WARNING", "WARNING"),
				healthReloaded("hr_b", "CLEAR", "CLEAR"), healthChangesOf("hr_a", 4+3), healthChangesOf("hr_b", 4+3)))
			h.compareNow(t, "reloaded: /api/v1/alarms?all", h.all, healthBoth(healthWant(start), healthIDsAre(ids)))
			h.processed(t, "reloaded", "hr_a", "WARNING")
			h.compareNow(t, "reloaded: the notifier's calls", transcript, healthCallsWant(1))
			h.compareNow(t, "reloaded: the tree", h.tree, healthBoth(healthTemplateNode, running(a), running(b),
				healthNodesAre(healthJobPrefix+" accepted", a+" running", b+" running")))

			// the file rewritten: hr_a's threshold is above the value, hr_b is gone, hr_c is new
			time.Sleep(healthUnlinkHold)
			h.rewrite(t, healthDcAlert("hr_a", 80)+healthDcAlert("hr_c", 60))
			h.reload(t, "the second reload")
			second := map[string]string{"hr_a": "CLEAR", "hr_c": "WARNING"}
			h.compareNow(t, "rewritten: the transitions", h.changes, healthBoth(healthReloaded("hr_a", "WARNING", "CLEAR"),
				healthLastChanges("hr_b", "REMOVED->UNINITIALIZED", "UNINITIALIZED->CLEAR", "CLEAR->REMOVED"),
				healthChangesOf("hr_c", 2), healthLastChanges("hr_c", "REMOVED->UNINITIALIZED", "UNINITIALIZED->WARNING")))
			h.compareNow(t, "rewritten: /api/v1/alarms?all", h.all, healthBoth(healthWant(second), healthIDsAre(map[string]string{"hr_a": ids["hr_a"]})))
			h.processed(t, "rewritten", "hr_a", "CLEAR")
			h.processed(t, "rewritten", "hr_c", "WARNING")
			// hr_a's CLEAR is notified, though it comes from UNINITIALIZED: its last notified status is WARNING
			h.compareNow(t, "rewritten: the notifier's calls", transcript, healthCallsWant(3,
				healthCallFor("hr_a", "CLEAR", 1, nil), healthCallFor("hr_c", "WARNING", 1, nil), healthCallsEnded(0)))
			h.compareNow(t, "rewritten: the tree", h.tree, healthBoth(healthTemplateNode, running(a), running(c), healthNoNode(b),
				healthNodesAre(healthJobPrefix+" accepted", a+" running", c+" running")))

			// a `[health]` key changed in the agent's configuration: a reload reads none of them
			time.Sleep(healthUnlinkHold)
			h.command(t, "write-config of a [health] key", "the silencers file, not written: ", "write-config", "netdata|health|enabled alarms|!*")
			h.reload(t, "the third reload")
			h.compareNow(t, "after the [health] key: the transitions", h.changes, healthBoth(healthReloaded("hr_a", "CLEAR", "CLEAR"),
				healthReloaded("hr_c", "WARNING", "WARNING"), healthChangesOf("hr_c", 2+3)))
			h.compareNow(t, "after the [health] key: /api/v1/alarms?all", h.all, healthWant(second))
			h.processed(t, "after the [health] key", "hr_c", "WARNING")
			h.compareNow(t, "after the [health] key: the notifier's calls", transcript, healthCallsWant(3))

			// the rule directory's key changed: the reload reads the other directory
			time.Sleep(healthUnlinkHold)
			h.command(t, "write-config of the rule directory", "the silencers file, not written: ", "write-config",
				"netdata|directories|health config|{run}/"+healthReloadOther)
			h.reload(t, "the fourth reload")
			h.compareNow(t, "the other directory: the transitions", h.changes, healthBoth(healthLastChange("hr_a", "CLEAR->REMOVED"),
				healthLastChange("hr_c", "WARNING->REMOVED"), healthChangesOf("hr_d", 2), healthLastChange("hr_d", "UNINITIALIZED->WARNING")))
			h.compareNow(t, "the other directory: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hr_d": "WARNING"}))
			h.processed(t, "the other directory", "hr_d", "WARNING")
			h.compareNow(t, "the other directory: the notifier's calls", transcript, healthCallsWant(4, healthCallFor("hr_d", "WARNING", 1, nil), healthCallsEnded(0)))
			h.compareNow(t, "the other directory: the tree", h.tree, healthBoth(healthTemplateNode, running(d),
				healthNodesAre(healthJobPrefix+" accepted", d+" running")))
			// the dump marks a key whose value a command set, and prints it without the comment mark
			h.compareNow(t, "the other directory: the directories' keys", h.dirKeys, healthDirsAre("#| >>> [directories].health config <<<",
				"health config = {run}/"+healthReloadOther))
			h.compareSaved(t, "at the end", map[string][]string{})
			// hr_a: its first pass's four, three per reload over its rule, and the last reload's unlink; hr_b: four,
			// three, and its unlink; hr_c: its link and status, three, and its unlink; hr_d: its link and status
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthLogEntries((4+3*3+1)+(4+3+1)+(2+3+1)+2))
		},
		after: func(t *testing.T, h *healthPair) {
			// the commands run on a worker thread of the command server
			h.compareCommandRecords(t, 4+2*2, map[string]int{
				"level=info … thread=UV_WORKER[n] " + healthRecCommand: 4,
				`msg="write-config netdata|health|enabled alarms|!*"`:  1,
				// the log masks cut a `key=` field at its first space
				`msg="write-config conf_file=netdata section=health key=K alarms value=!*"`:                                   1,
				`msg="write-config netdata|directories|health config|<RUN>/` + healthReloadOther + `"`:                        1,
				`msg="write-config conf_file=netdata section=directories key=K config value=<RUN>/` + healthReloadOther + `"`: 1,
			})
			h.compareCfgRecords(t, healthRecordsCount(0, nil))
			// a record per entry of the alert log
			h.compareHealthLog(t, (4+3*3+1)+(4+3+1)+(2+3+1)+2)
		},
	}
}

// healthReloadSignalCase is `signal`: one reload by SIGUSR2 over the same file, at debug level.
func healthReloadSignalCase() healthCase {
	return healthCase{
		conf: healthDcAlert("hr_a", 50) + healthDcAlert("hr_b", 90),
		logs: healthLogsDebug,
		sc:   healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			transcript := h.calls(t)
			start := map[string]string{"hr_a": "WARNING", "hr_b": "CLEAR"}
			ids := healthAlarmIDs(h.compareNow(t, "at the start: /api/v1/alarms?all", h.all, healthWant(start)))
			h.processed(t, "at the start", "hr_a", "WARNING")
			h.compareNow(t, "at the start: the notifier's calls", transcript, healthCallsWant(1, healthCallFor("hr_a", "WARNING", 1, nil)))
			time.Sleep(healthUnlinkHold)
			h.signalReload(t)
			h.compareNow(t, "reloaded: the transitions", h.changes, healthBoth(healthReloaded("hr_a", "WARNING", "WARNING"),
				healthReloaded("hr_b", "CLEAR", "CLEAR"), healthChangesOf("hr_a", 4+3), healthChangesOf("hr_b", 4+3)))
			h.compareNow(t, "reloaded: /api/v1/alarms?all", h.all, healthBoth(healthWant(start), healthIDsAre(ids)))
			h.processed(t, "reloaded", "hr_a", "WARNING")
			h.compareNow(t, "reloaded: the notifier's calls", transcript, healthCallsWant(1))
			h.compareNow(t, "reloaded: the tree", h.tree, healthNodesAre(healthJobPrefix+" accepted", healthJob("hr_a")+" running", healthJob("hr_b")+" running"))
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthLogEntries(7+7))
		},
		after: func(t *testing.T, h *healthPair) {
			// the signal's record, then the command's, both on the main thread (C prints no thread field for it), as are
			// health.log's records of the reload
			h.compareCommandRecords(t, 2, map[string]int{"level=info tid=N  " + healthRecSignal: 1, "level=info tid=N  " + healthRecCommand: 1,
				"thread=": 0})
			h.compareHealthLog(t, 14)
		},
	}
}

// healthFirstDifference says where two views part: how many lines each has, and the first line that differs, as each
// side has it.
func healthFirstDifference(oracle, candidate string) string {
	o, c := strings.Split(oracle, "\n"), strings.Split(candidate, "\n")
	k := 0
	for k < len(o) && k < len(c) && o[k] == c[k] {
		k++
	}
	line := func(lines []string) string {
		if k < len(lines) {
			return fmt.Sprintf("%q", lines[k])
		}
		return "(no such line)"
	}
	return fmt.Sprintf("%d lines on the oracle, %d on the candidate; line %d is %s on the oracle, %s on the candidate", len(o), len(c), k+1, line(o), line(c))
}

// healthReloadOffCase is `off`: health off, the stock rules off, one rule file, one saved DynCfg job and a chart the
// rules name.
func healthReloadOffCase() healthCase {
	file, saved, added := healthJob("ho_file"), healthJob("ho_saved"), healthJob("ho_new")
	return healthCase{
		off:  true,
		conf: healthDcAlert("ho_file", 50),
		saved: map[string]string{
			healthSavedName(saved): healthSavedText("ho_saved", "dyncfg", healthSavedSource, false, 1, healthSavedCmds, healthPayload(healthDcRule(60))),
		},
		sc: healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.createChart(t)
			plain := func(path string) func(i int) string { return func(i int) string { return h.plain(i, path) } }
			// the start loaded nothing: no template, no job of the file; the saved job is an orphan
			h.compareNow(t, "before: the tree", h.tree, healthBoth(healthNoNode(healthJobPrefix, file), healthNode(saved, healthOrphan),
				healthNodesAre(saved+" orphan")))
			h.compareNow(t, "before: the directories' keys", h.dirKeys, healthDirsAre())
			h.compareNow(t, "before: /api/v1/alarms?all", h.all, healthHostOff)

			h.reload(t, "the first reload")
			h.compareNow(t, "reloaded: the tree", h.tree, healthBoth(healthTemplateNode,
				healthNode(file, `"status":"running",`+healthCmdsFile, `"source_type":"user"`, `"saves":0`),
				healthNode(saved, `"status":"accepted",`+healthCmdsDynCfg, `"source_type":"dyncfg"`, `"saves":1`),
				healthNodesAre(healthJobPrefix+" accepted", file+" running", saved+" accepted")))
			h.compareNow(t, "reloaded: the directories' keys", h.dirKeys, healthDirsAre("# health config = {run}/etc/health.d"))
			got := h.cfgStep(t, healthCfgStep{label: "get the file's job", query: "action=get&id=" + file, code: 200,
				parts: []string{`"name":"ho_file"`, `"source_type":"user"`, `"warning_condition":"$this > 50"`}})
			h.cfgStep(t, healthCfgStep{label: "get the saved job", query: "action=get&id=" + saved, code: 200,
				parts: []string{`"name":"ho_saved"`, `"source_type":"dyncfg"`, `"warning_condition":"$this > 60"`}})
			hash := healthRuleHashRe.FindStringSubmatch(got)
			if hash == nil {
				t.Fatalf("oracle: the file's job names no rule hash:\n%s", got)
			}
			// the rule's row is stored by the metadata thread
			h.compareNow(t, "reloaded: the rule's alert_config", plain("/api/v2/alert_config?config="+hash[1]), healthHolds(`"name":"ho_file"`))
			// localhost's health is off: no alert, though the chart the rules name is collected
			h.compareNow(t, "reloaded: /api/v1/alarms?all", h.all, healthHostOff)

			h.cfgStep(t, healthCfgStep{label: "add", query: "action=add&id=" + healthJobPrefix + "&name=ho_new", body: healthPayload(healthDcRule(50)),
				code: 202, parts: []string{healthMsg(202, "accepted")}})
			h.compareNow(t, "added: the tree", h.tree, healthBoth(healthNode(added, `"status":"accepted",`+healthCmdsAdded, `"source_type":"dyncfg"`),
				healthNodesAre(healthJobPrefix+" accepted", file+" running", added+" accepted", saved+" accepted")))
			time.Sleep(healthCalcHold)
			h.compareNow(t, "added: /api/v1/alarms?all", h.all, healthHostOff)

			h.reload(t, "the second reload")
			h.compareNow(t, "reloaded again: the tree", h.tree, healthBoth(healthTemplateNode,
				healthNode(file, `"status":"running",`+healthCmdsFile), healthNode(saved, `"status":"accepted",`+healthCmdsDynCfg),
				healthNode(added, `"status":"accepted",`+healthCmdsDynCfg, `!"test"`),
				healthNodesAre(healthJobPrefix+" accepted", file+" running", added+" accepted", saved+" accepted")))
			h.compareNow(t, "reloaded again: /api/v1/alarms?all", h.all, healthHostOff)
			h.compareNow(t, "reloaded again: /api/v1/alarm_log", func(i int) string { return h.get(i, "/api/v1/alarm_log") }, healthIs(healthLogEmpty))
			h.compareNow(t, "reloaded again: the notifier's calls", h.transcript(t), healthCallsWant(0))
			h.compareSaved(t, "at the end", map[string][]string{saved: {`"saves=1\n"`}, added: {`"saves=1\n"`, `$this > 50`}})
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCommandRecords(t, 2, map[string]int{"level=info … thread=UV_WORKER[n] " + healthRecCommand: 2})
			h.compareCfgRecords(t, healthRecordsCount(1, map[string]int{
				"level=notice … msg=\"DYNCFG USER ACTION 'add' ho_new on template '" + healthJobPrefix + "' by user": 1,
			}))
		},
	}
}

// healthReloadDynCfgConf are the `dyncfg` case's rules at the start; the file the reload reads has no hr_gone.
var healthReloadDynCfgConf = healthDcAlert("hr_keep", 90) + healthDcAlert("hr_upd", 50) + healthDcAlert("hr_dis", 90)

// healthAlarmsOrder is a guard on a view of `/api/v1/alarms?all`: the alerts are listed in this order (the order
// the host links them in).
func healthAlarmsOrder(names ...string) func(string) error {
	return func(view string) error {
		var got []string
		for _, m := range healthAlarmStatusRe.FindAllStringSubmatch(healthBody(view), -1) {
			got = append(got, m[1])
		}
		if !slices.Equal(got, names) {
			return fmt.Errorf("the alerts are listed as %v, want %v", got, names)
		}
		return nil
	}
}

// healthReloadDynCfgCase is `dyncfg`, at debug level: what a reload makes of a session's DynCfg changes. Two jobs
// are added (hr_z, then hr_new: the core replays them in the order it got them, which is not the tree's); a rule
// file's alert is updated and then removed (hr_upd: the reload brings the file's rule back); a file's job is
// disabled (hr_dis: disabled again by its first echo); and another is disabled and its rule then taken out of the
// file (hr_gone: the core keeps the node of a job it saved, though nothing registers it any more).
func healthReloadDynCfgCase() healthCase {
	keep, upd, dis, gone := healthJob("hr_keep"), healthJob("hr_upd"), healthJob("hr_dis"), healthJob("hr_gone")
	z, added := healthJob("hr_z"), healthJob("hr_new")
	add := func(name string) healthCfgStep {
		return healthCfgStep{label: "add " + name, query: "action=add&id=" + healthJobPrefix + "&name=" + name, body: healthPayload(healthDcRule(60)),
			code: 202, parts: []string{healthMsg(202, "accepted")}}
	}
	return healthCase{
		conf: healthReloadDynCfgConf + healthDcAlert("hr_gone", 95),
		logs: healthLogsDebug,
		sc:   healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			transcript := h.calls(t)
			h.compareNow(t, "at the start: /api/v1/alarms?all", h.all,
				healthWant(map[string]string{"hr_keep": "CLEAR", "hr_upd": "WARNING", "hr_dis": "CLEAR", "hr_gone": "CLEAR"}))
			h.processed(t, "at the start", "hr_upd", "WARNING")

			// two jobs added; a file's job updated (to CLEAR, which is notified) and then removed; two disabled
			time.Sleep(healthUnlinkHold)
			h.cfgSteps(t, add("hr_z"), add("hr_new"))
			time.Sleep(healthSkipHold)
			h.cfgStep(t, healthCfgStep{label: "update a file's alert", query: "action=update&id=" + upd, body: healthPayload(healthDcRule(80)),
				code: 202, parts: []string{healthMsg(202, "updated")}})
			h.compareNow(t, "updated: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hr_keep": "CLEAR", "hr_upd": "CLEAR", "hr_dis": "CLEAR",
				"hr_gone": "CLEAR", "hr_z": "WARNING", "hr_new": "WARNING"}))
			h.processed(t, "updated", "hr_upd", "CLEAR")
			h.processed(t, "updated", "hr_z", "WARNING")
			h.processed(t, "updated", "hr_new", "WARNING")
			time.Sleep(healthUnlinkHold)
			h.cfgSteps(t,
				healthCfgStep{label: "remove the updated alert", query: "action=remove&id=" + upd, code: 200, parts: []string{healthMsg(200, "deleted")}},
				healthCfgStep{label: "disable a file's alert", query: "action=disable&id=" + dis, code: 200, parts: []string{healthMsg(200, "disabled")}},
				healthCfgStep{label: "disable another", query: "action=disable&id=" + gone, code: 200, parts: []string{healthMsg(200, "disabled")}})
			before := map[string]string{"hr_keep": "CLEAR", "hr_z": "WARNING", "hr_new": "WARNING"}
			ids := healthAlarmIDs(h.compareNow(t, "before the reload: /api/v1/alarms?all", h.all,
				healthBoth(healthWant(before), healthAlarmsOrder("hr_keep", "hr_z", "hr_new"))))
			h.compareNow(t, "before the reload: the transitions", h.changes, healthBoth(healthLastChange("hr_upd", "CLEAR->REMOVED"),
				healthLastChange("hr_dis", "CLEAR->REMOVED"), healthLastChange("hr_gone", "CLEAR->REMOVED")))
			// the tree lists the nodes by id, whatever order they were registered in (dyncfg-tree.c:6-17, :109)
			h.compareNow(t, "before the reload: the tree", h.tree, healthBoth(healthNoNode(upd),
				healthNode(added, `"status":"accepted",`+healthCmdsAdded, `"source_type":"dyncfg"`, `"saves":1`),
				healthNode(dis, `"status":"disabled",`+healthCmdsFile, `"user_disabled":true`, `"saves":1`),
				healthNodesAre(healthJobPrefix+" accepted", dis+" disabled", gone+" disabled", keep+" running", added+" accepted", z+" accepted")))
			h.compareNow(t, "before the reload: the notifier's calls", transcript, healthCallsWant(4, healthCallFor("hr_upd", "WARNING", 1, nil),
				healthCallFor("hr_z", "WARNING", 1, nil), healthCallFor("hr_new", "WARNING", 1, nil), healthCallFor("hr_upd", "CLEAR", 1, nil), healthCallsEnded(0)))
			job := []string{`"source_type=dyncfg\n"`, `"saves=1\n"`, `"cmds=` + healthSavedCmds + `\n"`, `$this > 60`}
			off := []string{`"user_disabled=true\n"`, `"saves=1\n"`, "!---"}
			saved := map[string][]string{z: job, added: job, dis: off, gone: off}
			h.compareSaved(t, "before the reload", saved)

			// hr_gone's rule leaves the file; then the reload
			h.rewrite(t, healthReloadDynCfgConf)
			time.Sleep(healthUnlinkHold)
			h.reload(t, "the reload")
			// the removed rule is back from its file, as the file has it; the added jobs are replayed, without `test`;
			// the disabled job is disabled again by its first echo; the job whose rule is gone keeps its node
			after := map[string]string{"hr_keep": "CLEAR", "hr_upd": "WARNING", "hr_z": "WARNING", "hr_new": "WARNING"}
			h.compareNow(t, "reloaded: the tree", h.tree, healthBoth(healthTemplateNode,
				healthNode(upd, `"status":"running",`+healthCmdsFile, `"source_type":"user"`, `"saves":0`),
				healthNode(added, `"status":"accepted",`+healthCmdsDynCfg, `!"test"`, `"source_type":"dyncfg"`, `"saves":1`),
				healthNode(z, `"status":"accepted",`+healthCmdsDynCfg, `!"test"`, `"source_type":"dyncfg"`, `"saves":1`),
				healthNode(dis, `"status":"disabled",`+healthCmdsFile, `"user_disabled":true`, `"saves":1`),
				// nothing registered hr_gone's node again, and the tree prints a node without a method as an orphan
				// (dyncfg-tree.c:93-94), whatever its status was
				healthNode(gone, `"type":"job","template":"`+healthJobPrefix+`",`+healthOrphan, `"source_type":"user"`, `"user_disabled":true`, `"saves":1`),
				healthNode(keep, `"status":"running",`+healthCmdsFile, `"saves":0`),
				healthNodesAre(healthJobPrefix+" accepted", dis+" disabled", gone+" orphan", keep+" running", added+" accepted", upd+" running", z+" accepted")))
			h.cfgSteps(t,
				healthCfgStep{label: "get the rule that is back", query: "action=get&id=" + upd, code: 200,
					parts: []string{`"source_type":"user"`, `"warning_condition":"$this > 50"`}},
				healthCfgStep{label: "get an added job", query: "action=get&id=" + added, code: 200,
					parts: []string{`"source_type":"dyncfg","source":""`, `"warning_condition":"$this > 60"`}},
				healthCfgStep{label: "get the disabled job", query: "action=get&id=" + dis, code: 200, parts: []string{`"source_type":"user"`}},
				healthCfgStep{label: "get the job nothing was done to", query: "action=get&id=" + keep, code: 200, parts: []string{`"source_type":"user"`}},
				// the node the core kept has no method: the core answers for it (dyncfg-tree.c:236-289)
				healthCfgStep{label: "get the job whose rule is gone", query: "action=get&id=" + gone, code: 404,
					parts: []string{`"errorMessage":"Unknown config id given."`}})
			// a file's alert: its unlink and its link; a DynCfg alert: twice each, once by its replay and once by the
			// reload's walk over the hosts
			twice := []string{"WARNING->REMOVED", "REMOVED->UNINITIALIZED", "UNINITIALIZED->REMOVED", "REMOVED->UNINITIALIZED", "UNINITIALIZED->WARNING"}
			h.compareNow(t, "reloaded: the transitions", h.changes, healthBoth(healthReloaded("hr_keep", "CLEAR", "CLEAR"),
				healthLastChanges("hr_z", twice...), healthLastChanges("hr_new", twice...),
				healthLastChanges("hr_upd", "CLEAR->REMOVED", "REMOVED->UNINITIALIZED", "UNINITIALIZED->WARNING"),
				healthLastChange("hr_dis", "CLEAR->REMOVED"), healthLastChange("hr_gone", "CLEAR->REMOVED")))
			// the file's rules in the file's order, then the replayed jobs in the order the session added them
			h.compareNow(t, "reloaded: /api/v1/alarms?all", h.all, healthBoth(healthWant(after), healthIDsAre(ids),
				healthAlarmsOrder("hr_keep", "hr_upd", "hr_z", "hr_new")))
			h.processed(t, "reloaded", "hr_upd", "WARNING")
			h.processed(t, "reloaded", "hr_z", "WARNING")
			h.processed(t, "reloaded", "hr_new", "WARNING")
			// hr_upd's WARNING is notified again: its last notified status is the update's CLEAR
			h.compareNow(t, "reloaded: the notifier's calls", transcript, healthCallsWant(5, healthCallFor("hr_upd", "WARNING", 2, nil), healthCallsEnded(0)))
			h.compareSaved(t, "reloaded", saved)
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthHolds())
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCommandRecords(t, 1, map[string]int{"level=info … thread=UV_WORKER[n] " + healthRecCommand: 1})
			h.compareCfgRecords(t, healthRecordsCount(7, map[string]int{
				// the `get` of the node without a method
				"level=error … msg=\"DYNCFG: unknown config id '" + gone + "' in call: 'config " + gone + " get'.": 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'add' hr_z on template":                                   1,
				"level=notice … msg=\"DYNCFG USER ACTION 'add' hr_new on template":                                 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'update' on job '" + upd + "'":                            1,
				"level=notice … msg=\"DYNCFG USER ACTION 'remove' on job '" + upd + "'":                            1,
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + dis + "'":                           1,
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + gone + "'":                          1,
			}))
			// a rule a user adds or updates is pushed to the Cloud, by the web worker that took the request; the rules a
			// reload registers are not (health_prototypes.c:517-520: `registering` is on)
			h.compareLines(t, "the access log's records of a push to the Cloud", func(i int) []string {
				d := h.p.Each()[i].Daemon
				var out []string
				for _, l := range logLines(t, d.Opts.RunDir, "access.log") {
					if strings.Contains(l, healthAclkReq) {
						l = strings.ReplaceAll(l, " transaction="+fnTxPrefix, " transaction=sent:"+fnTxPrefix)
						out = append(out, healthConnRe.ReplaceAllString(h.n[i].paths(normalizeLog(l, d.Opts.RunDir, "")), " conn=N"))
					}
				}
				return out
			}, healthRecordsCount(3, map[string]int{"thread=WEB[n] ": 3, "transaction=sent:" + healthCfgTx(1) + " ": 1,
				"transaction=sent:" + healthCfgTx(2) + " ": 1, "transaction=sent:" + healthCfgTx(3) + " ": 1}))
		},
	}
}

// healthReloadSilencedCase is `silenced`: two alerts on a chart that goes from 10 to 70 once every alert is
// silenced; a reload; then a DISABLE selector (with `all` still set it disables every alert, health_silencers.c:497-
// 509) and a second reload.
func healthReloadSilencedCase() healthCase {
	J := healthSilencersJSON
	silenced := []string{"hs_a disabled=false silenced=true", "hs_b disabled=false silenced=true"}
	disabled := []string{"hs_a disabled=true silenced=false", "hs_b disabled=true silenced=false"}
	return healthCase{
		conf: healthDcAlert("hs_a", 50) + healthDcAlert("hs_b", 90),
		sc:   healthValues(healthDcChart, healthDcContext, []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70}),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			transcript := h.calls(t)
			h.compareNow(t, "at 10: /api/v1/alarms?all", h.all, healthAll("CLEAR", "hs_a", "hs_b"))
			h.manageStep(t, healthSaved("cmd=SILENCE ALL", healthMsgSilenceAll, J(true, "SILENCE")).with(silenced...))
			h.release(t, "p1", 1, healthCalcHold)
			raised := map[string]string{"hs_a": "WARNING", "hs_b": "CLEAR"}
			h.compareNow(t, "silenced, at 70: /api/v1/alarms?all", h.all, healthWant(raised))
			h.processed(t, "silenced, at 70", "hs_a", "WARNING")
			h.compareNow(t, "silenced, at 70: the notifier's calls", transcript, healthCallsWant(0))

			time.Sleep(healthUnlinkHold)
			h.reload(t, "the reload with every alert silenced")
			h.compareNow(t, "silenced, reloaded: the transitions", h.changes, healthBoth(healthReloaded("hs_a", "WARNING", "WARNING"),
				healthReloaded("hs_b", "CLEAR", "CLEAR")))
			h.compareNow(t, "silenced, reloaded: /api/v1/alarms?all", h.all, healthWant(raised))
			h.compareNow(t, "silenced, reloaded: the alerts' flags", h.flags, healthFlagsWant(silenced...))
			h.processed(t, "silenced, reloaded", "hs_a", "WARNING")
			h.compareNow(t, "silenced, reloaded: the notifier's calls", transcript, healthCallsWant(0))

			time.Sleep(healthUnlinkHold)
			h.manageStep(t, healthSaved("cmd=DISABLE&alarm=hs_b", healthMsgDisable+healthMsgAdded, J(true, "DISABLE", healthSel{"alarm", "hs_b"})).with(disabled...))
			time.Sleep(healthUnlinkHold)
			h.reload(t, "the reload with every alert disabled")
			h.compareNow(t, "disabled, reloaded: the transitions", h.changes, healthBoth(healthLastChanges("hs_a", "WARNING->REMOVED", "REMOVED->UNINITIALIZED"),
				healthLastChanges("hs_b", "CLEAR->REMOVED", "REMOVED->UNINITIALIZED")))
			h.compareNow(t, "disabled, reloaded: the alerts' flags", h.flags, healthFlagsWant(disabled...))
			h.compareNow(t, "disabled, reloaded: /api/v1/alarms?all", h.all, healthAll("UNINITIALIZED", "hs_a", "hs_b"))
			h.compareNow(t, "disabled, reloaded: the silencers", func(i int) string { return h.manage(i, healthManagePath+"?cmd=LIST", "X-Auth-Token: "+h.token(i)) },
				healthHas(`"all": true`, `"type": "DISABLE"`, `"alarm": "hs_b"`))
			h.compareNow(t, "at the end: the notifier's calls", transcript, healthCallsWant(0))
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthHolds())
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCommandRecords(t, 2, map[string]int{"level=info … thread=UV_WORKER[n] " + healthRecCommand: 2})
			// the file's read at the start, the two requests' writes, and HEALTH's record of every alert whose flags a
			// pass changed: an alert a reload linked again starts without flags
			h.compareLines(t, "the records about the silencers and the key", func(i int) []string { return h.silencerRecords(t, i) },
				healthRecordsWant(map[string]int{`msg="Cannot open the file `: 1, `msg="Silencer changes written to `: 2,
					`': Disabled false->false Silenced false->true"`: 4, `': Disabled false->true Silenced true->false"`: 2,
					`': Disabled false->true Silenced false->false"`: 2}))
		},
	}
}

// healthReloadOffChildCase is `off-child`: the parent's health is off and the child's section turns the child's on
// (the rail takes it: the health runner names the stub with health off too). The child's health runs with no rule,
// the parent loaded none at its start; a reload loads them, and the child's alert is linked, raised and notified,
// while localhost's health stays off (health/health.c:215-218, health_prototypes.c:706-739). Then the child leaves.
func healthReloadOffChildCase() healthCase {
	const name = "hch_calc"
	return healthCase{
		off:    true,
		conf:   healthChildConf,
		stream: healthChildSection("health enabled = yes", "postpone alerts on connect = 0"),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			v := h.childViews(t)
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			first := child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70})
			log := func(i int) string { return h.getAs(v.cn[i], i, healthChildBase+"/api/v1/alarm_log") }
			// two passes of a health with rules would have raised the alert by now
			time.Sleep(healthCalcHold)
			h.compareNow(t, "before: the child's /api/v1/alarms?all", v.all, healthBoth(healthWant(map[string]string{}), healthHostOn))
			h.compareNow(t, "before: the child's /api/v1/alarm_log", log, healthIs(healthLogEmpty))
			h.compareNow(t, "before: /api/v1/alarms?all", h.all, healthHostOff)

			h.reload(t, "the reload")
			v.settle(first)
			h.compareNow(t, "reloaded: the child's /api/v1/alarms?all", v.all, healthBoth(healthAll("WARNING", name), healthHostOn))
			h.compareNow(t, "reloaded: the child's alert log", v.changes, healthSequenceIs(name, "REMOVED", "UNINITIALIZED", "WARNING"))
			v.processed("reloaded", name, "WARNING")
			h.compareNow(t, "reloaded: the notifier's calls", v.calls, healthChildCalls(name, "WARNING"))
			h.compareNow(t, "reloaded: /api/v1/alarms?all", h.all, healthHostOff)
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected", v.changes,
				healthSequenceIs(name, "REMOVED", "UNINITIALIZED", "WARNING", "REMOVED"))
			h.compareNow(t, "the child's /api/v1/alarms?all after it disconnected", v.all, healthHostOff)
			h.compareNow(t, "the notifier's calls after the child disconnected", v.calls, healthChildCalls(name, "WARNING"))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCommandRecords(t, 1, map[string]int{"level=info … thread=UV_WORKER[n] " + healthRecCommand: 1})
		},
	}
}

// healthReloadChildConf are the `child` case's rules: an alarm on localhost's chart and the template on the child's
// context.
var healthReloadChildConf = healthDcAlert("hr_local", 50) + healthChildConf

// healthReloadChildCase is `child`: localhost's chart and a streaming child's, each with an alert raised. A reload
// with the child connected removes and links again both hosts' alerts (health_prototypes.c:733-739 walks every host);
// then the child disconnects, and a second reload passes its host by (its health is off: :707).
func healthReloadChildCase() healthCase {
	const local, name = "hr_local", "hch_calc"
	return healthCase{
		conf:   healthReloadChildConf,
		stream: healthChildSection("health enabled = yes", "postpone alerts on connect = 0"),
		sc:     healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			v := h.childViews(t)
			h.create(t)
			// localhost's alert has its first status and two more passes behind it before the child connects: the
			// unlink that connection brings (healthParentRelink) carries the value of the pass before the last, which
			// is null after one pass alone (the two C sides differed there when the child connected at once)
			h.compareNow(t, "before the child: the transitions", h.changes, healthBoth(healthChangesOf(local, 4), healthLastChange(local, "UNINITIALIZED->WARNING")))
			time.Sleep(healthUnlinkHold)
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			v.settle(child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70}))
			mine := func(i int) string {
				return healthCallsSinceLink(strings.Join(h.n[i].callsOf(t, h.p.Each()[i].Daemon, parentIdentity.MachineGUID), "\n"))
			}
			localCalls := healthHostCalls(parentIdentity.Hostname, parentIdentity.MachineGUID, local, "WARNING")
			ids := healthAlarmIDs(h.compareNow(t, "at the start: /api/v1/alarms?all", h.all, healthAll("WARNING", local)))
			childIDs := healthAlarmIDs(h.compareNow(t, "at the start: the child's /api/v1/alarms?all", v.all, healthAll("WARNING", name)))
			h.processed(t, "at the start", local, "WARNING")
			v.processed("at the start", name, "WARNING")
			// localhost's alert: its first pass's four entries, and the three of the child's connection (healthParentRelink)
			entries := 4 + healthParentRelink
			h.compareNow(t, "at the start: the transitions", h.changes, healthBoth(healthReloaded(local, "WARNING", "WARNING"), healthChangesOf(local, entries)))
			h.compareNow(t, "at the start: the notifier's calls for localhost", mine, localCalls)
			h.compareNow(t, "at the start: the notifier's calls for the child", v.calls, healthChildCalls(name, "WARNING"))

			time.Sleep(healthUnlinkHold)
			h.reload(t, "the reload with the child connected")
			h.compareNow(t, "reloaded: the transitions", h.changes, healthBoth(healthReloaded(local, "WARNING", "WARNING"), healthChangesOf(local, entries+3)))
			h.compareNow(t, "reloaded: the child's alert log", v.changes, healthBoth(healthReloaded(name, "WARNING", "WARNING"), healthChangesOf(name, 2+3)))
			h.compareNow(t, "reloaded: /api/v1/alarms?all", h.all, healthBoth(healthAll("WARNING", local), healthIDsAre(ids)))
			h.compareNow(t, "reloaded: the child's /api/v1/alarms?all", v.all, healthBoth(healthAll("WARNING", name), healthIDsAre(childIDs)))
			h.processed(t, "reloaded", local, "WARNING")
			v.processed("reloaded", name, "WARNING")
			h.compareNow(t, "reloaded: the notifier's calls for localhost", mine, localCalls)
			h.compareNow(t, "reloaded: the notifier's calls for the child", v.calls, healthChildCalls(name, "WARNING"))

			// the child leaves; the next reload reaches localhost alone
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected", v.changes, healthBoth(healthLastChange(name, "WARNING->REMOVED"), healthChangesOf(name, 2+3+1)))
			h.compareNow(t, "the child's /api/v1/alarms?all after it disconnected", v.all, healthHostOff)
			// localhost is no parent any more: its alert was linked again (healthParentRelink)
			h.compareNow(t, "the child disconnected: the transitions", h.changes, healthBoth(healthReloaded(local, "WARNING", "WARNING"),
				healthChangesOf(local, entries+3+healthParentRelink)))
			time.Sleep(healthUnlinkHold)
			h.reload(t, "the reload with the child gone")
			h.compareNow(t, "reloaded again: the transitions", h.changes, healthBoth(healthReloaded(local, "WARNING", "WARNING"),
				healthChangesOf(local, entries+3+healthParentRelink+3)))
			h.compareNow(t, "reloaded again: the child's alert log", v.changes, healthBoth(healthLastChange(name, "WARNING->REMOVED"), healthChangesOf(name, 2+3+1)))
			h.compareNow(t, "reloaded again: the child's /api/v1/alarms?all", v.all, healthHostOff)
			h.processed(t, "reloaded again", local, "WARNING")
			h.compareNow(t, "reloaded again: the notifier's calls for localhost", mine, localCalls)
			h.compareNow(t, "reloaded again: the notifier's calls for the child", v.calls, healthChildCalls(name, "WARNING"))
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthLogEntries(entries+3+healthParentRelink+3))
			h.compareNow(t, "at the end: the child's /api/v1/alarm_log", v.log, healthLogEntries(2+3+1))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCommandRecords(t, 2, map[string]int{"level=info … thread=UV_WORKER[n] " + healthRecCommand: 2})
		},
	}
}

// The units of the reload and child cases' helpers: the rule rail of a rewritten file, the directories' keys of a
// `/netdata.conf` dump, health's nodes in a tree, the alerts' ids, and the guards on the transitions, on a host whose
// health is off and on one host's calls.
func TestHealthReloadNorm(t *testing.T) {
	const run = "/ndt/parity-oracle-1"
	// a rewritten rule file: `{run}` is the side's directory, and a rule's own notifier stays under its notifier
	// directory
	for name, c := range map[string]struct {
		conf string
		ok   bool
	}{
		"no exec":               {healthDcAlert("a", 1), true},
		"the side's stub":       {healthDcAlert("a", 1) + "  exec: {run}/notify/stub\n", true},
		"another program":       {healthDcAlert("a", 1) + "  exec: /bin/true\n", false},
		"the other side's stub": {healthDcAlert("a", 1) + "  exec: /ndt/parity-candidate-2/notify/stub\n", false},
	} {
		text, err := healthRulesFor(run, c.conf)
		if (err == nil) != c.ok {
			t.Errorf("the rule rail, %s: %v, want written: %v", name, err, c.ok)
		}
		if err == nil && strings.Contains(text, "{run}") || err != nil && text != "" {
			t.Errorf("the rule rail, %s: the text is %q", name, text)
		}
	}

	// the directories' keys: only the [directories] section's, a default as its comment line
	conf := "[global]\n\t# health config = x\n\n[directories]\n\t# config = " + run + "/etc\n\t# stock health config = /usr/lib/netdata/conf.d/health.d\n" +
		"\thealth config = " + run + "/etc/other.d\n\n[health]\n\t# enabled = no\n\t# enable stock health configuration = yes\n"
	if got, want := healthDirLines(conf, run), []string{"# stock health config = /usr/lib/netdata/conf.d/health.d", "health config = {run}/etc/other.d"}; !slices.Equal(got, want) {
		t.Errorf("the directories' keys: %q, want %q", got, want)
	}
	if got := healthDirLines("[directories]\n\t# config = /x\n", run); got != nil {
		t.Errorf("a dump without the keys: %q", got)
	}
	if err := healthDirsAre()("HTTP 200, text/plain; charset=utf-8\n"); err != nil {
		t.Errorf("no key: %v", err)
	}
	if err := healthDirsAre("# health config = {run}/etc/health.d")("HTTP 200, text/plain; charset=utf-8\n"); err == nil {
		t.Error("a missing key passes")
	}

	// health's nodes in a tree, in its order
	tree := "HTTP 200, x\n" + `{"version":1,"tree":{"/health/alerts/prototypes":{"health:alert:prototype:b":{"type":"job","template":"health:alert:prototype",` +
		`"status":"disabled","cmds":["get"],"source":"a \"status\":\"x\" in a text"},"health:alert:prototype":{"type":"template","status":"accepted"},` +
		`"health:alert:prototype:a":{"type":"job","status":"running"}}}}`
	nodes := []string{"health:alert:prototype:b disabled", "health:alert:prototype accepted", "health:alert:prototype:a running"}
	if got := healthNodeStatuses(tree); !slices.Equal(got, nodes) {
		t.Errorf("the nodes: %q, want %q", got, nodes)
	}
	if err := healthNodesAre(nodes...)(tree); err != nil {
		t.Errorf("the nodes' guard: %v", err)
	}
	if err := healthNodesAre(nodes[1], nodes[0], nodes[2])(tree); err == nil {
		t.Error("the nodes in another order pass")
	}
	if err := healthNodesAre(nodes[:2]...)(tree); err == nil {
		t.Error("a node more passes")
	}
	if err := healthNodesAre()("HTTP 404, x\n"); err == nil {
		t.Error("an answer that is no tree passes")
	}

	// the alerts' ids in a view of /api/v1/alarms?all
	alarms := "HTTP 200, x\n{\n\t\"hostname\": \"h\",\n\t\"latest_alarm_log_unique_id\": u+9,\n\t\"status\": true,\n\t\"now\": T,\n\t\"alarms\": {\n" +
		"\t\t\"c.a\": {\n\t\t\t\"id\": a+1,\n\t\t\t\"config_hash_id\": \"x\",\n\t\t\t\"name\": \"one\",\n\t\t\t\"status\": \"CLEAR\"\n\t\t},\n" +
		"\t\t\"c.b\": {\n\t\t\t\"id\": a+3,\n\t\t\t\"config_hash_id\": \"y\",\n\t\t\t\"name\": \"two\",\n\t\t\t\"status\": \"WARNING\"\n\t\t}\n\t}\n}\n"
	if got := healthAlarmIDs(alarms); len(got) != 2 || got["one"] != "a+1" || got["two"] != "a+3" {
		t.Errorf("the ids: %v", got)
	}
	for name, c := range map[string]struct {
		want map[string]string
		ok   bool
	}{
		"both":            {map[string]string{"one": "a+1", "two": "a+3"}, true},
		"one of them":     {map[string]string{"two": "a+3"}, true},
		"another id":      {map[string]string{"one": "a+2"}, false},
		"an alert gone":   {map[string]string{"three": "a+4"}, false},
		"an id not known": {map[string]string{"one": ""}, false},
		"nothing to hold": {map[string]string{}, false},
	} {
		if err := healthIDsAre(c.want)(alarms); (err == nil) != c.ok {
			t.Errorf("the ids' guard, %s: %v, want passed: %v", name, err, c.ok)
		}
	}
	if err := healthAlarmsOrder("one", "two")(alarms); err != nil {
		t.Errorf("the alerts' order: %v", err)
	}
	if err := healthAlarmsOrder("two", "one")(alarms); err == nil {
		t.Error("the alerts in another order pass")
	}
	if err := healthAlarmsOrder("one")(alarms); err == nil {
		t.Error("an alert more passes for the order")
	}
	if err := healthHostOn(alarms); err != nil {
		t.Errorf("a host whose health is on: %v", err)
	}
	if err := healthHostOff(alarms); err == nil {
		t.Error("a host with alerts passes for one whose health is off")
	}
	off := "HTTP 200, x\n{\n\t\"hostname\": \"h\",\n\t\"latest_alarm_log_unique_id\": u+9,\n\t\"status\": false,\n\t\"now\": T,\n\t\"alarms\": {\n\n\t}\n}\n"
	if err := healthHostOff(off); err != nil {
		t.Errorf("a host whose health is off: %v", err)
	}
	if err := healthHostOn(off); err == nil {
		t.Error("a host whose health is off passes for one whose health is on")
	}
	if err := healthHostOff(strings.Replace(off, "false", "true", 1)); err == nil {
		t.Error("a host whose health is on, without alerts, passes for one whose health is off")
	}
	if err := healthHostOff(strings.Replace(off, "\"alarms\": {\n\n\t}", "\"alarms\": {\n\t\t\"c.a\": {\n\t\t\t\"id\": a+1\n\t\t}\n\t}", 1)); err == nil {
		t.Error("a host whose health is off and that lists an alert passes")
	}

	// the transitions' guards
	changes := strings.Join([]string{"a: REMOVED->UNINITIALIZED -", "a: UNINITIALIZED->WARNING 70 things", "a: WARNING->REMOVED 70 things",
		"a: REMOVED->UNINITIALIZED -", "a: UNINITIALIZED->CLEAR 70 things", "ab: REMOVED->UNINITIALIZED -", "ab: UNINITIALIZED->CLEAR 70 things"}, "\n")
	for name, c := range map[string]struct {
		guard func(string) error
		ok    bool
	}{
		"reloaded":                     {healthReloaded("a", "WARNING", "CLEAR"), true},
		"reloaded to another status":   {healthReloaded("a", "WARNING", "WARNING"), false},
		"reloaded from another status": {healthReloaded("a", "CLEAR", "CLEAR"), false},
		"the last two":                 {healthLastChanges("a", "REMOVED->UNINITIALIZED", "UNINITIALIZED->CLEAR"), true},
		"all of them and one more":     {healthLastChanges("ab", "X->Y", "REMOVED->UNINITIALIZED", "UNINITIALIZED->CLEAR"), false},
		"another alert's":              {healthLastChanges("ab", "WARNING->REMOVED", "REMOVED->UNINITIALIZED", "UNINITIALIZED->CLEAR"), false},
		"an alert not there":           {healthLastChanges("b", "REMOVED->UNINITIALIZED"), false},
		"five entries":                 {healthChangesOf("a", 5), true},
		"a name that begins another":   {healthChangesOf("ab", 2), true},
		"six entries":                  {healthChangesOf("a", 6), false},
		"no entry":                     {healthChangesOf("b", 0), true},
		"the sequence":                 {healthSequenceIs("a", "REMOVED", "UNINITIALIZED", "WARNING", "REMOVED", "UNINITIALIZED", "CLEAR"), true},
		"a shorter sequence":           {healthSequenceIs("a", "REMOVED", "UNINITIALIZED", "WARNING"), false},
	} {
		if err := c.guard(changes); (err == nil) != c.ok {
			t.Errorf("the transitions' guard, %s: %v, want passed: %v", name, err, c.ok)
		}
	}

	// one host's calls: each for its status, in turn, naming the host
	call := func(k int, host, guid, status string) string {
		return fmt.Sprintf("call %[1]d: argv[%[2]d]=%[6]s\ncall %[1]d: argv[%[3]d]=hch_calc\ncall %[1]d: argv[%[4]d]=%[8]s\ncall %[1]d: argv[%[5]d]=%[7]s\ncall %[1]d: end exit 0",
			k, healthArgHost, notify.ArgName, notify.ArgStatus, healthArgGUID, host, guid, status)
	}
	two := call(1, healthChild.Hostname, healthChild.MachineGUID, "WARNING") + "\n" + call(2, healthChild.Hostname, healthChild.MachineGUID, "CLEAR")
	for name, c := range map[string]struct {
		transcript string
		statuses   []string
		ok         bool
	}{
		"two calls":           {two, []string{"WARNING", "CLEAR"}, true},
		"in the other order":  {two, []string{"CLEAR", "WARNING"}, false},
		"one call more":       {two, []string{"WARNING"}, false},
		"one call less":       {two, []string{"WARNING", "CLEAR", "WARNING"}, false},
		"none, and none":      {"", nil, true},
		"another host's name": {call(1, "parity-parent", healthChild.MachineGUID, "WARNING"), []string{"WARNING"}, false},
		"another host's GUID": {call(1, healthChild.Hostname, parentIdentity.MachineGUID, "WARNING"), []string{"WARNING"}, false},
	} {
		if err := healthChildCalls("hch_calc", c.statuses...)(c.transcript); (err == nil) != c.ok {
			t.Errorf("a host's calls, %s: %v, want passed: %v", name, err, c.ok)
		}
	}

	// the child's section: the lines under its machine GUID
	if got, want := healthChildSection("health enabled = auto", "postpone alerts on connect = 0"),
		"\n["+healthChild.MachineGUID+"]\n    type = machine\n    health enabled = auto\n    postpone alerts on connect = 0\n"; got != want {
		t.Errorf("the child's section: %q, want %q", got, want)
	}

	// the commands' records: the reload's, write-config's two and the signal's, normalized
	lines := []string{
		`time=2026-10-05T07:00:00.123+00:00 comm=netdata source=daemon level=info tid=123 thread=UV_WORKER[3] msg="COMMAND: Reloading HEALTH configuration."`,
		`time=2026-10-05T07:00:00.123+00:00 comm=netdata source=daemon level=info tid=5 msg="SIGNAL: Received SIGUSR2. Reloading HEALTH configuration..."`,
		`time=2026-10-05T07:00:00.123+00:00 comm=netdata source=daemon level=info tid=123 thread=UV_WORKER[1] msg="write-config netdata|directories|health config|` + run + `/etc/other.d"`,
		`time=2026-10-05T07:00:00.123+00:00 comm=netdata source=daemon level=info tid=123 thread=HEALTH msg="another record"`,
	}
	got := commandRecordsOf(lines, run)
	want := []string{
		`time=T comm=netdata source=daemon level=info tid=N thread=UV_WORKER[n] msg="COMMAND: Reloading HEALTH configuration."`,
		`time=T comm=netdata source=daemon level=info tid=N msg="SIGNAL: Received SIGUSR2. Reloading HEALTH configuration..."`,
		`time=T comm=netdata source=daemon level=info tid=N thread=UV_WORKER[n] msg="write-config netdata|directories|health config|<RUN>/etc/other.d"`,
	}
	if !slices.Equal(got, want) {
		t.Errorf("the commands' records:\n%s\nwant\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	if got, want := healthFirstDifference("a\nb\nc", "a\nx"), `3 lines on the oracle, 2 on the candidate; line 2 is "b" on the oracle, "x" on the candidate`; got != want {
		t.Errorf("the first difference: %s, want %s", got, want)
	}
	if got, want := healthFirstDifference("a\nb", "a"), `2 lines on the oracle, 1 on the candidate; line 2 is "b" on the oracle, (no such line) on the candidate`; got != want {
		t.Errorf("the first difference at an end: %s, want %s", got, want)
	}
}
