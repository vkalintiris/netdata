// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The cases of `health.dyncfg` that change an agent through health's DynCfg nodes (M9 commit 8, D212 F9; plan
// evidence/2026-10-05-plan-m9-commit8.md §6.3). Each request is a step (healthPair.cfgStep): sent once to each side
// as the admin, with a transaction of its own, the two answers compared whole (status line, headers, body). What a
// request left is compared after it: the tree under /health, `/api/v1/alarms?all`, the alert log's transitions, the
// saved files; and, both stopped, the DynCfg records.

// The chart of the DynCfg cases that evaluate an alert: one dimension, 70 for the whole run.
const (
	healthDcChart   = "hdc.values"
	healthDcContext = "hdc.ctx"
)

// healthDcScenario is the fake plugin's scenario of such a case: the chart, created at the `create` release, with
// `a` at 70. With `again` it has a second start, for a hand-back case: the same chart, created at the `again`
// release.
func healthDcScenario(again bool) *plugin.Scenario {
	sc := healthValues(healthDcChart, healthDcContext, []string{"a"}, map[string]int64{"a": 70})
	if again {
		sc.Starts = append(sc.Starts, plugin.Start{Steps: []plugin.Step{{WaitFile: "again"},
			{Values: &plugin.Values{Chart: healthDcChart, Context: healthDcContext, Dims: []string{"a"},
				Phases: []plugin.Phase{{Set: map[string]int64{"a": 70}, Until: "never"}}}}}})
	}
	return sc
}

// healthUnlinkHold is how long a case waits, after an alert's status settled on both sides, before a command that
// unlinks the alert (disable, update, remove, the template's disable): the entry an unlink logs carries the seconds
// since the alert's last change of status and the value of its pass before the last (rrdcalc.c:340-357), and C
// against C a command sent within a second of the status gave 0 s and no old value on one side, a second and the
// value on the other. After three seconds both sides ran the alert twice at its value.
const healthUnlinkHold = 3 * time.Second

// healthStoreBound is how far apart, in seconds, the two sides may hold the time of one event in the cases that send
// commands to a running health (healthPair.bound), in place of the runner's two seconds. An alert a command linked
// gets its first status when the side's metadata thread stored the link's entry, in that thread's own round: its
// one-second timer finds the last store more than five seconds old (sqlite_metadata.c:221-222, :1716-1721,
// :2795-2798), then HEALTH's next pass runs the alert. The two sides' rounds are a second or two apart (their last
// stores followed their own HEALTH passes), so C against C a first status was 0 to 2 s apart side to side, where the
// runner fails "more than 2"; and a command that lands between the two sides' rounds puts them a round apart.
const healthStoreBound = 7

// healthSkipHold is how long a case waits between a command that queues an alert log entry (a disable, an add) and an
// `update` of a linked alert. The update unlinks the alert, asks the alert log's table for its alarm id (its rule's
// hash is another one: rrdcalc.c:130-169) and links it again, each with an entry; a pass of HEALTH whose notification
// scan falls between the two marks the unlink's entry processed and has the entries stored at once, so the alert's
// new status comes seconds before the other side's (seen C against C once in four runs: probes.md). A queued entry
// makes HEALTH skip the host from its next pass on, until the metadata thread's round stored it
// (health_event_loop.c:396-401): the pass that was running when the entry was queued has ended after this long.
const healthSkipHold = 150 * time.Millisecond

// healthDcAlert is a file's rule on the chart: the last value of `a`, WARNING above `warn`.
func healthDcAlert(name string, warn int) string {
	return fmt.Sprintf(" alarm: %s\n    on: %s\n  calc: $a\n every: 1s\n  warn: $this > %d\n units: things\n  info: the rule of %s in the file\n\n",
		name, healthDcChart, warn, name)
}

// healthDcRule is a payload's rule on the chart: the last value of `a`, WARNING above `warn`.
func healthDcRule(warn int, set ...any) map[string]any {
	return healthRuleDoc(healthDcChart, "$a", fmt.Sprintf("$this > %d", warn), set...)
}

// all, changes and log are the views of the alerts the DynCfg cases compare.
func (h *healthPair) all(i int) string     { return h.get(i, "/api/v1/alarms?all") }
func (h *healthPair) changes(i int) string { return h.transitions(i, "") }
func (h *healthPair) log(i int) string     { return h.logOf(h.n[i], i, "/api/v1/alarm_log") }

// logOf is log for the alert log at path (a child's: `/host/<name>/api/v1/alarm_log`), with its host's normalizer.
func (h *healthPair) logOf(n *healthNorm, i int, path string) string {
	view := h.getAs(n, i, path)
	return h.keep(i, healthReplaced(healthSinceLink(view)), h.raw[i][view])
}

// calls is the view of the notifier's transcript the DynCfg cases compare (healthCallsSinceLink).
func (h *healthPair) calls(t *testing.T) func(i int) string {
	transcript := h.transcript(t)
	return func(i int) string { return healthCallsSinceLink(transcript(i)) }
}

// compareSaved compares both sides' DynCfg directories.
func (h *healthPair) compareSaved(t *testing.T, at string, want map[string][]string) {
	t.Helper()
	h.compareLines(t, at+": the saved files", func(i int) []string { return h.saves(t, i) }, healthSavedAre(want))
}

// healthTemplateNode is the guard on the template's node of every tree of a running health: accepted, its five
// commands, C's default access (dyncfg.c:395-399).
var healthTemplateNode = healthNode(healthJobPrefix, `"type":"template","status":"accepted",`+healthCmdsTemplate,
	`"access":{"view":["signed-in","same-space","view-config"],"edit":["signed-in","same-space","commercial","edit-config"]}`,
	`"source_type":"internal","source":"internal","sync":true`)

// healthActionsConf are the `actions` case's rules: hd_one, which the case acts on; hd_two, a chain of a template
// and an alarm on a chart that does not exist; hd_three, whose job the case disables right before the update (its
// `info` text predates that).
const healthActionsConf = ` alarm: hd_one
    on: hdc.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: the alert the case acts on

template: hd_two
      on: hdc.ctx
    calc: $a
   every: 1s
    warn: $this > 90
   units: things
    info: the first rule of a chain, on the context

 alarm: hd_two
    on: hdc.absent
  calc: $a
 every: 1s
  warn: $this > 95
 units: things
  info: the second rule of a chain, on a chart that does not exist

 alarm: hd_three
    on: hdc.values
  calc: $a * 2
 every: 1s
  crit: $this > 500
 units: things
  info: an alert nothing is done to
`

