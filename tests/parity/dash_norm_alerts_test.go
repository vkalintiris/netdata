// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"strings"
	"testing"
)

// testDashNormAlerts pins the alert families on the answers and the alert logs of one C-against-C run
// (dash_norm_alerts_data_test.go): what alertsV2Render names against each side's own alert log and what it leaves, the
// seconds it hands to the bound, each wrong answer the review named (R96's D8), and the families with the health
// runner's normalizers and with the zero pair.
func testDashNormAlerts(t *testing.T) {
	norm := func() *healthNorm {
		return &healthNorm{entries: map[int64]healthEntry{}, tids: map[string]int64{}, moved: map[string]int64{}}
	}
	// the family of one case's pair: each side's normalizer, and its log as recorded
	family := func(logs [2]string) v2Family {
		return alertsV2Family([2]*healthNorm{norm(), norm()}, func(i int) ([]byte, error) { return []byte(logs[i]), nil })
	}
	asked := [2]int64{dashNormAlertsAsked, dashNormAlertsAsked}
	diffs := func(logs [2]string, o, c string) string {
		t.Helper()
		fam := family(logs)
		return dashNormDiffs(t, fam, fam.masks, asked, o, c)
	}

	// the recorded pairs show no difference: each side's ids, times and spans are its own log's
	for name, c := range map[string]struct {
		logs, bodies [2]string
	}{
		"raised":    {dashNormAlertsAlertsLog, dashNormAlertsRaised},
		"by-name":   {dashNormAlertsAlertsLog, dashNormAlertsByName},
		"v2":        {dashNormAlertsAlertsLog, dashNormAlertsPretty},
		"anchor":    {dashNormAlertsTransitionsLog, dashNormAlertsAnchor},
		"one":       {dashNormAlertsTransitionsLog, dashNormAlertsOne},
		"one-clear": {dashNormAlertsTransitionsLog, dashNormAlertsOneClear},
	} {
		if got := diffs(c.logs, c.bodies[0], c.bodies[1]); got != "" {
			t.Errorf("%s: the recorded pair differs at %q", name, got)
		}
	}

	// what the render writes, on the oracle's side: an alert's instance and a transition
	render := func(logBody, body string) (string, []int64) {
		t.Helper()
		n := norm()
		n.observe(logBody)
		log, err := alertsV2LogOf([]byte(logBody))
		if err != nil {
			t.Fatal(err)
		}
		got, clocks := alertsV2Render(n, log, asked, []byte(body))
		return string(got), clocks
	}
	got, clocks := render(dashNormAlertsAlertsLog[0], dashNormAlertsRaised[0])
	for _, want := range []string{`"gi":"G","nm":"ha_low"`, `"tr_i":"t+16","tr_v":70,"tr_t":"WHEN"`, `"v":70,"t":"T"}`} {
		if !strings.Contains(got, want) {
			t.Errorf("the raised alert, rendered, does not hold %s: %s", want, got)
		}
	}
	if want := []int64{1791438106, 1791438106, 1791438106}; !slices.Equal(clocks, want) {
		t.Errorf("the raised alert's seconds %v, want %v", clocks, want)
	}
	got, clocks = render(dashNormAlertsTransitionsLog[0], dashNormAlertsOne[0])
	for _, want := range []string{`{"gi":"G","alert":"hs_calc","transition_id":"t+13",`, `"type":null,"when":"WHEN",`,
		`"old":{"status":"CLEAR","value":10,"duration":"DURATION","raised_duration":0}`,
		`"notification":{"when":"EXEC_RUN","delay":0,"delay_up_to_time":"DELAY_UP_TO",`} {
		if !strings.Contains(got, want) {
			t.Errorf("the transition, rendered, does not hold %s: %s", want, got)
		}
	}
	if want := []int64{1791438148, 1791438148, 13, 1791438148, 1791438148}; !slices.Equal(clocks, want) {
		t.Errorf("the transition's seconds %v, want %v", clocks, want)
	}
	// a first status: its notification never ran, and the 0 stays
	got, _ = render(dashNormAlertsTransitionsLog[0], dashNormAlertsOneClear[0])
	if want := `"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,` +
		`"delay":0,"delay_up_to_time":"DELAY_UP_TO",`; !strings.Contains(got, want) {
		t.Errorf("the first status, rendered, does not hold %s: %s", want, got)
	}
	// members that hold no clock keep their numbers, and a body without a global id is left as it is
	plain := `{"v":70,"tr_v":70,"to":"root","tp":"","delay":0,"duration_ms":3,"total_ms":0.349,"exec_code":0,` +
		`"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","ati":0,"after":2,"when_key":5,"when":7}`
	if got, clocks := render(dashNormAlertsTransitionsLog[0], plain); got != plain || clocks != nil {
		t.Errorf("a body without a global id: rendered %s, the seconds %v", got, clocks)
	}

	// The wrong answers: each planted in the candidate's body (and, where the log must differ for the plant to be
	// one, in the candidate's log), each reported at its member and nowhere else.
	one, oneLog := dashNormAlertsOne, dashNormAlertsTransitionsLog
	anchor := dashNormAlertsAnchor
	raised, alertsLog := dashNormAlertsRaised, dashNormAlertsAlertsLog
	replace := func(s, old, new string) string {
		t.Helper()
		if strings.Count(s, old) != 1 {
			t.Fatalf("the recorded answer holds %q %d times", old, strings.Count(s, old))
		}
		return strings.Replace(s, old, new, 1)
	}
	// the candidate's log with its notification of hs_calc's change run a second after the change
	lateLog := [2]string{oneLog[0], replace(oneLog[1], `"when":1791438148,"duration":14,"non_clear_duration":0,`+
		`"exec_run":1791438148,`, `"when":1791438148,"duration":14,"non_clear_duration":0,"exec_run":1791438149,`)}
	// An alert that never had a status names its link's entry, and C reads one clock as it makes the alert and
	// another for that entry (alertsV2Entry.lastChange). The recorded sides read both in one second; here the
	// candidate's entry for hm_plain is made a second after its alert, so its duration is 1 and its global id of
	// that later second, while the alert's last change stays the second it was made in.
	pretty := dashNormAlertsPretty
	linkLog := [2]string{alertsLog[0], replace(alertsLog[1], `"transition_id":"89805474-7bdb-4c91-bbac-7d1255814c9b",`+
		`"when":1791438103,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791438103,`,
		`"transition_id":"89805474-7bdb-4c91-bbac-7d1255814c9b","when":1791438104,"duration":1,`+
			`"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791438104,`)}
	linked := replace(pretty[1], `"gi":1791438103088474,`, `"gi":1791438104088474,`)
	// An alert that left REMOVED by a status change has an entry from REMOVED as well, to its new status: here the
	// candidate's ha_low came to WARNING from REMOVED, three seconds after it was removed. Its last change is the
	// entry's time, as for any status change.
	backLog := [2]string{alertsLog[0], replace(alertsLog[1], `"status":"WARNING","old_status":"CLEAR"}`,
		`"status":"WARNING","old_status":"REMOVED"}`)}
	const plainChange, lowChange = `"tr_v":null,` + "\n            " + `"tr_t":`, `"tr_v":70,` + "\n            " + `"tr_t":`
	tr := "$.transitions[0]."
	for name, c := range map[string]struct {
		logs              [2]string
		oracle, candidate string
		want              string
	}{
		"a duration off by one": {oneLog, one[0], replace(one[1], `"duration":14,`, `"duration":15,`),
			tr + "old.duration"},
		"a time off by one": {oneLog, one[0], replace(one[1], `"type":null,"when":1791438148,`,
			`"type":null,"when":1791438149,`), tr + "when"},
		"the end of the delay off by one": {oneLog, one[0], replace(one[1], `"delay_up_to_time":1791438148`,
			`"delay_up_to_time":1791438147`), tr + "notification.delay_up_to_time"},
		// the notification's time is the entry's exec_run, not its `when`
		"the notification, run a second later": {lateLog, one[0], replace(one[1],
			`"notification":{"when":1791438148`, `"notification":{"when":1791438149`), ""},
		"the notification's time the transition's": {lateLog, one[0], one[1], tr + "notification.when"},
		"a raised duration off by one": {oneLog, anchor[0], replace(anchor[1], `"raised_duration":32}`,
			`"raised_duration":31}`), "$.transitions[1].old.raised_duration"},
		// a global id is microseconds of its entry's second
		"a global id in seconds": {oneLog, one[0], replace(one[1], `"gi":1791438148875597,`, `"gi":1791438148,`),
			tr + "gi"},
		// C reads the clock for it after the one that gave the entry's time: the seconds of the bound after it
		"a global id two seconds late": {oneLog, one[0], replace(one[1], `"gi":1791438148875597,`,
			`"gi":1791438150875597,`), ""},
		"a global id three seconds late": {oneLog, one[0], replace(one[1], `"gi":1791438148875597,`,
			`"gi":1791438151875597,`), tr + "gi"},
		"a global id a second early": {oneLog, one[0], replace(one[1], `"gi":1791438148875597,`,
			`"gi":1791438147875597,`), tr + "gi"},
		"global ids without microseconds": {oneLog, anchor[0], dashNormWholeSeconds(anchor[1]),
			"$.transitions[0].gi $.transitions[1].gi $.transitions[2].gi $.transitions[3].gi"},
		// an id the side's log does not hold stays as it is, and nothing of its transition is named
		"an id the log does not hold": {oneLog, one[0], replace(one[1], `"transition_id":"3e3dbcb9-`,
			`"transition_id":"4e3dbcb9-`), tr + "gi " + tr + "transition_id " + tr + "when " + tr +
			"old.duration " + tr + "notification.when " + tr + "notification.delay_up_to_time"},
		// an alert that never had a status: its last change is the second it was made in, its entry's time less
		// the entry's duration, and not the entry's time
		"an alert made a second before its link's entry": {linkLog, pretty[0], linked, ""},
		"its last change, the entry's time": {linkLog, pretty[0], replace(linked, plainChange+"1791438103,",
			plainChange+"1791438104,"), "$.alert_instances[0].tr_t"},
		// an alert with a status: its last change is its entry's time, not that less the entry's duration (3 here)
		"a status' time less its duration": {alertsLog, pretty[0], replace(pretty[1], lowChange+"1791438106,",
			lowChange+"1791438103,"), "$.alert_instances[1].tr_t"},
		"an alert back from REMOVED": {backLog, pretty[0], pretty[1], ""},
		"an alert back from REMOVED, its time less the duration": {backLog, pretty[0], replace(pretty[1],
			lowChange+"1791438106,", lowChange+"1791438103,"), "$.alert_instances[1].tr_t"},
		// an alert: its last change is its last entry's, its last evaluation a second of the request or of the two
		// before it
		"an alert's last change off by one": {alertsLog, raised[0], replace(raised[1], `"tr_t":1791438106,`,
			`"tr_t":1791438105,`), "$.alert_instances[0].tr_t"},
		"an alert evaluated three seconds ago": {alertsLog, raised[0], replace(raised[1], `"v":70,"t":1791438106}`,
			`"v":70,"t":1791438103}`), "$.alert_instances[0].t"},
		"an alert evaluated two seconds ago": {alertsLog, raised[0], replace(raised[1], `"v":70,"t":1791438106}`,
			`"v":70,"t":1791438104}`), ""},
		"an alert evaluated after the request": {alertsLog, raised[0], replace(raised[1], `"v":70,"t":1791438106}`,
			`"v":70,"t":1791438107}`), "$.alert_instances[0].t"},
	} {
		if got := diffs(c.logs, c.oracle, c.candidate); got != c.want {
			t.Errorf("%s: differences at %q, want %q", name, got, c.want)
		}
	}
	// an id the log does not hold is left as the agent wrote it, not named
	if got, _ := render(oneLog[1], replace(one[1], `"transition_id":"3e3dbcb9-`, `"transition_id":"4e3dbcb9-`)); !strings.Contains(got,
		`"transition_id":"4e3dbcb9-693f-4ca1-973c-074ef998755f"`) || strings.Contains(got, `"t?"`) {
		t.Errorf("an id the log does not hold, rendered: %s", got)
	}
	// one global id that is a whole second is a microsecond like another (a body with one cannot tell)
	if got := diffs(oneLog, one[0], replace(one[1], `"gi":1791438148875597,`, `"gi":1791438148000000,`)); got != "" {
		t.Errorf("one global id on a whole second: differences at %q", got)
	}

	// the zero pair (health off: the dashboard replay): the v2 envelope's masks alone
	if fam := alertsV2Family([2]*healthNorm{}, nil); fam.render != nil || fam.check != nil || fam.settle != 0 ||
		fam.unordered != nil || fam.now != nil || !slices.Equal(fam.masks, infoV2Volatile) {
		t.Errorf("the zero pair's family renders, checks, waits or masks more than the v2 envelope")
	}

	// with the runner's normalizers: the family's wait and masks, and the bound beside the render: the recorded
	// sides' first statuses were a second apart, within it; four seconds apart they are not
	fam := family(oneLog)
	if fam.render == nil || fam.check == nil || fam.settle != healthCandidateWait || fam.now != nil ||
		!slices.Equal(fam.masks, infoV2Volatile) {
		t.Fatalf("the family with normalizers has no render, no check, or another wait or masks")
	}
	if got := string(fam.render(0, asked, []byte(one[0]))); !strings.Contains(got, `"transition_id":"t+13",`) {
		t.Errorf("the family's render does not name the transition by its side's log: %s", got)
	}
	fam.render(0, asked, []byte(dashNormAlertsOneClear[0]))
	fam.render(1, asked, []byte(dashNormAlertsOneClear[1]))
	fam.check(t, "the first statuses, a second apart", Value{}, Value{})
	_, near := render(oneLog[0], dashNormAlertsOneClear[0])
	_, far := render(strings.ReplaceAll(oneLog[0], "1791438135", "1791438131"),
		strings.ReplaceAll(dashNormAlertsOneClear[0], "1791438135", "1791438131"))
	if len(near) != 3 || len(far) != 3 || healthClocksNear(near, far) == nil {
		t.Errorf("four seconds apart, the bound holds: %v, %v", near, far)
	}

	// a side whose alert log cannot be read: its body is left unparsable, with the reason first
	broken := alertsV2Family([2]*healthNorm{norm(), norm()}, func(int) ([]byte, error) { return nil, fmt.Errorf("no log") })
	if got := string(broken.render(0, asked, []byte(`{"gi":1}`))); got != "the side's alert log: no log\n"+`{"gi":1}` {
		t.Errorf("a side without its alert log: %q", got)
	}

	// the `one-clear` row's guard (alertsV2OneClear): C's answer, and not a change to WARNING
	for name, c := range map[string]struct {
		body string
		bad  bool
	}{
		"C's answer":          {dashNormAlertsOneClear[0], false},
		"a change to WARNING": {dashNormAlertsOne[0], true},
		"no transition": {`{"api":2,"transitions":[],"items":{"evaluated":0,"matched":0,"returned":0,` +
			`"max_to_return":1,"before":0,"after":0}}`, true},
	} {
		rendered, _ := render(oneLog[0], c.body)
		v, err := ParseJSON([]byte(rendered))
		if err != nil {
			t.Fatalf("%v: %s", err, rendered)
		}
		if err := alertsV2OneClear(v); (err != nil) != c.bad {
			t.Errorf("the one-clear guard, %s: %v", name, err)
		}
	}
}

// dashNormWholeSeconds is body with every global id cut to its whole second, in microseconds.
func dashNormWholeSeconds(body string) string {
	return alertsV2ItemRe.ReplaceAllStringFunc(body, func(m string) string {
		return m[:len(m)-6] + "000000"
	})
}
