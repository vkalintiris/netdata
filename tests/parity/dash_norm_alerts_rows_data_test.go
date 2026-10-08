// SPDX-License-Identifier: GPL-3.0-or-later

package parity

// The alerts rows of D234 F2 and F3 as one C-against-C run answered them, both sides (0 the oracle): each row's
// bodies with the side's run directory written `<run>` as v2Round writes it, the seconds each request was in
// flight, and the names of the alert logs the family read for it (dashNormAlertsLogs: each side's
// `/api/v1/alarm_log`, the agent's own and, for the rows of two hosts, the child's, trimmed to the members the
// render and the views read; one per case, side and host: the longest read, which holds the entries of the earlier
// ones); and the raw rows' answers whole. Generated from the hooked copy's dumps
// (H36's run P6, 2026-10-08 13:19-13:22Z).

// dashNormAlertsRow is one recorded row.
type dashNormAlertsRow struct {
	// target is what the row asked: its v2Req.target, which names each side's own id where the row has one
	target string
	bodies [2]string
	flight [2][2]int64
	// logs are the names of each side's own alert log and, with host, of that other host's
	logs, more [2]string
	host       string
}

// dashNormAlertsRows are the recorded rows by `<case>/<row>`.
var dashNormAlertsRows = map[string]dashNormAlertsRow{
	"alerts/raised": {
		target: "/api/v3/alerts?options=summary,values,instances,minify&status=raised",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.364}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.15}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"two-hosts/alerts": {
		target: "/api/v3/alerts?options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[1],"nm":"hch_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hchild.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"fanout","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465754286217,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"71dd129e-df5f-4360-baa3-b373ab1fb1aa","tr_v":70,"tr_t":1791465754,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465754},{"ati":1,"ni":1,"gi":1791465754297708,"nm":"hch_calc","ctx":"hchild.ctx","ch":"hchild.values","ch_n":"hchild.values","st":"WARNING","fami":"family","info":"the child s last value of a","sum":"","units":"things","tr_i":"42575a46-5fbf-45d9-bfcb-3e6c0f36c049","tr_v":70,"tr_t":1791465754,"cfg":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465754}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.443}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[1],"nm":"hch_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hchild.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":2,"cl":0,"er":0,"running":2,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"fanout","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465754027712,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"f472b8c0-962f-4ea5-8965-f0dcb54f56e4","tr_v":70,"tr_t":1791465754,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465755},{"ati":1,"ni":1,"gi":1791465755044646,"nm":"hch_calc","ctx":"hchild.ctx","ch":"hchild.values","ch_n":"hchild.values","st":"WARNING","fami":"family","info":"the child s last value of a","sum":"","units":"things","tr_i":"af135c4f-c9a5-4538-8249-7f12c4f5e9b2","tr_v":70,"tr_t":1791465755,"cfg":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465755}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.119}}`,
		},
		flight: [2][2]int64{{1791465755, 1791465755}, {1791465755, 1791465755}},
		logs:   [2]string{"L03", "L04"}, more: [2]string{"L05", "L06"}, host: "health-child",
	},
	"alerts/configs": {
		target: "/api/v3/alerts?options=minify,summary",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.076}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.055}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"two-hosts/alerts-mcp": {
		target: "/api/v3/alerts?options=mcp,instances,values",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        },{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000c1",
            "hostname":"health-child",
            "relationship":"child",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hloc_calc","parity-parent","hloc.ctx","hloc.values","WARNING","family","localhost s last value of a","","things","71dd129e-df5f-4360-baa3-b373ab1fb1aa",70,1791465754,"74b7ae27-c469-4178-939f-9d2d07296c22","line=10,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465754],
        ["hch_calc","health-child","hchild.ctx","hchild.values","WARNING","family","the child s last value of a","","things","42575a46-5fbf-45d9-bfcb-3e6c0f36c049",70,1791465754,"c7437f24-7140-4a8e-bf1f-ff52ef52214c","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465754]]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        },{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000c1",
            "hostname":"health-child",
            "relationship":"child",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hloc_calc","parity-parent","hloc.ctx","hloc.values","WARNING","family","localhost s last value of a","","things","f472b8c0-962f-4ea5-8965-f0dcb54f56e4",70,1791465754,"74b7ae27-c469-4178-939f-9d2d07296c22","line=10,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465755],
        ["hch_calc","health-child","hchild.ctx","hchild.values","WARNING","family","the child s last value of a","","things","af135c4f-c9a5-4538-8249-7f12c4f5e9b2",70,1791465755,"c7437f24-7140-4a8e-bf1f-ff52ef52214c","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465755]]
}
`,
		},
		flight: [2][2]int64{{1791465755, 1791465755}, {1791465755, 1791465755}},
		logs:   [2]string{"L03", "L04"}, more: [2]string{"L05", "L06"}, host: "health-child",
	},
	"alerts/by-name": {
		target: "/api/v3/alerts?options=summary,values,instances,minify&alert=ha_mid%7Cha_high",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583466818,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"a82ae512-576f-4841-b1e2-0c20b5aade00","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587},{"ati":1,"ni":0,"gi":1791465583466901,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"0181f8e1-7c89-4236-9568-3ea915317cd3","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.062}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583514572,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"770fcb6c-764d-4df3-bb3f-53c54c95b43d","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587},{"ati":1,"ni":0,"gi":1791465583514730,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"09926cdb-fdef-47ce-9d05-5556b8811e62","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.055}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/v2": {
		target: "/api/v2/alerts?options=summary,instances,values",
		bodies: [2]string{
			`{
    "api":2,
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
    "alert_instances":[{
            "ati":0,
            "ni":0,
            "gi":1791465583459827,
            "nm":"hm_plain",
            "ctx":"hsig.ctx",
            "ch":"hsig.plain",
            "ch_n":"hsig.plain",
            "st":"UNINITIALIZED",
            "fami":"family",
            "info":"b above 1000",
            "sum":"",
            "units":"things",
            "tr_i":"57cfc223-97a6-4399-83b1-8517cdacdcc5",
            "tr_v":null,
            "tr_t":1791465583,
            "cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1",
            "src":"line=27,file=<run>/etc/health.d/parity.conf",
            "to":"silent",
            "tp":"Parity Check",
            "cm":"Fixture Part",
            "cl":"Workload",
            "v":null,
            "t":0
        },{
            "ati":1,
            "ni":0,
            "gi":1791465587467917,
            "nm":"ha_low",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"WARNING",
            "fami":"family",
            "info":"a above 50",
            "sum":"",
            "units":"things",
            "tr_i":"a8554664-9427-451c-a84a-e411647c4f55",
            "tr_v":70,
            "tr_t":1791465587,
            "cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3",
            "src":"line=2,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        },{
            "ati":2,
            "ni":0,
            "gi":1791465583466818,
            "nm":"ha_mid",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"CLEAR",
            "fami":"family",
            "info":"a above 100",
            "sum":"",
            "units":"things",
            "tr_i":"a82ae512-576f-4841-b1e2-0c20b5aade00",
            "tr_v":10,
            "tr_t":1791465583,
            "cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a",
            "src":"line=11,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        },{
            "ati":3,
            "ni":0,
            "gi":1791465583466901,
            "nm":"ha_high",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"CLEAR",
            "fami":"family",
            "info":"a above 200",
            "sum":"",
            "units":"things",
            "tr_i":"0181f8e1-7c89-4236-9568-3ea915317cd3",
            "tr_v":10,
            "tr_t":1791465583,
            "cfg":"d59244af-a365-4eee-836e-0475985cfd6b",
            "src":"line=19,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.063
    }
}
`,
			`{
    "api":2,
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
    "alert_instances":[{
            "ati":0,
            "ni":0,
            "gi":1791465583514253,
            "nm":"hm_plain",
            "ctx":"hsig.ctx",
            "ch":"hsig.plain",
            "ch_n":"hsig.plain",
            "st":"UNINITIALIZED",
            "fami":"family",
            "info":"b above 1000",
            "sum":"",
            "units":"things",
            "tr_i":"cf3caaf4-7f12-476c-b551-288022d4fac0",
            "tr_v":null,
            "tr_t":1791465583,
            "cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1",
            "src":"line=27,file=<run>/etc/health.d/parity.conf",
            "to":"silent",
            "tp":"Parity Check",
            "cm":"Fixture Part",
            "cl":"Workload",
            "v":null,
            "t":0
        },{
            "ati":1,
            "ni":0,
            "gi":1791465587515730,
            "nm":"ha_low",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"WARNING",
            "fami":"family",
            "info":"a above 50",
            "sum":"",
            "units":"things",
            "tr_i":"c9536868-a35e-4d21-a854-80e7703f5478",
            "tr_v":70,
            "tr_t":1791465587,
            "cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3",
            "src":"line=2,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        },{
            "ati":2,
            "ni":0,
            "gi":1791465583514572,
            "nm":"ha_mid",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"CLEAR",
            "fami":"family",
            "info":"a above 100",
            "sum":"",
            "units":"things",
            "tr_i":"770fcb6c-764d-4df3-bb3f-53c54c95b43d",
            "tr_v":10,
            "tr_t":1791465583,
            "cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a",
            "src":"line=11,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        },{
            "ati":3,
            "ni":0,
            "gi":1791465583514730,
            "nm":"ha_high",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"CLEAR",
            "fami":"family",
            "info":"a above 200",
            "sum":"",
            "units":"things",
            "tr_i":"09926cdb-fdef-47ce-9d05-5556b8811e62",
            "tr_v":10,
            "tr_t":1791465583,
            "cfg":"d59244af-a365-4eee-836e-0475985cfd6b",
            "src":"line=19,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.067
    }
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/clear": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=clear",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583466818,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"a82ae512-576f-4841-b1e2-0c20b5aade00","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791465583466901,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"0181f8e1-7c89-4236-9568-3ea915317cd3","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.044}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583514572,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"770fcb6c-764d-4df3-bb3f-53c54c95b43d","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791465583514730,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"09926cdb-fdef-47ce-9d05-5556b8811e62","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.037}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/critical": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=critical",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.036}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.019}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/bogus": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=bogus",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583459827,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"57cfc223-97a6-4399-83b1-8517cdacdcc5","tr_v":null,"tr_t":1791465583,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791465583466818,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"a82ae512-576f-4841-b1e2-0c20b5aade00","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791465583466901,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"0181f8e1-7c89-4236-9568-3ea915317cd3","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.099}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583514253,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"cf3caaf4-7f12-476c-b551-288022d4fac0","tr_v":null,"tr_t":1791465583,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791465583514572,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"770fcb6c-764d-4df3-bb3f-53c54c95b43d","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791465583514730,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"09926cdb-fdef-47ce-9d05-5556b8811e62","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.061}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/active": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=active",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.045}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.055}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/replaced": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=clear&status=critical",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.042}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.029}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/uninitialized": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=uninitialized",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583459827,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"57cfc223-97a6-4399-83b1-8517cdacdcc5","tr_v":null,"tr_t":1791465583,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.057}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583514253,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"cf3caaf4-7f12-476c-b551-288022d4fac0","tr_v":null,"tr_t":1791465583,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.083}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/critical-window": {
		target: "/api/v3/alerts?options=summary,minify&status=critical&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.023}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.017}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/contexts-nomatch": {
		target: "/api/v3/alerts?options=summary,instances,minify&contexts=nomatch*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583459827,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"57cfc223-97a6-4399-83b1-8517cdacdcc5","tr_v":null,"tr_t":1791465583,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791465583466818,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"a82ae512-576f-4841-b1e2-0c20b5aade00","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791465583466901,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"0181f8e1-7c89-4236-9568-3ea915317cd3","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.078}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465583514253,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"cf3caaf4-7f12-476c-b551-288022d4fac0","tr_v":null,"tr_t":1791465583,"cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload"},{"ati":1,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791465583514572,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"770fcb6c-764d-4df3-bb3f-53c54c95b43d","tr_v":10,"tr_t":1791465583,"cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":3,"ni":0,"gi":1791465583514730,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"09926cdb-fdef-47ce-9d05-5556b8811e62","tr_v":10,"tr_t":1791465583,"cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.065}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/negative": {
		target: "/api/v3/alerts?options=summary,minify&alert=!ha_low%20*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.043}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.015}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/negative-or": {
		target: "/api/v3/alerts?options=summary,minify&alert=!ha_low%7C*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.066}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":0,"cl":2,"er":0,"running":2,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.048}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/alert-star": {
		target: "/api/v3/alerts?options=summary,minify&alert=*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.088}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.05}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/alert-wordless": {
		target: "/api/v3/alerts?options=summary,minify&alert=,",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.055}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hm_plain","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":["Workload"],"cp":["Fixture_Part"],"ty":["Parity_Check"],"to":["silent"]},{"ati":1,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"ha_mid","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"ha_high","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1}],"alerts_by_recipient":[{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1,"available":1},{"name":"root","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":1},{"name":"parity.mod","cr":0,"wr":1,"cl":2,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.046}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/long-debug": {
		target: "/api/v3/alerts?options=summary,instances,values,long-json-keys,debug&status=warning,critical",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","instances","values","summary","long-json-keys"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":["warning","critical"],
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
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "nodes_array_index":0
        }],
    "alerts":[{
            "alerts_array_index_id":0,
            "nodes_array_index":[0],
            "alert":"ha_low",
            "summary":"",
            "critical":0,
            "warning":1,
            "clear":0,
            "error":0,
            "instances_count":1,
            "nodes_count":1,
            "configurations_count":1,
            "contexts":["hsig.ctx"],
            "classifications":[],
            "components":[],
            "types":[],
            "recipients":["root"]
        }],
    "alerts_by_type":[{
            "name":"Parity Check",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_component":[{
            "name":"Fixture Part",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_classification":[{
            "name":"Workload",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_recipient":[{
            "name":"root",
            "critical":0,
            "warning":1,
            "clear":0,
            "error":0,
            "running":1,
            "running_silent":0,
            "available":3
        },{
            "name":"silent",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_module":[{
            "name":"parity.mod",
            "critical":0,
            "warning":1,
            "clear":0,
            "error":0,
            "running":1,
            "running_silent":0
        }],
    "alert_instances":[{
            "alerts_array_index_id":0,
            "nodes_array_index":0,
            "global_id":1791465587467917,
            "alert":"ha_low",
            "context":"hsig.ctx",
            "instance_id":"hsig.values",
            "instance":"hsig.values",
            "status":"WARNING",
            "family":"family",
            "info":"a above 50",
            "summary":"",
            "units":"things",
            "last_transition_id":"a8554664-9427-451c-a84a-e411647c4f55",
            "last_transition_value":70,
            "last_transition_timestamp":1791465587,
            "config_hash_id":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3",
            "source":"line=2,file=<run>/etc/health.d/parity.conf",
            "recipients":"root",
            "type":"",
            "component":"",
            "classification":"",
            "last_updated_value":70,
            "last_updated_timestamp":1791465587
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.132
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","instances","values","summary","long-json-keys"],
        "scope":{
            "scope_nodes":null,
            "scope_contexts":null
        },
        "selectors":{
            "nodes":null,
            "contexts":null,
            "alerts":{
                "status":["warning","critical"],
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
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "nodes_array_index":0
        }],
    "alerts":[{
            "alerts_array_index_id":0,
            "nodes_array_index":[0],
            "alert":"ha_low",
            "summary":"",
            "critical":0,
            "warning":1,
            "clear":0,
            "error":0,
            "instances_count":1,
            "nodes_count":1,
            "configurations_count":1,
            "contexts":["hsig.ctx"],
            "classifications":[],
            "components":[],
            "types":[],
            "recipients":["root"]
        }],
    "alerts_by_type":[{
            "name":"Parity Check",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_component":[{
            "name":"Fixture Part",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_classification":[{
            "name":"Workload",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_recipient":[{
            "name":"root",
            "critical":0,
            "warning":1,
            "clear":0,
            "error":0,
            "running":1,
            "running_silent":0,
            "available":3
        },{
            "name":"silent",
            "critical":0,
            "warning":0,
            "clear":0,
            "error":0,
            "running":0,
            "running_silent":0,
            "available":1
        }],
    "alerts_by_module":[{
            "name":"parity.mod",
            "critical":0,
            "warning":1,
            "clear":0,
            "error":0,
            "running":1,
            "running_silent":0
        }],
    "alert_instances":[{
            "alerts_array_index_id":0,
            "nodes_array_index":0,
            "global_id":1791465587515730,
            "alert":"ha_low",
            "context":"hsig.ctx",
            "instance_id":"hsig.values",
            "instance":"hsig.values",
            "status":"WARNING",
            "family":"family",
            "info":"a above 50",
            "summary":"",
            "units":"things",
            "last_transition_id":"c9536868-a35e-4d21-a854-80e7703f5478",
            "last_transition_value":70,
            "last_transition_timestamp":1791465587,
            "config_hash_id":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3",
            "source":"line=2,file=<run>/etc/health.d/parity.conf",
            "recipients":"root",
            "type":"",
            "component":"",
            "classification":"",
            "last_updated_value":70,
            "last_updated_timestamp":1791465587
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.042
    }
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/debug-selectors": {
		target: "/api/v3/alerts?options=summary,instances,values,configurations,debug,minify&alert=ha_low&scope_nodes=*&nodes=*&scope_contexts=hsig*&contexts=*sig*&after=-600&before=0&cardinality=3",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["minify","debug","instances","values","summary"],
        "scope":{
            "scope_nodes":"*",
            "scope_contexts":"hsig*"
        },
        "selectors":{
            "nodes":"*",
            "contexts":"*sig*",
            "alerts":{
                "status":[],
                "alert":"ha_low",
                "transition":null
            }
        },
        "filters":{
            "after":-600,
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
    "alert_instances":[{
            "ati":0,
            "ni":0,
            "gi":1791465587467917,
            "nm":"ha_low",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"WARNING",
            "fami":"family",
            "info":"a above 50",
            "sum":"",
            "units":"things",
            "tr_i":"a8554664-9427-451c-a84a-e411647c4f55",
            "tr_v":70,
            "tr_t":1791465587,
            "cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3",
            "src":"line=2,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.051
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["minify","debug","instances","values","summary"],
        "scope":{
            "scope_nodes":"*",
            "scope_contexts":"hsig*"
        },
        "selectors":{
            "nodes":"*",
            "contexts":"*sig*",
            "alerts":{
                "status":[],
                "alert":"ha_low",
                "transition":null
            }
        },
        "filters":{
            "after":-600,
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
    "alert_instances":[{
            "ati":0,
            "ni":0,
            "gi":1791465587515730,
            "nm":"ha_low",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"WARNING",
            "fami":"family",
            "info":"a above 50",
            "sum":"",
            "units":"things",
            "tr_i":"c9536868-a35e-4d21-a854-80e7703f5478",
            "tr_v":70,
            "tr_t":1791465587,
            "cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3",
            "src":"line=2,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":70,
            "t":1791465587
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.099
    }
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/mcp-summary": {
		target: "/api/v3/alerts?options=mcp,summary&cardinality=1",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1]],
    "__all_alerts_info__":{
        "status":"truncated",
        "total_alerts":4,
        "shown_alerts":1,
        "cardinality_limit":1
    }
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1]],
    "__all_alerts_info__":{
        "status":"truncated",
        "total_alerts":4,
        "shown_alerts":1,
        "cardinality_limit":1
    }
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/mcp-instances": {
		target: "/api/v3/alerts?options=mcp,instances,values",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","57cfc223-97a6-4399-83b1-8517cdacdcc5",null,1791465583,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,0],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","a8554664-9427-451c-a84a-e411647c4f55",70,1791465587,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","a82ae512-576f-4841-b1e2-0c20b5aade00",10,1791465583,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","0181f8e1-7c89-4236-9568-3ea915317cd3",10,1791465583,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587]]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","cf3caaf4-7f12-476c-b551-288022d4fac0",null,1791465583,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,0],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","c9536868-a35e-4d21-a854-80e7703f5478",70,1791465587,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","770fcb6c-764d-4df3-bb3f-53c54c95b43d",10,1791465583,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","09926cdb-fdef-47ce-9d05-5556b8811e62",10,1791465583,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587]]
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/mcp-rfc3339": {
		target: "/api/v3/alerts?options=mcp,summary,instances,values,rfc3339&cardinality=2",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1]],
    "__all_alerts_info__":{
        "status":"truncated",
        "total_alerts":4,
        "shown_alerts":2,
        "cardinality_limit":2
    },
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","57cfc223-97a6-4399-83b1-8517cdacdcc5",null,"2026-10-08T13:19:43Z","8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,null],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","a8554664-9427-451c-a84a-e411647c4f55",70,"2026-10-08T13:19:47Z","9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,"2026-10-08T13:19:47Z"]],
    "__alert_instances_info__":{
        "status":"truncated",
        "total_instances":4,
        "shown_instances":2,
        "cardinality_limit":2
    }
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1]],
    "__all_alerts_info__":{
        "status":"truncated",
        "total_alerts":4,
        "shown_alerts":2,
        "cardinality_limit":2
    },
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","cf3caaf4-7f12-476c-b551-288022d4fac0",null,"2026-10-08T13:19:43Z","8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,null],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","c9536868-a35e-4d21-a854-80e7703f5478",70,"2026-10-08T13:19:47Z","9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,"2026-10-08T13:19:47Z"]],
    "__alert_instances_info__":{
        "status":"truncated",
        "total_instances":4,
        "shown_instances":2,
        "cardinality_limit":2
    }
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/mcp-limit": {
		target: "/api/v3/alerts?options=mcp,summary,instances,values&cardinality_limit=4",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1],
        ["ha_mid","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1],
        ["ha_high","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1]],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","57cfc223-97a6-4399-83b1-8517cdacdcc5",null,1791465583,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,0],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","a8554664-9427-451c-a84a-e411647c4f55",70,1791465587,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","a82ae512-576f-4841-b1e2-0c20b5aade00",10,1791465583,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","0181f8e1-7c89-4236-9568-3ea915317cd3",10,1791465583,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587]]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1],
        ["ha_mid","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1],
        ["ha_high","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1]],
    "alert_instances_header":["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units","Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source","Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"],
    "alert_instances":[
        ["hm_plain","parity-parent","hsig.ctx","hsig.plain","UNINITIALIZED","family","b above 1000","","things","cf3caaf4-7f12-476c-b551-288022d4fac0",null,1791465583,"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","line=27,file=<run>/etc/health.d/parity.conf","silent","Parity Check","Fixture Part","Workload",null,0],
        ["ha_low","parity-parent","hsig.ctx","hsig.values","WARNING","family","a above 50","","things","c9536868-a35e-4d21-a854-80e7703f5478",70,1791465587,"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","line=2,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_mid","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 100","","things","770fcb6c-764d-4df3-bb3f-53c54c95b43d",10,1791465583,"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","line=11,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587],
        ["ha_high","parity-parent","hsig.ctx","hsig.values","CLEAR","family","a above 200","","things","09926cdb-fdef-47ce-9d05-5556b8811e62",10,1791465583,"d59244af-a365-4eee-836e-0475985cfd6b","line=19,file=<run>/etc/health.d/parity.conf","root","","","",70,1791465587]]
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/mcp-debug": {
		target: "/api/v3/alerts?options=mcp,debug,summary",
		bodies: [2]string{
			`{
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","summary","mcp"],
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
            "after":0,
            "before":0
        }
    },
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1],
        ["ha_mid","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1],
        ["ha_high","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1]]
}
`,
			`{
    "request":{
        "mode":["nodes","alerts"],
        "options":["debug","summary","mcp"],
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
            "after":0,
            "before":0
        }
    },
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["hm_plain","","hsig.ctx","Workload","Fixture_Part","Parity_Check","silent",0,0,0,0,1,1,1],
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1],
        ["ha_mid","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1],
        ["ha_high","","hsig.ctx",null,null,null,"root",0,0,1,0,1,1,1]]
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/mcp-alone": {
		target: "/api/v3/alerts?options=mcp",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }]
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/rfc3339": {
		target: "/api/v3/alerts?options=instances,values,minify,rfc3339",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"gi":1791465583459827,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"57cfc223-97a6-4399-83b1-8517cdacdcc5","tr_v":null,"tr_t":"2026-10-08T13:19:43Z","cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload","v":null,"t":null},{"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":"2026-10-08T13:19:47Z","cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":"2026-10-08T13:19:47Z"},{"ni":0,"gi":1791465583466818,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"a82ae512-576f-4841-b1e2-0c20b5aade00","tr_v":10,"tr_t":"2026-10-08T13:19:43Z","cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":"2026-10-08T13:19:47Z"},{"ni":0,"gi":1791465583466901,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"0181f8e1-7c89-4236-9568-3ea915317cd3","tr_v":10,"tr_t":"2026-10-08T13:19:43Z","cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":"2026-10-08T13:19:47Z"}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.052}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alert_instances":[{"ni":0,"gi":1791465583514253,"nm":"hm_plain","ctx":"hsig.ctx","ch":"hsig.plain","ch_n":"hsig.plain","st":"UNINITIALIZED","fami":"family","info":"b above 1000","sum":"","units":"things","tr_i":"cf3caaf4-7f12-476c-b551-288022d4fac0","tr_v":null,"tr_t":"2026-10-08T13:19:43Z","cfg":"8a6bb5a2-c0d2-4e68-aa80-c06e7ba596f1","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"silent","tp":"Parity Check","cm":"Fixture Part","cl":"Workload","v":null,"t":null},{"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":"2026-10-08T13:19:47Z","cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":"2026-10-08T13:19:47Z"},{"ni":0,"gi":1791465583514572,"nm":"ha_mid","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 100","sum":"","units":"things","tr_i":"770fcb6c-764d-4df3-bb3f-53c54c95b43d","tr_v":10,"tr_t":"2026-10-08T13:19:43Z","cfg":"5fe1391f-3156-44ac-9ae7-f53b3e2fe72a","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":"2026-10-08T13:19:47Z"},{"ni":0,"gi":1791465583514730,"nm":"ha_high","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CLEAR","fami":"family","info":"a above 200","sum":"","units":"things","tr_i":"09926cdb-fdef-47ce-9d05-5556b8811e62","tr_v":10,"tr_t":"2026-10-08T13:19:43Z","cfg":"d59244af-a365-4eee-836e-0475985cfd6b","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":"2026-10-08T13:19:47Z"}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.119}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.102}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-first": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.084}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.077}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-nodash": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.075}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.068}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-upper": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.07}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.069}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-appended": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.067}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.143}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-selectors": {
		target: "/api/v3/alerts?transition=<the side's own id>&scope_nodes=nothing*&nodes=nothing*&scope_contexts=nothing*&contexts=nothing*&options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587467917,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"a8554664-9427-451c-a84a-e411647c4f55","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.093}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"ha_low","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"parity.mod","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465587515730,"nm":"ha_low","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"WARNING","fami":"family","info":"a above 50","sum":"","units":"things","tr_i":"c9536868-a35e-4d21-a854-80e7703f5478","tr_v":70,"tr_t":1791465587,"cfg":"9eceed5c-209e-4193-aa9d-3a4b0ca83fa3","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791465587}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.089}}`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-bare": {
		target: "/api/v3/alerts?transition=<the side's own id>",
		bodies: [2]string{
			`{
    "api":2,
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.048
    }
}
`,
			`{
    "api":2,
    "nodes":[{
            "mg":"5a1e0000-0000-4000-8000-0000000000aa",
            "nm":"parity-parent",
            "ni":0
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.069
    }
}
`,
		},
		flight: [2][2]int64{{1791465587, 1791465587}, {1791465587, 1791465587}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-mcp": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=mcp,summary",
		bodies: [2]string{
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1]]
}
`,
			`{
    "nodes":[{
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "hostname":"parity-parent",
            "relationship":"localhost",
            "connected":true
        }],
    "all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],
    "all_alerts":[
        ["ha_low","","hsig.ctx",null,null,null,"root",0,1,0,0,1,1,1]]
}
`,
		},
		flight: [2][2]int64{{1791465588, 1791465588}, {1791465588, 1791465588}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"alerts/transition-critical": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,values,minify&status=critical",
		bodies: [2]string{
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.084}}`,
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[{"name":"Parity Check","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Fixture Part","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Workload","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.068}}`,
		},
		flight: [2][2]int64{{1791465588, 1791465588}, {1791465588, 1791465588}},
		logs:   [2]string{"L01", "L02"}, more: [2]string{"", ""}, host: "",
	},
	"off/raised-window": {
		target: "/api/v3/alerts?options=summary,minify&status=raised&after=-60",
		bodies: [2]string{
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.062}}`,
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.031}}`,
		},
		flight: [2][2]int64{{1791465612, 1791465612}, {1791465612, 1791465612}},
		logs:   [2]string{"", ""}, more: [2]string{"", ""}, host: "",
	},
	"off/window": {
		target: "/api/v3/alerts?options=summary,minify&after=-60",
		bodies: [2]string{
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.011}}`,
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.011}}`,
		},
		flight: [2][2]int64{{1791465612, 1791465612}, {1791465612, 1791465612}},
		logs:   [2]string{"", ""}, more: [2]string{"", ""}, host: "",
	},
	"off/raised": {
		target: "/api/v3/alerts?options=summary,minify&status=raised",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
		},
		flight: [2][2]int64{{1791465612, 1791465612}, {1791465612, 1791465612}},
		logs:   [2]string{"", ""}, more: [2]string{"", ""}, host: "",
	},
	"sets/summary": {
		target: "/api/v3/alerts?options=summary,minify",
		bodies: [2]string{
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0}],\"alerts\":[{\"ati\":0,\"ni\":[0],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]},{\"ati\":1,\"ni\":[0],\"nm\":\"hs_two\",\"sum\":\"\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":2,\"ctx\":[\"hset.ctx.b\",\"hset.ctx.a\"],\"cls\":[\"Latency\",\"Errors\"],\"cp\":[\"Part_One\",\"Part_Two\"],\"ty\":[\"Type_One\",\"Type_Two\"],\"to\":[\"silent\",\"sysadmin_webmaster\"]},{\"ati\":2,\"ni\":[0],\"nm\":\"hs_sum\",\"sum\":\"sum of ${family} at ${label:kind}\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"in\":1,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.b\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Type Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Part Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Latency\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":3,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"sysadmin webmaster\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":1},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"sets.mod\",\"cr\":0,\"wr\":1,\"cl\":1,\"er\":0,\"running\":2,\"running_silent\":0}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.162}}",
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0}],\"alerts\":[{\"ati\":0,\"ni\":[0],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]},{\"ati\":1,\"ni\":[0],\"nm\":\"hs_two\",\"sum\":\"\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":2,\"ctx\":[\"hset.ctx.b\",\"hset.ctx.a\"],\"cls\":[\"Errors\",\"Latency\"],\"cp\":[\"Part_Two\",\"Part_One\"],\"ty\":[\"Type_Two\",\"Type_One\"],\"to\":[\"silent\",\"sysadmin_webmaster\"]},{\"ati\":2,\"ni\":[0],\"nm\":\"hs_sum\",\"sum\":\"sum of ${family} at ${label:kind}\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"in\":1,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.b\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Type Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Part Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Latency\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":3,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"sysadmin webmaster\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":1},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"sets.mod\",\"cr\":0,\"wr\":1,\"cl\":1,\"er\":0,\"running\":2,\"running_silent\":0}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.125}}",
		},
		flight: [2][2]int64{{1791465629, 1791465629}, {1791465629, 1791465629}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"", ""}, host: "",
	},
	"sets/full": {
		target: "/api/v3/alerts?options=summary,instances,values,minify",
		bodies: [2]string{
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0}],\"alerts\":[{\"ati\":0,\"ni\":[0],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]},{\"ati\":1,\"ni\":[0],\"nm\":\"hs_two\",\"sum\":\"\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":2,\"ctx\":[\"hset.ctx.b\",\"hset.ctx.a\"],\"cls\":[\"Errors\",\"Latency\"],\"cp\":[\"Part_One\",\"Part_Two\"],\"ty\":[\"Type_One\",\"Type_Two\"],\"to\":[\"silent\",\"sysadmin_webmaster\"]},{\"ati\":2,\"ni\":[0],\"nm\":\"hs_sum\",\"sum\":\"sum of ${family} at ${label:kind}\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"in\":1,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.b\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Type Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Part Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Latency\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":3,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"sysadmin webmaster\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":1},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"sets.mod\",\"cr\":0,\"wr\":1,\"cl\":1,\"er\":0,\"running\":2,\"running_silent\":0}],\"alert_instances\":[{\"ati\":0,\"ni\":0,\"gi\":1791465625708252,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam one\",\"units\":\"things\",\"tr_i\":\"7a9cc29a-4537-4438-b1f0-8135a84091a6\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":1,\"ni\":0,\"gi\":1791465625708262,\"nm\":\"hs_two\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"two on one\",\"sum\":\"\",\"units\":\"things\",\"tr_i\":\"6e1ee7c9-afc2-46c8-b0b8-b196d685f93d\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"706f8ada-7307-40fe-8300-c50364850fe2\",\"src\":\"line=10,file=<run>/etc/health.d/parity.conf\",\"to\":\"silent\",\"tp\":\"Type One\",\"cm\":\"Part One\",\"cl\":\"Errors\",\"v\":null,\"t\":0},{\"ati\":0,\"ni\":0,\"gi\":1791465625708272,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.long\",\"ch_n\":\"hset.long\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam long\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam long\",\"units\":\"things\",\"tr_i\":\"6043d777-d678-41d5-8c05-ee9a0e28a6b3\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":1,\"ni\":0,\"gi\":1791465629718688,\"nm\":\"hs_two\",\"ctx\":\"hset.ctx.b\",\"ch\":\"hset.values\",\"ch_n\":\"hset.values\",\"st\":\"WARNING\",\"fami\":\"fam b\",\"info\":\"two on values\",\"sum\":\"two fam b\",\"units\":\"things\",\"tr_i\":\"069621b1-fc03-49a1-8a14-3f63ea3f50ba\",\"tr_v\":70,\"tr_t\":1791465629,\"cfg\":\"58017174-6670-41e5-89cb-e6a3d7401309\",\"src\":\"line=22,file=<run>/etc/health.d/parity.conf\",\"to\":\"sysadmin webmaster\",\"tp\":\"Type Two\",\"cm\":\"Part Two\",\"cl\":\"Latency\",\"v\":70,\"t\":1791465629},{\"ati\":2,\"ni\":0,\"gi\":1791465625717619,\"nm\":\"hs_sum\",\"ctx\":\"hset.ctx.b\",\"ch\":\"hset.values\",\"ch_n\":\"hset.values\",\"st\":\"CLEAR\",\"fami\":\"fam b\",\"info\":\"info of ${family} at ${label:kind}\",\"sum\":\"sum of fam b at x y\",\"units\":\"things\",\"tr_i\":\"be19154c-a854-4c6a-8f0d-590a89e0e7f6\",\"tr_v\":10,\"tr_t\":1791465625,\"cfg\":\"1060ebb9-e4f3-4f76-987b-991487ed242d\",\"src\":\"line=35,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":70,\"t\":1791465629}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.099}}",
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0}],\"alerts\":[{\"ati\":0,\"ni\":[0],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]},{\"ati\":1,\"ni\":[0],\"nm\":\"hs_two\",\"sum\":\"\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":2,\"ctx\":[\"hset.ctx.a\",\"hset.ctx.b\"],\"cls\":[\"Latency\",\"Errors\"],\"cp\":[\"Part_Two\",\"Part_One\"],\"ty\":[\"Type_Two\",\"Type_One\"],\"to\":[\"silent\",\"sysadmin_webmaster\"]},{\"ati\":2,\"ni\":[0],\"nm\":\"hs_sum\",\"sum\":\"sum of ${family} at ${label:kind}\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"in\":1,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.b\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Type Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Part Two\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"Latency\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":3,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1},{\"name\":\"sysadmin webmaster\",\"cr\":0,\"wr\":1,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":1},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"sets.mod\",\"cr\":0,\"wr\":1,\"cl\":1,\"er\":0,\"running\":2,\"running_silent\":0}],\"alert_instances\":[{\"ati\":0,\"ni\":0,\"gi\":1791465625694199,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam one\",\"units\":\"things\",\"tr_i\":\"5292efe0-c2ed-49b8-99c2-d8c74725124c\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":1,\"ni\":0,\"gi\":1791465625694209,\"nm\":\"hs_two\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"two on one\",\"sum\":\"\",\"units\":\"things\",\"tr_i\":\"f35448c3-fe35-4a2e-b8fc-d40a4d8c1db8\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"706f8ada-7307-40fe-8300-c50364850fe2\",\"src\":\"line=10,file=<run>/etc/health.d/parity.conf\",\"to\":\"silent\",\"tp\":\"Type One\",\"cm\":\"Part One\",\"cl\":\"Errors\",\"v\":null,\"t\":0},{\"ati\":0,\"ni\":0,\"gi\":1791465625694224,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.long\",\"ch_n\":\"hset.long\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam long\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam long\",\"units\":\"things\",\"tr_i\":\"88fb9353-d78a-4d84-8806-22cf878c6a84\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":1,\"ni\":0,\"gi\":1791465629695854,\"nm\":\"hs_two\",\"ctx\":\"hset.ctx.b\",\"ch\":\"hset.values\",\"ch_n\":\"hset.values\",\"st\":\"WARNING\",\"fami\":\"fam b\",\"info\":\"two on values\",\"sum\":\"two fam b\",\"units\":\"things\",\"tr_i\":\"b70f425b-4744-4550-8dd3-03a3e69c4017\",\"tr_v\":70,\"tr_t\":1791465629,\"cfg\":\"58017174-6670-41e5-89cb-e6a3d7401309\",\"src\":\"line=22,file=<run>/etc/health.d/parity.conf\",\"to\":\"sysadmin webmaster\",\"tp\":\"Type Two\",\"cm\":\"Part Two\",\"cl\":\"Latency\",\"v\":70,\"t\":1791465629},{\"ati\":2,\"ni\":0,\"gi\":1791465625694573,\"nm\":\"hs_sum\",\"ctx\":\"hset.ctx.b\",\"ch\":\"hset.values\",\"ch_n\":\"hset.values\",\"st\":\"CLEAR\",\"fami\":\"fam b\",\"info\":\"info of ${family} at ${label:kind}\",\"sum\":\"sum of fam b at x y\",\"units\":\"things\",\"tr_i\":\"f89b23ba-6dc9-48f0-b731-50e80b16f17b\",\"tr_v\":10,\"tr_t\":1791465625,\"cfg\":\"1060ebb9-e4f3-4f76-987b-991487ed242d\",\"src\":\"line=35,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":70,\"t\":1791465629}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.12}}",
		},
		flight: [2][2]int64{{1791465629, 1791465629}, {1791465629, 1791465629}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"", ""}, host: "",
	},
	"sets/mcp-summary": {
		target: "/api/v3/alerts?options=mcp,summary,minify",
		bodies: [2]string{
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true}],"all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],"all_alerts":[["hs_tpl","tpl ${family}","hset.ctx.a",null,null,null,"root",0,0,0,0,2,1,1],["hs_two","",["hset.ctx.a","hset.ctx.b"],["Latency","Errors"],["Part_One","Part_Two"],["Type_One","Type_Two"],["sysadmin_webmaster","silent"],0,1,0,0,2,1,2],["hs_sum","sum of ${family} at ${label:kind}","hset.ctx.b",null,null,null,"root",0,0,1,0,1,1,1]]}`,
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true}],"all_alerts_header":["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components","Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances","# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched","# of Alert Configurations"],"all_alerts":[["hs_tpl","tpl ${family}","hset.ctx.a",null,null,null,"root",0,0,0,0,2,1,1],["hs_two","",["hset.ctx.b","hset.ctx.a"],["Errors","Latency"],["Part_Two","Part_One"],["Type_Two","Type_One"],["sysadmin_webmaster","silent"],0,1,0,0,2,1,2],["hs_sum","sum of ${family} at ${label:kind}","hset.ctx.b",null,null,null,"root",0,0,1,0,1,1,1]]}`,
		},
		flight: [2][2]int64{{1791465629, 1791465629}, {1791465629, 1791465629}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"", ""}, host: "",
	},
	"sets/scope": {
		target: "/api/v3/alerts?options=summary,instances,minify&scope_contexts=hset.ctx.a",
		bodies: [2]string{
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0}],\"alerts\":[{\"ati\":0,\"ni\":[0],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]},{\"ati\":1,\"ni\":[0],\"nm\":\"hs_two\",\"sum\":\"\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":1,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[\"Errors\"],\"cp\":[\"Part_One\"],\"ty\":[\"Type_One\"],\"to\":[\"silent\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":1},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alert_instances\":[{\"ati\":0,\"ni\":0,\"gi\":1791465625708252,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam one\",\"units\":\"things\",\"tr_i\":\"7a9cc29a-4537-4438-b1f0-8135a84091a6\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\"},{\"ati\":1,\"ni\":0,\"gi\":1791465625708262,\"nm\":\"hs_two\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"two on one\",\"sum\":\"\",\"units\":\"things\",\"tr_i\":\"6e1ee7c9-afc2-46c8-b0b8-b196d685f93d\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"706f8ada-7307-40fe-8300-c50364850fe2\",\"src\":\"line=10,file=<run>/etc/health.d/parity.conf\",\"to\":\"silent\",\"tp\":\"Type One\",\"cm\":\"Part One\",\"cl\":\"Errors\"},{\"ati\":0,\"ni\":0,\"gi\":1791465625708272,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.long\",\"ch_n\":\"hset.long\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam long\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam long\",\"units\":\"things\",\"tr_i\":\"6043d777-d678-41d5-8c05-ee9a0e28a6b3\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\"}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.07}}",
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0}],\"alerts\":[{\"ati\":0,\"ni\":[0],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":2,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]},{\"ati\":1,\"ni\":[0],\"nm\":\"hs_two\",\"sum\":\"\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"in\":1,\"nd\":1,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[\"Errors\"],\"cp\":[\"Part_One\"],\"ty\":[\"Type_One\"],\"to\":[\"silent\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":1,\"available\":1}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":2,\"running_silent\":1},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0}],\"alert_instances\":[{\"ati\":0,\"ni\":0,\"gi\":1791465625694199,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam one\",\"units\":\"things\",\"tr_i\":\"5292efe0-c2ed-49b8-99c2-d8c74725124c\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\"},{\"ati\":1,\"ni\":0,\"gi\":1791465625694209,\"nm\":\"hs_two\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"two on one\",\"sum\":\"\",\"units\":\"things\",\"tr_i\":\"f35448c3-fe35-4a2e-b8fc-d40a4d8c1db8\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"706f8ada-7307-40fe-8300-c50364850fe2\",\"src\":\"line=10,file=<run>/etc/health.d/parity.conf\",\"to\":\"silent\",\"tp\":\"Type One\",\"cm\":\"Part One\",\"cl\":\"Errors\"},{\"ati\":0,\"ni\":0,\"gi\":1791465625694224,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.long\",\"ch_n\":\"hset.long\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam long\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam long\",\"units\":\"things\",\"tr_i\":\"88fb9353-d78a-4d84-8806-22cf878c6a84\",\"tr_v\":null,\"tr_t\":1791465625,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\"}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.062}}",
		},
		flight: [2][2]int64{{1791465629, 1791465629}, {1791465629, 1791465629}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"", ""}, host: "",
	},
	"sets/scope-other": {
		target: "/api/v3/alerts?transition=<the side's own id>&scope_contexts=hset.ctx.a&options=summary,instances,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.065}}`,
			`{"api":2,"nodes":[],"alerts":[],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.072}}`,
		},
		flight: [2][2]int64{{1791465629, 1791465629}, {1791465629, 1791465629}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"", ""}, host: "",
	},
	"sets/scope-pattern": {
		target: "/api/v3/alerts?transition=<the side's own id>&scope_contexts=hset.ctx.*&options=summary,instances,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_sum","sum":"sum of ${family} at ${label:kind}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"sets.mod","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465625717619,"nm":"hs_sum","ctx":"hset.ctx.b","ch":"hset.values","ch_n":"hset.values","st":"CLEAR","fami":"fam b","info":"info of ${family} at ${label:kind}","sum":"sum of fam b at x y","units":"things","tr_i":"be19154c-a854-4c6a-8f0d-590a89e0e7f6","tr_v":10,"tr_t":1791465625,"cfg":"1060ebb9-e4f3-4f76-987b-991487ed242d","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.123}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_sum","sum":"sum of ${family} at ${label:kind}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"sets.mod","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465625694573,"nm":"hs_sum","ctx":"hset.ctx.b","ch":"hset.values","ch_n":"hset.values","st":"CLEAR","fami":"fam b","info":"info of ${family} at ${label:kind}","sum":"sum of fam b at x y","units":"things","tr_i":"f89b23ba-6dc9-48f0-b731-50e80b16f17b","tr_v":10,"tr_t":1791465625,"cfg":"1060ebb9-e4f3-4f76-987b-991487ed242d","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.078}}`,
		},
		flight: [2][2]int64{{1791465629, 1791465629}, {1791465629, 1791465629}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"", ""}, host: "",
	},
	"sets/two-nodes": {
		target: "/api/v3/alerts?options=summary,instances,values,minify&alert=hs_tpl",
		bodies: [2]string{
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0},{\"mg\":\"5a1e0000-0000-4000-8000-0000000000c1\",\"nm\":\"health-child\",\"ni\":1}],\"alerts\":[{\"ati\":0,\"ni\":[0,1],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"in\":3,\"nd\":2,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":3,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"fanout\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":1,\"running_silent\":0}],\"alert_instances\":[{\"ati\":0,\"ni\":0,\"gi\":1791465633745934,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam one\",\"units\":\"things\",\"tr_i\":\"757a9a97-8a35-46b5-b504-c26239e8792f\",\"tr_v\":null,\"tr_t\":1791465633,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":0,\"ni\":0,\"gi\":1791465633745969,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.long\",\"ch_n\":\"hset.long\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam long\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam long\",\"units\":\"things\",\"tr_i\":\"80aeba7a-8204-49fa-8bd7-b3a3d8b8c71b\",\"tr_v\":null,\"tr_t\":1791465633,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":0,\"ni\":1,\"gi\":1791465637803553,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hsetc.values\",\"ch_n\":\"hsetc.values\",\"st\":\"CLEAR\",\"fami\":\"family\",\"info\":\"the template of ${family}\",\"sum\":\"tpl family\",\"units\":\"things\",\"tr_i\":\"4a5228cd-5bd6-4503-a3ad-a16659076a6e\",\"tr_v\":10,\"tr_t\":1791465637,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":10,\"t\":1791465637}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.106}}",
			"{\"api\":2,\"nodes\":[{\"mg\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"nm\":\"parity-parent\",\"ni\":0},{\"mg\":\"5a1e0000-0000-4000-8000-0000000000c1\",\"nm\":\"health-child\",\"ni\":1}],\"alerts\":[{\"ati\":0,\"ni\":[0,1],\"nm\":\"hs_tpl\",\"sum\":\"tpl ${family}\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"in\":3,\"nd\":2,\"cfg\":1,\"ctx\":[\"hset.ctx.a\"],\"cls\":[],\"cp\":[],\"ty\":[],\"to\":[\"root\"]}],\"alerts_by_type\":[{\"name\":\"Type One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_component\":[{\"name\":\"Part One\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_classification\":[{\"name\":\"Errors\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_recipient\":[{\"name\":\"root\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":3,\"running_silent\":0,\"available\":2},{\"name\":\"silent\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":0,\"running_silent\":0,\"available\":1}],\"alerts_by_module\":[{\"name\":\"[none]\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm\xc3\",\"cr\":0,\"wr\":0,\"cl\":0,\"er\":0,\"running\":1,\"running_silent\":0},{\"name\":\"fanout\",\"cr\":0,\"wr\":0,\"cl\":1,\"er\":0,\"running\":1,\"running_silent\":0}],\"alert_instances\":[{\"ati\":0,\"ni\":0,\"gi\":1791465633719335,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.one\",\"ch_n\":\"hset.one\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam one\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam one\",\"units\":\"things\",\"tr_i\":\"97a2b04f-6d68-4e61-b61b-cb6417b1aabd\",\"tr_v\":null,\"tr_t\":1791465633,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":0,\"ni\":0,\"gi\":1791465633719369,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hset.long\",\"ch_n\":\"hset.long\",\"st\":\"UNINITIALIZED\",\"fami\":\"fam long\",\"info\":\"the template of ${family}\",\"sum\":\"tpl fam long\",\"units\":\"things\",\"tr_i\":\"2f68c9eb-721a-4ae7-984c-03755ccbcb59\",\"tr_v\":null,\"tr_t\":1791465633,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":null,\"t\":0},{\"ati\":0,\"ni\":1,\"gi\":1791465637741788,\"nm\":\"hs_tpl\",\"ctx\":\"hset.ctx.a\",\"ch\":\"hsetc.values\",\"ch_n\":\"hsetc.values\",\"st\":\"CLEAR\",\"fami\":\"family\",\"info\":\"the template of ${family}\",\"sum\":\"tpl family\",\"units\":\"things\",\"tr_i\":\"9a250679-9bbb-492c-bec0-a8553e198b79\",\"tr_v\":10,\"tr_t\":1791465637,\"cfg\":\"22e01082-c646-4e6f-b69a-8845f27d1f4f\",\"src\":\"line=1,file=<run>/etc/health.d/parity.conf\",\"to\":\"root\",\"tp\":\"\",\"cm\":\"\",\"cl\":\"\",\"v\":10,\"t\":1791465637}],\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.096}}",
		},
		flight: [2][2]int64{{1791465638, 1791465638}, {1791465638, 1791465638}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"L09", "L10"}, host: "health-child",
	},
	"sets/child-transition": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_tpl","sum":"tpl ${family}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.a"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"fanout","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465637803553,"nm":"hs_tpl","ctx":"hset.ctx.a","ch":"hsetc.values","ch_n":"hsetc.values","st":"CLEAR","fami":"family","info":"the template of ${family}","sum":"tpl family","units":"things","tr_i":"4a5228cd-5bd6-4503-a3ad-a16659076a6e","tr_v":10,"tr_t":1791465637,"cfg":"22e01082-c646-4e6f-b69a-8845f27d1f4f","src":"line=1,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.142}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_tpl","sum":"tpl ${family}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.a"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"fanout","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465637741788,"nm":"hs_tpl","ctx":"hset.ctx.a","ch":"hsetc.values","ch_n":"hsetc.values","st":"CLEAR","fami":"family","info":"the template of ${family}","sum":"tpl family","units":"things","tr_i":"9a250679-9bbb-492c-bec0-a8553e198b79","tr_v":10,"tr_t":1791465637,"cfg":"22e01082-c646-4e6f-b69a-8845f27d1f4f","src":"line=1,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.114}}`,
		},
		flight: [2][2]int64{{1791465638, 1791465638}, {1791465638, 1791465638}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"L09", "L10"}, host: "health-child",
	},
	"sets/gone-critical-window": {
		target: "/api/v3/alerts?options=summary,minify&status=critical&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.048}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.044}}`,
		},
		flight: [2][2]int64{{1791465641, 1791465641}, {1791465641, 1791465641}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"L09", "L10"}, host: "health-child",
	},
	"sets/gone-clear-window": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=clear&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_sum","sum":"sum of ${family} at ${label:kind}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"sets.mod","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465641814409,"nm":"hs_sum","ctx":"hset.ctx.b","ch":"hset.values","ch_n":"hset.values","st":"CLEAR","fami":"fam b","info":"info of ${family} at ${label:kind}","sum":"sum of fam b at x y","units":"things","tr_i":"0ef81b11-0fea-4d32-8eac-4e644a5ec6ab","tr_v":70,"tr_t":1791465641,"cfg":"1060ebb9-e4f3-4f76-987b-991487ed242d","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.06}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_sum","sum":"sum of ${family} at ${label:kind}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"sets.mod","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791465641751169,"nm":"hs_sum","ctx":"hset.ctx.b","ch":"hset.values","ch_n":"hset.values","st":"CLEAR","fami":"fam b","info":"info of ${family} at ${label:kind}","sum":"sum of fam b at x y","units":"things","tr_i":"0b64c4dc-d242-4c72-b10d-32a0a828c262","tr_v":70,"tr_t":1791465641,"cfg":"1060ebb9-e4f3-4f76-987b-991487ed242d","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.073}}`,
		},
		flight: [2][2]int64{{1791465641, 1791465641}, {1791465641, 1791465641}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"L09", "L10"}, host: "health-child",
	},
	"sets/gone-uninitialized-window": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=uninitialized&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.034}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[],"alerts_by_type":[{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.019}}`,
		},
		flight: [2][2]int64{{1791465641, 1791465641}, {1791465641, 1791465641}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"L09", "L10"}, host: "health-child",
	},
	"sets/gone-window": {
		target: "/api/v3/alerts?options=summary,minify&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_two","sum":"two ${family}","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":["Latency"],"cp":["Part_Two"],"ty":["Type_Two"],"to":["sysadmin_webmaster"]},{"ati":1,"ni":[0],"nm":"hs_sum","sum":"sum of ${family} at ${label:kind}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type Two","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part Two","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Latency","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"sysadmin webmaster","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"sets.mod","cr":0,"wr":1,"cl":1,"er":0,"running":2,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.057}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_two","sum":"two ${family}","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":["Latency"],"cp":["Part_Two"],"ty":["Type_Two"],"to":["sysadmin_webmaster"]},{"ati":1,"ni":[0],"nm":"hs_sum","sum":"sum of ${family} at ${label:kind}","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hset.ctx.b"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type Two","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"Type One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part Two","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"Part One","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[{"name":"Latency","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"Errors","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_recipient":[{"name":"sysadmin webmaster","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0},{"name":"root","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0,"available":2},{"name":"silent","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"sets.mod","cr":0,"wr":1,"cl":1,"er":0,"running":2,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.048}}`,
		},
		flight: [2][2]int64{{1791465641, 1791465641}, {1791465641, 1791465641}},
		logs:   [2]string{"L07", "L08"}, more: [2]string{"L09", "L10"}, host: "health-child",
	},
}