// healthActionsCase (`actions`, plan E2): the commands of a rule file's job, one after the other, with the alert
// running: get, userconfig, disable twice, enable twice, update (right after another job's disable:
// healthSkipHold), get. After each command that changes the agent:
// the tree, the alerts, the alert log's transitions and the saved files. C unlinks the name's alerts and links
// them again on the web worker's thread (health_prototypes.c:668-702), so a disabled alert is gone at once and an
// enabled one is back at once, with its first status at HEALTH's next pass.
func healthActionsCase() healthCase {
	job, other := healthJob("hd_one"), healthJob("hd_three")
	base := map[string]string{"hd_two": "CLEAR", "hd_three": "CLEAR"}
	with := func(status string) map[string]string {
		m := maps.Clone(base)
		m["hd_one"] = status
		return m
	}
	return healthCase{
		conf: healthActionsConf,
		sc:   healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			transcript := h.calls(t)
			h.compareNow(t, "raised: /api/v1/alarms?all", h.all, healthWant(with("WARNING")))
			h.processed(t, "raised", "hd_one", "WARNING")
			h.compareNow(t, "raised: the notifier's calls", transcript, healthCallsWant(1,
				healthCallFor("hd_one", "WARNING", 1, nil), healthCallsEnded(0)))
			h.compareNow(t, "at the start: the tree", h.tree, healthBoth(healthTemplateNode,
				healthNode(job, `"status":"running",`+healthCmdsFile, `"source_type":"user"`, `"user_disabled":false`, `"saves":0`),
				healthNode(healthJob("hd_two"), `"status":"running"`), healthNode(healthJob("hd_three"), `"status":"running"`)))
			h.compareSaved(t, "at the start", map[string][]string{})

			time.Sleep(healthUnlinkHold)
			h.cfgSteps(t,
				healthCfgStep{label: "get", query: "action=get&id=" + job, code: 200,
					parts: []string{`"name":"hd_one"`, `"source_type":"user"`, `"warning_condition":"$this > 50"`}},
				healthCfgStep{label: "userconfig", query: "action=userconfig&id=" + job + "&name=hd_one", body: healthPayload(healthDcRule(80)),
					code: 200, parts: []string{"Content-Type: text/plain", "        alarm: hd_one\n", "         warn: $this > 80\n"}},
				healthCfgStep{label: "disable", query: "action=disable&id=" + job, code: 200, parts: []string{healthMsg(200, "disabled")}})
			h.compareNow(t, "disabled: the tree", h.tree, healthNode(job, `"status":"disabled",`+healthCmdsFile, `"user_disabled":true`, `"saves":1`))
			h.compareNow(t, "disabled: /api/v1/alarms?all", h.all, healthWant(base))
			h.compareNow(t, "disabled: the transitions", h.changes, healthLastChange("hd_one", "WARNING->REMOVED"))
			h.compareSaved(t, "disabled", map[string][]string{job: {`"user_disabled=true\n"`, `"saves=1\n"`, `"cmds=` + healthSavedFileCmds + `\n"`, "!---"}})

			h.cfgStep(t, healthCfgStep{label: "disable again", query: "action=disable&id=" + job, code: 200,
				parts: []string{healthMsg(200, "already disabled")}})
			h.compareNow(t, "disabled again: the tree", h.tree, healthNode(job, `"status":"disabled"`, `"user_disabled":true`, `"saves":1`))
			h.compareSaved(t, "disabled again", map[string][]string{job: {`"user_disabled=true\n"`, `"saves=1\n"`}})

			h.cfgStep(t, healthCfgStep{label: "enable", query: "action=enable&id=" + job, code: 202, parts: []string{healthMsg(202, "enabled")}})
			h.compareNow(t, "enabled: the tree", h.tree, healthNode(job, `"status":"accepted",`+healthCmdsFile, `"user_disabled":false`, `"saves":2`))
			h.compareNow(t, "enabled: /api/v1/alarms?all", h.all, healthWant(with("WARNING")))
			h.compareNow(t, "enabled: the transitions", h.changes, healthLastChange("hd_one", "UNINITIALIZED->WARNING"))
			h.compareSaved(t, "enabled", map[string][]string{job: {`"user_disabled=false\n"`, `"saves=2\n"`, "!---"}})
			h.processed(t, "enabled", "hd_one", "WARNING")
			// WARNING is the status the alert was last notified for: no second call (health_notifications.c:414-425)
			h.compareNow(t, "enabled: the notifier's calls", transcript, healthCallsWant(1))

			h.cfgStep(t, healthCfgStep{label: "enable again", query: "action=enable&id=" + job, code: 200,
				parts: []string{healthMsg(200, "already enabled")}})
			h.compareNow(t, "enabled again: the tree", h.tree, healthNode(job, `"status":"accepted"`, `"saves":2`))

			// the update, right after another job's disable (healthSkipHold)
			time.Sleep(healthUnlinkHold)
			h.cfgStep(t, healthCfgStep{label: "disable another job", query: "action=disable&id=" + other, code: 200,
				parts: []string{healthMsg(200, "disabled")}})
			time.Sleep(healthSkipHold)
			h.cfgStep(t, healthCfgStep{label: "update", query: "action=update&id=" + job, body: healthPayload(healthDcRule(80)),
				code: 202, parts: []string{healthMsg(202, "updated")}})
			h.compareNow(t, "updated: the tree", h.tree, healthBoth(healthNode(job, `"status":"accepted",`+healthCmdsDynCfg, `"source_type":"dyncfg"`,
				`"payload":{"available":true`, `"saves":3`), healthNode(other, `"status":"disabled",`+healthCmdsFile, `"user_disabled":true`, `"saves":1`)))
			h.compareNow(t, "updated: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hd_one": "CLEAR", "hd_two": "CLEAR"}))
			h.compareNow(t, "updated: the transitions", h.changes, healthBoth(healthLastChange("hd_one", "UNINITIALIZED->CLEAR"),
				healthLastChange("hd_three", "CLEAR->REMOVED")))
			h.compareSaved(t, "updated", map[string][]string{job: {`"saves=3\n"`, `"source_type=dyncfg\n"`, `"cmds=` + healthSavedDynCfgCmds + `\n"`, `"---\n"`,
				`$this > 80`}, other: {`"user_disabled=true\n"`, `"saves=1\n"`, "!---"}})
			h.processed(t, "updated", "hd_one", "CLEAR")
			// the alert's last notified status is WARNING: its CLEAR is notified, though it comes from UNINITIALIZED
			h.compareNow(t, "updated: the notifier's calls", transcript, healthCallsWant(2,
				healthCallFor("hd_one", "CLEAR", 1, nil), healthCallsEnded(0)))

			h.cfgStep(t, healthCfgStep{label: "get after the update", query: "action=get&id=" + job, code: 200,
				parts: []string{`"source_type":"dyncfg","source":""`, `"warning_condition":"$this > 80"`}})
			// the first pass's three links and the first status of each of the three alerts; then, for hd_one, the
			// disable's unlink, the enable's link and its status, the update's unlink, its link and its status; and
			// hd_three's unlink
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthLogEntries(3*4+6+1))
		},
		after: func(t *testing.T, h *healthPair) {
			// the core logs a user's action whatever health answered: `already disabled` and `already enabled` too
			h.compareCfgRecords(t, healthRecordsCount(6, map[string]int{
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + other + "' by user 'fnhttp-admin', IP 'localhost'\"": 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + job + "' by user 'fnhttp-admin', IP 'localhost'\"":   2,
				"level=notice … msg=\"DYNCFG USER ACTION 'enable' on job '" + job + "' by user 'fnhttp-admin', IP 'localhost'\"":    2,
				"level=notice … msg=\"DYNCFG USER ACTION 'update' on job '" + job + "' by user 'fnhttp-admin', IP 'localhost'\"":    1,
			}))
			h.compareLines(t, "health.log", func(i int) []string {
				lines := h.n[i].healthLog(t, h.p.Each()[i].Daemon)
				for k, l := range lines {
					lines[k] = healthLogSinceLink(l)
				}
				return lines
			},
				func(oracle []string) error {
					if len(oracle) == 0 {
						return fmt.Errorf("no record")
					}
					return nil
				})
		},
	}
}

// healthAddRemoveCase (`add-remove`, plan E3): a rule added through the template with the alert really evaluating:
// it rises, is notified once, is updated to a threshold the value does not cross (CLEAR, notified once; the file's
// job is disabled right before: healthSkipHold), and is removed (the alert REMOVED, the file gone, the job gone). A job a user added in this session lists `test`, which
// health refuses (C's defect, kept).
func healthAddRemoveCase() healthCase {
	const name = "hn_new"
	job, base := healthJob(name), healthJob("hn_base")
	return healthCase{
		conf: healthDcAlert("hn_base", 90),
		sc:   healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			transcript := h.calls(t)
			h.compareNow(t, "at the start: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hn_base": "CLEAR"}))
			h.compareNow(t, "at the start: the tree", h.tree, healthBoth(healthTemplateNode, healthNoNode(job)))

			h.cfgStep(t, healthCfgStep{label: "add", query: "action=add&id=" + healthJobPrefix + "&name=" + name,
				body: healthPayload(healthDcRule(50)), code: 202, parts: []string{healthMsg(202, "accepted")}})
			h.compareNow(t, "added: the tree", h.tree, healthNode(job, `"type":"job","template":"`+healthJobPrefix+`","status":"accepted",`+healthCmdsAdded,
				`"source_type":"dyncfg","source":"`+healthSavedSource, `"user_disabled":false`, `"payload":{"available":true`, `"saves":1`))
			h.cfgStep(t, healthCfgStep{label: "get", query: "action=get&id=" + job, code: 200,
				parts: []string{`"name":"` + name + `"`, `"source_type":"dyncfg","source":""`, `"warning_condition":"$this > 50"`,
					`"execute":"{run}/notify/stub","recipient":"root"`}})
			h.compareNow(t, "added: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hn_base": "CLEAR", name: "WARNING"}))
			h.processed(t, "added", name, "WARNING")
			h.compareNow(t, "added: the notifier's calls", transcript, healthCallsWant(1,
				healthCallFor(name, "WARNING", 1, nil), healthCallsEnded(0)))
			h.compareSaved(t, "added", map[string][]string{job: {`"type=job\n"`, `"template=` + healthJobPrefix + `\n"`, `"source_type=dyncfg\n"`,
				`"source=` + healthSavedSource, `"sync=true\n"`, `"user_disabled=false\n"`, `"saves=1\n"`, `"cmds=` + healthSavedCmds + `\n"`,
				`"content_type=application/json\n"`, `"---\n"`, `$this > 50`}})

			// the job lists `test` (the core's set for a job a user added), and health refuses it
			h.cfgStep(t, healthCfgStep{label: "test", query: "action=test&id=" + job + "&name=" + name, body: healthPayload(healthDcRule(50)),
				code: 400, parts: []string{healthMsg(400, "action given is not supported for the prototype job")}})

			// the update, right after the file's job is disabled (healthSkipHold)
			time.Sleep(healthUnlinkHold)
			h.cfgStep(t, healthCfgStep{label: "disable the file's job", query: "action=disable&id=" + base, code: 200,
				parts: []string{healthMsg(200, "disabled")}})
			time.Sleep(healthSkipHold)
			h.cfgStep(t, healthCfgStep{label: "update", query: "action=update&id=" + job, body: healthPayload(healthDcRule(80)),
				code: 202, parts: []string{healthMsg(202, "updated")}})
			h.compareNow(t, "updated: the tree", h.tree, healthBoth(healthNode(job, `"status":"accepted",`+healthCmdsAdded, `"saves":2`),
				healthNode(base, `"status":"disabled",`+healthCmdsFile, `"user_disabled":true`, `"saves":1`)))
			h.compareNow(t, "updated: /api/v1/alarms?all", h.all, healthWant(map[string]string{name: "CLEAR"}))
			h.processed(t, "updated", name, "CLEAR")
			h.compareNow(t, "updated: the notifier's calls", transcript, healthCallsWant(2,
				healthCallFor(name, "WARNING", 1, nil), healthCallFor(name, "CLEAR", 1, nil), healthCallsEnded(0)))
			h.compareNow(t, "updated: the transitions", h.changes, healthBoth(healthLastChange(name, "UNINITIALIZED->CLEAR"),
				healthLastChange("hn_base", "CLEAR->REMOVED")))
			baseSaved := []string{`"user_disabled=true\n"`, `"saves=1\n"`, "!---"}
			h.compareSaved(t, "updated", map[string][]string{job: {`"saves=2\n"`, `"cmds=` + healthSavedCmds + `\n"`, `$this > 80`}, base: baseSaved})

			time.Sleep(healthUnlinkHold)
			h.cfgStep(t, healthCfgStep{label: "remove", query: "action=remove&id=" + job, code: 200, parts: []string{healthMsg(200, "deleted")}})
			h.compareNow(t, "removed: the tree", h.tree, healthBoth(healthTemplateNode, healthNoNode(job)))
			h.compareNow(t, "removed: /api/v1/alarms?all", h.all, healthWant(map[string]string{}))
			h.compareNow(t, "removed: the transitions", h.changes, healthLastChange(name, "CLEAR->REMOVED"))
			h.compareSaved(t, "removed", map[string][]string{base: baseSaved})
			// the job is gone, and the core takes the id for a new job of the template, which has no `get`
			h.cfgStep(t, healthCfgStep{label: "get after the remove", query: "action=get&id=" + job, code: 400,
				parts: []string{healthMsg(400, "dyncfg functions intercept: this command is not supported by this configuration node")}})
			// the file's alert: its three links, its status and its unlink; the added one: its link and WARNING, the
			// update's unlink, link and CLEAR, the remove's unlink
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthLogEntries(5+6))
			h.compareNow(t, "at the end: the notifier's calls", transcript, healthCallsWant(2))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCfgRecords(t, healthRecordsCount(6, map[string]int{
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + base + "' by user":                              1,
				"level=notice … msg=\"DYNCFG USER ACTION 'add' " + name + " on template '" + healthJobPrefix + "' by user":     1,
				"level=error … msg=\"DYNCFG: plugin returned code 400 to user initiated call: config " + job + " test " + name: 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'update' on job '" + job + "' by user":                                1,
				"level=notice … msg=\"DYNCFG USER ACTION 'remove' on job '" + job + "' by user":                                1,
				// the `get` of the removed job, which the core sent to the template
				"level=error … msg=\"DYNCFG: this command is not supported by the configuration node: config " + healthJobPrefix + " get\"": 1,
			}))
		},
	}
}

