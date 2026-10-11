// SPDX-License-Identifier: GPL-3.0-or-later

package parity

// The closer rows of `api.v2-alerts` (SA-C) as one C-against-C run answered them, both sides (0 the oracle): each
// row's bodies with the side's run directory written `<run>` as v2Round writes it, the seconds each request was in
// flight, and the names of the alert logs the family read for it (dashNormClosersCLogs: each side's
// `/api/v1/alarm_log`, trimmed to the members the render reads; one per case and side: the longest read, which
// holds the entries of the earlier ones; none for a row of the search). Generated from the hooked copy's dumps
// (probe p2, run p2a, C against C, 2026-10-10 14:12:22-14:13:35Z).

// dashNormClosersCRow is one recorded row.
type dashNormClosersCRow struct {
	// target is what the row asked (its v2Req.target)
	target string
	bodies [2]string
	flight [2][2]int64
	// logs are the names of each side's alert log; empty for a row the alerts family does not compare
	logs [2]string
}

// dashNormClosersCRows are the recorded rows by `<case>/<row>`.
var dashNormClosersCRows = map[string]dashNormClosersCRow{
	"alerts/values-uninitialized": {
		target: "/api/v3/alerts?options=values,minify&status=uninitialized",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"nm":"hm_plain","ch":"hsig.plain","ch_n":"hsig.plain","v":null,"t":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.038}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"nm":"hm_plain","ch":"hsig.plain","ch_n":"hsig.plain","v":null,"t":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.029}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/values-alone": {
		target: "/api/v3/alerts?options=values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"nm":"hm_plain","ch":"hsig.plain","ch_n":"hsig.plain","v":null,"t":0},{"ni":0,"nm":"ha_low","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565},{"ni":0,"nm":"ha_mid","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565},{"ni":0,"nm":"ha_high","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.042}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"nm":"hm_plain","ch":"hsig.plain","ch_n":"hsig.plain","v":null,"t":0},{"ni":0,"nm":"ha_low","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565},{"ni":0,"nm":"ha_mid","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565},{"ni":0,"nm":"ha_high","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.029}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/values-summary": {
		target: "/api/v3/alerts?options=summary,values,minify&status=raised",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"nm":"ha_low","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.065}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"nm":"ha_low","ch":"hsig.values","ch_n":"hsig.values","v":70,"t":1791641565}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.042}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/status-pipe": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=clear%7Cwarning",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641565715051,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c19d7484-5587-4dc6-9f4c-95929cbeaa37","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791641561713608,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"6ea0d3fe-1543-4408-b62f-2c2a3834281e","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561713741,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"091b289d-1d8a-4314-b245-e74d8e4cc206","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.072}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641565616872,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c4f40889-bb7a-43f4-b483-4ca5f5c5070a","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791641561615012,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"9d7c4e96-c2e6-498e-a72d-34f921506c0d","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561615120,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"78917a74-067f-4609-ae19-e7df3fb8c80f","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.061}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/status-blank": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=clear%20warning",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641565715051,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c19d7484-5587-4dc6-9f4c-95929cbeaa37","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791641561713608,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"6ea0d3fe-1543-4408-b62f-2c2a3834281e","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561713741,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"091b289d-1d8a-4314-b245-e74d8e4cc206","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.086}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641565616872,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c4f40889-bb7a-43f4-b483-4ca5f5c5070a","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791641561615012,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"9d7c4e96-c2e6-498e-a72d-34f921506c0d","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561615120,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"78917a74-067f-4609-ae19-e7df3fb8c80f","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.083}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/status-plus": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=clear+warning",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641565715051,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c19d7484-5587-4dc6-9f4c-95929cbeaa37","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791641561713608,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"6ea0d3fe-1543-4408-b62f-2c2a3834281e","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561713741,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"091b289d-1d8a-4314-b245-e74d8e4cc206","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.118}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641565616872,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c4f40889-bb7a-43f4-b483-4ca5f5c5070a","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791641561615012,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"9d7c4e96-c2e6-498e-a72d-34f921506c0d","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561615120,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"78917a74-067f-4609-ae19-e7df3fb8c80f","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.064}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/status-upper": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=RAISED",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641561707250,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"78e6662e-a5f6-45c2-8476-a6e47b2f91a9","tr_v":null,"tr_t":1791641561,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791641565715051,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c19d7484-5587-4dc6-9f4c-95929cbeaa37","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561713608,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"6ea0d3fe-1543-4408-b62f-2c2a3834281e","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791641561713741,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"091b289d-1d8a-4314-b245-e74d8e4cc206","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.127}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641561614333,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"3c8b1092-a579-4760-905f-1b279c2e4303","tr_v":null,"tr_t":1791641561,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791641565616872,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c4f40889-bb7a-43f4-b483-4ca5f5c5070a","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561615012,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"9d7c4e96-c2e6-498e-a72d-34f921506c0d","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791641561615120,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"78917a74-067f-4609-ae19-e7df3fb8c80f","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.086}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/active-debug": {
		target: "/api/v3/alerts?options=summary,debug&status=active",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","summary"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":["raised"],
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":0,
            "before":0
        }
    },
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "alerts":[{
            "ati":0,
            "ni":[0],
            "nm":"ha_low",
            "sum":"",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        }],
    "alerts_by_type":[{
            "name":"Parity Check",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_component":[{
            "name":"Fixture Part",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_classification":[{
            "name":"Workload",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_recipient":[{
            "name":"root",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":0,
            "available":3
        },{
            "name":"silent",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_module":[{
            "name":"parity.mod",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.067
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","summary"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":["raised"],
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":0,
            "before":0
        }
    },
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "alerts":[{
            "ati":0,
            "ni":[0],
            "nm":"ha_low",
            "sum":"",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        }],
    "alerts_by_type":[{
            "name":"Parity Check",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_component":[{
            "name":"Fixture Part",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_classification":[{
            "name":"Workload",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_recipient":[{
            "name":"root",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":0,
            "available":3
        },{
            "name":"silent",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_module":[{
            "name":"parity.mod",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.085
    }
}
`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/debug-statuses": {
		target: "/api/v3/alerts?options=debug,summary&status=uninitialized,undefined,clear,active",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","summary"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":["uninitialized","undefined","clear","raised"],
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":0,
            "before":0
        }
    },
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "alerts":[{
            "ati":0,
            "ni":[0],
            "nm":"hm_plain",
            "sum":"",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":["Workload"],
            "cp":["Fixture_Part"],
            "ty":["Parity_Check"],
            "to":["silent"]
        },{
            "ati":1,
            "ni":[0],
            "nm":"ha_low",
            "sum":"",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        },{
            "ati":2,
            "ni":[0],
            "nm":"ha_mid",
            "sum":"",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        },{
            "ati":3,
            "ni":[0],
            "nm":"ha_high",
            "sum":"",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        }],
    "alerts_by_type":[{
            "name":"Parity Check",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        }],
    "alerts_by_component":[{
            "name":"Fixture Part",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        }],
    "alerts_by_classification":[{
            "name":"Workload",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        }],
    "alerts_by_recipient":[{
            "name":"silent",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        },{
            "name":"root",
            "cr":0,
            "wr":1,
            "cl":2,
            "er":0,
            "running":3,
            "running_silent":0,
            "available":3
        }],
    "alerts_by_module":[{
            "name":"[none]",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1
        },{
            "name":"parity.mod",
            "cr":0,
            "wr":1,
            "cl":2,
            "er":0,
            "running":3,
            "running_silent":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.09
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","summary"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":["uninitialized","undefined","clear","raised"],
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":0,
            "before":0
        }
    },
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "alerts":[{
            "ati":0,
            "ni":[0],
            "nm":"hm_plain",
            "sum":"",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":["Workload"],
            "cp":["Fixture_Part"],
            "ty":["Parity_Check"],
            "to":["silent"]
        },{
            "ati":1,
            "ni":[0],
            "nm":"ha_low",
            "sum":"",
            "cr":0,
            "wr":1,
            "cl":0,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        },{
            "ati":2,
            "ni":[0],
            "nm":"ha_mid",
            "sum":"",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        },{
            "ati":3,
            "ni":[0],
            "nm":"ha_high",
            "sum":"",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "in":1,
            "nd":1,
            "cfg":1,
            "ctx":["hsig.ctx"],
            "cls":[],
            "cp":[],
            "ty":[],
            "to":["root"]
        }],
    "alerts_by_type":[{
            "name":"Parity Check",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        }],
    "alerts_by_component":[{
            "name":"Fixture Part",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        }],
    "alerts_by_classification":[{
            "name":"Workload",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        }],
    "alerts_by_recipient":[{
            "name":"silent",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1,
            "available":1
        },{
            "name":"root",
            "cr":0,
            "wr":1,
            "cl":2,
            "er":0,
            "running":3,
            "running_silent":0,
            "available":3
        }],
    "alerts_by_module":[{
            "name":"[none]",
            "cr":0,
            "wr":0,
            "cl":0,
            "er":0,
            "running":1,
            "running_silent":1
        },{
            "name":"parity.mod",
            "cr":0,
            "wr":1,
            "cl":2,
            "er":0,
            "running":3,
            "running_silent":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.07
    }
}
`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/plain-cardinality": {
		target: "/api/v3/alerts?options=summary,instances,minify&cardinality=1",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641561707250,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"78e6662e-a5f6-45c2-8476-a6e47b2f91a9","tr_v":null,"tr_t":1791641561,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791641565715051,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c19d7484-5587-4dc6-9f4c-95929cbeaa37","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561713608,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"6ea0d3fe-1543-4408-b62f-2c2a3834281e","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791641561713741,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"091b289d-1d8a-4314-b245-e74d8e4cc206","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.119}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641561614333,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"3c8b1092-a579-4760-905f-1b279c2e4303","tr_v":null,"tr_t":1791641561,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791641565616872,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c4f40889-bb7a-43f4-b483-4ca5f5c5070a","tr_v":70,"tr_t":1791641565,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791641561615012,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"9d7c4e96-c2e6-498e-a72d-34f921506c0d","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791641561615120,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"78917a74-067f-4609-ae19-e7df3fb8c80f","tr_v":10,"tr_t":1791641561,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.114}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/debug-rfc3339": {
		target: "/api/v3/alerts?options=debug,rfc3339&after=-600",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","rfc3339"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":[],
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":-600,
            "before":null
        }
    },
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.055
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","rfc3339"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":[],
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":-600,
            "before":null
        }
    },
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.018
    }
}
`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/mcp-instances-only": {
		target: "/api/v3/alerts?options=mcp,instances",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","78e6662e-a5f6-45c2-8476-a6e47b2f91a9",null,1791641561,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload"],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","c19d7484-5587-4dc6-9f4c-95929cbeaa37",70,1791641565,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","",""],
        ["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","6ea0d3fe-1543-4408-b62f-2c2a3834281e",10,1791641561,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","",""],
        ["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","091b289d-1d8a-4314-b245-e74d8e4cc206",10,1791641561,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","",""]]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","3c8b1092-a579-4760-905f-1b279c2e4303",null,1791641561,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload"],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","c4f40889-bb7a-43f4-b483-4ca5f5c5070a",70,1791641565,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","",""],
        ["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","9d7c4e96-c2e6-498e-a72d-34f921506c0d",10,1791641561,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","",""],
        ["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","78917a74-067f-4609-ae19-e7df3fb8c80f",10,1791641561,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","",""]]
}
`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/mcp-values-only": {
		target: "/api/v3/alerts?options=mcp,values",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Instance Name","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.plain",null,0],
        ["ha_low","parity-parent","hsig.values",70,1791641565],
        ["ha_mid","parity-parent","hsig.values",70,1791641565],
        ["ha_high","parity-parent","hsig.values",70,1791641565]]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Instance Name","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.plain",null,0],
        ["ha_low","parity-parent","hsig.values",70,1791641565],
        ["ha_mid","parity-parent","hsig.values",70,1791641565],
        ["ha_high","parity-parent","hsig.values",70,1791641565]]
}
`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/alert-case": {
		target: "/api/v3/alerts?options=summary,instances,minify&alert=HA_LOW%7Cha_mid",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641561713608,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"6ea0d3fe-1543-4408-b62f-2c2a3834281e","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.061}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791641561615012,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"9d7c4e96-c2e6-498e-a72d-34f921506c0d","tr_v":10,"tr_t":1791641561,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.084}}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/v2-mcp": {
		target: "/api/v2/alerts?options=mcp,summary,instances,values,minify",
		bodies: [2]string{
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true}],"all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],"all_alerts":[["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1],["ha_mid","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1],["ha_high","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1]],"alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],"alert_instances":[["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","78e6662e-a5f6-45c2-8476-a6e47b2f91a9",null,1791641561,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,0],["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","c19d7484-5587-4dc6-9f4c-95929cbeaa37",70,1791641565,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791641565],["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","6ea0d3fe-1543-4408-b62f-2c2a3834281e",10,1791641561,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","","",70,1791641565],["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","091b289d-1d8a-4314-b245-e74d8e4cc206",10,1791641561,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","","",70,1791641565]]}`,
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true}],"all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],"all_alerts":[["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1],["ha_mid","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1],["ha_high","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1]],"alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],"alert_instances":[["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","3c8b1092-a579-4760-905f-1b279c2e4303",null,1791641561,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,0],["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","c4f40889-bb7a-43f4-b483-4ca5f5c5070a",70,1791641565,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791641565],["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","9d7c4e96-c2e6-498e-a72d-34f921506c0d",10,1791641561,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","","",70,1791641565],["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","78917a74-067f-4609-ae19-e7df3fb8c80f",10,1791641561,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","","",70,1791641565]]}`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"L01", "L02"},
	},
	"alerts/q-alert": {
		target: "/api/v2/q?q=ha_low",
		bodies: [2]string{
			`{
    "api":2,
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0,
            "st":{
                "ai":0,
                "code":200,
                "msg":""
            }
        }],
    "contexts":{
    },
    "searches":{
        "strings":8,
        "char":0,
        "total":8
    },
    "versions":{
        "routing_hard_hash":1,
        "nodes_hard_hash":1,
        "contexts_hard_hash":6,
        "contexts_soft_hash":0,
        "alerts_hard_hash":12,
        "alerts_soft_hash":16
    },
    "agents":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nd":null,
            "nm":"parity-parent",
            "now":1791641566,
            "ai":0,
            "timings":{
                "prep_ms":0,
                "query_ms":0.042,
                "output_ms":0.082,
                "total_ms":0.124,
                "cloud_ms":0.124
            }
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.124
    }
}
`,
			`{
    "api":2,
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0,
            "st":{
                "ai":0,
                "code":200,
                "msg":""
            }
        }],
    "contexts":{
    },
    "searches":{
        "strings":8,
        "char":0,
        "total":8
    },
    "versions":{
        "routing_hard_hash":1,
        "nodes_hard_hash":1,
        "contexts_hard_hash":6,
        "contexts_soft_hash":0,
        "alerts_hard_hash":12,
        "alerts_soft_hash":16
    },
    "agents":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nd":null,
            "nm":"parity-parent",
            "now":1791641566,
            "ai":0,
            "timings":{
                "prep_ms":0,
                "query_ms":0.013,
                "output_ms":0.019,
                "total_ms":0.032,
                "cloud_ms":0.032
            }
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.032
    }
}
`,
		},
		flight: [2][2]int64{{1791641566, 1791641566}, {1791641566, 1791641566}},
		logs:   [2]string{"", ""},
	},
	"two-charts/summary": {
		target: "/api/v3/alerts?options=summary,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hcl_named","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hcl.alone"],"cls":[],"cp":[],"ty":[],"to":["silent"]},{"ati":1,"ni":[0],"nm":"hcl_up","sum":"","cr":0,"wr":2,"cl":0,"er":0,"in":2,"nd":1,"cfg":1,"ctx":["hcl.two"],"cls":["Utilization"],"cp":["Two_Charts"],"ty":["Closer_Check"],"to":["silent_sysadmin"]},{"ati":2,"ni":[0],"nm":"hcl_flat","sum":"","cr":0,"wr":0,"cl":2,"er":0,"in":2,"nd":1,"cfg":1,"ctx":["hcl.two"],"cls":[],"cp":[],"ty":[],"to":["silent"]}],"alerts_by_type":[{"name":"Closer Check","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"Off Kind","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Two Charts","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"Off Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Utilization","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"Off Class","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":2,"er":0,"running":3,"running_silent":3,"available":2},{"name":"silent sysadmin","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"offline","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":1,"er":0,"running":3,"running_silent":2},{"name":"closers","cr":0,"wr":1,"cl":1,"er":0,"running":2,"running_silent":1}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.241}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hcl_named","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hcl.alone"],"cls":[],"cp":[],"ty":[],"to":["silent"]},{"ati":1,"ni":[0],"nm":"hcl_up","sum":"","cr":0,"wr":2,"cl":0,"er":0,"in":2,"nd":1,"cfg":1,"ctx":["hcl.two"],"cls":["Utilization"],"cp":["Two_Charts"],"ty":["Closer_Check"],"to":["silent_sysadmin"]},{"ati":2,"ni":[0],"nm":"hcl_flat","sum":"","cr":0,"wr":0,"cl":2,"er":0,"in":2,"nd":1,"cfg":1,"ctx":["hcl.two"],"cls":[],"cp":[],"ty":[],"to":["silent"]}],"alerts_by_type":[{"name":"Closer Check","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"Off Kind","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Two Charts","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"Off Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Utilization","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"Off Class","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":2,"er":0,"running":3,"running_silent":3,"available":2},{"name":"silent sysadmin","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":1},{"name":"offline","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":1,"er":0,"running":3,"running_silent":2},{"name":"closers","cr":0,"wr":1,"cl":1,"er":0,"running":2,"running_silent":1}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.12}}`,
		},
		flight: [2][2]int64{{1791641614, 1791641614}, {1791641614, 1791641614}},
		logs:   [2]string{"L03", "L04"},
	},
	"two-charts/instances": {
		target: "/api/v3/alerts?options=instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"gi":1791641613454982,"nm":"hcl_named","ctx":"hcl.alone","ch":"hcl.named","ch_n":"hcl.shown","st":"UNINITIALIZED","fami":"family","info":"b of the named chart","sum":"","units":"things","tr_i":"248175ef-1b49-4cc8-a776-289041145107","tr_v":null,"tr_t":1791641613,"cfg":"1d6d20ea-4f51-4790-b946-52b075fd7e28","src":"line=1,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"","cm":"","cl":"","v":null,"t":0},{"ni":0,"gi":1791641613455152,"nm":"hcl_up","ctx":"hcl.two","ch":"hcl.two","ch_n":"hcl.two","st":"WARNING","fami":"family","info":"both charts are up","sum":"","units":"things","tr_i":"e5b339d4-4094-4804-bb17-1399a5c6137d","tr_v":1,"tr_t":1791641613,"cfg":"1ded9710-be30-448c-8258-9f6b74e974d9","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"silent sysadmin","tp":"Closer Check","cm":"Two Charts","cl":"Utilization","v":1,"t":1791641613},{"ni":0,"gi":1791641613461282,"nm":"hcl_flat","ctx":"hcl.two","ch":"hcl.two","ch_n":"hcl.two","st":"CLEAR","fami":"family","info":"both charts are flat","sum":"","units":"things","tr_i":"cfc10733-9cd2-483d-9d61-4f82c838e205","tr_v":1,"tr_t":1791641613,"cfg":"19a5b700-c442-4531-a041-2103320c99b3","src":"line=22,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"","cm":"","cl":"","v":1,"t":1791641613},{"ni":0,"gi":1791641613461405,"nm":"hcl_up","ctx":"hcl.two","ch":"hcl.values","ch_n":"hcl.values","st":"WARNING","fami":"family","info":"both charts are up","sum":"","units":"things","tr_i":"798496a5-284e-413d-a361-ac5b02ae6910","tr_v":1,"tr_t":1791641613,"cfg":"1ded9710-be30-448c-8258-9f6b74e974d9","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"silent sysadmin","tp":"Closer Check","cm":"Two Charts","cl":"Utilization","v":1,"t":1791641613},{"ni":0,"gi":1791641613461538,"nm":"hcl_flat","ctx":"hcl.two","ch":"hcl.values","ch_n":"hcl.values","st":"CLEAR","fami":"family","info":"both charts are flat","sum":"","units":"things","tr_i":"2bc8d9d1-43f3-456e-96b1-4fa13d4a4c02","tr_v":1,"tr_t":1791641613,"cfg":"19a5b700-c442-4531-a041-2103320c99b3","src":"line=22,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"","cm":"","cl":"","v":1,"t":1791641613}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.053}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"gi":1791641614247286,"nm":"hcl_named","ctx":"hcl.alone","ch":"hcl.named","ch_n":"hcl.shown","st":"UNINITIALIZED","fami":"family","info":"b of the named chart","sum":"","units":"things","tr_i":"12cd9ed2-77ee-4282-9c85-aaac26f85a3b","tr_v":null,"tr_t":1791641614,"cfg":"1d6d20ea-4f51-4790-b946-52b075fd7e28","src":"line=1,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"","cm":"","cl":"","v":null,"t":0},{"ni":0,"gi":1791641614247439,"nm":"hcl_up","ctx":"hcl.two","ch":"hcl.two","ch_n":"hcl.two","st":"WARNING","fami":"family","info":"both charts are up","sum":"","units":"things","tr_i":"db250db1-43ed-4cf1-bb70-a2828be37b7b","tr_v":1,"tr_t":1791641614,"cfg":"1ded9710-be30-448c-8258-9f6b74e974d9","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"silent sysadmin","tp":"Closer Check","cm":"Two Charts","cl":"Utilization","v":1,"t":1791641614},{"ni":0,"gi":1791641614247871,"nm":"hcl_flat","ctx":"hcl.two","ch":"hcl.two","ch_n":"hcl.two","st":"CLEAR","fami":"family","info":"both charts are flat","sum":"","units":"things","tr_i":"8d4a3e3c-63a8-44d8-8e10-98b00386ca3c","tr_v":1,"tr_t":1791641614,"cfg":"19a5b700-c442-4531-a041-2103320c99b3","src":"line=22,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"","cm":"","cl":"","v":1,"t":1791641614},{"ni":0,"gi":1791641614248046,"nm":"hcl_up","ctx":"hcl.two","ch":"hcl.values","ch_n":"hcl.values","st":"WARNING","fami":"family","info":"both charts are up","sum":"","units":"things","tr_i":"9e258823-1cd5-457e-a61a-8a6e7095d6bb","tr_v":1,"tr_t":1791641614,"cfg":"1ded9710-be30-448c-8258-9f6b74e974d9","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"silent sysadmin","tp":"Closer Check","cm":"Two Charts","cl":"Utilization","v":1,"t":1791641614},{"ni":0,"gi":1791641614248247,"nm":"hcl_flat","ctx":"hcl.two","ch":"hcl.values","ch_n":"hcl.values","st":"CLEAR","fami":"family","info":"both charts are flat","sum":"","units":"things","tr_i":"5a979d67-4615-4c5c-ab8b-e8c21f5c6d0d","tr_v":1,"tr_t":1791641614,"cfg":"19a5b700-c442-4531-a041-2103320c99b3","src":"line=22,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"","cm":"","cl":"","v":1,"t":1791641614}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.045}}`,
		},
		flight: [2][2]int64{{1791641614, 1791641614}, {1791641614, 1791641614}},
		logs:   [2]string{"L03", "L04"},
	},
	"two-charts/values": {
		target: "/api/v3/alerts?options=values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"nm":"hcl_named","ch":"hcl.named","ch_n":"hcl.shown","v":null,"t":0},{"ni":0,"nm":"hcl_up","ch":"hcl.two","ch_n":"hcl.two","v":1,"t":1791641613},{"ni":0,"nm":"hcl_flat","ch":"hcl.two","ch_n":"hcl.two","v":1,"t":1791641613},{"ni":0,"nm":"hcl_up","ch":"hcl.values","ch_n":"hcl.values","v":1,"t":1791641613},{"ni":0,"nm":"hcl_flat","ch":"hcl.values","ch_n":"hcl.values","v":1,"t":1791641613}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.036}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"nm":"hcl_named","ch":"hcl.named","ch_n":"hcl.shown","v":null,"t":0},{"ni":0,"nm":"hcl_up","ch":"hcl.two","ch_n":"hcl.two","v":1,"t":1791641614},{"ni":0,"nm":"hcl_flat","ch":"hcl.two","ch_n":"hcl.two","v":1,"t":1791641614},{"ni":0,"nm":"hcl_up","ch":"hcl.values","ch_n":"hcl.values","v":1,"t":1791641614},{"ni":0,"nm":"hcl_flat","ch":"hcl.values","ch_n":"hcl.values","v":1,"t":1791641614}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.034}}`,
		},
		flight: [2][2]int64{{1791641614, 1791641614}, {1791641614, 1791641614}},
		logs:   [2]string{"L03", "L04"},
	},
	"two-charts/mcp-values": {
		target: "/api/v3/alerts?options=mcp,values",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Instance Name","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hcl_named","parity-parent","hcl.shown",null,0],
        ["hcl_up","parity-parent","hcl.two",1,1791641613],
        ["hcl_flat","parity-parent","hcl.two",1,1791641613],
        ["hcl_up","parity-parent","hcl.values",1,1791641613],
        ["hcl_flat","parity-parent","hcl.values",1,1791641613]]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Instance Name","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hcl_named","parity-parent","hcl.shown",null,0],
        ["hcl_up","parity-parent","hcl.two",1,1791641614],
        ["hcl_flat","parity-parent","hcl.two",1,1791641614],
        ["hcl_up","parity-parent","hcl.values",1,1791641614],
        ["hcl_flat","parity-parent","hcl.values",1,1791641614]]
}
`,
		},
		flight: [2][2]int64{{1791641614, 1791641614}, {1791641614, 1791641614}},
		logs:   [2]string{"L03", "L04"},
	},
	"two-charts/mcp-summary": {
		target: "/api/v3/alerts?options=mcp,summary,minify",
		bodies: [2]string{
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true}],"all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],"all_alerts":[["hcl_named","","hcl.alone",null,null,null,"silent",0,0,0,0,1,1,1],["hcl_up","","hcl.two","Utilization","Two_Charts","Closer_Check","silent_sysadmin",0,2,0,0,2,1,1],["hcl_flat","","hcl.two",null,null,null,"silent",0,0,2,0,2,1,1]]}`,
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true}],"all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],"all_alerts":[["hcl_named","","hcl.alone",null,null,null,"silent",0,0,0,0,1,1,1],["hcl_up","","hcl.two","Utilization","Two_Charts","Closer_Check","silent_sysadmin",0,2,0,0,2,1,1],["hcl_flat","","hcl.two",null,null,null,"silent",0,0,2,0,2,1,1]]}`,
		},
		flight: [2][2]int64{{1791641614, 1791641614}, {1791641614, 1791641614}},
		logs:   [2]string{"L03", "L04"},
	},
	"stock-rules/groupings": {
		target: "/api/v3/alerts?options=summary,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"System","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":281},{"name":"Web Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":53},{"name":"Kubernetes","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":34},{"name":"Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":118},{"name":"Power Supply","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Other","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":207},{"name":"DNS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Containers","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":21},{"name":"Database","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":204},{"name":"Certificates","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Netdata","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":9},{"name":"Messaging","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":83},{"name":"Data Sharing","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"DHCP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"SearchEngine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Network","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":17},{"name":"Ad Filtering","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"ServiceMesh","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"GPU","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Virtual Machine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"KV Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"NetworkDevice","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":27},{"name":"Switch","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Application Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Linux","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Message Queue","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Streaming","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Cgroups","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Computing","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"ethereum_node","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Network","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":39},{"name":"Web log","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"AKS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"ScaleIO","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"UPS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"UPS device","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"TCP endpoint","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"API Management","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"API Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Ceph","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":100},{"name":"Azure VMSS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":26},{"name":"Memory","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":15},{"name":"IPMI","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Deployment","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"CronJob","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"DNS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Application Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":9},{"name":"Container Apps","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Cosmos DB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":10},{"name":"Azure Key Vault","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"AWS EC2","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"AWS ALB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS NLB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS EBS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS NAT Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS EFS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"AWS ECS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"AWS OpenSearch","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"AWS ElastiCache","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS MSK","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"AWS RDS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"AWS Site-to-Site VPN","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS SNS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Azure Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"DB engine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Azure Service Bus","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":13},{"name":"Kubelet","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":9},{"name":"Clock","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Azure Stream Analytics","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Azure Data Factory","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":26},{"name":"Log Analytics","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":15},{"name":"Azure SQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"IPFS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"DHCPd","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"go.d.plugin","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Elasticsearch","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"PostgreSQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":14},{"name":"Azure Load Balancer","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Cato Networks","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Pi-hole","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Unbound","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Azure ML","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":16},{"name":"RAID","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":14},{"name":"VerneMQ","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":26},{"name":"Azure SQL Elastic Pool","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":14},{"name":"Battery","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"RabbitMQ","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Consul","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"Retroshare","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"CPU","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"WHOIS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"App Service","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"NVIDIA","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Disk","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"VMware vCenter","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":15},{"name":"HDFS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Nagios","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Beanstalk","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Memcached","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Load","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"File system","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":16},{"name":"IPC","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"SNMP traps","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":20},{"name":"OS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"VPC","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"HSRP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"BFD","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"NTP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"NX-OS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Data Explorer","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":21},{"name":"Event Grid","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"WebSphere PMI","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Azure ACR","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Application Insights","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":16},{"name":"MySQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"Redis","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Docker","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"S3","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"Azure VPN Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":18},{"name":"Systemd units","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Azure Container Instances","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Azure NAT Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"IBM MQ","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"ProxySQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Azure VM","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":24},{"name":"Microsoft SQL Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Processes","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Azure Synapse","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"Palo Alto Networks NGFW","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"AS/400","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Streaming","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Audit","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Cognitive Services","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":17},{"name":"Hardware","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":114},{"name":"Redfish","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Cryptography","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"x509 certificates","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"BOINC","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"WebSphere MP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"python.d.plugin","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Dnsmasq","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Azure Front Door","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Azure PostgreSQL Flexible","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"Azure Event Hubs","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"CockroachDB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"ClickHouse","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":10},{"name":"Process","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"IoT Hub","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":17},{"name":"HA","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Intrusion Prevention","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Wireless","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"HTTP endpoint","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"ML","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"DB2","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"Exporting engine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"WebSphere","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"Logic Apps","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"BGP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Azure SQL MI","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"LVM","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Azure Firewall","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Azure MySQL Flexible","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"Riak KV","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"ExpressRoute","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"ExpressRoute Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Licensing","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Azure Redis","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"geth","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":561},{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":108},{"name":"Latency","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":126},{"name":"Utilization","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":300},{"name":"Availability","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":54},{"name":"Error","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Performance","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Backup","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":139},{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":31},{"name":"webmaster","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":27},{"name":"sysadmin","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":788},{"name":"sitemgr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":13},{"name":"dba","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":177}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.76}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"System","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":281},{"name":"Web Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":53},{"name":"Kubernetes","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":34},{"name":"Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":118},{"name":"Power Supply","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Other","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":207},{"name":"DNS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Containers","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":21},{"name":"Database","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":204},{"name":"Certificates","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Netdata","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":9},{"name":"Messaging","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":83},{"name":"Data Sharing","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"DHCP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"SearchEngine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Network","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":17},{"name":"Ad Filtering","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"ServiceMesh","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"GPU","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Virtual Machine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"KV Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"NetworkDevice","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":27},{"name":"Switch","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Application Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Linux","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Message Queue","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Streaming","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Cgroups","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Computing","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"ethereum_node","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Network","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":39},{"name":"Web log","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"AKS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"ScaleIO","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"UPS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"UPS device","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"TCP endpoint","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"API Management","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"API Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Ceph","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":100},{"name":"Azure VMSS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":26},{"name":"Memory","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":15},{"name":"IPMI","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Deployment","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"CronJob","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"DNS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Application Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":9},{"name":"Container Apps","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Cosmos DB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":10},{"name":"Azure Key Vault","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"AWS EC2","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"AWS ALB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS NLB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS EBS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS NAT Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS EFS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"AWS ECS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"AWS OpenSearch","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"AWS ElastiCache","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS MSK","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"AWS RDS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"AWS Site-to-Site VPN","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"AWS SNS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Azure Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"DB engine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Azure Service Bus","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":13},{"name":"Kubelet","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":9},{"name":"Clock","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Azure Stream Analytics","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Azure Data Factory","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":26},{"name":"Log Analytics","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":15},{"name":"Azure SQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"IPFS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"DHCPd","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"go.d.plugin","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Elasticsearch","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"PostgreSQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":14},{"name":"Azure Load Balancer","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Cato Networks","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Pi-hole","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Unbound","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Azure ML","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":16},{"name":"RAID","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":14},{"name":"VerneMQ","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":26},{"name":"Azure SQL Elastic Pool","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":14},{"name":"Battery","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"RabbitMQ","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Consul","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"Retroshare","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"CPU","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"WHOIS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"App Service","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"NVIDIA","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Disk","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"VMware vCenter","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":15},{"name":"HDFS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Nagios","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Beanstalk","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Memcached","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Load","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"File system","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":16},{"name":"IPC","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"SNMP traps","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":20},{"name":"OS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"VPC","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"HSRP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"BFD","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"NTP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"NX-OS","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Data Explorer","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":21},{"name":"Event Grid","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"WebSphere PMI","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Azure ACR","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Application Insights","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":16},{"name":"MySQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"Redis","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Docker","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"S3","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"Azure VPN Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":18},{"name":"Systemd units","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Azure Container Instances","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Azure NAT Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"IBM MQ","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"ProxySQL","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Azure VM","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":24},{"name":"Microsoft SQL Server","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Processes","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Azure Synapse","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":8},{"name":"Palo Alto Networks NGFW","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"AS/400","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"Streaming","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Audit","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Cognitive Services","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":17},{"name":"Hardware","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":114},{"name":"Redfish","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Cryptography","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"x509 certificates","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"BOINC","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"WebSphere MP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":4},{"name":"python.d.plugin","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Dnsmasq","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Azure Front Door","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Azure PostgreSQL Flexible","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"Azure Event Hubs","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"CockroachDB","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"ClickHouse","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":10},{"name":"Process","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"IoT Hub","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":17},{"name":"HA","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Intrusion Prevention","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"Wireless","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"HTTP endpoint","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"ML","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1},{"name":"DB2","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"Exporting engine","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"WebSphere","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"Logic Apps","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":12},{"name":"BGP","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Azure SQL MI","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"LVM","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Azure Firewall","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"Azure MySQL Flexible","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":19},{"name":"Riak KV","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"ExpressRoute","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":6},{"name":"ExpressRoute Gateway","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"Storage","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"Licensing","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":7},{"name":"Azure Redis","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":11},{"name":"geth","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":561},{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":108},{"name":"Latency","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":126},{"name":"Utilization","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":300},{"name":"Availability","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":54},{"name":"Error","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Performance","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":5},{"name":"Backup","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":139},{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":31},{"name":"webmaster","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":27},{"name":"sysadmin","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":788},{"name":"sitemgr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":13},{"name":"dba","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":177}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.808}}`,
		},
		flight: [2][2]int64{{1791641602, 1791641602}, {1791641602, 1791641602}},
		logs:   [2]string{"L05", "L05"},
	},
}