// dashNormAlertsRaw are the recorded raw rows by `<case>/<row>`: each side's answer as it was read.
var dashNormAlertsRaw = map[string][2]string{
	"alerts/transition-unknown": {
		"HTTP/1.1 404 Not Found\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 13:19:48 GMT\r\nX-Transaction-ID: ab9ad291f6a64938967a72f26d82d814\r\n\r\n",
		"HTTP/1.1 404 Not Found\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 13:19:48 GMT\r\nX-Transaction-ID: 31e56f5ad1754f80aab41d3a7c2bdb41\r\n\r\n",
	},
	"alerts/transition-text": {
		"HTTP/1.1 404 Not Found\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 13:19:48 GMT\r\nX-Transaction-ID: 855703410fc14af0add3efa64015aea3\r\n\r\n",
		"HTTP/1.1 404 Not Found\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 13:19:48 GMT\r\nX-Transaction-ID: ab97b149d6264a42b75f6ca8bbdb02d2\r\n\r\n",
	},
	"alerts/transition-gzip": {
		"HTTP/1.1 404 Not Found\r\nConnection: keep-alive\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\nX-Transaction-ID: ad4f304f46c248f7974f143f7dffdd33\r\n\r\n",
		"HTTP/1.1 404 Not Found\r\nConnection: keep-alive\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 13:19:48 GMT\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\nX-Transaction-ID: 4ae04a9e0a1a4a5396786cd33b0bc82f\r\n\r\n",
	},
}