// healthRefusal is a payload health refuses, and the text of its refusal.
type healthRefusal struct {
	label, body, text string
}

// healthRefusedPayloads are payloads the parser refuses in an `add` and in an `update` (health_dyncfg.c:236-346,
// json-c-parser-inline.h), with the text of each refusal. Each is JSON to both agents but the two that are no
// document at all.
func healthRefusedPayloads() []healthRefusal {
	one := func(set ...any) string { return healthPayload(healthRuleDoc("hdc.values", "$a", "$this > 1", set...)) }
	const lookup = "config.value.database_lookup."
	return []healthRefusal{
		{"plain text", "not json", "failed to parse json payload: null expected"},
		{"a document cut short", `{"format_version":1,"rules":[`, "failed to parse json payload: continue"},
		{"version 2", `{"format_version":2,"rules":[]}`, "unsupported document version"},
		{"no version", `{"rules":[]}`, "missing '.format_version'"},
		{"no rules", `{"format_version":1}`, "the rules array is missing"},
		{"rules that are no array", `{"format_version":1,"rules":{}}`, "member 'rules' is not an array"},
		{"an empty rules array", `{"format_version":1,"rules":[]}`, "Item 0 has neither database lookup nor calculation"},
		{"a rule that is no object", `{"format_version":1,"rules":[5]}`, "missing '.enabled' boolean"},
		{"no enabled", one("enabled", nil), "missing '.enabled' boolean"},
		{"enabled maybe", one("enabled", "maybe"), "invalid boolean string 'maybe' for '.enabled'"},
		{"no type", one("type", nil), "missing '.type'"},
		{"a type that is neither", one("type", "bogus"), "type is 'bogus', but it can only be 'instance' or 'template'"},
		{"no config", one("config", nil), "missing '.config' object"},
		{"a config that is no object", one("config", 5), "not an object '.config'"},
		{"no value", one("config.value", nil), "missing 'config.value' object"},
		{"no lookup", one("config.value.database_lookup", nil), "missing 'config.value.database_lookup' object"},
		{"after past an int", one(lookup+"after", 2147483648), "value for 'config.value.database_lookup.after' is outside range [-2147483648, 2147483647]"},
		{"after that is a text", one(lookup+"after", "soon"), "cannot convert string 'soon' to int64 for 'config.value.database_lookup.after'"},
		{"no time_group", one(lookup+"time_group", nil), "missing 'config.value.database_lookup.time_group' enum"},
		{"an option that is no text", one(lookup+"options", []any{5}), "invalid type for 'config.value.database_lookup.options' at index 0"},
		{"an unknown option before a failure", one(lookup+"options", []any{"bogus"}, lookup+"dimensions", nil),
			"unknown option 'bogus' in 'config.value.database_lookup.options' at index 0missing 'config.value.database_lookup.dimensions'"},
		{"a percentile that is a text", one(lookup+"time_group", "percentile", lookup+"time_group_value", "abc"),
			"cannot convert string 'abc' to double for 'config.value.database_lookup.time_group_value'"},
		{"a calculation that cannot be parsed", one("config.value.calculation", "$a +"), "expression 'config.value.calculation' has a non-parseable expression '$a +': remaining characters after expression at '+'"},
		{"a negative update_every", one("config.value.update_every", -1), "negative value for 'config.value.update_every'"},
		{"an update_every past an int", one("config.value.update_every", 2147483648), "value for 'config.value.update_every' exceeds maximum 2147483647"},
		{"a warning that cannot be parsed", one("config.conditions.warning_condition", "$this >"),
			"expression 'config.conditions.warning_condition' has a non-parseable expression '$this >': remaining characters after expression at '>'"},
		{"a multiplier past a float", one("config.action.delay.up", 1, "config.action.delay.down", 1, "config.action.delay.max", 1,
			"config.action.delay.multiplier", 1e39, "config.action.options", []any{}, "config.action.execute", "", "config.action.recipient", ""),
			"non-finite or out-of-range value for 'config.action.delay.multiplier'"},
		{"a negative repeat", one("config.action.options", []any{}, "config.action.execute", "", "config.action.recipient", "",
			"config.action.delay.up", 0, "config.action.delay.down", 0, "config.action.delay.max", 0, "config.action.delay.multiplier", 1,
			"config.action.repeat.enabled", true, "config.action.repeat.warning", -1, "config.action.repeat.critical", 0),
			"value for 'config.action.repeat.warning' is outside range [0, 2147483647]"},
		{"no match", one("config.match", nil), "missing 'config.match' object"},
		{"an empty on", one("config.match.on", ""), "member 'config.match.on' cannot be empty"},
		{"neither lookup nor calculation", one("config.value.calculation", ""), "Item 0 has neither database lookup nor calculation"},
	}
}

// healthRefusalsConf are the `refusals` case's rules: a plain one, and one whose only rule is disabled (`host
// labels: !*` turns a rule off, health_config.c:541-544).
const healthRefusalsConf = ` alarm: hr_file
    on: hdc.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: a file's rule

 alarm: hr_off
    on: hdc.values
host labels: !*
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: a rule that is off
`