// dashNormClosersCLogs are the recorded alert logs by name.
var dashNormClosersCLogs = map[string]string{
	"L01": `[{"unique_id":1791641577,"alarm_id":1791641563,"alarm_event_id":5,"name":"ha_low","transition_id":"c19d7484-5587-4dc6-9f4c-95929cbeaa37","when":1791641565,"duration":4,"non_clear_duration":0,"exec_run":1791641565,"delay_up_to_timestamp":1791641565,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791641576,"alarm_id":1791641565,"alarm_event_id":4,"name":"ha_high","transition_id":"091b289d-1d8a-4314-b245-e74d8e4cc206","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641575,"alarm_id":1791641564,"alarm_event_id":4,"name":"ha_mid","transition_id":"6ea0d3fe-1543-4408-b62f-2c2a3834281e","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641574,"alarm_id":1791641563,"alarm_event_id":4,"name":"ha_low","transition_id":"07aa1f61-95b5-46b3-86f0-5fc6908775b6","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641573,"alarm_id":1791641565,"alarm_event_id":3,"name":"ha_high","transition_id":"af828f45-6217-46dd-b57d-cb4bff401520","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641572,"alarm_id":1791641564,"alarm_event_id":3,"name":"ha_mid","transition_id":"a6bcf1a7-0f07-4c8a-97e7-226d2a1ba701","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641571,"alarm_id":1791641563,"alarm_event_id":3,"name":"ha_low","transition_id":"92ead0b0-dcb7-411c-9c86-933a108ab2c8","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641570,"alarm_id":1791641565,"alarm_event_id":2,"name":"ha_high","transition_id":"cd0a940c-d992-4212-aaba-e851e218dbe8","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641569,"alarm_id":1791641564,"alarm_event_id":2,"name":"ha_mid","transition_id":"9646dac1-bdcc-4040-a64b-f8f5fd027c05","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641568,"alarm_id":1791641563,"alarm_event_id":2,"name":"ha_low","transition_id":"1e28ae46-cf6d-420f-a293-ed4b9bb0de90","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641567,"alarm_id":1791641562,"alarm_event_id":3,"name":"hm_plain","transition_id":"78e6662e-a5f6-45c2-8476-a6e47b2f91a9","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641566,"alarm_id":1791641562,"alarm_event_id":2,"name":"hm_plain","transition_id":"7afaf9cc-f737-4d72-a01a-10176a342bdd","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641565,"alarm_id":1791641565,"alarm_event_id":1,"name":"ha_high","transition_id":"166670ff-f607-4642-be99-97614ba28855","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641564,"alarm_id":1791641564,"alarm_event_id":1,"name":"ha_mid","transition_id":"83c90ab0-1596-4347-a79b-189c779ebdce","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641563,"alarm_id":1791641563,"alarm_event_id":1,"name":"ha_low","transition_id":"79bab82f-0a95-4fb1-9aa0-b50abfe3c6cc","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641562,"alarm_id":1791641562,"alarm_event_id":1,"name":"hm_plain","transition_id":"92236351-ccca-4f1c-b3b5-08e294b74f03","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L02": `[{"unique_id":1791641577,"alarm_id":1791641563,"alarm_event_id":5,"name":"ha_low","transition_id":"c4f40889-bb7a-43f4-b483-4ca5f5c5070a","when":1791641565,"duration":4,"non_clear_duration":0,"exec_run":1791641565,"delay_up_to_timestamp":1791641565,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791641576,"alarm_id":1791641565,"alarm_event_id":4,"name":"ha_high","transition_id":"78917a74-067f-4609-ae19-e7df3fb8c80f","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641575,"alarm_id":1791641564,"alarm_event_id":4,"name":"ha_mid","transition_id":"9d7c4e96-c2e6-498e-a72d-34f921506c0d","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641574,"alarm_id":1791641563,"alarm_event_id":4,"name":"ha_low","transition_id":"8a68a61b-6d27-4c05-b5f7-e77298fb4521","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641573,"alarm_id":1791641565,"alarm_event_id":3,"name":"ha_high","transition_id":"716b8b32-f30b-45c2-a895-42ec667979ec","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641572,"alarm_id":1791641564,"alarm_event_id":3,"name":"ha_mid","transition_id":"a4c01ba3-dba6-489f-9364-75456a88df45","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641571,"alarm_id":1791641563,"alarm_event_id":3,"name":"ha_low","transition_id":"132ba189-eeac-4c29-907d-e2caad364169","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641570,"alarm_id":1791641565,"alarm_event_id":2,"name":"ha_high","transition_id":"dc9f3987-7c33-4cc4-a6b8-f4f2098952aa","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641569,"alarm_id":1791641564,"alarm_event_id":2,"name":"ha_mid","transition_id":"9ab758e7-b0bb-4760-b70d-5a71d9ded65b","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641568,"alarm_id":1791641563,"alarm_event_id":2,"name":"ha_low","transition_id":"158e6616-4b93-48e9-906f-166cfdd6cfdd","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641567,"alarm_id":1791641562,"alarm_event_id":3,"name":"hm_plain","transition_id":"3c8b1092-a579-4760-905f-1b279c2e4303","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641566,"alarm_id":1791641562,"alarm_event_id":2,"name":"hm_plain","transition_id":"faf57bc2-a714-463f-acdb-cada5dd2ddf5","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641565,"alarm_id":1791641565,"alarm_event_id":1,"name":"ha_high","transition_id":"4be735a8-ab08-4cf7-a1dc-a258049125fc","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641564,"alarm_id":1791641564,"alarm_event_id":1,"name":"ha_mid","transition_id":"862f78c5-72de-4256-946f-54a943ed1ec8","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641563,"alarm_id":1791641563,"alarm_event_id":1,"name":"ha_low","transition_id":"cf594520-665e-4cae-bbef-4a8f0b8fb059","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641562,"alarm_id":1791641562,"alarm_event_id":1,"name":"hm_plain","transition_id":"bee2189a-2b54-44f3-ab72-ee855869a6f6","when":1791641561,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641561,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L03": `[{"unique_id":1791641632,"alarm_id":1791641618,"alarm_event_id":4,"name":"hcl_flat","transition_id":"2bc8d9d1-43f3-456e-96b1-4fa13d4a4c02","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641631,"alarm_id":1791641617,"alarm_event_id":4,"name":"hcl_up","transition_id":"798496a5-284e-413d-a361-ac5b02ae6910","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":1791641613,"delay_up_to_timestamp":1791641613,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791641630,"alarm_id":1791641616,"alarm_event_id":4,"name":"hcl_flat","transition_id":"cfc10733-9cd2-483d-9d61-4f82c838e205","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641629,"alarm_id":1791641615,"alarm_event_id":4,"name":"hcl_up","transition_id":"e5b339d4-4094-4804-bb17-1399a5c6137d","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":1791641613,"delay_up_to_timestamp":1791641613,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791641628,"alarm_id":1791641618,"alarm_event_id":3,"name":"hcl_flat","transition_id":"8e42ceda-77ef-418a-8c88-0c75af987d64","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641627,"alarm_id":1791641617,"alarm_event_id":3,"name":"hcl_up","transition_id":"5ec81b1f-882e-49c4-9db5-91f9e01f0b3a","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641626,"alarm_id":1791641618,"alarm_event_id":2,"name":"hcl_flat","transition_id":"54cf8ca9-263b-4b04-8517-a41b49fc8b54","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641625,"alarm_id":1791641617,"alarm_event_id":2,"name":"hcl_up","transition_id":"1259f6ae-9fcd-40ab-9aba-dfd0e34854f9","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641624,"alarm_id":1791641616,"alarm_event_id":3,"name":"hcl_flat","transition_id":"7547e8ca-bc05-40f2-8386-c28532c26c41","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641623,"alarm_id":1791641615,"alarm_event_id":3,"name":"hcl_up","transition_id":"e78b1049-92f2-40b4-9023-93ba9f74dbab","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641622,"alarm_id":1791641616,"alarm_event_id":2,"name":"hcl_flat","transition_id":"d0835da3-987a-4e31-9fde-acbbb50610a7","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641621,"alarm_id":1791641615,"alarm_event_id":2,"name":"hcl_up","transition_id":"165af686-9bb6-4933-983c-94f05991d048","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641620,"alarm_id":1791641614,"alarm_event_id":3,"name":"hcl_named","transition_id":"248175ef-1b49-4cc8-a776-289041145107","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641619,"alarm_id":1791641614,"alarm_event_id":2,"name":"hcl_named","transition_id":"03d68870-d20f-4c4c-9597-f4a9d444b0a4","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641618,"alarm_id":1791641618,"alarm_event_id":1,"name":"hcl_flat","transition_id":"72ff73c4-69e5-480d-a4cd-761654fa3507","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641617,"alarm_id":1791641617,"alarm_event_id":1,"name":"hcl_up","transition_id":"993e10fb-d0cf-49e7-8cc5-d87bb81200bd","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641616,"alarm_id":1791641616,"alarm_event_id":1,"name":"hcl_flat","transition_id":"190b15c9-1f2d-4a2f-b035-80932801427d","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641615,"alarm_id":1791641615,"alarm_event_id":1,"name":"hcl_up","transition_id":"1ca75e35-bb9c-4f25-b132-826a2f928050","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641614,"alarm_id":1791641614,"alarm_event_id":1,"name":"hcl_named","transition_id":"8863e86f-18f1-41db-a865-ad00536174a1","when":1791641613,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641613,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L04": `[{"unique_id":1791641633,"alarm_id":1791641619,"alarm_event_id":4,"name":"hcl_flat","transition_id":"5a979d67-4615-4c5c-ab8b-e8c21f5c6d0d","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641632,"alarm_id":1791641618,"alarm_event_id":4,"name":"hcl_up","transition_id":"9e258823-1cd5-457e-a61a-8a6e7095d6bb","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":1791641614,"delay_up_to_timestamp":1791641614,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791641631,"alarm_id":1791641617,"alarm_event_id":4,"name":"hcl_flat","transition_id":"8d4a3e3c-63a8-44d8-8e10-98b00386ca3c","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791641630,"alarm_id":1791641616,"alarm_event_id":4,"name":"hcl_up","transition_id":"db250db1-43ed-4cf1-bb70-a2828be37b7b","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":1791641614,"delay_up_to_timestamp":1791641614,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791641629,"alarm_id":1791641619,"alarm_event_id":3,"name":"hcl_flat","transition_id":"dee8986f-9904-4005-ad06-e7c23703224e","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641628,"alarm_id":1791641618,"alarm_event_id":3,"name":"hcl_up","transition_id":"4818092c-a13d-4c53-92ad-879dc011da5a","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641627,"alarm_id":1791641619,"alarm_event_id":2,"name":"hcl_flat","transition_id":"fd98aeb3-d4d4-428f-8491-3e3d231e9b55","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641626,"alarm_id":1791641618,"alarm_event_id":2,"name":"hcl_up","transition_id":"76d589e4-9329-4a71-a449-64f4e2820337","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641625,"alarm_id":1791641617,"alarm_event_id":3,"name":"hcl_flat","transition_id":"c067a89f-20f9-4c1c-80eb-2b6ab179581e","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641624,"alarm_id":1791641616,"alarm_event_id":3,"name":"hcl_up","transition_id":"9bb37c52-a433-4a29-a0c7-47636bb87915","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641623,"alarm_id":1791641617,"alarm_event_id":2,"name":"hcl_flat","transition_id":"23555726-36f6-4c98-bc02-7dbfb1d19fc7","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641622,"alarm_id":1791641616,"alarm_event_id":2,"name":"hcl_up","transition_id":"82d788e2-76dd-4941-bed2-f2155173de36","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641621,"alarm_id":1791641615,"alarm_event_id":3,"name":"hcl_named","transition_id":"12cd9ed2-77ee-4282-9c85-aaac26f85a3b","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641620,"alarm_id":1791641615,"alarm_event_id":2,"name":"hcl_named","transition_id":"769f5967-ac2d-4100-afb3-d1aef62db50c","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791641619,"alarm_id":1791641619,"alarm_event_id":1,"name":"hcl_flat","transition_id":"6ff7f6d9-9391-4d0b-9fa0-d3e02d291c19","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641618,"alarm_id":1791641618,"alarm_event_id":1,"name":"hcl_up","transition_id":"f0cc6d5c-f188-4023-86d0-c26bf120810b","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641617,"alarm_id":1791641617,"alarm_event_id":1,"name":"hcl_flat","transition_id":"d675ba48-6b82-4aa6-b8b8-d3a29bd2b5b3","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641616,"alarm_id":1791641616,"alarm_event_id":1,"name":"hcl_up","transition_id":"80417417-ff73-4887-b6f3-4db20b21f24f","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791641615,"alarm_id":1791641615,"alarm_event_id":1,"name":"hcl_named","transition_id":"ead9b64c-568b-4b99-87b1-3aecbcb83acb","when":1791641614,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791641614,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L05": `[]`,
}