// dashNormAlertsLogs are the recorded alert logs by name.
var dashNormAlertsLogs = map[string]string{
	"L01": `[{"unique_id":1791465599,"alarm_id":1791465585,"alarm_event_id":5,"name":"ha_low","transition_id":"a8554664-9427-451c-a84a-e411647c4f55","when":1791465587,"duration":4,"non_clear_duration":0,"exec_run":1791465587,"delay_up_to_timestamp":1791465587,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465598,"alarm_id":1791465587,"alarm_event_id":4,"name":"ha_high","transition_id":"0181f8e1-7c89-4236-9568-3ea915317cd3","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465597,"alarm_id":1791465586,"alarm_event_id":4,"name":"ha_mid","transition_id":"a82ae512-576f-4841-b1e2-0c20b5aade00","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465596,"alarm_id":1791465585,"alarm_event_id":4,"name":"ha_low","transition_id":"fe4a26de-6349-40ac-8848-909b76df2934","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465595,"alarm_id":1791465587,"alarm_event_id":3,"name":"ha_high","transition_id":"9de25bfa-f762-4e9c-8f00-4391f7dc071d","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465594,"alarm_id":1791465586,"alarm_event_id":3,"name":"ha_mid","transition_id":"25d75df8-84a7-4358-8d1f-42d79189195f","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465593,"alarm_id":1791465585,"alarm_event_id":3,"name":"ha_low","transition_id":"08d259bf-b2e2-44f2-ae18-2bcc8e2cc7fc","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465592,"alarm_id":1791465587,"alarm_event_id":2,"name":"ha_high","transition_id":"561287e7-c93b-4c0b-92f1-c74da4500a97","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465591,"alarm_id":1791465586,"alarm_event_id":2,"name":"ha_mid","transition_id":"a405febc-abeb-4e83-bce4-3ec86bdab197","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465590,"alarm_id":1791465585,"alarm_event_id":2,"name":"ha_low","transition_id":"93d51551-a224-43dd-b338-767188226c86","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465589,"alarm_id":1791465584,"alarm_event_id":3,"name":"hm_plain","transition_id":"57cfc223-97a6-4399-83b1-8517cdacdcc5","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465588,"alarm_id":1791465584,"alarm_event_id":2,"name":"hm_plain","transition_id":"5f4ce2a3-ddbf-4156-ad5a-ccc8086ed0e2","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465587,"alarm_id":1791465587,"alarm_event_id":1,"name":"ha_high","transition_id":"449f8ad8-4b9a-4e2c-b975-a0abfafe9745","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465586,"alarm_id":1791465586,"alarm_event_id":1,"name":"ha_mid","transition_id":"634f6fd4-8735-4746-8208-6ab353752411","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465585,"alarm_id":1791465585,"alarm_event_id":1,"name":"ha_low","transition_id":"63fa7d42-31be-40c1-b144-b5cf9517aa8f","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465584,"alarm_id":1791465584,"alarm_event_id":1,"name":"hm_plain","transition_id":"4d651f48-0aa3-4d08-8254-d4196f0b26ec","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L02": `[{"unique_id":1791465599,"alarm_id":1791465585,"alarm_event_id":5,"name":"ha_low","transition_id":"c9536868-a35e-4d21-a854-80e7703f5478","when":1791465587,"duration":4,"non_clear_duration":0,"exec_run":1791465587,"delay_up_to_timestamp":1791465587,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465598,"alarm_id":1791465587,"alarm_event_id":4,"name":"ha_high","transition_id":"09926cdb-fdef-47ce-9d05-5556b8811e62","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465597,"alarm_id":1791465586,"alarm_event_id":4,"name":"ha_mid","transition_id":"770fcb6c-764d-4df3-bb3f-53c54c95b43d","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465596,"alarm_id":1791465585,"alarm_event_id":4,"name":"ha_low","transition_id":"3113cc5f-294c-48e3-a0a7-2950d597e1b7","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465595,"alarm_id":1791465587,"alarm_event_id":3,"name":"ha_high","transition_id":"5a7351ca-7e32-439d-a0a9-7b2b407cba3f","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465594,"alarm_id":1791465586,"alarm_event_id":3,"name":"ha_mid","transition_id":"f2c7b716-343f-493c-a2c1-09f4d8b36139","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465593,"alarm_id":1791465585,"alarm_event_id":3,"name":"ha_low","transition_id":"1edb6631-678e-4474-909f-1d9d2e5c8d32","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465592,"alarm_id":1791465587,"alarm_event_id":2,"name":"ha_high","transition_id":"17621993-3b7e-4840-b625-2d4a8004f618","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465591,"alarm_id":1791465586,"alarm_event_id":2,"name":"ha_mid","transition_id":"1a33a222-e87e-487e-8b23-84537862639e","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465590,"alarm_id":1791465585,"alarm_event_id":2,"name":"ha_low","transition_id":"821deddf-9c93-42b8-9c99-ea4394a241ff","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465589,"alarm_id":1791465584,"alarm_event_id":3,"name":"hm_plain","transition_id":"cf3caaf4-7f12-476c-b551-288022d4fac0","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465588,"alarm_id":1791465584,"alarm_event_id":2,"name":"hm_plain","transition_id":"c146e539-451f-4ece-acdd-d887e339af20","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465587,"alarm_id":1791465587,"alarm_event_id":1,"name":"ha_high","transition_id":"127ab859-d3af-4642-b0e0-709d1d0be8fa","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465586,"alarm_id":1791465586,"alarm_event_id":1,"name":"ha_mid","transition_id":"b5dcfc58-441b-42e6-bbdb-8779aea000cf","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465585,"alarm_id":1791465585,"alarm_event_id":1,"name":"ha_low","transition_id":"b74e5e27-cf7c-4a54-b0dd-8b903ded8b52","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465584,"alarm_id":1791465584,"alarm_event_id":1,"name":"hm_plain","transition_id":"16dd8890-391f-4b38-9869-018209aed38d","when":1791465583,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465583,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L03": `[{"unique_id":1791465753,"alarm_id":1791465746,"alarm_event_id":8,"name":"hloc_calc","transition_id":"71dd129e-df5f-4360-baa3-b373ab1fb1aa","when":1791465754,"duration":5,"non_clear_duration":0,"exec_run":1791465754,"delay_up_to_timestamp":1791465754,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465752,"alarm_id":1791465746,"alarm_event_id":7,"name":"hloc_calc","transition_id":"2da8ec9f-fe86-4177-8f09-3f69a22cac10","when":1791465749,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465749,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465751,"alarm_id":1791465746,"alarm_event_id":6,"name":"hloc_calc","transition_id":"e8c71b03-1803-429d-b104-5cc6559be859","when":1791465749,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465749,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465750,"alarm_id":1791465746,"alarm_event_id":5,"name":"hloc_calc","transition_id":"be0217af-2636-4249-9b78-0917ee5389f1","when":1791465749,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465749,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465749,"alarm_id":1791465746,"alarm_event_id":4,"name":"hloc_calc","transition_id":"6360a9b8-693c-4d22-82d3-aa320c0dc64e","when":1791465745,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465745,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465748,"alarm_id":1791465746,"alarm_event_id":3,"name":"hloc_calc","transition_id":"3f7bde6e-c9ab-416d-8fe5-7258f093a96e","when":1791465745,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465745,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465747,"alarm_id":1791465746,"alarm_event_id":2,"name":"hloc_calc","transition_id":"a8c30d8b-434f-415d-a8e4-1613be57954f","when":1791465745,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465745,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465746,"alarm_id":1791465746,"alarm_event_id":1,"name":"hloc_calc","transition_id":"5a26cefc-f102-4ecf-9e41-894a0ac7e071","when":1791465745,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465745,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L04": `[{"unique_id":1791465752,"alarm_id":1791465745,"alarm_event_id":8,"name":"hloc_calc","transition_id":"f472b8c0-962f-4ea5-8965-f0dcb54f56e4","when":1791465754,"duration":6,"non_clear_duration":0,"exec_run":1791465754,"delay_up_to_timestamp":1791465754,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465751,"alarm_id":1791465745,"alarm_event_id":7,"name":"hloc_calc","transition_id":"9e659968-a068-4963-8779-1ce2d7626cb6","when":1791465748,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465748,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465750,"alarm_id":1791465745,"alarm_event_id":6,"name":"hloc_calc","transition_id":"73341089-b8be-4f6b-9af9-41eb6536b223","when":1791465748,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465748,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465749,"alarm_id":1791465745,"alarm_event_id":5,"name":"hloc_calc","transition_id":"5d36ff95-17cc-46e6-9767-d021ea7e83c3","when":1791465748,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465748,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465748,"alarm_id":1791465745,"alarm_event_id":4,"name":"hloc_calc","transition_id":"f62a70d4-3e16-4421-aa7c-e7d58d484781","when":1791465744,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465744,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465747,"alarm_id":1791465745,"alarm_event_id":3,"name":"hloc_calc","transition_id":"0c76db33-f1ae-4a79-942b-631f99d158d2","when":1791465744,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465744,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465746,"alarm_id":1791465745,"alarm_event_id":2,"name":"hloc_calc","transition_id":"171cae1e-127a-4de8-9275-f27863c0e0fa","when":1791465744,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465744,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465745,"alarm_id":1791465745,"alarm_event_id":1,"name":"hloc_calc","transition_id":"e70b31ce-9d57-4f10-a299-411bccda1aad","when":1791465744,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465744,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L05": `[{"unique_id":1791465755,"alarm_id":1791465753,"alarm_event_id":3,"name":"hch_calc","transition_id":"42575a46-5fbf-45d9-bfcb-3e6c0f36c049","when":1791465754,"duration":2,"non_clear_duration":0,"exec_run":1791465754,"delay_up_to_timestamp":1791465754,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465754,"alarm_id":1791465753,"alarm_event_id":2,"name":"hch_calc","transition_id":"dac3b74f-ba65-4f21-80a4-ed803670d25b","when":1791465752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465752,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465753,"alarm_id":1791465753,"alarm_event_id":1,"name":"hch_calc","transition_id":"f630cce5-f036-4e7a-b683-e21b2f2d8b3a","when":1791465752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465752,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L06": `[{"unique_id":1791465755,"alarm_id":1791465753,"alarm_event_id":3,"name":"hch_calc","transition_id":"af135c4f-c9a5-4538-8249-7f12c4f5e9b2","when":1791465755,"duration":2,"non_clear_duration":0,"exec_run":1791465755,"delay_up_to_timestamp":1791465755,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465754,"alarm_id":1791465753,"alarm_event_id":2,"name":"hch_calc","transition_id":"1b57a3b5-61f5-4faf-8161-7d6cc09e515f","when":1791465753,"duration":1,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465753,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465753,"alarm_id":1791465753,"alarm_event_id":1,"name":"hch_calc","transition_id":"7ce27854-b3b4-4471-b5d3-de602061a47e","when":1791465752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465752,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L07": `[{"unique_id":1791465667,"alarm_id":1791465630,"alarm_event_id":10,"name":"hs_sum","transition_id":"0ef81b11-0fea-4d32-8eac-4e644a5ec6ab","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465666,"alarm_id":1791465629,"alarm_event_id":11,"name":"hs_two","transition_id":"c371334a-4c01-4222-8351-dbbe890060d9","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791465665,"alarm_id":1791465630,"alarm_event_id":9,"name":"hs_sum","transition_id":"0a5b5d64-c214-494b-a789-96742b6613d9","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465664,"alarm_id":1791465629,"alarm_event_id":10,"name":"hs_two","transition_id":"38b5ec67-3d41-41b1-b5b3-352693ac2002","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465663,"alarm_id":1791465630,"alarm_event_id":8,"name":"hs_sum","transition_id":"7d0cc8c5-389a-4049-b59d-5c9b8e1a1929","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465662,"alarm_id":1791465629,"alarm_event_id":9,"name":"hs_two","transition_id":"99948cb0-4b68-44a9-99fb-d40ec2f210d9","when":1791465641,"duration":8,"non_clear_duration":8,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"WARNING"},{"unique_id":1791465661,"alarm_id":1791465628,"alarm_event_id":7,"name":"hs_tpl","transition_id":"aa7ae9e6-5095-4fc4-8daf-18696c3c5feb","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465660,"alarm_id":1791465628,"alarm_event_id":6,"name":"hs_tpl","transition_id":"1012b315-90c8-4893-8d0c-fa2086a891d5","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465659,"alarm_id":1791465627,"alarm_event_id":7,"name":"hs_two","transition_id":"3e893aac-2603-497c-906d-62183745b7e7","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465658,"alarm_id":1791465626,"alarm_event_id":7,"name":"hs_tpl","transition_id":"813225cb-ed25-41a8-85c0-283a4e7275d7","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465657,"alarm_id":1791465627,"alarm_event_id":6,"name":"hs_two","transition_id":"7a10d088-9d3f-482f-9cf1-51661a75e7bb","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465656,"alarm_id":1791465626,"alarm_event_id":6,"name":"hs_tpl","transition_id":"2145f5d6-cec4-4b5a-b2a5-b519c2e567bd","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465655,"alarm_id":1791465630,"alarm_event_id":7,"name":"hs_sum","transition_id":"63ee5422-2874-4e22-a3db-31511d8c7b3d","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465654,"alarm_id":1791465629,"alarm_event_id":8,"name":"hs_two","transition_id":"f00e9523-8733-461b-a885-2436c21c2672","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791465653,"alarm_id":1791465630,"alarm_event_id":6,"name":"hs_sum","transition_id":"fbf07fc3-fb6a-41ec-871a-2d3cb26df132","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465652,"alarm_id":1791465629,"alarm_event_id":7,"name":"hs_two","transition_id":"bab09966-3a6f-4275-b0cd-8b7483150d76","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465651,"alarm_id":1791465630,"alarm_event_id":5,"name":"hs_sum","transition_id":"b333aafe-a892-49cb-91a7-f2c877529cfb","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465650,"alarm_id":1791465629,"alarm_event_id":6,"name":"hs_two","transition_id":"d4234678-a784-463e-b234-4d2210ba0065","when":1791465633,"duration":4,"non_clear_duration":4,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"WARNING"},{"unique_id":1791465649,"alarm_id":1791465628,"alarm_event_id":5,"name":"hs_tpl","transition_id":"80aeba7a-8204-49fa-8bd7-b3a3d8b8c71b","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465648,"alarm_id":1791465628,"alarm_event_id":4,"name":"hs_tpl","transition_id":"f4470c8b-49b8-4773-91c6-445185e285ea","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465647,"alarm_id":1791465627,"alarm_event_id":5,"name":"hs_two","transition_id":"3067912e-810f-45d4-8374-b482a2b21aca","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465646,"alarm_id":1791465626,"alarm_event_id":5,"name":"hs_tpl","transition_id":"757a9a97-8a35-46b5-b504-c26239e8792f","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465645,"alarm_id":1791465627,"alarm_event_id":4,"name":"hs_two","transition_id":"8819ff81-9184-4428-90a4-8215f99fd022","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465644,"alarm_id":1791465626,"alarm_event_id":4,"name":"hs_tpl","transition_id":"07ba9181-ba8c-43f3-98d8-93c936650b8e","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465643,"alarm_id":1791465629,"alarm_event_id":5,"name":"hs_two","transition_id":"069621b1-fc03-49a1-8a14-3f63ea3f50ba","when":1791465629,"duration":4,"non_clear_duration":0,"exec_run":1791465629,"delay_up_to_timestamp":1791465629,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465642,"alarm_id":1791465630,"alarm_event_id":4,"name":"hs_sum","transition_id":"be19154c-a854-4c6a-8f0d-590a89e0e7f6","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465641,"alarm_id":1791465629,"alarm_event_id":4,"name":"hs_two","transition_id":"0c3c93b2-95e4-4105-a183-894ca9f909d0","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465640,"alarm_id":1791465630,"alarm_event_id":3,"name":"hs_sum","transition_id":"1765ec83-b969-4c9e-99d3-523d21a74bb1","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465639,"alarm_id":1791465629,"alarm_event_id":3,"name":"hs_two","transition_id":"78cd2991-a0ea-472a-a68a-db706687a992","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465638,"alarm_id":1791465630,"alarm_event_id":2,"name":"hs_sum","transition_id":"62d4e2ee-47aa-4fb5-a863-a535a791a39d","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465637,"alarm_id":1791465629,"alarm_event_id":2,"name":"hs_two","transition_id":"8561e388-fa39-448b-a63a-7afb740e3a4d","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465636,"alarm_id":1791465628,"alarm_event_id":3,"name":"hs_tpl","transition_id":"6043d777-d678-41d5-8c05-ee9a0e28a6b3","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465635,"alarm_id":1791465628,"alarm_event_id":2,"name":"hs_tpl","transition_id":"9aa7d57d-382b-4af5-8b4d-4641eef524cf","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465634,"alarm_id":1791465627,"alarm_event_id":3,"name":"hs_two","transition_id":"6e1ee7c9-afc2-46c8-b0b8-b196d685f93d","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465633,"alarm_id":1791465626,"alarm_event_id":3,"name":"hs_tpl","transition_id":"7a9cc29a-4537-4438-b1f0-8135a84091a6","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465632,"alarm_id":1791465627,"alarm_event_id":2,"name":"hs_two","transition_id":"960f8781-7342-4780-8c8f-f35b163b436a","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465631,"alarm_id":1791465626,"alarm_event_id":2,"name":"hs_tpl","transition_id":"86b3d599-8ec8-42ca-a504-5fbb06f56b04","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465630,"alarm_id":1791465630,"alarm_event_id":1,"name":"hs_sum","transition_id":"3f63fcc4-22ab-466c-9ca6-e0369bd4aa71","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465629,"alarm_id":1791465629,"alarm_event_id":1,"name":"hs_two","transition_id":"0bd60a6e-5544-46a3-bc9c-35419df8a175","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465628,"alarm_id":1791465628,"alarm_event_id":1,"name":"hs_tpl","transition_id":"78ddf35b-0f57-478c-aa0c-eae11917089f","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465627,"alarm_id":1791465627,"alarm_event_id":1,"name":"hs_two","transition_id":"9a57ccd0-15c2-4776-a51e-3bea666e4183","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465626,"alarm_id":1791465626,"alarm_event_id":1,"name":"hs_tpl","transition_id":"4bd09c72-9b6d-4373-840d-e118a735c17b","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L08": `[{"unique_id":1791465667,"alarm_id":1791465630,"alarm_event_id":10,"name":"hs_sum","transition_id":"0b64c4dc-d242-4c72-b10d-32a0a828c262","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465666,"alarm_id":1791465629,"alarm_event_id":11,"name":"hs_two","transition_id":"7d62e155-bcc5-4729-b69a-0cb9657373ba","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791465665,"alarm_id":1791465630,"alarm_event_id":9,"name":"hs_sum","transition_id":"1fd77aa3-678c-4fa8-b4f8-8cdcf92ee0f3","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465664,"alarm_id":1791465629,"alarm_event_id":10,"name":"hs_two","transition_id":"cbb94327-a0cc-4161-9b7f-389fe4b16d13","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465663,"alarm_id":1791465630,"alarm_event_id":8,"name":"hs_sum","transition_id":"b22409f5-086e-4d94-81ea-23ab95270781","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465662,"alarm_id":1791465629,"alarm_event_id":9,"name":"hs_two","transition_id":"008bc4e3-24df-4381-912b-c57d6100a589","when":1791465641,"duration":8,"non_clear_duration":8,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"WARNING"},{"unique_id":1791465661,"alarm_id":1791465628,"alarm_event_id":7,"name":"hs_tpl","transition_id":"d0f966fb-aaf9-4079-8ade-99d9c34afa36","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465660,"alarm_id":1791465628,"alarm_event_id":6,"name":"hs_tpl","transition_id":"36f56261-b995-427d-bd84-a89dca6b9f27","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465659,"alarm_id":1791465627,"alarm_event_id":7,"name":"hs_two","transition_id":"15970670-2924-4867-b0c6-465ef0ad4f95","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465658,"alarm_id":1791465626,"alarm_event_id":7,"name":"hs_tpl","transition_id":"e0817b0e-9bdd-4e77-99bc-047d5643a9a8","when":1791465641,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465657,"alarm_id":1791465627,"alarm_event_id":6,"name":"hs_two","transition_id":"dadb0a99-dded-4b32-b048-a19eeea664b0","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465656,"alarm_id":1791465626,"alarm_event_id":6,"name":"hs_tpl","transition_id":"8f29e718-29d4-4d90-a87f-3619ba09d3fb","when":1791465641,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465655,"alarm_id":1791465630,"alarm_event_id":7,"name":"hs_sum","transition_id":"e5299731-8cc4-4298-81aa-536d8563b444","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465654,"alarm_id":1791465629,"alarm_event_id":8,"name":"hs_two","transition_id":"35f9838f-82c5-4e33-b7c8-a2b1cbcbfdb2","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791465653,"alarm_id":1791465630,"alarm_event_id":6,"name":"hs_sum","transition_id":"f16cc5ec-f1fc-4cce-98ee-8244f605bdf9","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465652,"alarm_id":1791465629,"alarm_event_id":7,"name":"hs_two","transition_id":"6f0676a4-c697-4789-9c28-521df5268c2a","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465651,"alarm_id":1791465630,"alarm_event_id":5,"name":"hs_sum","transition_id":"db476803-7854-413e-acd5-e481e343fb6b","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465650,"alarm_id":1791465629,"alarm_event_id":6,"name":"hs_two","transition_id":"55f25bf6-4230-49fd-b282-bd8343f2f572","when":1791465633,"duration":4,"non_clear_duration":4,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"WARNING"},{"unique_id":1791465649,"alarm_id":1791465628,"alarm_event_id":5,"name":"hs_tpl","transition_id":"2f68c9eb-721a-4ae7-984c-03755ccbcb59","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465648,"alarm_id":1791465628,"alarm_event_id":4,"name":"hs_tpl","transition_id":"caebb52e-8048-422c-8b99-fa9fa3c2d914","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465647,"alarm_id":1791465627,"alarm_event_id":5,"name":"hs_two","transition_id":"e11c5898-fe4b-427a-8c7b-09c6ccd93668","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465646,"alarm_id":1791465626,"alarm_event_id":5,"name":"hs_tpl","transition_id":"97a2b04f-6d68-4e61-b61b-cb6417b1aabd","when":1791465633,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465645,"alarm_id":1791465627,"alarm_event_id":4,"name":"hs_two","transition_id":"59ecce57-9ff7-4a04-a068-7da408d86b6b","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465644,"alarm_id":1791465626,"alarm_event_id":4,"name":"hs_tpl","transition_id":"9b99e208-0ff8-41ef-b924-31b42f3065dc","when":1791465633,"duration":8,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465633,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465643,"alarm_id":1791465629,"alarm_event_id":5,"name":"hs_two","transition_id":"b70f425b-4744-4550-8dd3-03a3e69c4017","when":1791465629,"duration":4,"non_clear_duration":0,"exec_run":1791465629,"delay_up_to_timestamp":1791465629,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791465642,"alarm_id":1791465630,"alarm_event_id":4,"name":"hs_sum","transition_id":"f89b23ba-6dc9-48f0-b731-50e80b16f17b","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465641,"alarm_id":1791465629,"alarm_event_id":4,"name":"hs_two","transition_id":"c6df38ba-e433-45b3-be8d-b4c02c1d4ade","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465640,"alarm_id":1791465630,"alarm_event_id":3,"name":"hs_sum","transition_id":"a6f0e8e4-bdf8-4c6d-826a-22b53446dafd","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465639,"alarm_id":1791465629,"alarm_event_id":3,"name":"hs_two","transition_id":"8cccd621-63bc-46f8-8dc2-745c7f05a7e2","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465638,"alarm_id":1791465630,"alarm_event_id":2,"name":"hs_sum","transition_id":"69e2fd99-89c7-4295-8eaf-56588d27c201","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465637,"alarm_id":1791465629,"alarm_event_id":2,"name":"hs_two","transition_id":"b6cd2ce7-a956-4a4a-87d6-984073aabbcb","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465636,"alarm_id":1791465628,"alarm_event_id":3,"name":"hs_tpl","transition_id":"88fb9353-d78a-4d84-8806-22cf878c6a84","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465635,"alarm_id":1791465628,"alarm_event_id":2,"name":"hs_tpl","transition_id":"e2db44d6-b016-4510-932a-82e5e544002c","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465634,"alarm_id":1791465627,"alarm_event_id":3,"name":"hs_two","transition_id":"f35448c3-fe35-4a2e-b8fc-d40a4d8c1db8","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465633,"alarm_id":1791465626,"alarm_event_id":3,"name":"hs_tpl","transition_id":"5292efe0-c2ed-49b8-99c2-d8c74725124c","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465632,"alarm_id":1791465627,"alarm_event_id":2,"name":"hs_two","transition_id":"cbe0f220-a2fb-424e-a3a7-6e09733ebab6","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465631,"alarm_id":1791465626,"alarm_event_id":2,"name":"hs_tpl","transition_id":"36b50bf2-b457-4263-8abf-15f92c3c1ab5","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791465630,"alarm_id":1791465630,"alarm_event_id":1,"name":"hs_sum","transition_id":"25c83c25-4bf1-4381-9d7d-02130ae87773","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465629,"alarm_id":1791465629,"alarm_event_id":1,"name":"hs_two","transition_id":"e0ab3013-30d4-41da-bacb-e1d871cd8447","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465628,"alarm_id":1791465628,"alarm_event_id":1,"name":"hs_tpl","transition_id":"17a62334-bd42-49c4-a33a-b64eb7a6d33e","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465627,"alarm_id":1791465627,"alarm_event_id":1,"name":"hs_two","transition_id":"f543f1fc-12a7-41da-85c0-b72a0f72c4b0","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791465626,"alarm_id":1791465626,"alarm_event_id":1,"name":"hs_tpl","transition_id":"325031e1-4102-4a0e-897e-e8c5e709d28a","when":1791465625,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465625,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L09": `[{"unique_id":1791465639,"alarm_id":1791465637,"alarm_event_id":3,"name":"hs_tpl","transition_id":"70cc95f2-ef4c-4642-a4dd-1fe89bc87d20","when":1791465641,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465638,"alarm_id":1791465637,"alarm_event_id":2,"name":"hs_tpl","transition_id":"4a5228cd-5bd6-4503-a3ad-a16659076a6e","when":1791465637,"duration":1,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465637,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465637,"alarm_id":1791465637,"alarm_event_id":1,"name":"hs_tpl","transition_id":"d210908f-d979-4699-b998-ed81e1c5c1cf","when":1791465636,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465636,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L10": `[{"unique_id":1791465639,"alarm_id":1791465637,"alarm_event_id":3,"name":"hs_tpl","transition_id":"5b92b6bb-61e0-4d62-8c54-2d36bd257a57","when":1791465641,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465641,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791465638,"alarm_id":1791465637,"alarm_event_id":2,"name":"hs_tpl","transition_id":"9a250679-9bbb-492c-bec0-a8553e198b79","when":1791465637,"duration":1,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465637,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791465637,"alarm_id":1791465637,"alarm_event_id":1,"name":"hs_tpl","transition_id":"b020ccc6-f4ce-4d38-86ea-47fd710cb045","when":1791465636,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791465636,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
}