// healthRefusalsCase (`refusals`, plan E4): what health and the core refuse, each with its status and its exact text,
// and nothing saved. No chart: nothing is linked. The case's stock directory has no schema file, so a schema request
// reaches health (501). Then the names health takes with a record (an unknown grouping of dimensions, an unknown
// data source, an unknown condition) or silently (an unknown time grouping), and a rule added disabled (298).
func healthRefusalsCase() healthCase {
	file, off := healthJob("hr_file"), healthJob("hr_off")
	const intercept = "dyncfg functions intercept: "
	unsupported := healthMsg(400, intercept+"this command is not supported by this configuration node")
	add := func(name string) string { return "action=add&id=" + healthJobPrefix + "&name=" + name }
	refused := healthRefusedPayloads()
	// through `update` and `userconfig`: these of the table
	again := []int{0, 2, 11, 22}
	return healthCase{
		conf:     healthRefusalsConf,
		stockDir: true,
		play: func(t *testing.T, h *healthPair) {
			h.compareNow(t, "at the start: the tree", h.tree, healthBoth(healthTemplateNode,
				healthNode(file, `"status":"running",`+healthCmdsFile), healthNode(off, `"status":"disabled",`+healthCmdsFile, `"user_disabled":false`)))
			var steps []healthCfgStep
			for k, r := range refused {
				steps = append(steps, healthCfgStep{label: "add: " + r.label, query: add(fmt.Sprintf("hr_n%d", k)), body: r.body, code: 400,
					parts: []string{healthMsg(400, r.text)}})
			}
			for _, k := range again {
				r := refused[k]
				steps = append(steps,
					healthCfgStep{label: "update: " + r.label, query: "action=update&id=" + file, body: r.body, code: 400,
						parts: []string{healthMsg(400, r.text)}},
					healthCfgStep{label: "userconfig of the template: " + r.label, query: "action=userconfig&id=" + healthJobPrefix + "&name=hr_u", body: r.body,
						code: 400, parts: []string{healthMsg(400, r.text)}})
			}
			steps = append(steps,
				// health's validation, after the parser took the payload (health_prototypes.c:418-436)
				healthCfgStep{label: "add: update_every 0", query: add("hr_v0"), body: healthPayload(healthDcRule(1, "config.value.update_every", 0)),
					code: 400, parts: []string{healthMsg(400, "missing update frequency")}},
				healthCfgStep{label: "update: update_every 0", query: "action=update&id=" + file, body: healthPayload(healthDcRule(1, "config.value.update_every", 0)),
					code: 400, parts: []string{healthMsg(400, "missing update frequency")}},
				// the core, before health is asked (dyncfg-intercept.c:340-479)
				healthCfgStep{label: "add of a name that exists", query: add("hr_file"), body: healthPayload(healthDcRule(1)), code: 400,
					parts: []string{healthMsg(400, intercept+"a configuration with this name already exists")}},
				healthCfgStep{label: "add without a payload", query: add("hr_none"), code: 400,
					parts: []string{healthMsg(400, intercept+"this action requires a payload")}},
				healthCfgStep{label: "get with a payload", query: "action=get&id=" + file, body: healthPayload(healthDcRule(1)), code: 400,
					parts: []string{healthMsg(400, intercept+"this action does not require a payload")}},
				healthCfgStep{label: "remove of a file's job", query: "action=remove&id=" + file, code: 400, parts: []string{unsupported}},
				healthCfgStep{label: "restart of a job", query: "action=restart&id=" + file, code: 400, parts: []string{unsupported}},
				healthCfgStep{label: "add on a job", query: "action=add&id=" + file + "&name=x", body: healthPayload(healthDcRule(1)), code: 400,
					parts: []string{unsupported}},
				healthCfgStep{label: "test of a file's job", query: "action=test&id=" + file + "&name=x", body: healthPayload(healthDcRule(1)), code: 400,
					parts: []string{unsupported}},
				healthCfgStep{label: "get of the template", query: "action=get&id=" + healthJobPrefix, code: 400, parts: []string{unsupported}},
				healthCfgStep{label: "update of the template", query: "action=update&id=" + healthJobPrefix, body: healthPayload(healthDcRule(1)), code: 400,
					parts: []string{unsupported}},
				healthCfgStep{label: "test of the template", query: "action=test&id=" + healthJobPrefix + "&name=x", body: healthPayload(healthDcRule(1)),
					code: 400, parts: []string{unsupported}},
				healthCfgStep{label: "remove of the template", query: "action=remove&id=" + healthJobPrefix, code: 400, parts: []string{unsupported}},
				// a name with no enabled rule: registered disabled, and health refuses to enable it
				healthCfgStep{label: "enable of a name with no enabled rule", query: "action=enable&id=" + off, code: 400,
					parts: []string{healthMsg(400, "all rules in this alert are disabled, so enabling the alert has no effect")}},
				// health says it is disabled already; the core still takes the user's choice, and saves it (below)
				healthCfgStep{label: "disable of it", query: "action=disable&id=" + off, code: 200, parts: []string{healthMsg(200, "already disabled")}},
				// ids nothing registered: the core's catch-all takes a job id of the template for a new job's
				healthCfgStep{label: "get of an unknown name", query: "action=get&id=" + healthJob("hr_nope"), code: 400, parts: []string{unsupported}},
				healthCfgStep{label: "update of an unknown name", query: "action=update&id=" + healthJob("hr_nope"), body: healthPayload(healthDcRule(1)),
					code: 404, parts: []string{`"errorMessage":"Unknown config id given."`}},
				healthCfgStep{label: "get of another tree's id", query: "action=get&id=health:alert:nope", code: 404,
					parts: []string{`"errorMessage":"Unknown config id given."`}},
				healthCfgStep{label: "userconfig of a new job's id", query: "action=userconfig&id=" + healthJob("hr_new") + "&name=hr_new",
					body: healthPayload(healthDcRule(1)), code: 200, parts: []string{"Content-Type: text/plain", "        alarm: hr_new\n"}},
				healthCfgStep{label: "userconfig of the template", query: "action=userconfig&id=" + healthJobPrefix + "&name=hr_u",
					body: healthPayload(healthDcRule(1, "type", "template", "config.action.execute", "{run}/notify/absent", "config.action.recipient", "nobody")),
					code: 200, parts: []string{"Content-Type: text/plain", "     template: hr_u\n", "         exec: {run}/notify/absent\n", "           to: nobody\n"}},
				// no schema file in the user's or the case's stock directory: health is asked
				healthCfgStep{label: "the template's schema, no file", query: "action=schema&id=" + healthJobPrefix, code: 501,
					parts: []string{healthMsg(501, "schema not implemented yet for prototype templates")}},
				healthCfgStep{label: "a job's schema, no file", query: "action=schema&id=" + file, code: 501,
					parts: []string{healthMsg(501, "schema not implemented yet")}},
			)
			h.cfgSteps(t, steps...)
			// no refusal saved a file or added a node, and the file's rule is as it was; the one file is the disable's
			offSaved := []string{`"source_type=internal\n"`, `"user_disabled=true\n"`, `"saves=1\n"`, `"cmds=` + healthSavedFileCmds + `\n"`, "!---"}
			h.compareSaved(t, "after the refusals", map[string][]string{off: offSaved})
			h.compareNow(t, "after the refusals: the tree", h.tree, healthBoth(healthNode(file, `"status":"running"`, `"source_type":"user"`, `"saves":0`),
				healthNode(off, `"status":"disabled"`, `"user_disabled":true`, `"saves":1`),
				healthNoNode(healthJob("hr_n0"), healthJob("hr_v0"), healthJob("hr_new"), healthJob("hr_u"), healthJob("hr_nope"))))
			h.cfgStep(t, healthCfgStep{label: "get of the file's job", query: "action=get&id=" + file, code: 200,
				parts: []string{`"source_type":"user"`, `"warning_condition":"$this > 50"`}})

			// names health takes: an unknown grouping of dimensions and data source (the first of each, with a record),
			// an unknown condition of `countif` (`=`, with a record), an unknown time grouping (`average`, silently)
			const lookup = "config.value.database_lookup."
			h.cfgSteps(t,
				healthCfgStep{label: "add with unknown names", query: add("hr_odd"), code: 202, parts: []string{healthMsg(202, "accepted")},
					body: healthPayload(healthDcRule(1, lookup+"after", -10, lookup+"time_group", "countif", lookup+"time_group_condition", "~",
						lookup+"time_group_value", 5, lookup+"dims_group", "bogus", lookup+"data_source", "bogus"))},
				healthCfgStep{label: "get of it", query: "action=get&id=" + healthJob("hr_odd"), code: 200,
					parts: []string{`"after":-10,"before":0,"time_group":"countif","time_group_condition":"=","time_group_value":5,"dims_group":"sum","data_source":"samples"`}},
				healthCfgStep{label: "add with an unknown time grouping", query: add("hr_avg"), code: 202, parts: []string{healthMsg(202, "accepted")},
					body: healthPayload(healthDcRule(1, lookup+"after", -10, lookup+"time_group", "bogus"))},
				healthCfgStep{label: "get of it", query: "action=get&id=" + healthJob("hr_avg"), code: 200, parts: []string{`"time_group":"average"`}},
				// a rule that is not enabled: accepted, 298
				healthCfgStep{label: "add of a rule that is off", query: add("hr_298"), code: 298, parts: []string{healthMsg(298, "accepted")},
					body: healthPayload(healthDcRule(1, "enabled", false))},
				healthCfgStep{label: "enable of it", query: "action=enable&id=" + healthJob("hr_298"), code: 400,
					parts: []string{healthMsg(400, "all rules in this alert are disabled, so enabling the alert has no effect")}},
			)
			h.compareNow(t, "at the end: the tree", h.tree, healthBoth(healthNode(healthJob("hr_odd"), `"status":"accepted",`+healthCmdsAdded),
				healthNode(healthJob("hr_298"), `"status":"disabled",`+healthCmdsAdded)))
			h.compareSaved(t, "at the end", map[string][]string{off: offSaved, healthJob("hr_odd"): {`"saves=1\n"`},
				healthJob("hr_avg"): {`"saves=1\n"`}, healthJob("hr_298"): {`"saves=1\n"`, `enabled\":false`}})
		},
		after: func(t *testing.T, h *healthPair) {
			const returned = "level=error … msg=\"DYNCFG: plugin returned code "
			const unsupported = "level=error … msg=\"DYNCFG: this command is not supported by the configuration node: config "
			h.compareCfgRecords(t, healthRecordsCount(104, map[string]int{
				// health's record of a payload its parser refused, in an `add` and in an `update`; none for `userconfig`
				"level=error … msg=\"HEALTH DYNCFG: rejected 'add' of alert prototype 'hr_n":          len(refused),
				"level=error … msg=\"HEALTH DYNCFG: rejected 'update' of alert prototype 'hr_file'\"": len(again),
				"HEALTH DYNCFG: rejected 'userconfig'":                                                0,
				// the evaluator's own record of an expression it refused, in every mode
				"level=error … msg=\"failed to parse expression '$a +': remaining characters after expression at character 4 (i.e.: '+').\"":    3,
				"level=error … msg=\"failed to parse expression '$this >': remaining characters after expression at character 7 (i.e.: '>').\"": 1,
				// health's validation: its own record, and not the parser's
				"level=error … msg=\"HEALTH: alert 'hr_v0' rule 0 is invalid: missing update frequency. Source: \"":   1,
				"level=error … msg=\"HEALTH: alert 'hr_file' rule 0 is invalid: missing update frequency. Source: \"": 1,
				// the core's record of every call health refused: the parser's and the validation's refusals of `add`,
				// `update` and `userconfig`, the two enables of a name with no enabled rule, the two schemas
				returned + "400 to user initiated call: config ":                                 len(refused) + 2*len(again) + 2 + 2,
				returned + "501 to user initiated call: config " + healthJobPrefix + " schema\"": 1,
				returned + "501 to user initiated call: config " + file + " schema\"":            1,
				// the core's own refusals of a command a node does not have
				unsupported + file + " remove\"":                                                 1,
				unsupported + file + " restart\"":                                                1,
				unsupported + file + " add x\"":                                                  1,
				unsupported + file + " test x\"":                                                 1,
				unsupported + healthJobPrefix + " get\"":                                         2,
				unsupported + healthJobPrefix + " update\"":                                      1,
				unsupported + healthJobPrefix + " test x\"":                                      1,
				unsupported + healthJobPrefix + " remove\"":                                      1,
				"level=error … msg=\"DYNCFG: unknown config id '":                                2,
				"level=warning … msg=\"Alert lookup dimensions grouping 'bogus' is not valid\"":  1,
				"level=warning … msg=\"Alert data source 'bogus' is not valid\"":                 1,
				"level=warning … msg=\"Alert data source '~' is not valid\"":                     1,
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + off + "' by user": 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'add' hr_":                              3,
			}))
		},
	}
}

// healthTemplateConf are the `template` case's rules.
var healthTemplateConf = healthDcAlert("ht_warn", 50) + healthDcAlert("ht_clear", 90) + healthDcAlert("ht_off", 95)

