// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"slices"
	"strings"
	"testing"
)

// The alert families' answers as C gave them C against C (s4, 2026-10-06 20:22-20:24Z, the oracle's side, its run
// directory already `<run>`): `/api/v3/alerts?options=summary,values,instances,minify&status=raised` (`alerts`) and
// `/api/v3/alert_transitions?options=minify&transition=<hs_calc's change to WARNING>` (`transitions`), with the alert log
// entries that name their ids, trimmed from the same run's `/api/v1/alarm_log` (the lowest entry, which sets the base,
// and the entry of the id).
const (
	dashNormAlertsRaised     = `{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791318134275409,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c5738268-36a6-401a-9ac9-4c374b33f3ad","tr_v":70,"tr_t":1791318134,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791318134}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.349}}`
	dashNormAlertsRaisedWant = `{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":"G","nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"t+13","tr_v":70,"tr_t":"T","cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":"T"}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.349}}`
	dashNormAlertsRaisedLog  = `[{"unique_id": 1791318132, "alarm_id": 1791318132, "alarm_event_id": 1, "name": "ha_low", "transition_id": "2b3a5a8a-1815-422d-b401-92d5373d06ca"}, {"unique_id": 1791318144, "alarm_id": 1791318132, "alarm_event_id": 5, "name": "ha_low", "transition_id": "c5738268-36a6-401a-9ac9-4c374b33f3ad"}]`
	dashNormAlertsOne        = `{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":1791318193745297,"alert":"hs_calc","transition_id":"7f50f733-a185-4e4c-8266-6ae2a83bbce7","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791318193,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791318193,"delay":0,"delay_up_to_time":1791318193,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.151}}`
	dashNormAlertsOneWant    = `{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":"G","alert":"hs_calc","transition_id":"t+13","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"T","info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":"T","raised_duration":0},"notification":{"when":"T","delay":0,"delay_up_to_time":"T","flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.151}}`
	dashNormAlertsOneLog     = `[{"unique_id": 1791318180, "alarm_id": 1791318180, "alarm_event_id": 1, "name": "hs_calc", "transition_id": "0668e082-efef-4bc9-9085-ac59c241416c"}, {"unique_id": 1791318192, "alarm_id": 1791318180, "alarm_event_id": 5, "name": "hs_calc", "transition_id": "7f50f733-a185-4e4c-8266-6ae2a83bbce7"}]`
)

// testDashNormAlerts pins the alert families on the bodies above: what alertsV2Render replaces and keeps, the seconds
// it hands to the bound, and the families with the health runner's normalizers and with the zero pair.
func testDashNormAlerts(t *testing.T) {
	norm := func(log string) *healthNorm {
		n := &healthNorm{entries: map[int64]healthEntry{}, tids: map[string]int64{}, moved: map[string]int64{}}
		n.observe(log)
		return n
	}
	for name, c := range map[string]struct {
		n          *healthNorm
		body, want string
		clocks     []int64
		unchanged  bool
	}{
		// gi as G, tr_t and t as T, tr_i by the entry that first showed it (u+13 of the log's base)
		"an alert's clocks and last transition": {n: norm(dashNormAlertsRaisedLog), body: dashNormAlertsRaised,
			want: dashNormAlertsRaisedWant, clocks: []int64{1791318134, 1791318134, 1791318134}},
		// gi, when, the old status' duration, the notification's time and the delay's end; the raised duration of a
		// change from CLEAR is 0 and kept
		"a transition's clocks, spans and id": {n: norm(dashNormAlertsOneLog), body: dashNormAlertsOne, want: dashNormAlertsOneWant,
			clocks: []int64{1791318193, 1791318193, 14, 1791318193, 1791318193}},
		// spaces after the colon, an id the log never showed, a clock that is 0
		"spaces, an unknown id, a zero": {n: norm(dashNormAlertsRaisedLog),
			body:   `{"gi": 1791318134275409, "tr_i": "0668e082-efef-4bc9-9085-ac59c241416c", "tr_t":0, "when":  12}`,
			want:   `{"gi": "G", "tr_i": "t?", "tr_t":0, "when":  "T"}`,
			clocks: []int64{1791318134, 12}},
		// members that hold no clock keep their numbers, whatever their name starts or ends with
		"not clocks": {n: norm(dashNormAlertsRaisedLog),
			body: `{"v":70,"tr_v":70,"to":"root","tp":"","delay":0,"duration_ms":3,"total_ms":0.349,"exec_code":0,` +
				`"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","ati":0,"after":2,"when_key":5}`,
			unchanged: true},
	} {
		got, clocks := alertsV2Render(c.n, []byte(c.body))
		want := c.want
		if c.unchanged {
			want = c.body
		}
		if string(got) != want {
			t.Errorf("%s: rendered\n%s\nwant\n%s", name, got, want)
		}
		if !slices.Equal(clocks, c.clocks) {
			t.Errorf("%s: the seconds %v, want %v", name, clocks, c.clocks)
		}
	}

	// the zero pair (health off: the dashboard replay): the v2 envelope's masks alone
	if fam := alertsV2Family([2]*healthNorm{}); fam.render != nil || fam.check != nil || fam.settle != 0 ||
		fam.unordered != nil || fam.now != nil || !slices.Equal(fam.masks, infoV2Volatile) {
		t.Errorf("the zero pair's family renders, checks, waits or masks more than the v2 envelope")
	}

	// with the runner's normalizers each side renders with its own; one second apart (as C was against C in that run's
	// by-name row) the rendered answers are equal and the bound holds, three seconds apart it does not
	n := [2]*healthNorm{norm(dashNormAlertsRaisedLog), norm(dashNormAlertsRaisedLog)}
	fam := alertsV2Family(n)
	if fam.render == nil || fam.check == nil || fam.settle != healthCandidateWait || fam.now != nil ||
		!slices.Equal(fam.masks, infoV2Volatile) {
		t.Fatalf("the family with normalizers has no render, no check, or another wait or masks")
	}
	o := fam.render(0, []byte(dashNormAlertsRaised))
	c := fam.render(1, []byte(strings.ReplaceAll(dashNormAlertsRaised, "1791318134", "1791318133")))
	if string(o) != string(c) || string(o) != dashNormAlertsRaisedWant {
		t.Errorf("one second apart the renders differ:\n%s\n%s", o, c)
	}
	fam.check(t, "one second apart", Value{}, Value{})
	_, oc := alertsV2Render(n[0], []byte(dashNormAlertsRaised))
	_, cc := alertsV2Render(n[1], []byte(strings.ReplaceAll(dashNormAlertsRaised, "1791318134", "1791318131")))
	if healthClocksNear(oc, cc) == nil {
		t.Errorf("three seconds apart, the bound holds: %v, %v", oc, cc)
	}
}