// healthTemplateCase (`template`, plan E5): the template disabled with alerts running: every job is disabled and
// every alert removed, a job cannot be enabled, and a restart keeps it so (the template's saved file). Then the
// template is enabled: every job is back but the one a user disabled before. The second run starts each side's own
// directory again.
func healthTemplateCase() healthCase {
	warn, clr, off := healthJob("ht_warn"), healthJob("ht_clear"), healthJob("ht_off")
	disabled := healthMsg(400, "dyncfg functions intercept: this job belongs to disabled template")
	none := func(view string) error {
		if err := healthWant(map[string]string{})(view); err != nil {
			return err
		}
		if !strings.Contains(view, `"status": true,`) {
			return fmt.Errorf("the host's health is not running")
		}
		return nil
	}
	return healthCase{
		conf:   healthTemplateConf,
		dbMode: "alloc",
		sc:     healthDcScenario(true),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			h.compareNow(t, "raised: /api/v1/alarms?all", h.all, healthWant(map[string]string{"ht_warn": "WARNING", "ht_clear": "CLEAR", "ht_off": "CLEAR"}))
			h.processed(t, "raised", "ht_warn", "WARNING")
			time.Sleep(healthUnlinkHold)
			h.cfgStep(t, healthCfgStep{label: "disable one job", query: "action=disable&id=" + off, code: 200, parts: []string{healthMsg(200, "disabled")}})
			h.compareNow(t, "one job disabled: /api/v1/alarms?all", h.all, healthWant(map[string]string{"ht_warn": "WARNING", "ht_clear": "CLEAR"}))
			h.cfgStep(t, healthCfgStep{label: "disable the template", query: "action=disable&id=" + healthJobPrefix, code: 200,
				parts: []string{healthMsg(200, "applied to all template job")}})
			h.compareNow(t, "the template disabled: the tree", h.tree, healthBoth(
				healthNode(healthJobPrefix, `"type":"template","status":"accepted"`, `"user_disabled":true`, `"saves":1`),
				healthNode(warn, `"status":"disabled"`, `"user_disabled":false`, `"saves":0`),
				healthNode(clr, `"status":"disabled"`, `"user_disabled":false`, `"saves":0`),
				healthNode(off, `"status":"disabled"`, `"user_disabled":true`, `"saves":1`)))
			h.compareNow(t, "the template disabled: /api/v1/alarms?all", h.all, none)
			h.compareNow(t, "the template disabled: the transitions", h.changes, healthBoth(healthLastChange("ht_warn", "WARNING->REMOVED"),
				healthLastChange("ht_clear", "CLEAR->REMOVED"), healthLastChange("ht_off", "CLEAR->REMOVED")))
			h.compareSaved(t, "the template disabled", map[string][]string{
				healthJobPrefix: {`"type=template\n"`, `"user_disabled=true\n"`, `"saves=1\n"`, `"cmds=` + healthSavedTemplateCmds + `\n"`, "!---"},
				off:             {`"user_disabled=true\n"`, `"saves=1\n"`}})
			h.cfgSteps(t,
				healthCfgStep{label: "enable a job of the disabled template", query: "action=enable&id=" + warn, code: 400, parts: []string{disabled}},
				healthCfgStep{label: "enable the job a user disabled", query: "action=enable&id=" + off, code: 400, parts: []string{disabled}})
			// each alert's three links, its status and its unlink
			h.compareNow(t, "before the stop: /api/v1/alarm_log", h.log, healthLogEntries(3*5))
		},
		again: func(t *testing.T, h *healthPair) {
			time.Sleep(2 * time.Second)
			h.release(t, "again", 0, 0)
			// HEALTH's first pass on the host loads its alert log: the last id it gave is the first run's
			h.waitOracle(t, "after the second start: the host's health", func() (string, error) {
				a, status, latest := healthState(h.p.Oracle, "/api/v1/alarms?all")
				if !status || latest == 0 {
					return string(a.Body), fmt.Errorf("the host's health has not run")
				}
				return "", nil
			})
			h.compareNow(t, "after the second start: /api/v1/alarms?all", h.all, none)
			h.compareNow(t, "after the second start: the tree", h.tree, healthBoth(
				healthNode(healthJobPrefix, `"type":"template","status":"accepted"`, `"user_disabled":true`, `"saves":1`),
				healthNode(warn, `"status":"disabled"`, `"user_disabled":false`), healthNode(clr, `"status":"disabled"`),
				healthNode(off, `"status":"disabled"`, `"user_disabled":true`)))
			h.cfgSteps(t,
				healthCfgStep{label: "enable a job of the disabled template, again", query: "action=enable&id=" + warn, code: 400, parts: []string{disabled}},
				healthCfgStep{label: "enable the template", query: "action=enable&id=" + healthJobPrefix, code: 200,
					parts: []string{healthMsg(200, "applied to all template job")}})
			h.compareNow(t, "the template enabled: the tree", h.tree, healthBoth(
				healthNode(healthJobPrefix, `"user_disabled":false`, `"saves":2`),
				healthNode(warn, `"status":"accepted"`), healthNode(clr, `"status":"accepted"`),
				healthNode(off, `"status":"disabled"`, `"user_disabled":true`)))
			h.compareNow(t, "the template enabled: /api/v1/alarms?all", h.all, healthWant(map[string]string{"ht_warn": "WARNING", "ht_clear": "CLEAR"}))
			h.compareNow(t, "the template enabled: the transitions", h.changes, healthBoth(healthLastChange("ht_warn", "UNINITIALIZED->WARNING"),
				healthLastChange("ht_clear", "UNINITIALIZED->CLEAR"), healthLastChange("ht_off", "CLEAR->REMOVED")))
			h.compareSaved(t, "the template enabled", map[string][]string{
				healthJobPrefix: {`"user_disabled=false\n"`, `"saves=2\n"`}, off: {`"user_disabled=true\n"`, `"saves=1\n"`}})
			h.processed(t, "the template enabled", "ht_warn", "WARNING")
			h.compareNow(t, "at the end: the notifier's calls", h.calls(t), healthCallsWant(1, healthCallFor("ht_warn", "WARNING", 1, nil)))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCfgRecords(t, healthRecordsCount(6, map[string]int{
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + off + "' by user":                       1,
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on template '" + healthJobPrefix + "' by user":      1,
				"level=notice … msg=\"DYNCFG USER ACTION 'enable' on template '" + healthJobPrefix + "' by user":       1,
				"level=error … msg=\"DYNCFG: cannot enable a job of a disabled template: config " + warn + " enable\"": 2,
				"level=error … msg=\"DYNCFG: cannot enable a job of a disabled template: config " + off + " enable\"":  1,
			}))
		},
	}
}

// healthHandBackConf are the hand-back cases' rules.
var healthHandBackConf = healthDcAlert("hh_upd", 50) + healthDcAlert("hh_dis", 90) + healthDcAlert("hh_keep", 90)

// healthDynCfgHandBack (the `handback` cases, plan E6): the first run adds two rules (one with no enabled rule),
// updates a file's alert and disables another; then both agents stop and each side's directory is started again,
// with the binaries of `bins`. At its start the agent replays the saved jobs before it registers the file's
// (dyncfg.c:299-318): the added jobs are back, without `test`; the updated alert is a DynCfg job with `remove`; the
// disabled one is disabled; and the start changes no file. Then the updated alert is removed: C deletes the name,
// and the file's rule does not come back before a reload or a restart.
func healthDynCfgHandBack(bins [2][2]Role) healthCase {
	upd, dis, keep, added, none := healthJob("hh_upd"), healthJob("hh_dis"), healthJob("hh_keep"), healthJob("hh_add"), healthJob("hh_none")
	alerts := map[string]string{"hh_upd": "CLEAR", "hh_keep": "CLEAR", "hh_add": "WARNING"}
	var kept [2][]string
	files := func(t *testing.T, h *healthPair, i int) []string {
		return dcFiles(t, "saved", dcConfigDir(h.p.Each()[i].Daemon.Opts.RunDir))
	}
	saved := map[string][]string{
		added: {`"source_type=dyncfg\n"`, `"saves=1\n"`, `"cmds=` + healthSavedCmds + `\n"`, `$this > 60`},
		none:  {`"source_type=dyncfg\n"`, `"saves=1\n"`, `enabled\":false`},
		upd:   {`"source_type=dyncfg\n"`, `"saves=1\n"`, `$this > 80`},
		dis:   {`"user_disabled=true\n"`, `"saves=1\n"`, "!---"},
	}
	return healthCase{
		conf:   healthHandBackConf,
		dbMode: "alloc",
		sc:     healthDcScenario(true),
		bins:   bins,
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			h.compareNow(t, "raised: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hh_upd": "WARNING", "hh_dis": "CLEAR", "hh_keep": "CLEAR"}))
			h.processed(t, "raised", "hh_upd", "WARNING")
			time.Sleep(healthUnlinkHold)
			h.cfgSteps(t,
				healthCfgStep{label: "add", query: "action=add&id=" + healthJobPrefix + "&name=hh_add", body: healthPayload(healthDcRule(60)),
					code: 202, parts: []string{healthMsg(202, "accepted")}},
				healthCfgStep{label: "add a rule that is off", query: "action=add&id=" + healthJobPrefix + "&name=hh_none",
					body: healthPayload(healthDcRule(60, "enabled", false)), code: 298, parts: []string{healthMsg(298, "accepted")}})
			// the update, once the pass of HEALTH that may have been running when the add linked its alert has ended
			// (healthSkipHold)
			time.Sleep(healthSkipHold)
			h.cfgSteps(t,
				healthCfgStep{label: "update a file's alert", query: "action=update&id=" + upd, body: healthPayload(healthDcRule(80)),
					code: 202, parts: []string{healthMsg(202, "updated")}},
				healthCfgStep{label: "disable a file's alert", query: "action=disable&id=" + dis, code: 200, parts: []string{healthMsg(200, "disabled")}})
			h.compareNow(t, "the first run: /api/v1/alarms?all", h.all, healthWant(alerts))
			h.processed(t, "the first run", "hh_add", "WARNING")
			h.processed(t, "the first run", "hh_upd", "CLEAR")
			h.compareNow(t, "the first run: the notifier's calls", h.calls(t), healthCallsWant(3,
				healthCallFor("hh_upd", "WARNING", 1, nil), healthCallFor("hh_add", "WARNING", 1, nil), healthCallFor("hh_upd", "CLEAR", 1, nil)))
			h.compareNow(t, "the first run: the tree", h.tree, healthBoth(
				healthNode(added, `"status":"accepted",`+healthCmdsAdded, `"source_type":"dyncfg"`),
				healthNode(none, `"status":"disabled",`+healthCmdsAdded, `"source_type":"dyncfg"`),
				healthNode(upd, healthCmdsDynCfg, `"source_type":"dyncfg"`),
				healthNode(dis, `"status":"disabled",`+healthCmdsFile, `"user_disabled":true`),
				healthNode(keep, `"status":"running",`+healthCmdsFile, `"saves":0`)))
			h.compareSaved(t, "the first run", saved)
			// the three file alerts' three links and status; hh_add's link and status; hh_upd's unlink, link and status;
			// hh_dis's unlink
			h.compareNow(t, "the first run: /api/v1/alarm_log", h.log, healthLogEntries(3*4+2+3+1))
			for i := range kept {
				kept[i] = files(t, h, i)
			}
		},
		again: func(t *testing.T, h *healthPair) {
			// the start changed no file, on either side
			for i, s := range h.p.Each() {
				if now := files(t, h, i); !slices.Equal(now, kept[i]) {
					t.Fatalf("%s: the start changed the saved files\nbefore the stop:\n%s\nafter the start:\n%s", s.Role,
						strings.Join(kept[i], "\n"), strings.Join(now, "\n"))
				}
			}
			h.compareNow(t, "after the second start: the tree", h.tree, healthBoth(healthTemplateNode,
				healthNode(added, `"status":"accepted",`+healthCmdsDynCfg, `!"test"`, `"source_type":"dyncfg"`, `"user_disabled":false`, `"saves":1`),
				healthNode(none, `"status":"disabled",`+healthCmdsDynCfg, `!"test"`, `"source_type":"dyncfg"`, `"saves":1`),
				healthNode(upd, `"status":"accepted",`+healthCmdsDynCfg, `"source_type":"dyncfg"`, `"saves":1`),
				healthNode(dis, `"status":"disabled",`+healthCmdsFile, `"source_type":"user"`, `"user_disabled":true`, `"saves":1`),
				healthNode(keep, `"status":"running",`+healthCmdsFile, `"source_type":"user"`, `"saves":0`)))
			h.cfgSteps(t,
				healthCfgStep{label: "get the added job", query: "action=get&id=" + added, code: 200,
					parts: []string{`"source_type":"dyncfg","source":""`, `"warning_condition":"$this > 60"`}},
				healthCfgStep{label: "get the job that is off", query: "action=get&id=" + none, code: 200, parts: []string{`"rules":[{"enabled":false`}},
				healthCfgStep{label: "get the updated job", query: "action=get&id=" + upd, code: 200,
					parts: []string{`"source_type":"dyncfg","source":""`, `"warning_condition":"$this > 80"`}},
				healthCfgStep{label: "get the disabled job", query: "action=get&id=" + dis, code: 200, parts: []string{`"source_type":"user"`}},
				healthCfgStep{label: "get the job nothing was done to", query: "action=get&id=" + keep, code: 200, parts: []string{`"source_type":"user"`}})
			time.Sleep(2 * time.Second)
			h.release(t, "again", 0, 0)
			h.settle(t, h.n, "")
			h.compareNow(t, "after the second start: /api/v1/alarms?all", h.all, healthWant(alerts))
			h.processed(t, "after the second start", "hh_add", "WARNING")
			h.compareNow(t, "after the second start: the transitions", h.changes, healthBoth(healthLastChange("hh_add", "UNINITIALIZED->WARNING"),
				healthLastChange("hh_upd", "UNINITIALIZED->CLEAR"), healthLastChange("hh_dis", "CLEAR->REMOVED")))
			h.compareSaved(t, "after the second start", saved)

			// the updated alert removed: the name is gone, the file's rule with it
			time.Sleep(healthUnlinkHold)
			h.cfgStep(t, healthCfgStep{label: "remove the updated alert", query: "action=remove&id=" + upd, code: 200,
				parts: []string{healthMsg(200, "deleted")}})
			h.compareNow(t, "removed: the tree", h.tree, healthBoth(healthNoNode(upd), healthNode(added, `"status":"accepted"`)))
			h.compareNow(t, "removed: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hh_keep": "CLEAR", "hh_add": "WARNING"}))
			h.compareNow(t, "removed: the transitions", h.changes, healthLastChange("hh_upd", "CLEAR->REMOVED"))
			delete(saved, upd)
			h.compareSaved(t, "removed", saved)
			// the second run's: the load's entry for hh_upd and hh_add, the two alerts whose last stored entry is a status
			// (hh_keep's row still names its first pass's unlink, hh_dis's the disable's: sqlite_health.c:528-615); the
			// three links and the status of hh_upd, hh_keep and hh_add; hh_upd's unlink
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthLogEntries(18+2+3*4+1))
			// after a restart C notifies again the status an alert was last notified for (the load's entry stands
			// between): hh_upd's CLEAR and hh_add's WARNING
			h.compareNow(t, "at the end: the notifier's calls", h.calls(t), healthCallsWant(5,
				healthCallFor("hh_upd", "CLEAR", 2, nil), healthCallFor("hh_add", "WARNING", 2, nil), healthCallsEnded(0)))
		},
		after: func(t *testing.T, h *healthPair) {
			// the first run's four actions and the second's one: neither start wrote a record
			h.compareCfgRecords(t, healthRecordsCount(5, map[string]int{
				"level=notice … msg=\"DYNCFG USER ACTION 'add' hh_add on template":       1,
				"level=notice … msg=\"DYNCFG USER ACTION 'add' hh_none on template":      1,
				"level=notice … msg=\"DYNCFG USER ACTION 'update' on job '" + upd + "'":  1,
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + dis + "'": 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'remove' on job '" + upd + "'":  1,
			}))
		},
	}
}

// healthReplayConf are the `replay-rejected` case's rules: the names two of the saved payloads also have, the name
// a saved file disables, and one nothing is saved for.
var healthReplayConf = healthDcAlert("hj_parse_file", 50) + healthDcAlert("hj_valid_file", 50) + healthDcAlert("hj_dis", 50) +
	healthDcAlert("hj_plain", 50)

// healthReplayCase (`replay-rejected`, plan E7): saved files an agent finds at its start: a payload the parser
// refuses and one health's validation refuses, each for a new name and for a name a rule file also has; a payload
// health takes; and a file that disables a rule file's job. C writes its records about each refused replay, marks
// the node rejected and leaves it an orphan, and the tree's `attention` counts them. No chart.
func healthReplayCase() healthCase {
	job := func(name, payload string) string {
		return healthSavedText(name, "dyncfg", healthSavedSource, false, 1, healthSavedCmds, payload)
	}
	version2 := `{"format_version":2,"rules":[]}`
	every0 := healthPayload(healthDcRule(60, "config.value.update_every", 0))
	names := []string{"hj_parse", "hj_parse_file", "hj_valid", "hj_valid_file", "hj_ok", "hj_dis"}
	return healthCase{
		conf: healthReplayConf,
		saved: map[string]string{
			healthSavedName(healthJob("hj_parse")):      job("hj_parse", version2),
			healthSavedName(healthJob("hj_parse_file")): job("hj_parse_file", version2),
			healthSavedName(healthJob("hj_valid")):      job("hj_valid", every0),
			healthSavedName(healthJob("hj_valid_file")): job("hj_valid_file", every0),
			healthSavedName(healthJob("hj_ok")):         job("hj_ok", healthPayload(healthDcRule(60))),
			healthSavedName(healthJob("hj_dis")):        healthSavedText("hj_dis", "internal", "", true, 1, healthSavedFileCmds, ""),
		},
		play: func(t *testing.T, h *healthPair) {
			h.compareNow(t, "the tree under /health", h.tree, healthBoth(healthTemplateNode,
				healthNode(healthJob("hj_parse"), healthOrphan, `"plugin_rejected":true`),
				healthNode(healthJob("hj_valid"), healthOrphan, `"plugin_rejected":true`),
				healthNode(healthJob("hj_parse_file"), `"status":"running"`, `"plugin_rejected":true`),
				healthNode(healthJob("hj_valid_file"), `"status":"running"`, `"plugin_rejected":true`),
				healthNode(healthJob("hj_ok"), `"status":"accepted",`+healthCmdsDynCfg, `"plugin_rejected":false`),
				healthNode(healthJob("hj_dis"), `"status":"disabled",`+healthCmdsFile, `"user_disabled":true`),
				healthNode(healthJob("hj_plain"), `"status":"running",`+healthCmdsFile, `"saves":0`)))
			h.compareNow(t, "the whole tree", func(i int) string { return h.config(i, "action=tree", time.Now().Add(-9*time.Minute)) },
				healthHas(`"attention":{"degraded":true`, `"plugin_rejected":4`))
			var steps []healthCfgStep
			for _, n := range names {
				steps = append(steps, healthCfgStep{label: "get " + n, query: "action=get&id=" + healthJob(n), code: 200})
			}
			steps[0].code, steps[2].code = 404, 404
			h.cfgSteps(t, steps...)
			h.compareLines(t, "the saved files", func(i int) []string { return h.saves(t, i) }, func(lines []string) error {
				if n := len(lines); n < 1+len(names) {
					return fmt.Errorf("%d lines, want the six files", n)
				}
				return nil
			})
		},
		after: func(t *testing.T, h *healthPair) {
			// a refused replay: health's record, then the core's; a name a rule file also has: the same two again, when
			// the file's job is registered and the core sends it the saved payload as an update (dyncfg.c:295-297)
			received := func(name, cmd string) string {
				return "DYNCFG: received response code 400 on request to id '" + healthJob(name) + "', cmd: " + cmd + `"`
			}
			h.compareCfgRecords(t, healthRecordsCount(12+2, map[string]int{
				// the two `get` of the orphans the refused replays left
				"level=error … msg=\"DYNCFG: unknown config id '":                                      2,
				`HEALTH DYNCFG: rejected 'add' of alert prototype 'hj_parse'"`:                         1,
				`HEALTH DYNCFG: rejected 'add' of alert prototype 'hj_parse_file'"`:                    1,
				`HEALTH DYNCFG: rejected 'update' of alert prototype 'hj_parse_file'"`:                 1,
				`HEALTH: alert 'hj_valid' rule 0 is invalid: missing update frequency. Source: "`:      1,
				`HEALTH: alert 'hj_valid_file' rule 0 is invalid: missing update frequency. Source: "`: 2,
				received("hj_parse", "add hj_parse"):                                                   1,
				received("hj_parse_file", "add hj_parse_file"):                                         1,
				received("hj_parse_file", "update"):                                                    1,
				received("hj_valid", "add hj_valid"):                                                   1,
				received("hj_valid_file", "add hj_valid_file"):                                         1,
				received("hj_valid_file", "update"):                                                    1,
				"errno=":                                                                               0,
			}))
		},
	}
}

// healthDynCfgOffCase (`off`, plan E8): health off, with the saved files a C agent whose health was on leaves: the
// template's (disabled by a user), an added job's, a disabled file job's. Nothing registers a node: the three are
// orphans whose one command is `remove`, which the core answers itself.
func healthDynCfgOffCase() healthCase {
	added, dis := healthJob("ho_add"), healthJob("ho_dis")
	return healthCase{
		off:  true,
		conf: healthDcAlert("ho_dis", 50),
		saved: map[string]string{
			healthSavedName(healthJobPrefix): healthSavedText("", "internal", "", true, 1, healthSavedTemplateCmds, ""),
			healthSavedName(added):           healthSavedText("ho_add", "dyncfg", healthSavedSource, false, 1, healthSavedCmds, healthPayload(healthDcRule(60))),
			healthSavedName(dis):             healthSavedText("ho_dis", "internal", "", true, 1, healthSavedFileCmds, ""),
		},
		play: func(t *testing.T, h *healthPair) {
			h.compareNow(t, "the tree under /health", h.tree, healthBoth(
				healthNode(healthJobPrefix, `"type":"template",`+healthOrphan, `"user_disabled":true`),
				healthNode(added, `"type":"job","template":"`+healthJobPrefix+`",`+healthOrphan, `"payload":{"available":true`),
				healthNode(dis, `"type":"job","template":"`+healthJobPrefix+`",`+healthOrphan, `"user_disabled":true`)))
			h.cfgSteps(t,
				healthCfgStep{label: "get an orphan", query: "action=get&id=" + added, code: 404, parts: []string{`"errorMessage":"Unknown config id given."`}},
				healthCfgStep{label: "add on the orphan template", query: "action=add&id=" + healthJobPrefix + "&name=ho_new", body: healthPayload(healthDcRule(60)),
					code: 404, parts: []string{`"errorMessage":"Unknown config id given."`}},
				healthCfgStep{label: "enable the orphan template", query: "action=enable&id=" + healthJobPrefix, code: 404,
					parts: []string{`"errorMessage":"Unknown config id given."`}},
				healthCfgStep{label: "remove an orphan", query: "action=remove&id=" + added, code: 200, parts: []string{healthMsg(200, "")}},
				// the id is now a new job's of the orphan template to the core, which "removes" it: nothing is there, 200
				healthCfgStep{label: "remove it again", query: "action=remove&id=" + added, code: 200, parts: []string{healthMsg(200, "")}},
				// a node loaded from a file has no access of its own: anybody may remove it (the core's, milestone 8)
				healthCfgStep{label: "remove an orphan, as nobody", query: "action=remove&id=" + dis, auth: healthNobody, code: 200,
					parts: []string{healthMsg(200, "")}})
			h.compareNow(t, "after the removes: the tree", h.tree, healthBoth(healthNode(healthJobPrefix, healthOrphan), healthNoNode(added, dis)))
			h.compareSaved(t, "after the removes", map[string][]string{healthJobPrefix: {`"type=template\n"`, `"user_disabled=true\n"`, `"saves=1\n"`}})
			h.compareNow(t, "/api/v1/alarms", func(i int) string { return h.get(i, "/api/v1/alarms") }, healthHas(`"status": false,`))
		},
		after: func(t *testing.T, h *healthPair) {
			// the calls the core found no node for; a remove it answered itself leaves no record
			h.compareCfgRecords(t, healthRecordsCount(3, map[string]int{
				"level=error … msg=\"DYNCFG: unknown config id '" + added + "' in call: 'config " + added + " get'.":                            1,
				"level=error … msg=\"DYNCFG: unknown config id '" + healthJobPrefix + "' in call: 'config " + healthJobPrefix + " add ho_new'.": 1,
				"level=error … msg=\"DYNCFG: unknown config id '" + healthJobPrefix + "' in call: 'config " + healthJobPrefix + " enable'.":     1,
			}))
		},
	}
}

// healthAccessCase (`access`, plan E9): who may call health's nodes. They take C's default access (dyncfg.c:395-399:
// view needs a signed-in client of the same space with view-config, edit also edit-config and a commercial space),
// and the method needs what both sets share, so nRPC's gate refuses a client that is not signed in (412) and a
// member (403) before DynCfg is asked. The tree is open: an anonymous client sees the nodes, a DynCfg job's source
// hidden.
func healthAccessCase() healthCase {
	file, added := healthJob("hx_file"), healthJob("hx_add")
	const hidden = `"source":"User details hidden in anonymous mode. Sign in to access configuration details."`
	denied := `You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature.`
	return healthCase{
		conf: healthDcAlert("hx_file", 50),
		play: func(t *testing.T, h *healthPair) {
			h.cfgSteps(t,
				healthCfgStep{label: "the admin adds a job", query: "action=add&id=" + healthJobPrefix + "&name=hx_add", body: healthPayload(healthDcRule(60)),
					code: 202, parts: []string{healthMsg(202, "accepted")}},
				healthCfgStep{label: "nobody: get", query: "action=get&id=" + file, auth: healthNobody, code: 412, parts: []string{denied}},
				healthCfgStep{label: "a member: get", query: "action=get&id=" + file, auth: dcMember, code: 403},
				healthCfgStep{label: "nobody: update", query: "action=update&id=" + file, body: healthPayload(healthDcRule(80)), auth: healthNobody,
					code: 412, parts: []string{denied}},
				healthCfgStep{label: "a member: update", query: "action=update&id=" + file, body: healthPayload(healthDcRule(80)), auth: dcMember, code: 403},
				healthCfgStep{label: "nobody: add", query: "action=add&id=" + healthJobPrefix + "&name=hx_no", body: healthPayload(healthDcRule(80)),
					auth: healthNobody, code: 412, parts: []string{denied}},
				healthCfgStep{label: "a member: disable", query: "action=disable&id=" + file, auth: dcMember, code: 403},
				healthCfgStep{label: "a member: disable the template", query: "action=disable&id=" + healthJobPrefix, auth: dcMember, code: 403},
				healthCfgStep{label: "nobody: remove the added job", query: "action=remove&id=" + added, auth: healthNobody, code: 412, parts: []string{denied}},
				healthCfgStep{label: "nobody: the schema", query: "action=schema&id=" + healthJobPrefix, auth: healthNobody, code: 412, parts: []string{denied}},
				healthCfgStep{label: "nobody: the tree", query: "action=tree&path=/health", auth: healthNobody, code: 200,
					parts: []string{`"` + added + `":{"type":"job"`, hidden, `"source":"line=1,file={run}/etc/health.d/` + healthConfFile + `"`}},
				healthCfgStep{label: "a member: the tree", query: "action=tree&path=/health", auth: dcMember, code: 200, parts: []string{hidden}},
				healthCfgStep{label: "the admin: get", query: "action=get&id=" + file, code: 200, parts: []string{`"source_type":"user"`}})
			h.compareNow(t, "at the end: the tree", h.tree, healthBoth(healthNode(file, `"status":"running"`, `"source_type":"user"`, `"saves":0`),
				healthNode(added, `"status":"accepted"`, `"source":"`+healthSavedSource), healthNoNode(healthJob("hx_no"))))
			h.compareSaved(t, "at the end", map[string][]string{added: {`"saves=1\n"`}})
		},
		after: func(t *testing.T, h *healthPair) {
			// the gate's refusals leave no DynCfg record
			h.compareCfgRecords(t, healthRecordsCount(1, map[string]int{"level=notice … msg=\"DYNCFG USER ACTION 'add' hx_add on template": 1}))
		},
	}
}

// The access log's record of a rule's push to the Cloud (sqlite_aclk.c:1199-1209), and HEALTH's record of a host it
// skips while the entries a change logged wait to be stored (health_event_loop.c:396-401).
const (
	healthAclkReq   = `msg="ACLK REQ [`
	healthPostponed = ` has pending alert transitions to save, postponing health checks`
)

// healthDebugCase (`debug`, plan E10): the start with the installed stock rules, then one rule added, at debug
// level. C's registration of the template and of its jobs writes no record at any level, so what is compared is
// what the add leaves: the core's record of the user's action; the access log's record of the rule's push to the
// Cloud (an agent that was never claimed writes it with an empty node id, and sends nothing); the record of the
// alert's link in health.log, which the web worker writes with its request; and that HEALTH skipped the host while
// the link's entry waited to be stored (how many passes it skipped is each side's timing).
func healthDebugCase() healthCase {
	return healthCase{
		conf:  healthDcAlert("hg_base", 90),
		stock: true,
		logs:  healthLogsDebug,
		sc:    healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			h.compareNow(t, "at the start: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hg_base": "CLEAR"}))
			h.cfgStep(t, healthCfgStep{label: "add", query: "action=add&id=" + healthJobPrefix + "&name=hg_new", body: healthPayload(healthDcRule(50)),
				code: 202, parts: []string{healthMsg(202, "accepted")}})
			h.compareNow(t, "added: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hg_base": "CLEAR", "hg_new": "WARNING"}))
			h.processed(t, "added", "hg_new", "WARNING")
			h.compareNow(t, "added: the notifier's calls", h.calls(t), healthCallsWant(1, healthCallFor("hg_new", "WARNING", 1, nil)))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCfgRecords(t, healthRecordsCount(1, map[string]int{"level=notice … msg=\"DYNCFG USER ACTION 'add' hg_new on template": 1}))
			web := func(i int, file, part string) []string {
				d := h.p.Each()[i].Daemon
				var out []string
				for _, l := range logLines(t, d.Opts.RunDir, file) {
					if strings.Contains(l, part) && strings.HasPrefix(threadOf(l), "WEB[") {
						l = strings.ReplaceAll(l, " transaction="+fnTxPrefix, " transaction=sent:"+fnTxPrefix)
						out = append(out, healthConnRe.ReplaceAllString(h.n[i].paths(normalizeLog(l, d.Opts.RunDir, "")), " conn=N"))
					}
				}
				return out
			}
			h.compareLines(t, "the access log's records of a push to the Cloud", func(i int) []string { return web(i, "access.log", healthAclkReq) },
				healthRecordsCount(1, map[string]int{`msg="ACLK REQ [ (` + h.p.Oracle.Hostname + `)]: Request to send alert config `: 1,
					"transaction=sent:" + healthCfgTx(1) + " ": 1}))
			h.compareLines(t, "health.log's records of the web workers", func(i int) []string {
				d := h.p.Each()[i].Daemon
				var out []string
				for _, l := range h.n[i].healthLog(t, d) {
					if strings.HasPrefix(threadOf(l), "WEB[") {
						out = append(out, healthConnRe.ReplaceAllString(l, " conn=N"))
					}
				}
				return out
			}, healthRecordsCount(1, map[string]int{"level=debug": 1, "alert=hg_new": 1}))
			h.compareLines(t, "HEALTH skipped the host while the add's entry waited", func(i int) []string {
				return []string{fmt.Sprintf("records of a skipped pass: %t",
					len(h.threadRecords(t, i, "HEALTH", `Host \"`+h.p.Each()[i].Daemon.Hostname+`\"`+healthPostponed)) > 0)}
			}, func(oracle []string) error {
				if oracle[0] != "records of a skipped pass: true" {
					return fmt.Errorf("HEALTH wrote no record of a skipped pass")
				}
				return nil
			})
		},
	}
}

// healthGetStockCase (`get-stock`, plan E11): `get` of every job of the installed stock rules: each rule chain as
// JSON, with its hashes. No chart.
func healthGetStockCase() healthCase {
	return healthCase{
		stock: true,
		play: func(t *testing.T, h *healthPair) {
			tree := h.compareNow(t, "the DynCfg tree under /health", h.tree, healthTemplateNode)
			jobs := healthJobs(tree)
			if len(jobs) < 100+1 {
				t.Fatalf("oracle: %d health nodes, want the template and at least 100 jobs", len(jobs))
			}
			guard := healthCfgIs(200, "Content-Type: application/json", `{"format_version":1,"name":"`)
			rules := 0
			for _, id := range jobs {
				if id == healthJobPrefix {
					continue
				}
				h.tx++
				var views [2]string
				for i := range views {
					views[i] = h.send(t, i, h.tx, "action=get&id="+id, "", "")
				}
				if err := guard(views[0]); err != nil {
					t.Fatalf("oracle: get of %s: %v:\n%s", id, err, views[0])
				}
				if views[0] != views[1] {
					t.Fatalf("get of %s differs\noracle:\n%s\ncandidate:\n%s", id, views[0], views[1])
				}
				rules += strings.Count(views[0], `"hash":"`)
			}
			t.Logf("get of %d jobs, %d rules: equal on both sides", len(jobs)-1, rules)
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCfgRecords(t, healthRecordsCount(0, nil))
		},
	}
}

// healthColonCase (`colon`, plan E12): a rule added under a name with a colon. Health takes it and links its alert,
// but the core takes what precedes the name's colon for the job's template (dyncfg.c:334-351, :406-409), which does not
// exist: health's own registration is refused with a record, and the node the core makes for the added job has no
// method, so it is an orphan that names a template nobody registered. Its one command is `remove`, which the core
// answers itself: the file and the node go, the alert stays.
func healthColonCase() healthCase {
	const name = "hk:b"
	job := healthJob(name)
	alerts := map[string]string{"hk_base": "CLEAR", name: "WARNING"}
	return healthCase{
		conf: healthDcAlert("hk_base", 90),
		sc:   healthDcScenario(false),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			h.create(t)
			h.compareNow(t, "at the start: /api/v1/alarms?all", h.all, healthWant(map[string]string{"hk_base": "CLEAR"}))
			h.cfgStep(t, healthCfgStep{label: "add", query: "action=add&id=" + healthJobPrefix + "&name=" + name, body: healthPayload(healthDcRule(50)),
				code: 202, parts: []string{healthMsg(202, "accepted")}})
			h.compareNow(t, "added: the tree", h.tree, healthNode(job, `"type":"job","template":"`+healthJob("hk")+`",`+healthOrphan,
				`"source_type":"dyncfg"`, `"payload":{"available":true`, `"saves":1`))
			h.compareNow(t, "added: /api/v1/alarms?all", h.all, healthWant(alerts))
			h.processed(t, "added", name, "WARNING")
			h.compareNow(t, "added: the notifier's calls", h.calls(t), healthCallsWant(1, healthCallFor(name, "WARNING", 1, nil)))
			h.compareSaved(t, "added", map[string][]string{job: {`"template=` + healthJob("hk") + `\n"`, `"saves=1\n"`}})
			h.cfgSteps(t,
				healthCfgStep{label: "get", query: "action=get&id=" + job, code: 404, parts: []string{`"errorMessage":"Unknown config id given."`}},
				healthCfgStep{label: "remove", query: "action=remove&id=" + job, code: 200, parts: []string{healthMsg(200, "")}})
			h.compareNow(t, "removed: the tree", h.tree, healthNoNode(job))
			h.compareSaved(t, "removed", map[string][]string{})
			// the alert is still there: nothing told health
			h.compareNow(t, "removed: /api/v1/alarms?all", h.all, healthWant(alerts))
			// the file's alert: its three links and its status; the added one: its link and its status
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthLogEntries(4+2))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCfgRecords(t, healthRecordsCount(3, map[string]int{
				"level=error … msg=\"DYNCFG: job id '" + job + "' does not have a registered template. Ignoring dynamic configuration for it.\"": 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'add' " + name + " on template '" + healthJobPrefix + "' by user":                       1,
				"level=error … msg=\"DYNCFG: unknown config id '" + job + "' in call: 'config " + job + " get'.":                                 1,
			}))
		},
	}
}

// healthRoundTripCase (`roundtrip`): a job's own JSON sent back as its update, unchanged. The rule gets another
// hash: `get` prints the notifier and the recipient health filled in after it hashed the file's rule
// (health_prototypes.c:396-411, :479-483), and the payload's `execute` is then part of the rule. The hash of a rule
// that names the side's own notifier directory differs side to side, so the hashes print by the order each side
// first showed them (healthPair.aliased): that the hash changed, and that it does not change again, is compared.
func healthRoundTripCase() healthCase {
	job := healthJob("hb_file")
	return healthCase{
		conf: healthDcAlert("hb_file", 50),
		play: func(t *testing.T, h *healthPair) {
			h.hashes = [2][]string{{}, {}}
			// each side is sent its own answer: its own run directory in `execute` (the rail takes the side's own only)
			var own [2]string
			h.tx++
			for i, s := range h.p.Each() {
				raw, err := rawExchange(s.Daemon.Addr, rawRequest("GET", "/api/v1/config?action=get&id="+job,
					[]string{"X-Transaction-Id: " + healthCfgTx(h.tx), dcAdmin}, nil), fnWait)
				if err != nil {
					t.Fatalf("%s: get: %v", s.Role, err)
				}
				_, own[i], _ = strings.Cut(string(raw), "\r\n\r\n")
				if !strings.Contains(own[i], `"execute":"`+s.Daemon.Opts.RunDir+`/notify/stub"`) && i == 0 {
					t.Fatalf("oracle: the job's JSON does not name the side's notifier:\n%s", own[i])
				}
			}
			before := h.cfgStep(t, healthCfgStep{label: "get", query: "action=get&id=" + job, code: 200,
				parts: []string{`"hash":"<hash 1>","source_type":"user"`, `"execute":"{run}/notify/stub","recipient":"root"`}})
			h.tx++
			var views [2]string
			for i := range views {
				views[i] = h.send(t, i, h.tx, "action=update&id="+job, own[i], "")
			}
			if err := healthCfgIs(202, healthMsg(202, "updated"))(views[0]); err != nil {
				t.Fatalf("oracle: the job's own JSON sent back: %v:\n%s", err, views[0])
			}
			if views[0] != views[1] {
				t.Fatalf("the job's own JSON sent back: the answers differ\noracle:\n%s\ncandidate:\n%s", views[0], views[1])
			}
			after := h.cfgStep(t, healthCfgStep{label: "get after the update", query: "action=get&id=" + job, code: 200,
				parts: []string{`"hash":"<hash 2>","source_type":"dyncfg","source":""`, `"execute":"{run}/notify/stub","recipient":"root"`}})
			// the rule itself is as it was: its hash and its source alone changed
			rule := func(view, head string) string {
				_, body, _ := strings.Cut(view, "\n\n")
				return strings.Replace(body, head, "", 1)
			}
			b := rule(before, `"hash":"<hash 1>","source_type":"user","source":"line=1,file={run}/etc/health.d/`+healthConfFile+`",`)
			a := rule(after, `"hash":"<hash 2>","source_type":"dyncfg","source":"",`)
			if b != a {
				t.Fatalf("oracle: the rule changed in more than its hash and its source\nbefore: %s\nafter:  %s", b, a)
			}
			// the same JSON again: the hash stays
			h.tx++
			for i := range views {
				views[i] = h.send(t, i, h.tx, "action=update&id="+job, own[i], "")
			}
			if err := healthCfgIs(202, healthMsg(202, "updated"))(views[0]); err != nil || views[0] != views[1] {
				t.Fatalf("the job's own JSON sent back again: %v\noracle:\n%s\ncandidate:\n%s", err, views[0], views[1])
			}
			h.cfgStep(t, healthCfgStep{label: "get after the second update", query: "action=get&id=" + job, code: 200,
				parts: []string{`"hash":"<hash 2>","source_type":"dyncfg","source":""`}})
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCfgRecords(t, healthRecordsCount(2, map[string]int{"level=notice … msg=\"DYNCFG USER ACTION 'update' on job '" + job + "'": 2}))
		},
	}
}

// healthDynCfgCases are the cases of `health.dyncfg` that send commands (TestHealthDynCfg has the two that only
// read).
func healthDynCfgCases() map[string]healthCase {
	return map[string]healthCase{
		"actions":               healthActionsCase(),
		"add-remove":            healthAddRemoveCase(),
		"refusals":              healthRefusalsCase(),
		"template":              healthTemplateCase(),
		"handback":              healthDynCfgHandBack([2][2]Role{{Oracle, Candidate}, {Oracle, Candidate}}),
		"handback-c-after-rust": healthDynCfgHandBack([2][2]Role{{Oracle, Candidate}, {Oracle, Oracle}}),
		"handback-rust-after-c": healthDynCfgHandBack([2][2]Role{{Oracle, Oracle}, {Oracle, Candidate}}),
		"replay-rejected":       healthReplayCase(),
		"off":                   healthDynCfgOffCase(),
		"access":                healthAccessCase(),
		"debug":                 healthDebugCase(),
		"get-stock":             healthGetStockCase(),
		"colon":                 healthColonCase(),
		"roundtrip":             healthRoundTripCase(),
	}
}
