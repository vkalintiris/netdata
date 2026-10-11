// SPDX-License-Identifier: GPL-3.0-or-later

package parity

// The closer rows of the alert endpoints' transitions side (alerts_closers_l_test.go) as one C-against-C run
// answered them (0 the oracle): each row's bodies with the side's run directory written `<run>` as v2Round writes
// it, the seconds each request was in flight, and the names of the alert logs the family read for it
// (dashNormClosersLLogs: each side's `/api/v1/alarm_log` and, in `two-hosts`, the child's, trimmed to the
// members the render and the views read; one per case, host and side: the longest read, which holds the entries
// of the earlier ones). Generated from the hooked copy's dumps by SA-L's tools/pins.py
// (SA-L's run r1, C against C, 2026-10-10T15:03-15:06Z).

// dashNormClosersLRow is one recorded row.
type dashNormClosersLRow struct {
	// target is what the row asked: its v2Req.target, which names each side's own id where the row has one
	target string
	bodies [2]string
	flight [2][2]int64
	// logs are the names of each side's alert log, and more of the child's (empty: no child)
	logs, more [2]string
}

// dashNormClosersLRows are the recorded rows by `<case>/<row>`.
var dashNormClosersLRows = map[string]dashNormClosersLRow{
	"states/window": {
		target: "/api/v3/alert_transitions?after=-600&last=200&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":2},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":5}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":5}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":5}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":5}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":5}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":2},{"id":"hst_delay","name":"hst_delay","count":2},{"id":"hst_named","name":"hst_named","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":4},{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":4},{"id":"hst.nctx","name":"hst.nctx","count":1}]}],"transitions":[{"gi":1791644633516394,"alert":"hst_fail","transition_id":"ec502839-9787-44f9-8931-3ebba578b61a","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":0,"to":"root"}},{"gi":1791644633516231,"alert":"hst_delay","transition_id":"395c34db-b59e-40e9-8f37-f3d86014983a","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621477053,"alert":"hst_fail","transition_id":"ddee3f31-a073-45cd-845b-ec26f1e7c980","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","UPDATED","EXEC_RUN","EXEC_FAILED","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":127,"to":"root"}},{"gi":1791644621476987,"alert":"hst_delay","transition_id":"43f5eaeb-353f-4990-b3d3-9b2d65aa7848","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644631,"delay":10,"delay_up_to_time":1791644631,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621476865,"alert":"hst_named","transition_id":"e2f3a325-64f2-4795-b5bd-e4692199bb13","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.idn","instance_n":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":5,"matched":5,"returned":5,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":1.195}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":2},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":5}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":5}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":5}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":5}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":5}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":2},{"id":"hst_delay","name":"hst_delay","count":2},{"id":"hst_named","name":"hst_named","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":4},{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":4},{"id":"hst.nctx","name":"hst.nctx","count":1}]}],"transitions":[{"gi":1791644633470567,"alert":"hst_fail","transition_id":"b12dcf7f-c7e4-4bb3-8d72-7018da1ba90e","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":0,"to":"root"}},{"gi":1791644633470399,"alert":"hst_delay","transition_id":"95390ff5-f42a-4c47-9b13-01e1c8e9eb57","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621433200,"alert":"hst_fail","transition_id":"9dc72e96-efe9-4849-a137-226f57d2139c","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","UPDATED","EXEC_RUN","EXEC_FAILED","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":127,"to":"root"}},{"gi":1791644621433132,"alert":"hst_delay","transition_id":"32cde91b-36a8-4bf2-9da6-139af0b9b8fb","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644631,"delay":10,"delay_up_to_time":1791644631,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621433017,"alert":"hst_named","transition_id":"ff99a8a1-6463-4fc8-9e7e-9c1623b8778f","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.idn","instance_n":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":5,"matched":5,"returned":5,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.694}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"half-off/raised": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=raised",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735593697,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"1d3e3090-2ebc-494d-a05e-f378aef83bbf","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.237}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735611300,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"5213ac57-0cf5-4dd2-a60d-4f093be14e7a","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.217}}`,
		},
		flight: [2][2]int64{{1791644738, 1791644738}, {1791644738, 1791644738}},
		logs:   [2]string{"L03", "L04"},
		more:   [2]string{"", ""},
	},
	"states/instance-name": {
		target: "/api/v3/alert_transitions?after=-600&last=200&options=minify&f_instance=hst.named",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":0},{"id":"hst_delay","name":"hst_delay","count":0},{"id":"hst_named","name":"hst_named","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":4},{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":0},{"id":"hst.nctx","name":"hst.nctx","count":1}]}],"transitions":[{"gi":1791644621476865,"alert":"hst_named","transition_id":"e2f3a325-64f2-4795-b5bd-e4692199bb13","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.idn","instance_n":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":5,"matched":1,"returned":1,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.525}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":0},{"id":"hst_delay","name":"hst_delay","count":0},{"id":"hst_named","name":"hst_named","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":4},{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":0},{"id":"hst.nctx","name":"hst.nctx","count":1}]}],"transitions":[{"gi":1791644621433017,"alert":"hst_named","transition_id":"ff99a8a1-6463-4fc8-9e7e-9c1623b8778f","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.idn","instance_n":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":5,"matched":1,"returned":1,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.429}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"half-off/raised-window": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=raised&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735593697,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"1d3e3090-2ebc-494d-a05e-f378aef83bbf","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.082}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735611300,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"5213ac57-0cf5-4dd2-a60d-4f093be14e7a","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.087}}`,
		},
		flight: [2][2]int64{{1791644738, 1791644738}, {1791644738, 1791644738}},
		logs:   [2]string{"L03", "L04"},
		more:   [2]string{"", ""},
	},
	"states/instance-id": {
		target: "/api/v3/alert_transitions?after=-600&last=200&options=minify&f_instance=hst.idn",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"WARNING","name":"WARNING","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":0},{"id":"hst_delay","name":"hst_delay","count":0},{"id":"hst_named","name":"hst_named","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":4},{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":0},{"id":"hst.nctx","name":"hst.nctx","count":0}]}],"transitions":[],"items":{"evaluated":5,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.296}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"WARNING","name":"WARNING","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":0},{"id":"hst_delay","name":"hst_delay","count":0},{"id":"hst_named","name":"hst_named","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":4},{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":0},{"id":"hst.nctx","name":"hst.nctx","count":0}]}],"transitions":[],"items":{"evaluated":5,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.295}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"half-off/plain": {
		target: "/api/v3/alerts?options=summary,instances,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735593697,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"1d3e3090-2ebc-494d-a05e-f378aef83bbf","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.08}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0},{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":1}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735611300,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"5213ac57-0cf5-4dd2-a60d-4f093be14e7a","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.109}}`,
		},
		flight: [2][2]int64{{1791644738, 1791644738}, {1791644738, 1791644738}},
		logs:   [2]string{"L03", "L04"},
		more:   [2]string{"", ""},
	},
	"states/mcp": {
		target: "/api/v3/alert_transitions?after=-600&last=200&options=mcp,minify",
		bodies: [2]string{
			`{"transitions":[{"gi":1791644633516394,"alert":"hst_fail","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":0,"to":"root"}},{"gi":1791644633516231,"alert":"hst_delay","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621477053,"alert":"hst_fail","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","UPDATED","EXEC_RUN","EXEC_FAILED","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":127,"to":"root"}},{"gi":1791644621476987,"alert":"hst_delay","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644631,"delay":10,"delay_up_to_time":1791644631,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621476865,"alert":"hst_named","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":5,"matched":5,"returned":5,"max_to_return":200,"before":0,"after":0}}`,
			`{"transitions":[{"gi":1791644633470567,"alert":"hst_fail","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":0,"to":"root"}},{"gi":1791644633470399,"alert":"hst_delay","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644633,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":12,"raised_duration":12},"notification":{"when":1791644633,"delay":0,"delay_up_to_time":1791644633,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621433200,"alert":"hst_fail","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","UPDATED","EXEC_RUN","EXEC_FAILED","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":127,"to":"root"}},{"gi":1791644621433132,"alert":"hst_delay","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644631,"delay":10,"delay_up_to_time":1791644631,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644621433017,"alert":"hst_named","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":5,"matched":5,"returned":5,"max_to_return":200,"before":0,"after":0}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"half-off/window": {
		target: "/api/v3/alerts?options=summary,instances,minify&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735593697,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"1d3e3090-2ebc-494d-a05e-f378aef83bbf","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.083}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644735611300,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"5213ac57-0cf5-4dd2-a60d-4f093be14e7a","tr_v":70,"tr_t":1791644735,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.084}}`,
		},
		flight: [2][2]int64{{1791644738, 1791644738}, {1791644738, 1791644738}},
		logs:   [2]string{"L03", "L04"},
		more:   [2]string{"", ""},
	},
	"states/alerts": {
		target: "/api/v3/alerts?options=summary,instances,values,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_named","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.nctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hst_gone","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.gctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hst_idle","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","sum":"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssézz","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],"cls":[],"cp":["Part_ppppppppppppppppppppppppppppppppppppppppppppppppppppppp"],"ty":["Type_ttttttttttttttttttttttttttttttttttttttttttttttttttttttt"],"to":["rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"]},{"ati":4,"ni":[0],"nm":"hst_delay","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":5,"ni":[0],"nm":"hst_fail","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":6,"ni":[0],"nm":"hst_nan","sum":"","cr":0,"wr":0,"cl":0,"er":1,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":7,"ni":[0],"nm":"hst_undef","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":1,"running":7,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":2,"er":1,"running":8,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644621476865,"nm":"hst_named","ctx":"hst.nctx","ch":"hst.idn","ch_n":"hst.named","st":"WARNING","fami":"fam","info":"on a chart with a name of its own","sum":"","units":"things","tr_i":"e2f3a325-64f2-4795-b5bd-e4692199bb13","tr_v":70,"tr_t":1791644621,"cfg":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644635},{"ati":1,"ni":0,"gi":1791644621471409,"nm":"hst_gone","ctx":"hst.gctx","ch":"hst.gone","ch_n":"hst.gone","st":"REMOVED","fami":"fam","info":"on a chart made obsolete","sum":"","units":"things","tr_i":"515d028a-0646-457a-a76c-356fa222f4e6","tr_v":null,"tr_t":1791644621,"cfg":"52ae24a6-c510-42a4-bef4-6ee3f4a6f8eb","src":"line=43,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":1791644621},{"ati":2,"ni":0,"gi":1791644617457703,"nm":"hst_idle","ctx":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","ch":"hst.idle","ch_n":"hst.idle","st":"UNINITIALIZED","fami":"fam","info":"no units on a chart without units","sum":"","units":"","tr_i":"f40997b7-f92b-4537-a50b-5ee7d8ff3e12","tr_v":null,"tr_t":1791644617,"cfg":"8f066671-1bf2-4e99-a673-c9ba5d5dbb48","src":"line=51,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":0},{"ati":3,"ni":0,"gi":1791644617457716,"nm":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","ctx":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","ch":"hst.idle","ch_n":"hst.idle","st":"UNINITIALIZED","fami":"fam","info":"long texts","sum":"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssézz","units":"uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuézz","tr_i":"82968c52-13a6-4748-ae69-34c3930ef299","tr_v":null,"tr_t":1791644617,"cfg":"56fd8536-7a03-4571-b490-1c8c9a2ab213","src":"line=58,file=<run>/etc/health.d/parity.conf","to":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","tp":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cm":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cl":"","v":null,"t":0},{"ati":4,"ni":0,"gi":1791644633516231,"nm":"hst_delay","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"CLEAR","fami":"family","info":"notified 10 s after it rose","sum":"","units":"things","tr_i":"395c34db-b59e-40e9-8f37-f3d86014983a","tr_v":10,"tr_t":1791644633,"cfg":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","src":"line=1,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635},{"ati":5,"ni":0,"gi":1791644633516394,"nm":"hst_fail","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"CLEAR","fami":"family","info":"its notifier cannot run","sum":"","units":"things","tr_i":"ec502839-9787-44f9-8931-3ebba578b61a","tr_v":10,"tr_t":1791644633,"cfg":"17ec2091-927b-440b-9d74-bfd7976c160e","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635},{"ati":6,"ni":0,"gi":1791644617470046,"nm":"hst_nan","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its value is not a number","sum":"","units":"things","tr_i":"61135a5a-7e8e-4bc6-9cb8-42dee9294c00","tr_v":null,"tr_t":1791644617,"cfg":"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":1791644635},{"ati":7,"ni":0,"gi":1791644617470127,"nm":"hst_undef","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its warning cannot be evaluated","sum":"","units":"things","tr_i":"35c17fae-7a55-473d-969d-6b12b31c4e11","tr_v":10,"tr_t":1791644617,"cfg":"9b8ef469-dc2d-45b0-9557-c605c1ee040b","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.362}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_named","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.nctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hst_gone","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.gctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hst_idle","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":3,"ni":[0],"nm":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","sum":"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssézz","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],"cls":[],"cp":["Part_ppppppppppppppppppppppppppppppppppppppppppppppppppppppp"],"ty":["Type_ttttttttttttttttttttttttttttttttttttttttttttttttttttttt"],"to":["rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"]},{"ati":4,"ni":[0],"nm":"hst_delay","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":5,"ni":[0],"nm":"hst_fail","sum":"","cr":0,"wr":0,"cl":1,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":6,"ni":[0],"nm":"hst_nan","sum":"","cr":0,"wr":0,"cl":0,"er":1,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":7,"ni":[0],"nm":"hst_undef","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":2,"er":1,"running":7,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":1,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":2,"er":1,"running":8,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644621433017,"nm":"hst_named","ctx":"hst.nctx","ch":"hst.idn","ch_n":"hst.named","st":"WARNING","fami":"fam","info":"on a chart with a name of its own","sum":"","units":"things","tr_i":"ff99a8a1-6463-4fc8-9e7e-9c1623b8778f","tr_v":70,"tr_t":1791644621,"cfg":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644635},{"ati":1,"ni":0,"gi":1791644621425598,"nm":"hst_gone","ctx":"hst.gctx","ch":"hst.gone","ch_n":"hst.gone","st":"REMOVED","fami":"fam","info":"on a chart made obsolete","sum":"","units":"things","tr_i":"3cb99235-55e8-431d-b359-bc1693a92787","tr_v":null,"tr_t":1791644621,"cfg":"52ae24a6-c510-42a4-bef4-6ee3f4a6f8eb","src":"line=43,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":1791644621},{"ati":2,"ni":0,"gi":1791644617423448,"nm":"hst_idle","ctx":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","ch":"hst.idle","ch_n":"hst.idle","st":"UNINITIALIZED","fami":"fam","info":"no units on a chart without units","sum":"","units":"","tr_i":"c6dba1fd-d84a-4c3f-bd7c-0ce8d6d93720","tr_v":null,"tr_t":1791644617,"cfg":"8f066671-1bf2-4e99-a673-c9ba5d5dbb48","src":"line=51,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":0},{"ati":3,"ni":0,"gi":1791644617423464,"nm":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","ctx":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","ch":"hst.idle","ch_n":"hst.idle","st":"UNINITIALIZED","fami":"fam","info":"long texts","sum":"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssézz","units":"uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuézz","tr_i":"1ce39840-3147-471c-b0cb-5c7a004635be","tr_v":null,"tr_t":1791644617,"cfg":"56fd8536-7a03-4571-b490-1c8c9a2ab213","src":"line=58,file=<run>/etc/health.d/parity.conf","to":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","tp":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cm":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cl":"","v":null,"t":0},{"ati":4,"ni":0,"gi":1791644633470399,"nm":"hst_delay","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"CLEAR","fami":"family","info":"notified 10 s after it rose","sum":"","units":"things","tr_i":"95390ff5-f42a-4c47-9b13-01e1c8e9eb57","tr_v":10,"tr_t":1791644633,"cfg":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","src":"line=1,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635},{"ati":5,"ni":0,"gi":1791644633470567,"nm":"hst_fail","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"CLEAR","fami":"family","info":"its notifier cannot run","sum":"","units":"things","tr_i":"b12dcf7f-c7e4-4bb3-8d72-7018da1ba90e","tr_v":10,"tr_t":1791644633,"cfg":"17ec2091-927b-440b-9d74-bfd7976c160e","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635},{"ati":6,"ni":0,"gi":1791644617424150,"nm":"hst_nan","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its value is not a number","sum":"","units":"things","tr_i":"8f8d2a08-1c2e-47f1-bf9a-5860c1b2ad3c","tr_v":null,"tr_t":1791644617,"cfg":"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":1791644635},{"ati":7,"ni":0,"gi":1791644617424252,"nm":"hst_undef","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its warning cannot be evaluated","sum":"","units":"things","tr_i":"04e51ed3-0874-44f3-b6a8-132b88e778f8","tr_v":10,"tr_t":1791644617,"cfg":"9b8ef469-dc2d-45b0-9557-c605c1ee040b","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.247}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"half-off/transitions": {
		target: "/api/v2/alert_transitions?after=-600&last=200&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hloc_calc","name":"hloc_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hloc.values","name":"hloc.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hloc.ctx","name":"hloc.ctx","count":1}]}],"transitions":[{"gi":1791644735593697,"alert":"hloc_calc","transition_id":"1d3e3090-2ebc-494d-a05e-f378aef83bbf","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644735,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644735,"delay":0,"delay_up_to_time":1791644735,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.356}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hloc_calc","name":"hloc_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hloc.values","name":"hloc.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hloc.ctx","name":"hloc.ctx","count":1}]}],"transitions":[{"gi":1791644735611300,"alert":"hloc_calc","transition_id":"5213ac57-0cf5-4dd2-a60d-4f093be14e7a","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644735,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644735,"delay":0,"delay_up_to_time":1791644735,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.304}}`,
		},
		flight: [2][2]int64{{1791644738, 1791644738}, {1791644738, 1791644738}},
		logs:   [2]string{"L03", "L04"},
		more:   [2]string{"", ""},
	},
	"states/alerts-undefined": {
		target: "/api/v3/alerts?options=summary,instances,values,minify&status=undefined",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_nan","sum":"","cr":0,"wr":0,"cl":0,"er":1,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hst_undef","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644617470046,"nm":"hst_nan","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its value is not a number","sum":"","units":"things","tr_i":"61135a5a-7e8e-4bc6-9cb8-42dee9294c00","tr_v":null,"tr_t":1791644617,"cfg":"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":1791644635},{"ati":1,"ni":0,"gi":1791644617470127,"nm":"hst_undef","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its warning cannot be evaluated","sum":"","units":"things","tr_i":"35c17fae-7a55-473d-969d-6b12b31c4e11","tr_v":10,"tr_t":1791644617,"cfg":"9b8ef469-dc2d-45b0-9557-c605c1ee040b","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.085}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_nan","sum":"","cr":0,"wr":0,"cl":0,"er":1,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hst_undef","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644617424150,"nm":"hst_nan","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its value is not a number","sum":"","units":"things","tr_i":"8f8d2a08-1c2e-47f1-bf9a-5860c1b2ad3c","tr_v":null,"tr_t":1791644617,"cfg":"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","src":"line=19,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":null,"t":1791644635},{"ati":1,"ni":0,"gi":1791644617424252,"nm":"hst_undef","ctx":"hst.ctx","ch":"hst.values","ch_n":"hst.values","st":"UNDEFINED","fami":"family","info":"its warning cannot be evaluated","sum":"","units":"things","tr_i":"04e51ed3-0874-44f3-b6a8-132b88e778f8","tr_v":10,"tr_t":1791644617,"cfg":"9b8ef469-dc2d-45b0-9557-c605c1ee040b","src":"line=27,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":10,"t":1791644635}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.081}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"states/alerts-undefined-window": {
		target: "/api/v3/alerts?options=summary,minify&status=undefined&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_nan","sum":"","cr":0,"wr":0,"cl":0,"er":1,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hst_undef","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.049}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_nan","sum":"","cr":0,"wr":0,"cl":0,"er":1,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hst_undef","sum":"","cr":0,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":0,"cl":0,"er":1,"running":2,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.055}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"states/alerts-raised": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=raised",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_named","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.nctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644621476865,"nm":"hst_named","ctx":"hst.nctx","ch":"hst.idn","ch_n":"hst.named","st":"WARNING","fami":"fam","info":"on a chart with a name of its own","sum":"","units":"things","tr_i":"e2f3a325-64f2-4795-b5bd-e4692199bb13","tr_v":70,"tr_t":1791644621,"cfg":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.045}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hst_named","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hst.nctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[{"name":"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_component":[{"name":"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":7},{"name":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":1}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644621433017,"nm":"hst_named","ctx":"hst.nctx","ch":"hst.idn","ch_n":"hst.named","st":"WARNING","fami":"fam","info":"on a chart with a name of its own","sum":"","units":"things","tr_i":"ff99a8a1-6463-4fc8-9e7e-9c1623b8778f","tr_v":70,"tr_t":1791644621,"cfg":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","src":"line=35,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.061}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/nodes-child": {
		target: "/api/v3/alerts?options=summary,instances,values,minify&nodes=health-child",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hch_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hchild.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"fanout","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762603900,"nm":"hch_calc","ctx":"hchild.ctx","ch":"hchild.values","ch_n":"hchild.values","st":"WARNING","fami":"family","info":"the child s last value of a","sum":"","units":"things","tr_i":"00c9ea18-d256-47f5-877e-0adafdc0ed7c","tr_v":70,"tr_t":1791644762,"cfg":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.047}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hch_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hchild.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"fanout","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762427065,"nm":"hch_calc","ctx":"hchild.ctx","ch":"hchild.values","ch_n":"hchild.values","st":"WARNING","fami":"family","info":"the child s last value of a","sum":"","units":"things","tr_i":"9f5c35c4-61cd-487b-8d9c-f6479ead484c","tr_v":70,"tr_t":1791644762,"cfg":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.059}}`,
		},
		flight: [2][2]int64{{1791644762, 1791644762}, {1791644762, 1791644762}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/alerts-mcp": {
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
        ["hst_named","parity-parent","hst.nctx","hst.named","WARNING","fam","on a chart with a name of its own","","things","e2f3a325-64f2-4795-b5bd-e4692199bb13",70,1791644621,"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","line=35,file=<run>/etc/health.d/parity.conf","root","","","",70,1791644635],
        ["hst_gone","parity-parent","hst.gctx","hst.gone","REMOVED","fam","on a chart made obsolete","","things","515d028a-0646-457a-a76c-356fa222f4e6",null,1791644621,"52ae24a6-c510-42a4-bef4-6ee3f4a6f8eb","line=43,file=<run>/etc/health.d/parity.conf","root","","","",null,1791644621],
        ["hst_idle","parity-parent","hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","hst.idle","UNINITIALIZED","fam","no units on a chart without units","","","f40997b7-f92b-4537-a50b-5ee7d8ff3e12",null,1791644617,"8f066671-1bf2-4e99-a673-c9ba5d5dbb48","line=51,file=<run>/etc/health.d/parity.conf","root","","","",null,0],
        ["hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","parity-parent","hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","hst.idle","UNINITIALIZED","fam","long texts","ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssézz","uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuézz","82968c52-13a6-4748-ae69-34c3930ef299",null,1791644617,"56fd8536-7a03-4571-b490-1c8c9a2ab213","line=58,file=<run>/etc/health.d/parity.conf","rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","",null,0],
        ["hst_delay","parity-parent","hst.ctx","hst.values","CLEAR","family","notified 10 s after it rose","","things","395c34db-b59e-40e9-8f37-f3d86014983a",10,1791644633,"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","line=1,file=<run>/etc/health.d/parity.conf","root","","","",10,1791644635],
        ["hst_fail","parity-parent","hst.ctx","hst.values","CLEAR","family","its notifier cannot run","","things","ec502839-9787-44f9-8931-3ebba578b61a",10,1791644633,"17ec2091-927b-440b-9d74-bfd7976c160e","line=10,file=<run>/etc/health.d/parity.conf","root","","","",10,1791644635],
        ["hst_nan","parity-parent","hst.ctx","hst.values","UNDEFINED","family","its value is not a number","","things","61135a5a-7e8e-4bc6-9cb8-42dee9294c00",null,1791644617,"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","line=19,file=<run>/etc/health.d/parity.conf","root","","","",null,1791644635],
        ["hst_undef","parity-parent","hst.ctx","hst.values","UNDEFINED","family","its warning cannot be evaluated","","things","35c17fae-7a55-473d-969d-6b12b31c4e11",10,1791644617,"9b8ef469-dc2d-45b0-9557-c605c1ee040b","line=27,file=<run>/etc/health.d/parity.conf","root","","","",10,1791644635]]
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
        ["hst_named","parity-parent","hst.nctx","hst.named","WARNING","fam","on a chart with a name of its own","","things","ff99a8a1-6463-4fc8-9e7e-9c1623b8778f",70,1791644621,"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","line=35,file=<run>/etc/health.d/parity.conf","root","","","",70,1791644635],
        ["hst_gone","parity-parent","hst.gctx","hst.gone","REMOVED","fam","on a chart made obsolete","","things","3cb99235-55e8-431d-b359-bc1693a92787",null,1791644621,"52ae24a6-c510-42a4-bef4-6ee3f4a6f8eb","line=43,file=<run>/etc/health.d/parity.conf","root","","","",null,1791644621],
        ["hst_idle","parity-parent","hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","hst.idle","UNINITIALIZED","fam","no units on a chart without units","","","c6dba1fd-d84a-4c3f-bd7c-0ce8d6d93720",null,1791644617,"8f066671-1bf2-4e99-a673-c9ba5d5dbb48","line=51,file=<run>/etc/health.d/parity.conf","root","","","",null,0],
        ["hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","parity-parent","hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","hst.idle","UNINITIALIZED","fam","long texts","ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssézz","uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuézz","1ce39840-3147-471c-b0cb-5c7a004635be",null,1791644617,"56fd8536-7a03-4571-b490-1c8c9a2ab213","line=58,file=<run>/etc/health.d/parity.conf","rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr","Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt","Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp","",null,0],
        ["hst_delay","parity-parent","hst.ctx","hst.values","CLEAR","family","notified 10 s after it rose","","things","95390ff5-f42a-4c47-9b13-01e1c8e9eb57",10,1791644633,"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","line=1,file=<run>/etc/health.d/parity.conf","root","","","",10,1791644635],
        ["hst_fail","parity-parent","hst.ctx","hst.values","CLEAR","family","its notifier cannot run","","things","b12dcf7f-c7e4-4bb3-8d72-7018da1ba90e",10,1791644633,"17ec2091-927b-440b-9d74-bfd7976c160e","line=10,file=<run>/etc/health.d/parity.conf","root","","","",10,1791644635],
        ["hst_nan","parity-parent","hst.ctx","hst.values","UNDEFINED","family","its value is not a number","","things","8f8d2a08-1c2e-47f1-bf9a-5860c1b2ad3c",null,1791644617,"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","line=19,file=<run>/etc/health.d/parity.conf","root","","","",null,1791644635],
        ["hst_undef","parity-parent","hst.ctx","hst.values","UNDEFINED","family","its warning cannot be evaluated","","things","04e51ed3-0874-44f3-b6a8-132b88e778f8",10,1791644617,"9b8ef469-dc2d-45b0-9557-c605c1ee040b","line=27,file=<run>/etc/health.d/parity.conf","root","","","",10,1791644635]]
}
`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/nodes-child-guid": {
		target: "/api/v3/alerts?options=summary,instances,values,minify&nodes=5a1e0000-0000-4000-8000-0000000000c1",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hch_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hchild.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"fanout","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762603900,"nm":"hch_calc","ctx":"hchild.ctx","ch":"hchild.values","ch_n":"hchild.values","st":"WARNING","fami":"family","info":"the child s last value of a","sum":"","units":"things","tr_i":"00c9ea18-d256-47f5-877e-0adafdc0ed7c","tr_v":70,"tr_t":1791644762,"cfg":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.093}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000c1","nm":"health-child","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hch_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hchild.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"fanout","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762427065,"nm":"hch_calc","ctx":"hchild.ctx","ch":"hchild.values","ch_n":"hchild.values","st":"WARNING","fami":"family","info":"the child s last value of a","sum":"","units":"things","tr_i":"9f5c35c4-61cd-487b-8d9c-f6479ead484c","tr_v":70,"tr_t":1791644762,"cfg":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.043}}`,
		},
		flight: [2][2]int64{{1791644762, 1791644762}, {1791644762, 1791644762}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/delay": {
		target: "/api/v3/alert_transitions?transition=<delay>&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_delay","name":"hst_delay","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644621476987,"alert":"hst_delay","transition_id":"43f5eaeb-353f-4990-b3d3-9b2d65aa7848","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644631,"delay":10,"delay_up_to_time":1791644631,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.201}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_delay","name":"hst_delay","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644621433132,"alert":"hst_delay","transition_id":"32cde91b-36a8-4bf2-9da6-139af0b9b8fb","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"63b2e1ff-5cfb-4f0c-8faa-aa1a04bf067c","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"notified 10 s after it rose","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644631,"delay":10,"delay_up_to_time":1791644631,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.225}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"states/failed": {
		target: "/api/v3/alert_transitions?transition=<failed>&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644621477053,"alert":"hst_fail","transition_id":"ddee3f31-a073-45cd-845b-ec26f1e7c980","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","UPDATED","EXEC_RUN","EXEC_FAILED","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":127,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.227}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_fail","name":"hst_fail","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644621433200,"alert":"hst_fail","transition_id":"9dc72e96-efe9-4849-a137-226f57d2139c","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"17ec2091-927b-440b-9d74-bfd7976c160e","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"its notifier cannot run","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","UPDATED","EXEC_RUN","EXEC_FAILED","SAVED"],"exec":"/dev/null/hst-notifier","exec_code":127,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.15}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/scope-nodes-parent": {
		target: "/api/v3/alerts?options=summary,instances,values,minify&scope_nodes=parity-parent",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762602849,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"f155eb85-88b7-43e1-a639-39b7338ea72a","tr_v":70,"tr_t":1791644762,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.059}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762425978,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"1e836cf1-ac97-4953-ac8c-2b221145b279","tr_v":70,"tr_t":1791644762,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.041}}`,
		},
		flight: [2][2]int64{{1791644762, 1791644762}, {1791644762, 1791644762}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/nan": {
		target: "/api/v3/alert_transitions?transition=<nan>&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNDEFINED","name":"UNDEFINED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_nan","name":"hst_nan","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644617470046,"alert":"hst_nan","transition_id":"61135a5a-7e8e-4bc6-9cb8-42dee9294c00","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644617,"info":"its value is not a number","summary":"","units":"things","new":{"status":"UNDEFINED","value":0},"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644617,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.297}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNDEFINED","name":"UNDEFINED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_nan","name":"hst_nan","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644617424150,"alert":"hst_nan","transition_id":"8f8d2a08-1c2e-47f1-bf9a-5860c1b2ad3c","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"3d34e74f-aa91-44d4-8f43-df8409c9bc7d","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644617,"info":"its value is not a number","summary":"","units":"things","new":{"status":"UNDEFINED","value":0},"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644617,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.164}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/nodes-not-child": {
		target: "/api/v3/alerts?options=summary,instances,values,minify&nodes=!health-child%7C*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762602849,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"f155eb85-88b7-43e1-a639-39b7338ea72a","tr_v":70,"tr_t":1791644762,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.03}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hloc_calc","sum":"","cr":0,"wr":1,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hloc.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"available":2}],"alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644762425978,"nm":"hloc_calc","ctx":"hloc.ctx","ch":"hloc.values","ch_n":"hloc.values","st":"WARNING","fami":"family","info":"localhost s last value of a","sum":"","units":"things","tr_i":"1e836cf1-ac97-4953-ac8c-2b221145b279","tr_v":70,"tr_t":1791644762,"cfg":"74b7ae27-c469-4178-939f-9d2d07296c22","src":"line=10,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":70,"t":1791644762}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.033}}`,
		},
		flight: [2][2]int64{{1791644762, 1791644762}, {1791644762, 1791644762}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/undef": {
		target: "/api/v3/alert_transitions?transition=<undef>&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNDEFINED","name":"UNDEFINED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_undef","name":"hst_undef","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644617470127,"alert":"hst_undef","transition_id":"35c17fae-7a55-473d-969d-6b12b31c4e11","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"9b8ef469-dc2d-45b0-9557-c605c1ee040b","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644617,"info":"its warning cannot be evaluated","summary":"","units":"things","new":{"status":"UNDEFINED","value":10},"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644617,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.236}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNDEFINED","name":"UNDEFINED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_undef","name":"hst_undef","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.values","name":"hst.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.ctx","name":"hst.ctx","count":1}]}],"transitions":[{"gi":1791644617424252,"alert":"hst_undef","transition_id":"04e51ed3-0874-44f3-b6a8-132b88e778f8","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"9b8ef469-dc2d-45b0-9557-c605c1ee040b","hostname":"parity-parent","instance":"hst.values","instance_n":"hst.values","context":"hst.ctx","component":null,"classification":null,"type":null,"when":1791644617,"info":"its warning cannot be evaluated","summary":"","units":"things","new":{"status":"UNDEFINED","value":10},"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644617,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.269}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/transitions": {
		target: "/api/v2/alert_transitions?after=-600&last=200&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":4},{"id":"CLEAR","name":"CLEAR","count":2}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":6}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000c1","name":"health-child","count":3},{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hch_calc","name":"hch_calc","count":3},{"id":"hloc_calc","name":"hloc_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hchild.values","name":"hchild.values","count":3},{"id":"hloc.values","name":"hloc.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hchild.ctx","name":"hchild.ctx","count":3},{"id":"hloc.ctx","name":"hloc.ctx","count":3}]}],"transitions":[{"gi":1791644772665082,"alert":"hch_calc","transition_id":"6958a836-a18e-4c6b-8085-7c8b5f08660b","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644772654817,"alert":"hloc_calc","transition_id":"8a52723e-8c75-42c6-ae9d-6c0ae5a188b0","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":1791644772,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767639223,"alert":"hch_calc","transition_id":"7ea45887-8f3a-4881-b98d-46d09d93408c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767632979,"alert":"hloc_calc","transition_id":"d8ef399e-ebb5-456a-84f8-0d43b26465e6","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":1791644767,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762603900,"alert":"hch_calc","transition_id":"00c9ea18-d256-47f5-877e-0adafdc0ed7c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762602849,"alert":"hloc_calc","transition_id":"f155eb85-88b7-43e1-a639-39b7338ea72a","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":6,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":6,"returned":6,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.558}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":4},{"id":"CLEAR","name":"CLEAR","count":2}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":6}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000c1","name":"health-child","count":3},{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hch_calc","name":"hch_calc","count":3},{"id":"hloc_calc","name":"hloc_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hchild.values","name":"hchild.values","count":3},{"id":"hloc.values","name":"hloc.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hchild.ctx","name":"hchild.ctx","count":3},{"id":"hloc.ctx","name":"hloc.ctx","count":3}]}],"transitions":[{"gi":1791644772486128,"alert":"hch_calc","transition_id":"4f1768b6-e353-43c7-86da-59d02d237221","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644772479375,"alert":"hloc_calc","transition_id":"a5be155b-29f6-471b-995d-8ab6b12293ff","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":1791644772,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767462525,"alert":"hch_calc","transition_id":"14c602f8-b09e-41a2-852d-58d25d23a501","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767455476,"alert":"hloc_calc","transition_id":"9cdb2132-5506-41cc-a61d-d577984cad75","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":1791644767,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762427065,"alert":"hch_calc","transition_id":"9f5c35c4-61cd-487b-8d9c-f6479ead484c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762425978,"alert":"hloc_calc","transition_id":"1e836cf1-ac97-4953-ac8c-2b221145b279","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","instance_n":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":6,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":6,"returned":6,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.444}}`,
		},
		flight: [2][2]int64{{1791644775, 1791644775}, {1791644775, 1791644775}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/named": {
		target: "/api/v3/alert_transitions?transition=<named>&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_named","name":"hst_named","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.nctx","name":"hst.nctx","count":1}]}],"transitions":[{"gi":1791644621476865,"alert":"hst_named","transition_id":"e2f3a325-64f2-4795-b5bd-e4692199bb13","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.idn","instance_n":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.241}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_named","name":"hst_named","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.named","name":"hst.named","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.nctx","name":"hst.nctx","count":1}]}],"transitions":[{"gi":1791644621433017,"alert":"hst_named","transition_id":"ff99a8a1-6463-4fc8-9e7e-9c1623b8778f","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.idn","instance_n":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.162}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"states/named-mcp": {
		target: "/api/v3/alert_transitions?transition=<named-mcp>&options=mcp,minify",
		bodies: [2]string{
			`{"transitions":[{"gi":1791644621476865,"alert":"hst_named","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0}}`,
			`{"transitions":[{"gi":1791644621433017,"alert":"hst_named","config_hash_id":"c3e1552e-ae0f-49cb-8c5e-5d21518046e9","hostname":"parity-parent","instance":"hst.named","context":"hst.nctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart with a name of its own","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":1791644621,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/transitions-f-node": {
		target: "/api/v2/alert_transitions?after=-600&last=200&options=minify&f_node=5a1e0000-0000-4000-8000-0000000000c1",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":2},{"id":"CLEAR","name":"CLEAR","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000c1","name":"health-child","count":3},{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hch_calc","name":"hch_calc","count":3},{"id":"hloc_calc","name":"hloc_calc","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hchild.values","name":"hchild.values","count":3},{"id":"hloc.values","name":"hloc.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hchild.ctx","name":"hchild.ctx","count":3},{"id":"hloc.ctx","name":"hloc.ctx","count":0}]}],"transitions":[{"gi":1791644772665082,"alert":"hch_calc","transition_id":"6958a836-a18e-4c6b-8085-7c8b5f08660b","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767639223,"alert":"hch_calc","transition_id":"7ea45887-8f3a-4881-b98d-46d09d93408c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762603900,"alert":"hch_calc","transition_id":"00c9ea18-d256-47f5-877e-0adafdc0ed7c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.552}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":2},{"id":"CLEAR","name":"CLEAR","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000c1","name":"health-child","count":3},{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hch_calc","name":"hch_calc","count":3},{"id":"hloc_calc","name":"hloc_calc","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hchild.values","name":"hchild.values","count":3},{"id":"hloc.values","name":"hloc.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hchild.ctx","name":"hchild.ctx","count":3},{"id":"hloc.ctx","name":"hloc.ctx","count":0}]}],"transitions":[{"gi":1791644772486128,"alert":"hch_calc","transition_id":"4f1768b6-e353-43c7-86da-59d02d237221","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767462525,"alert":"hch_calc","transition_id":"14c602f8-b09e-41a2-852d-58d25d23a501","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762427065,"alert":"hch_calc","transition_id":"9f5c35c4-61cd-487b-8d9c-f6479ead484c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.399}}`,
		},
		flight: [2][2]int64{{1791644775, 1791644775}, {1791644775, 1791644775}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/idle": {
		target: "/api/v3/alert_transitions?transition=<idle>&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNINITIALIZED","name":"UNINITIALIZED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_idle","name":"hst_idle","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.idle","name":"hst.idle","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","name":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","count":1}]}],"transitions":[{"gi":1791644617457703,"alert":"hst_idle","transition_id":"f40997b7-f92b-4537-a50b-5ee7d8ff3e12","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"8f066671-1bf2-4e99-a673-c9ba5d5dbb48","hostname":"parity-parent","instance":"hst.idle","instance_n":"hst.idle","context":"hst.ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","component":null,"classification":null,"type":null,"when":1791644617,"info":"no units on a chart without units","summary":"","units":null,"new":{"status":"UNINITIALIZED","value":0},"old":{"status":"REMOVED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644617,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.335}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNINITIALIZED","name":"UNINITIALIZED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_idle","name":"hst_idle","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.idle","name":"hst.idle","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","name":"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","count":1}]}],"transitions":[{"gi":1791644617423448,"alert":"hst_idle","transition_id":"c6dba1fd-d84a-4c3f-bd7c-0ce8d6d93720","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"8f066671-1bf2-4e99-a673-c9ba5d5dbb48","hostname":"parity-parent","instance":"hst.idle","instance_n":"hst.idle","context":"hst.ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","component":null,"classification":null,"type":null,"when":1791644617,"info":"no units on a chart without units","summary":"","units":null,"new":{"status":"UNINITIALIZED","value":0},"old":{"status":"REMOVED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644617,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.194}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/transitions-nodes": {
		target: "/api/v2/alert_transitions?after=-600&last=200&options=minify&nodes=health-child",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":2},{"id":"CLEAR","name":"CLEAR","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000c1","name":"health-child","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hch_calc","name":"hch_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hchild.values","name":"hchild.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hchild.ctx","name":"hchild.ctx","count":3}]}],"transitions":[{"gi":1791644772665082,"alert":"hch_calc","transition_id":"6958a836-a18e-4c6b-8085-7c8b5f08660b","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767639223,"alert":"hch_calc","transition_id":"7ea45887-8f3a-4881-b98d-46d09d93408c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762603900,"alert":"hch_calc","transition_id":"00c9ea18-d256-47f5-877e-0adafdc0ed7c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":3,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.449}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":2},{"id":"CLEAR","name":"CLEAR","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000c1","name":"health-child","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hch_calc","name":"hch_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hchild.values","name":"hchild.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hchild.ctx","name":"hchild.ctx","count":3}]}],"transitions":[{"gi":1791644772486128,"alert":"hch_calc","transition_id":"4f1768b6-e353-43c7-86da-59d02d237221","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767462525,"alert":"hch_calc","transition_id":"14c602f8-b09e-41a2-852d-58d25d23a501","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762427065,"alert":"hch_calc","transition_id":"9f5c35c4-61cd-487b-8d9c-f6479ead484c","machine_guid":"5a1e0000-0000-4000-8000-0000000000c1","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","instance_n":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":3,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.339}}`,
		},
		flight: [2][2]int64{{1791644775, 1791644775}, {1791644775, 1791644775}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/long": {
		target: "/api/v3/alert_transitions?transition=<long>&options=minify",
		bodies: [2]string{
			"{\"api\":2,\"facets\":[{\"id\":\"f_status\",\"name\":\"Alert Status\",\"order\":1,\"options\":[{\"id\":\"UNINITIALIZED\",\"name\":\"UNINITIALIZED\",\"count\":1}]},{\"id\":\"f_class\",\"name\":\"Alert Class\",\"order\":4,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":1}]},{\"id\":\"f_type\",\"name\":\"Alert Type\",\"order\":2,\"options\":[{\"id\":\"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt\",\"name\":\"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt\",\"count\":1}]},{\"id\":\"f_component\",\"name\":\"Alert Component\",\"order\":5,\"options\":[{\"id\":\"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp\",\"name\":\"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp\",\"count\":1}]},{\"id\":\"f_role\",\"name\":\"Recipient Role\",\"order\":3,\"options\":[{\"id\":\"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr\",\"name\":\"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr\",\"count\":1}]},{\"id\":\"f_node\",\"name\":\"Alert Node\",\"order\":6,\"options\":[{\"id\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"name\":\"parity-parent\",\"count\":1}]},{\"id\":\"f_alert\",\"name\":\"Alert Name\",\"order\":7,\"options\":[{\"id\":\"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\",\"name\":\"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\",\"count\":1}]},{\"id\":\"f_instance\",\"name\":\"Instance Name\",\"order\":8,\"options\":[{\"id\":\"hst.idle\",\"name\":\"hst.idle\",\"count\":1}]},{\"id\":\"f_context\",\"name\":\"Context\",\"order\":9,\"options\":[{\"id\":\"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"count\":1}]}],\"transitions\":[{\"gi\":1791644617457716,\"alert\":\"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\",\"transition_id\":\"82968c52-13a6-4748-ae69-34c3930ef299\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"56fd8536-7a03-4571-b490-1c8c9a2ab213\",\"hostname\":\"parity-parent\",\"instance\":\"hst.idle\",\"instance_n\":\"hst.idle\",\"context\":\"hst.ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"component\":\"Part pppppppppppppppppppppppppppppppppppppppppp\",\"classification\":null,\"type\":\"Type tttttttttttttttttttttttttttttttttttttttttt\",\"when\":1791644617,\"info\":\"long texts\",\"summary\":\"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss\xc3\",\"units\":\"uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu\xc3\",\"new\":{\"status\":\"UNINITIALIZED\",\"value\":0},\"old\":{\"status\":\"REMOVED\",\"value\":0,\"duration\":0,\"raised_duration\":0},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791644617,\"flags\":[\"PROCESSED\",\"SAVED\"],\"exec\":\"/dev/null/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx/yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\",\"exec_code\":0,\"to\":\"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr\"}}],\"items\":{\"evaluated\":1,\"matched\":1,\"returned\":1,\"max_to_return\":1,\"before\":0,\"after\":0},\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.271}}",
			"{\"api\":2,\"facets\":[{\"id\":\"f_status\",\"name\":\"Alert Status\",\"order\":1,\"options\":[{\"id\":\"UNINITIALIZED\",\"name\":\"UNINITIALIZED\",\"count\":1}]},{\"id\":\"f_class\",\"name\":\"Alert Class\",\"order\":4,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":1}]},{\"id\":\"f_type\",\"name\":\"Alert Type\",\"order\":2,\"options\":[{\"id\":\"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt\",\"name\":\"Type ttttttttttttttttttttttttttttttttttttttttttttttttttttttt\",\"count\":1}]},{\"id\":\"f_component\",\"name\":\"Alert Component\",\"order\":5,\"options\":[{\"id\":\"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp\",\"name\":\"Part ppppppppppppppppppppppppppppppppppppppppppppppppppppppp\",\"count\":1}]},{\"id\":\"f_role\",\"name\":\"Recipient Role\",\"order\":3,\"options\":[{\"id\":\"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr\",\"name\":\"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr\",\"count\":1}]},{\"id\":\"f_node\",\"name\":\"Alert Node\",\"order\":6,\"options\":[{\"id\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"name\":\"parity-parent\",\"count\":1}]},{\"id\":\"f_alert\",\"name\":\"Alert Name\",\"order\":7,\"options\":[{\"id\":\"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\",\"name\":\"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\",\"count\":1}]},{\"id\":\"f_instance\",\"name\":\"Instance Name\",\"order\":8,\"options\":[{\"id\":\"hst.idle\",\"name\":\"hst.idle\",\"count\":1}]},{\"id\":\"f_context\",\"name\":\"Context\",\"order\":9,\"options\":[{\"id\":\"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"hst.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"count\":1}]}],\"transitions\":[{\"gi\":1791644617423464,\"alert\":\"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\",\"transition_id\":\"1ce39840-3147-471c-b0cb-5c7a004635be\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"56fd8536-7a03-4571-b490-1c8c9a2ab213\",\"hostname\":\"parity-parent\",\"instance\":\"hst.idle\",\"instance_n\":\"hst.idle\",\"context\":\"hst.ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"component\":\"Part pppppppppppppppppppppppppppppppppppppppppp\",\"classification\":null,\"type\":\"Type tttttttttttttttttttttttttttttttttttttttttt\",\"when\":1791644617,\"info\":\"long texts\",\"summary\":\"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss\xc3\",\"units\":\"uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu\xc3\",\"new\":{\"status\":\"UNINITIALIZED\",\"value\":0},\"old\":{\"status\":\"REMOVED\",\"value\":0,\"duration\":0,\"raised_duration\":0},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791644617,\"flags\":[\"PROCESSED\",\"SAVED\"],\"exec\":\"/dev/null/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx/yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\",\"exec_code\":0,\"to\":\"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr\"}}],\"items\":{\"evaluated\":1,\"matched\":1,\"returned\":1,\"max_to_return\":1,\"before\":0,\"after\":0},\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.232}}",
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/transitions-mcp": {
		target: "/api/v2/alert_transitions?after=-600&last=200&options=mcp,minify",
		bodies: [2]string{
			`{"transitions":[{"gi":1791644772665082,"alert":"hch_calc","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644772654817,"alert":"hloc_calc","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":1791644772,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767639223,"alert":"hch_calc","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767632979,"alert":"hloc_calc","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":1791644767,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762603900,"alert":"hch_calc","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762602849,"alert":"hloc_calc","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":6,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":6,"returned":6,"max_to_return":200,"before":0,"after":0}}`,
			`{"transitions":[{"gi":1791644772486128,"alert":"hch_calc","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644772479375,"alert":"hloc_calc","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644772,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":5,"raised_duration":0},"notification":{"when":1791644772,"delay":0,"delay_up_to_time":1791644772,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767462525,"alert":"hch_calc","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","SILENCED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644767455476,"alert":"hloc_calc","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644767,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":1791644767,"delay":0,"delay_up_to_time":1791644767,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762427065,"alert":"hch_calc","config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c","hostname":"health-child","instance":"hchild.values","context":"hchild.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"the child s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":2,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791644762425978,"alert":"hloc_calc","config_hash_id":"74b7ae27-c469-4178-939f-9d2d07296c22","hostname":"parity-parent","instance":"hloc.values","context":"hloc.ctx","component":null,"classification":null,"type":null,"when":1791644762,"info":"localhost s last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":6,"raised_duration":0},"notification":{"when":1791644762,"delay":0,"delay_up_to_time":1791644762,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":6,"returned":6,"max_to_return":200,"before":0,"after":0}}`,
		},
		flight: [2][2]int64{{1791644775, 1791644775}, {1791644775, 1791644775}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"states/gone": {
		target: "/api/v3/alert_transitions?transition=<gone>&options=minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"REMOVED","name":"REMOVED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_gone","name":"hst_gone","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.gone","name":"hst.gone","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.gctx","name":"hst.gctx","count":1}]}],"transitions":[{"gi":1791644621471409,"alert":"hst_gone","transition_id":"515d028a-0646-457a-a76c-356fa222f4e6","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"52ae24a6-c510-42a4-bef4-6ee3f4a6f8eb","hostname":"parity-parent","instance":"hst.gone","instance_n":"hst.gone","context":"hst.gctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart made obsolete","summary":"","units":"things","new":{"status":"REMOVED","value":0},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.257}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"REMOVED","name":"REMOVED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hst_gone","name":"hst_gone","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hst.gone","name":"hst.gone","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hst.gctx","name":"hst.gctx","count":1}]}],"transitions":[{"gi":1791644621425598,"alert":"hst_gone","transition_id":"3cb99235-55e8-431d-b359-bc1693a92787","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"52ae24a6-c510-42a4-bef4-6ee3f4a6f8eb","hostname":"parity-parent","instance":"hst.gone","instance_n":"hst.gone","context":"hst.gctx","component":null,"classification":null,"type":null,"when":1791644621,"info":"on a chart made obsolete","summary":"","units":"things","new":{"status":"REMOVED","value":0},"old":{"status":"UNINITIALIZED","value":0,"duration":4,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791644621,"flags":["PROCESSED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.151}}`,
		},
		flight: [2][2]int64{{1791644636, 1791644636}, {1791644636, 1791644636}},
		logs:   [2]string{"L01", "L02"},
		more:   [2]string{"", ""},
	},
	"two-hosts/transition-child-debug": {
		target: "/api/v3/alert_transitions?transition=<transition-child-debug>&options=minify,debug",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":1,
                "alert":null,
                "transition":"6958a836-a18e-4c6b-8085-7c8b5f08660b"
            }
        },
        "filters":{
            "after":0,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":null,
            "f_instance":null,
            "f_context":null
        }
    },
    "facets":[{
            "id":"f_status",
            "name":"Alert Status",
            "order":1,
            "options":[{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":1
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":1
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000c1",
                    "name":"health-child",
                    "count":1
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hch_calc",
                    "name":"hch_calc",
                    "count":1
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hchild.values",
                    "name":"hchild.values",
                    "count":1
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hchild.ctx",
                    "name":"hchild.ctx",
                    "count":1
                }]
        }],
    "transitions":[{
            "gi":1791644772665082,
            "alert":"hch_calc",
            "transition_id":"6958a836-a18e-4c6b-8085-7c8b5f08660b",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000c1",
            "config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c",
            "hostname":"health-child",
            "instance":"hchild.values",
            "instance_n":"hchild.values",
            "context":"hchild.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791644772,
            "info":"the child s last value of a",
            "summary":"",
            "units":"things",
            "new":{
                "status":"WARNING",
                "value":70
            },
            "old":{
                "status":"CLEAR",
                "value":10,
                "duration":5,
                "raised_duration":0
            },
            "notification":{
                "when":0,
                "delay":0,
                "delay_up_to_time":1791644772,
                "flags":["PROCESSED","SILENCED","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":1,
        "matched":1,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":0
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.168
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":1,
                "alert":null,
                "transition":"4f1768b6-e353-43c7-86da-59d02d237221"
            }
        },
        "filters":{
            "after":0,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":null,
            "f_instance":null,
            "f_context":null
        }
    },
    "facets":[{
            "id":"f_status",
            "name":"Alert Status",
            "order":1,
            "options":[{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":1
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":1
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000c1",
                    "name":"health-child",
                    "count":1
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hch_calc",
                    "name":"hch_calc",
                    "count":1
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hchild.values",
                    "name":"hchild.values",
                    "count":1
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hchild.ctx",
                    "name":"hchild.ctx",
                    "count":1
                }]
        }],
    "transitions":[{
            "gi":1791644772486128,
            "alert":"hch_calc",
            "transition_id":"4f1768b6-e353-43c7-86da-59d02d237221",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000c1",
            "config_hash_id":"c7437f24-7140-4a8e-bf1f-ff52ef52214c",
            "hostname":"health-child",
            "instance":"hchild.values",
            "instance_n":"hchild.values",
            "context":"hchild.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791644772,
            "info":"the child s last value of a",
            "summary":"",
            "units":"things",
            "new":{
                "status":"WARNING",
                "value":70
            },
            "old":{
                "status":"CLEAR",
                "value":10,
                "duration":5,
                "raised_duration":0
            },
            "notification":{
                "when":0,
                "delay":0,
                "delay_up_to_time":1791644772,
                "flags":["PROCESSED","SILENCED","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":1,
        "matched":1,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":0
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.196
    }
}
`,
		},
		flight: [2][2]int64{{1791644775, 1791644775}, {1791644775, 1791644775}},
		logs:   [2]string{"L05", "L06"},
		more:   [2]string{"L07", "L08"},
	},
	"transitions/alerts-critical": {
		target: "/api/v3/alerts?options=summary,instances,values,minify&status=critical",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644678166707,"nm":"hs_calc","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the last value of a","sum":"","units":"things","tr_i":"02beef4c-d982-4046-ad22-0bbda80ebcc7","tr_v":95,"tr_t":1791644678,"cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":95,"t":1791644686},{"ati":1,"ni":0,"gi":1791644679188430,"nm":"hs_max","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the maximum of a over 3 seconds","sum":"","units":"things","tr_i":"7bc29f89-b733-42bc-985f-ae7effe6fc24","tr_v":95,"tr_t":1791644679,"cfg":"f090fc72-965e-4e84-a2e4-99a07b484c62","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":95,"t":1791644686},{"ati":2,"ni":0,"gi":1791644686208141,"nm":"hs_avg","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the average of a over 5 aligned seconds","sum":"","units":"things","tr_i":"f07c2f5d-6047-4cd2-80ad-072788f722f3","tr_v":95,"tr_t":1791644686,"cfg":"674523c2-cd53-486b-b06c-8abd3d0a83d1","src":"line=20,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":95,"t":1791644686}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.417}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644678956137,"nm":"hs_calc","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the last value of a","sum":"","units":"things","tr_i":"d92cad9d-8361-4d54-9c25-62ae2d394b28","tr_v":95,"tr_t":1791644678,"cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":95,"t":1791644686},{"ati":1,"ni":0,"gi":1791644679981705,"nm":"hs_max","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the maximum of a over 3 seconds","sum":"","units":"things","tr_i":"114fb247-141f-4235-8c8e-238fba0fbe45","tr_v":95,"tr_t":1791644679,"cfg":"f090fc72-965e-4e84-a2e4-99a07b484c62","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":95,"t":1791644686},{"ati":2,"ni":0,"gi":1791644686006305,"nm":"hs_avg","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the average of a over 5 aligned seconds","sum":"","units":"things","tr_i":"4d89b465-8a9f-4e87-89c2-1373d9765abf","tr_v":95,"tr_t":1791644686,"cfg":"674523c2-cd53-486b-b06c-8abd3d0a83d1","src":"line=20,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":"","v":95,"t":1791644686}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.153}}`,
		},
		flight: [2][2]int64{{1791644686, 1791644686}, {1791644686, 1791644686}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
	"transitions/alerts-warning": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=warning",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.021}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3}],"alerts_by_module":[],"alert_instances":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
		},
		flight: [2][2]int64{{1791644686, 1791644686}, {1791644686, 1791644686}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
	"transitions/alerts-raised": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=raised",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644678166707,"nm":"hs_calc","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the last value of a","sum":"","units":"things","tr_i":"02beef4c-d982-4046-ad22-0bbda80ebcc7","tr_v":95,"tr_t":1791644678,"cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791644679188430,"nm":"hs_max","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the maximum of a over 3 seconds","sum":"","units":"things","tr_i":"7bc29f89-b733-42bc-985f-ae7effe6fc24","tr_v":95,"tr_t":1791644679,"cfg":"f090fc72-965e-4e84-a2e4-99a07b484c62","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791644686208141,"nm":"hs_avg","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the average of a over 5 aligned seconds","sum":"","units":"things","tr_i":"f07c2f5d-6047-4cd2-80ad-072788f722f3","tr_v":95,"tr_t":1791644686,"cfg":"674523c2-cd53-486b-b06c-8abd3d0a83d1","src":"line=20,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.097}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644678956137,"nm":"hs_calc","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the last value of a","sum":"","units":"things","tr_i":"d92cad9d-8361-4d54-9c25-62ae2d394b28","tr_v":95,"tr_t":1791644678,"cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791644679981705,"nm":"hs_max","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the maximum of a over 3 seconds","sum":"","units":"things","tr_i":"114fb247-141f-4235-8c8e-238fba0fbe45","tr_v":95,"tr_t":1791644679,"cfg":"f090fc72-965e-4e84-a2e4-99a07b484c62","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791644686006305,"nm":"hs_avg","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the average of a over 5 aligned seconds","sum":"","units":"things","tr_i":"4d89b465-8a9f-4e87-89c2-1373d9765abf","tr_v":95,"tr_t":1791644686,"cfg":"674523c2-cd53-486b-b06c-8abd3d0a83d1","src":"line=20,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.075}}`,
		},
		flight: [2][2]int64{{1791644686, 1791644686}, {1791644686, 1791644686}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
	"transitions/alerts-summary": {
		target: "/api/v3/alerts?options=summary,minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.053}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.076}}`,
		},
		flight: [2][2]int64{{1791644686, 1791644686}, {1791644686, 1791644686}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
	"transitions/alerts-warning-window": {
		target: "/api/v3/alerts?options=summary,minify&status=warning&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.04}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":0,"wr":0,"cl":0,"er":0,"running":0,"running_silent":0,"available":3}],"alerts_by_module":[],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.022}}`,
		},
		flight: [2][2]int64{{1791644686, 1791644686}, {1791644686, 1791644686}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
	"transitions/alerts-critical-window": {
		target: "/api/v3/alerts?options=summary,instances,minify&status=critical&after=-600",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644678166707,"nm":"hs_calc","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the last value of a","sum":"","units":"things","tr_i":"02beef4c-d982-4046-ad22-0bbda80ebcc7","tr_v":95,"tr_t":1791644678,"cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791644679188430,"nm":"hs_max","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the maximum of a over 3 seconds","sum":"","units":"things","tr_i":"7bc29f89-b733-42bc-985f-ae7effe6fc24","tr_v":95,"tr_t":1791644679,"cfg":"f090fc72-965e-4e84-a2e4-99a07b484c62","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791644686208141,"nm":"hs_avg","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the average of a over 5 aligned seconds","sum":"","units":"things","tr_i":"f07c2f5d-6047-4cd2-80ad-072788f722f3","tr_v":95,"tr_t":1791644686,"cfg":"674523c2-cd53-486b-b06c-8abd3d0a83d1","src":"line=20,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.082}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0}],"alerts":[{"ati":0,"ni":[0],"nm":"hs_calc","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":1,"ni":[0],"nm":"hs_max","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]},{"ati":2,"ni":[0],"nm":"hs_avg","sum":"","cr":1,"wr":0,"cl":0,"er":0,"in":1,"nd":1,"cfg":1,"ctx":["hsig.ctx"],"cls":[],"cp":[],"ty":[],"to":["root"]}],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"alerts_by_recipient":[{"name":"root","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0,"available":3}],"alerts_by_module":[{"name":"[none]","cr":3,"wr":0,"cl":0,"er":0,"running":3,"running_silent":0}],"alert_instances":[{"ati":0,"ni":0,"gi":1791644678956137,"nm":"hs_calc","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the last value of a","sum":"","units":"things","tr_i":"d92cad9d-8361-4d54-9c25-62ae2d394b28","tr_v":95,"tr_t":1791644678,"cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","src":"line=2,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":1,"ni":0,"gi":1791644679981705,"nm":"hs_max","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the maximum of a over 3 seconds","sum":"","units":"things","tr_i":"114fb247-141f-4235-8c8e-238fba0fbe45","tr_v":95,"tr_t":1791644679,"cfg":"f090fc72-965e-4e84-a2e4-99a07b484c62","src":"line=11,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""},{"ati":2,"ni":0,"gi":1791644686006305,"nm":"hs_avg","ctx":"hsig.ctx","ch":"hsig.values","ch_n":"hsig.values","st":"CRITICAL","fami":"family","info":"the average of a over 5 aligned seconds","sum":"","units":"things","tr_i":"4d89b465-8a9f-4e87-89c2-1373d9765abf","tr_v":95,"tr_t":1791644686,"cfg":"674523c2-cd53-486b-b06c-8abd3d0a83d1","src":"line=20,file=<run>/etc/health.d/parity.conf","to":"root","tp":"","cm":"","cl":""}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.08}}`,
		},
		flight: [2][2]int64{{1791644686, 1791644686}, {1791644686, 1791644686}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
	"transitions/echo-alerts": {
		target: "/api/v3/alerts?transition=<the side's own id>&options=summary,instances,values,minify,debug",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["minify","debug","instances","values","summary"],
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
                "transition":"4634b8cf-f215-427f-883e-54b5e54cb509"
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
            "nm":"hs_calc",
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
    "alerts_by_type":[],
    "alerts_by_component":[],
    "alerts_by_classification":[],
    "alerts_by_recipient":[{
            "name":"root",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "running":1,
            "running_silent":0,
            "available":3
        }],
    "alerts_by_module":[{
            "name":"[none]",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "running":1,
            "running_silent":0
        }],
    "alert_instances":[{
            "ati":0,
            "ni":0,
            "gi":1791644693230863,
            "nm":"hs_calc",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"CLEAR",
            "fami":"family",
            "info":"the last value of a",
            "sum":"",
            "units":"things",
            "tr_i":"98c945e6-6230-479b-b57e-2a79314ce760",
            "tr_v":10,
            "tr_t":1791644693,
            "cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "src":"line=2,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":10,
            "t":1791644699
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.129
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alerts"],
        "options":["minify","debug","instances","values","summary"],
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
                "transition":"b6d4a11d-e3a4-4ebd-97d8-baa0719d6126"
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
            "nm":"hs_calc",
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
    "alerts_by_type":[],
    "alerts_by_component":[],
    "alerts_by_classification":[],
    "alerts_by_recipient":[{
            "name":"root",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "running":1,
            "running_silent":0,
            "available":3
        }],
    "alerts_by_module":[{
            "name":"[none]",
            "cr":0,
            "wr":0,
            "cl":1,
            "er":0,
            "running":1,
            "running_silent":0
        }],
    "alert_instances":[{
            "ati":0,
            "ni":0,
            "gi":1791644693043227,
            "nm":"hs_calc",
            "ctx":"hsig.ctx",
            "ch":"hsig.values",
            "ch_n":"hsig.values",
            "st":"CLEAR",
            "fami":"family",
            "info":"the last value of a",
            "sum":"",
            "units":"things",
            "tr_i":"25bf326b-1e0f-4c9e-ae82-7e523ce97789",
            "tr_v":10,
            "tr_t":1791644693,
            "cfg":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "src":"line=2,file=<run>/etc/health.d/parity.conf",
            "to":"root",
            "tp":"",
            "cm":"",
            "cl":"",
            "v":10,
            "t":1791644699
        }],
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.085
    }
}
`,
		},
		flight: [2][2]int64{{1791644699, 1791644699}, {1791644699, 1791644699}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
	"transitions/echo-transitions": {
		target: "/api/v3/alert_transitions?transition=<echo-transitions>&options=minify,debug",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":1,
                "alert":null,
                "transition":"4634b8cf-f215-427f-883e-54b5e54cb509"
            }
        },
        "filters":{
            "after":0,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":null,
            "f_instance":null,
            "f_context":null
        }
    },
    "facets":[{
            "id":"f_status",
            "name":"Alert Status",
            "order":1,
            "options":[{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":1
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":1
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":1
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hs_calc",
                    "name":"hs_calc",
                    "count":1
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hsig.values",
                    "name":"hsig.values",
                    "count":1
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hsig.ctx",
                    "name":"hsig.ctx",
                    "count":1
                }]
        }],
    "transitions":[{
            "gi":1791644663033777,
            "alert":"hs_calc",
            "transition_id":"4634b8cf-f215-427f-883e-54b5e54cb509",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791644663,
            "info":"the last value of a",
            "summary":"",
            "units":"things",
            "new":{
                "status":"WARNING",
                "value":70
            },
            "old":{
                "status":"CLEAR",
                "value":10,
                "duration":13,
                "raised_duration":0
            },
            "notification":{
                "when":1791644663,
                "delay":0,
                "delay_up_to_time":1791644663,
                "flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":1,
        "matched":1,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":0
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.151
    }
}
`,
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":1,
                "alert":null,
                "transition":"b6d4a11d-e3a4-4ebd-97d8-baa0719d6126"
            }
        },
        "filters":{
            "after":0,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":null,
            "f_instance":null,
            "f_context":null
        }
    },
    "facets":[{
            "id":"f_status",
            "name":"Alert Status",
            "order":1,
            "options":[{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":1
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":1
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":1
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":1
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hs_calc",
                    "name":"hs_calc",
                    "count":1
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hsig.values",
                    "name":"hsig.values",
                    "count":1
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hsig.ctx",
                    "name":"hsig.ctx",
                    "count":1
                }]
        }],
    "transitions":[{
            "gi":1791644663851802,
            "alert":"hs_calc",
            "transition_id":"b6d4a11d-e3a4-4ebd-97d8-baa0719d6126",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791644663,
            "info":"the last value of a",
            "summary":"",
            "units":"things",
            "new":{
                "status":"WARNING",
                "value":70
            },
            "old":{
                "status":"CLEAR",
                "value":10,
                "duration":14,
                "raised_duration":0
            },
            "notification":{
                "when":1791644663,
                "delay":0,
                "delay_up_to_time":1791644663,
                "flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":1,
        "matched":1,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":0
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.257
    }
}
`,
		},
		flight: [2][2]int64{{1791644699, 1791644699}, {1791644699, 1791644699}},
		logs:   [2]string{"L09", "L10"},
		more:   [2]string{"", ""},
	},
}

// dashNormClosersLLogs are the recorded alert logs by name.
var dashNormClosersLLogs = map[string]string{
	"L01": `[{"unique_id":1791644651,"alarm_id":1791644623,"alarm_event_id":6,"name":"hst_fail","transition_id":"ec502839-9787-44f9-8931-3ebba578b61a","when":1791644633,"duration":12,"non_clear_duration":12,"exec_run":1791644633,"delay_up_to_timestamp":1791644633,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644650,"alarm_id":1791644622,"alarm_event_id":6,"name":"hst_delay","transition_id":"395c34db-b59e-40e9-8f37-f3d86014983a","when":1791644633,"duration":12,"non_clear_duration":12,"exec_run":1791644633,"delay_up_to_timestamp":1791644633,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644649,"alarm_id":1791644623,"alarm_event_id":5,"name":"hst_fail","transition_id":"ddee3f31-a073-45cd-845b-ec26f1e7c980","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":1791644621,"delay_up_to_timestamp":1791644621,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644648,"alarm_id":1791644622,"alarm_event_id":5,"name":"hst_delay","transition_id":"43f5eaeb-353f-4990-b3d3-9b2d65aa7848","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":1791644631,"delay_up_to_timestamp":1791644631,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644647,"alarm_id":1791644618,"alarm_event_id":4,"name":"hst_named","transition_id":"e2f3a325-64f2-4795-b5bd-e4692199bb13","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":1791644621,"delay_up_to_timestamp":1791644621,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791644646,"alarm_id":1791644619,"alarm_event_id":4,"name":"hst_gone","transition_id":"515d028a-0646-457a-a76c-356fa222f4e6","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644621,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644645,"alarm_id":1791644625,"alarm_event_id":4,"name":"hst_undef","transition_id":"35c17fae-7a55-473d-969d-6b12b31c4e11","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNDEFINED","old_status":"UNINITIALIZED"},{"unique_id":1791644644,"alarm_id":1791644624,"alarm_event_id":4,"name":"hst_nan","transition_id":"61135a5a-7e8e-4bc6-9cb8-42dee9294c00","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNDEFINED","old_status":"UNINITIALIZED"},{"unique_id":1791644643,"alarm_id":1791644623,"alarm_event_id":4,"name":"hst_fail","transition_id":"f23b2469-de41-420e-8f39-b915b0239fab","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644642,"alarm_id":1791644622,"alarm_event_id":4,"name":"hst_delay","transition_id":"89d65081-f5fb-4b89-a8b9-a6947426911a","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644627,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644641,"alarm_id":1791644625,"alarm_event_id":3,"name":"hst_undef","transition_id":"4bc69cd5-9e25-4d3d-921f-5340a00f76e8","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644640,"alarm_id":1791644624,"alarm_event_id":3,"name":"hst_nan","transition_id":"01b7234d-976c-44a1-978a-1651c702c2a0","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644639,"alarm_id":1791644623,"alarm_event_id":3,"name":"hst_fail","transition_id":"e611ce1d-e8a2-4a28-9fbd-5dbf641c533a","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644638,"alarm_id":1791644622,"alarm_event_id":3,"name":"hst_delay","transition_id":"54e4ea0d-1f96-4f59-a590-fd8ee7222c7d","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644637,"alarm_id":1791644625,"alarm_event_id":2,"name":"hst_undef","transition_id":"8b51747a-d3ea-40b6-ad9a-694cf06ef8d2","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644636,"alarm_id":1791644624,"alarm_event_id":2,"name":"hst_nan","transition_id":"b4852ee0-b378-428f-b53b-0a85cebc8e76","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644635,"alarm_id":1791644623,"alarm_event_id":2,"name":"hst_fail","transition_id":"5e76c645-f004-4f8c-ad4f-1c99d6871a12","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644634,"alarm_id":1791644622,"alarm_event_id":2,"name":"hst_delay","transition_id":"fdfd9e1f-ae8b-46d4-bf1e-06649186785b","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644633,"alarm_id":1791644621,"alarm_event_id":3,"name":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","transition_id":"82968c52-13a6-4748-ae69-34c3930ef299","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644632,"alarm_id":1791644620,"alarm_event_id":3,"name":"hst_idle","transition_id":"f40997b7-f92b-4537-a50b-5ee7d8ff3e12","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644631,"alarm_id":1791644621,"alarm_event_id":2,"name":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","transition_id":"d353f36a-346d-4a29-acad-ae27143d7b50","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644630,"alarm_id":1791644620,"alarm_event_id":2,"name":"hst_idle","transition_id":"b741e698-4979-4a8f-bdf8-ff0c6a5281fc","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644629,"alarm_id":1791644619,"alarm_event_id":3,"name":"hst_gone","transition_id":"48061717-25b1-4e10-93d8-a6ddfa3b9b5b","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644628,"alarm_id":1791644619,"alarm_event_id":2,"name":"hst_gone","transition_id":"0d696dfd-a1be-4573-b249-ca3bb167820e","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644627,"alarm_id":1791644618,"alarm_event_id":3,"name":"hst_named","transition_id":"b07f04fe-3c27-416b-a5c7-fa13958b291a","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644626,"alarm_id":1791644618,"alarm_event_id":2,"name":"hst_named","transition_id":"50825f58-ef35-480f-8970-aabac178aece","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644625,"alarm_id":1791644625,"alarm_event_id":1,"name":"hst_undef","transition_id":"05fab4c8-f47c-4cb4-b307-181da8f9b08a","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644624,"alarm_id":1791644624,"alarm_event_id":1,"name":"hst_nan","transition_id":"c0c7949d-0da8-4bdf-82a4-fbf17f703e52","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644623,"alarm_id":1791644623,"alarm_event_id":1,"name":"hst_fail","transition_id":"0536b55c-fbeb-4d20-be0d-a9bffe951747","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644622,"alarm_id":1791644622,"alarm_event_id":1,"name":"hst_delay","transition_id":"4c6bc0af-0ee1-4721-8f8e-23919c6411cd","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644621,"alarm_id":1791644621,"alarm_event_id":1,"name":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","transition_id":"e6112bb5-7e66-4d1a-9c10-d6c42268efcc","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644620,"alarm_id":1791644620,"alarm_event_id":1,"name":"hst_idle","transition_id":"a1bb36f7-a6be-495b-b3e5-af1c85239d75","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644619,"alarm_id":1791644619,"alarm_event_id":1,"name":"hst_gone","transition_id":"f240bd85-5a34-49df-9c4d-ad05f72e49ae","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644618,"alarm_id":1791644618,"alarm_event_id":1,"name":"hst_named","transition_id":"1151140d-7a43-4963-b6ad-39573d3fd4ae","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L02": `[{"unique_id":1791644651,"alarm_id":1791644623,"alarm_event_id":6,"name":"hst_fail","transition_id":"b12dcf7f-c7e4-4bb3-8d72-7018da1ba90e","when":1791644633,"duration":12,"non_clear_duration":12,"exec_run":1791644633,"delay_up_to_timestamp":1791644633,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644650,"alarm_id":1791644622,"alarm_event_id":6,"name":"hst_delay","transition_id":"95390ff5-f42a-4c47-9b13-01e1c8e9eb57","when":1791644633,"duration":12,"non_clear_duration":12,"exec_run":1791644633,"delay_up_to_timestamp":1791644633,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644649,"alarm_id":1791644623,"alarm_event_id":5,"name":"hst_fail","transition_id":"9dc72e96-efe9-4849-a137-226f57d2139c","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":1791644621,"delay_up_to_timestamp":1791644621,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644648,"alarm_id":1791644622,"alarm_event_id":5,"name":"hst_delay","transition_id":"32cde91b-36a8-4bf2-9da6-139af0b9b8fb","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":1791644631,"delay_up_to_timestamp":1791644631,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644647,"alarm_id":1791644618,"alarm_event_id":4,"name":"hst_named","transition_id":"ff99a8a1-6463-4fc8-9e7e-9c1623b8778f","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":1791644621,"delay_up_to_timestamp":1791644621,"status":"WARNING","old_status":"UNINITIALIZED"},{"unique_id":1791644646,"alarm_id":1791644619,"alarm_event_id":4,"name":"hst_gone","transition_id":"3cb99235-55e8-431d-b359-bc1693a92787","when":1791644621,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644621,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644645,"alarm_id":1791644625,"alarm_event_id":4,"name":"hst_undef","transition_id":"04e51ed3-0874-44f3-b6a8-132b88e778f8","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNDEFINED","old_status":"UNINITIALIZED"},{"unique_id":1791644644,"alarm_id":1791644624,"alarm_event_id":4,"name":"hst_nan","transition_id":"8f8d2a08-1c2e-47f1-bf9a-5860c1b2ad3c","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNDEFINED","old_status":"UNINITIALIZED"},{"unique_id":1791644643,"alarm_id":1791644623,"alarm_event_id":4,"name":"hst_fail","transition_id":"1bb08432-7a19-4cf3-a4aa-d7e887566406","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644642,"alarm_id":1791644622,"alarm_event_id":4,"name":"hst_delay","transition_id":"1395dffd-88aa-45a2-8592-b0d0ee5e5ea9","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644627,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644641,"alarm_id":1791644625,"alarm_event_id":3,"name":"hst_undef","transition_id":"359e1b34-2e06-435c-b8f5-5118b1528a7c","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644640,"alarm_id":1791644624,"alarm_event_id":3,"name":"hst_nan","transition_id":"fe10abda-29a6-4745-8494-0ef9670b3d1a","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644639,"alarm_id":1791644623,"alarm_event_id":3,"name":"hst_fail","transition_id":"5a29d472-4819-4e95-9246-a9edac5e3063","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644638,"alarm_id":1791644622,"alarm_event_id":3,"name":"hst_delay","transition_id":"fac87c80-e00f-4903-8b94-2936ca2e1b82","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644637,"alarm_id":1791644625,"alarm_event_id":2,"name":"hst_undef","transition_id":"b4fa860a-197d-470c-8f57-00ccd9a3f497","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644636,"alarm_id":1791644624,"alarm_event_id":2,"name":"hst_nan","transition_id":"866b8812-4a84-4ec0-939b-acf82215e14d","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644635,"alarm_id":1791644623,"alarm_event_id":2,"name":"hst_fail","transition_id":"1ba2e0e9-15e2-492e-afa0-83bdf617bf57","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644634,"alarm_id":1791644622,"alarm_event_id":2,"name":"hst_delay","transition_id":"0148338a-42fc-478a-b011-2feeedf89df1","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644633,"alarm_id":1791644621,"alarm_event_id":3,"name":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","transition_id":"1ce39840-3147-471c-b0cb-5c7a004635be","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644632,"alarm_id":1791644620,"alarm_event_id":3,"name":"hst_idle","transition_id":"c6dba1fd-d84a-4c3f-bd7c-0ce8d6d93720","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644631,"alarm_id":1791644621,"alarm_event_id":2,"name":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","transition_id":"849d3885-2f60-4f8f-ae83-36cfcb772af2","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644630,"alarm_id":1791644620,"alarm_event_id":2,"name":"hst_idle","transition_id":"7841088d-42be-4908-8e9f-1ab109427e8b","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644629,"alarm_id":1791644619,"alarm_event_id":3,"name":"hst_gone","transition_id":"9addce43-d579-446b-9385-b4ab57168fb1","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644628,"alarm_id":1791644619,"alarm_event_id":2,"name":"hst_gone","transition_id":"8cc8a7fa-ac7d-4939-baef-f1a9d3b7f5aa","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644627,"alarm_id":1791644618,"alarm_event_id":3,"name":"hst_named","transition_id":"2b1df1c6-c024-4eb1-8d05-cba601d74b06","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644626,"alarm_id":1791644618,"alarm_event_id":2,"name":"hst_named","transition_id":"e4bd66eb-035b-4a4d-8c5f-1f614825c9bb","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644625,"alarm_id":1791644625,"alarm_event_id":1,"name":"hst_undef","transition_id":"a232d3d3-cef7-4003-adfa-45bc4b7cd953","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644624,"alarm_id":1791644624,"alarm_event_id":1,"name":"hst_nan","transition_id":"7e35aeac-e684-461e-a3fd-df7d15ba5fb7","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644623,"alarm_id":1791644623,"alarm_event_id":1,"name":"hst_fail","transition_id":"4daf1208-134d-493a-9be0-a09e732953aa","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644622,"alarm_id":1791644622,"alarm_event_id":1,"name":"hst_delay","transition_id":"b5092b0c-e6b3-44f1-8f6d-3721ce9efeab","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644621,"alarm_id":1791644621,"alarm_event_id":1,"name":"hst_long_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn","transition_id":"bae495a2-0763-4142-8edf-6790b5507c17","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644620,"alarm_id":1791644620,"alarm_event_id":1,"name":"hst_idle","transition_id":"94568ed5-0c49-4e3a-a33f-b8ed30afa040","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644619,"alarm_id":1791644619,"alarm_event_id":1,"name":"hst_gone","transition_id":"dfe8c944-2a14-4e33-a0c9-f2978d7f2466","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644618,"alarm_id":1791644618,"alarm_event_id":1,"name":"hst_named","transition_id":"ec646821-5d84-4266-b9bd-1e7a34724a41","when":1791644617,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644617,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L03": `[{"unique_id":1791644735,"alarm_id":1791644728,"alarm_event_id":8,"name":"hloc_calc","transition_id":"1d3e3090-2ebc-494d-a05e-f378aef83bbf","when":1791644735,"duration":4,"non_clear_duration":0,"exec_run":1791644735,"delay_up_to_timestamp":1791644735,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644734,"alarm_id":1791644728,"alarm_event_id":7,"name":"hloc_calc","transition_id":"510abc34-5ea8-40e7-b318-c4cc41ef2f6d","when":1791644731,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644731,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644733,"alarm_id":1791644728,"alarm_event_id":6,"name":"hloc_calc","transition_id":"13fb8adc-8c48-4cdf-8d0e-92088011e349","when":1791644731,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644731,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644732,"alarm_id":1791644728,"alarm_event_id":5,"name":"hloc_calc","transition_id":"b2617fc9-7351-4211-b245-4f9f42bbb00f","when":1791644731,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644731,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791644731,"alarm_id":1791644728,"alarm_event_id":4,"name":"hloc_calc","transition_id":"009ba19e-f13e-488f-88e8-5346417a1843","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644730,"alarm_id":1791644728,"alarm_event_id":3,"name":"hloc_calc","transition_id":"f3f5897f-3f03-497c-a501-31e8579913da","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644729,"alarm_id":1791644728,"alarm_event_id":2,"name":"hloc_calc","transition_id":"ce7cbaf3-6bc9-490e-bd6c-86f588784cb9","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644728,"alarm_id":1791644728,"alarm_event_id":1,"name":"hloc_calc","transition_id":"8c848f00-c8ed-4c43-8bfd-ca0f3e2290a3","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L04": `[{"unique_id":1791644735,"alarm_id":1791644728,"alarm_event_id":8,"name":"hloc_calc","transition_id":"5213ac57-0cf5-4dd2-a60d-4f093be14e7a","when":1791644735,"duration":4,"non_clear_duration":0,"exec_run":1791644735,"delay_up_to_timestamp":1791644735,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644734,"alarm_id":1791644728,"alarm_event_id":7,"name":"hloc_calc","transition_id":"ba91fcd7-7ebd-4fce-91f0-53cb0d54928d","when":1791644731,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644731,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644733,"alarm_id":1791644728,"alarm_event_id":6,"name":"hloc_calc","transition_id":"e804cf6a-8954-4309-a78f-e1b761b38c4f","when":1791644731,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644731,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644732,"alarm_id":1791644728,"alarm_event_id":5,"name":"hloc_calc","transition_id":"c83d40da-e0a0-4ac3-a688-5106cf99d6fe","when":1791644731,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644731,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791644731,"alarm_id":1791644728,"alarm_event_id":4,"name":"hloc_calc","transition_id":"4f1b976c-cfdd-410d-ad0f-3a8f9ee6759a","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644730,"alarm_id":1791644728,"alarm_event_id":3,"name":"hloc_calc","transition_id":"638ac6cd-78e7-4679-9a5b-0009983406e6","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644729,"alarm_id":1791644728,"alarm_event_id":2,"name":"hloc_calc","transition_id":"a869658a-a3bc-4479-832e-fc89a571597d","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644728,"alarm_id":1791644728,"alarm_event_id":1,"name":"hloc_calc","transition_id":"f3426860-e32c-476b-9dfb-95e6a4396f30","when":1791644727,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644727,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L05": `[{"unique_id":1791644762,"alarm_id":1791644753,"alarm_event_id":10,"name":"hloc_calc","transition_id":"8a52723e-8c75-42c6-ae9d-6c0ae5a188b0","when":1791644772,"duration":5,"non_clear_duration":0,"exec_run":1791644772,"delay_up_to_timestamp":1791644772,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644761,"alarm_id":1791644753,"alarm_event_id":9,"name":"hloc_calc","transition_id":"d8ef399e-ebb5-456a-84f8-0d43b26465e6","when":1791644767,"duration":5,"non_clear_duration":5,"exec_run":1791644767,"delay_up_to_timestamp":1791644767,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644760,"alarm_id":1791644753,"alarm_event_id":8,"name":"hloc_calc","transition_id":"f155eb85-88b7-43e1-a639-39b7338ea72a","when":1791644762,"duration":6,"non_clear_duration":0,"exec_run":1791644762,"delay_up_to_timestamp":1791644762,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644759,"alarm_id":1791644753,"alarm_event_id":7,"name":"hloc_calc","transition_id":"452ebc97-505a-4d3d-a284-261d4486f9bd","when":1791644756,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644756,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644758,"alarm_id":1791644753,"alarm_event_id":6,"name":"hloc_calc","transition_id":"e1f282ee-d249-4aff-949a-d3ba1f5b9fbf","when":1791644756,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644756,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644757,"alarm_id":1791644753,"alarm_event_id":5,"name":"hloc_calc","transition_id":"90c24ec5-990f-49c9-a577-927a3ef6c70b","when":1791644756,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644756,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791644756,"alarm_id":1791644753,"alarm_event_id":4,"name":"hloc_calc","transition_id":"3bb8dddf-cd21-4bc8-9258-b6cb29e98665","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644755,"alarm_id":1791644753,"alarm_event_id":3,"name":"hloc_calc","transition_id":"038c2486-15fa-4227-8b19-1be59a4baeee","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644754,"alarm_id":1791644753,"alarm_event_id":2,"name":"hloc_calc","transition_id":"66dd4f03-a0ad-4cd0-a30c-bd97a757e214","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644753,"alarm_id":1791644753,"alarm_event_id":1,"name":"hloc_calc","transition_id":"ba71759a-bc67-46a8-8229-7cfb4a059007","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L06": `[{"unique_id":1791644762,"alarm_id":1791644753,"alarm_event_id":10,"name":"hloc_calc","transition_id":"a5be155b-29f6-471b-995d-8ab6b12293ff","when":1791644772,"duration":5,"non_clear_duration":0,"exec_run":1791644772,"delay_up_to_timestamp":1791644772,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644761,"alarm_id":1791644753,"alarm_event_id":9,"name":"hloc_calc","transition_id":"9cdb2132-5506-41cc-a61d-d577984cad75","when":1791644767,"duration":5,"non_clear_duration":5,"exec_run":1791644767,"delay_up_to_timestamp":1791644767,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644760,"alarm_id":1791644753,"alarm_event_id":8,"name":"hloc_calc","transition_id":"1e836cf1-ac97-4953-ac8c-2b221145b279","when":1791644762,"duration":6,"non_clear_duration":0,"exec_run":1791644762,"delay_up_to_timestamp":1791644762,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644759,"alarm_id":1791644753,"alarm_event_id":7,"name":"hloc_calc","transition_id":"74ebdddf-97d2-4aed-b661-1460b5977c42","when":1791644756,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644756,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644758,"alarm_id":1791644753,"alarm_event_id":6,"name":"hloc_calc","transition_id":"694c71de-b6e8-485c-a98a-893b7d4f8771","when":1791644756,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644756,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644757,"alarm_id":1791644753,"alarm_event_id":5,"name":"hloc_calc","transition_id":"89703944-296b-42eb-a4ee-2804e9246bfe","when":1791644756,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644756,"status":"REMOVED","old_status":"CLEAR"},{"unique_id":1791644756,"alarm_id":1791644753,"alarm_event_id":4,"name":"hloc_calc","transition_id":"cfbb80ff-ef62-4c75-aad0-28e6e73f1e03","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644755,"alarm_id":1791644753,"alarm_event_id":3,"name":"hloc_calc","transition_id":"ac4c84fb-e187-42fb-9cac-cec9ace8196d","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644754,"alarm_id":1791644753,"alarm_event_id":2,"name":"hloc_calc","transition_id":"852ac462-9fce-44f8-aa4d-d2685cebcb1a","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644753,"alarm_id":1791644753,"alarm_event_id":1,"name":"hloc_calc","transition_id":"603db31b-2ef9-4b0f-ad97-0511c4a27949","when":1791644752,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644752,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L07": `[{"unique_id":1791644764,"alarm_id":1791644760,"alarm_event_id":5,"name":"hch_calc","transition_id":"6958a836-a18e-4c6b-8085-7c8b5f08660b","when":1791644772,"duration":5,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644772,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644763,"alarm_id":1791644760,"alarm_event_id":4,"name":"hch_calc","transition_id":"7ea45887-8f3a-4881-b98d-46d09d93408c","when":1791644767,"duration":5,"non_clear_duration":5,"exec_run":0,"delay_up_to_timestamp":1791644767,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644762,"alarm_id":1791644760,"alarm_event_id":3,"name":"hch_calc","transition_id":"00c9ea18-d256-47f5-877e-0adafdc0ed7c","when":1791644762,"duration":2,"non_clear_duration":0,"exec_run":1791644762,"delay_up_to_timestamp":1791644762,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644761,"alarm_id":1791644760,"alarm_event_id":2,"name":"hch_calc","transition_id":"e4fe9c10-c067-4891-9a83-eacd498816c3","when":1791644760,"duration":1,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644760,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644760,"alarm_id":1791644760,"alarm_event_id":1,"name":"hch_calc","transition_id":"e691a6e5-d59b-4089-afd7-988477245e26","when":1791644759,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644759,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L08": `[{"unique_id":1791644764,"alarm_id":1791644760,"alarm_event_id":5,"name":"hch_calc","transition_id":"4f1768b6-e353-43c7-86da-59d02d237221","when":1791644772,"duration":5,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644772,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644763,"alarm_id":1791644760,"alarm_event_id":4,"name":"hch_calc","transition_id":"14c602f8-b09e-41a2-852d-58d25d23a501","when":1791644767,"duration":5,"non_clear_duration":5,"exec_run":0,"delay_up_to_timestamp":1791644767,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791644762,"alarm_id":1791644760,"alarm_event_id":3,"name":"hch_calc","transition_id":"9f5c35c4-61cd-487b-8d9c-f6479ead484c","when":1791644762,"duration":2,"non_clear_duration":0,"exec_run":1791644762,"delay_up_to_timestamp":1791644762,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644761,"alarm_id":1791644760,"alarm_event_id":2,"name":"hch_calc","transition_id":"26adda05-701f-4f94-9cfd-f26dec7134f1","when":1791644760,"duration":1,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644760,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644760,"alarm_id":1791644760,"alarm_event_id":1,"name":"hch_calc","transition_id":"a6da28d7-f27e-43f4-9843-9a4ea67050e7","when":1791644759,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644759,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L09": `[{"unique_id":1791644671,"alarm_id":1791644653,"alarm_event_id":7,"name":"hs_avg","transition_id":"f540113c-77de-43f9-8e58-b4f5c7ed3cef","when":1791644696,"duration":10,"non_clear_duration":25,"exec_run":1791644696,"delay_up_to_timestamp":1791644696,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791644670,"alarm_id":1791644652,"alarm_event_id":7,"name":"hs_max","transition_id":"1ffa89ef-440a-4ba8-ae22-58ae55b69a22","when":1791644696,"duration":17,"non_clear_duration":32,"exec_run":1791644696,"delay_up_to_timestamp":1791644696,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791644669,"alarm_id":1791644651,"alarm_event_id":7,"name":"hs_calc","transition_id":"98c945e6-6230-479b-b57e-2a79314ce760","when":1791644693,"duration":15,"non_clear_duration":30,"exec_run":1791644693,"delay_up_to_timestamp":1791644693,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791644668,"alarm_id":1791644653,"alarm_event_id":6,"name":"hs_avg","transition_id":"f07c2f5d-6047-4cd2-80ad-072788f722f3","when":1791644686,"duration":15,"non_clear_duration":15,"exec_run":1791644686,"delay_up_to_timestamp":1791644686,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791644667,"alarm_id":1791644652,"alarm_event_id":6,"name":"hs_max","transition_id":"7bc29f89-b733-42bc-985f-ae7effe6fc24","when":1791644679,"duration":15,"non_clear_duration":15,"exec_run":1791644679,"delay_up_to_timestamp":1791644679,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791644666,"alarm_id":1791644651,"alarm_event_id":6,"name":"hs_calc","transition_id":"02beef4c-d982-4046-ad22-0bbda80ebcc7","when":1791644678,"duration":15,"non_clear_duration":15,"exec_run":1791644678,"delay_up_to_timestamp":1791644678,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791644665,"alarm_id":1791644653,"alarm_event_id":5,"name":"hs_avg","transition_id":"1c7baaf6-2339-4f8b-8fcf-1e17f0f66cb5","when":1791644671,"duration":18,"non_clear_duration":0,"exec_run":1791644671,"delay_up_to_timestamp":1791644671,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644664,"alarm_id":1791644652,"alarm_event_id":5,"name":"hs_max","transition_id":"13eb7872-04f5-41bd-b495-f0414762635b","when":1791644664,"duration":13,"non_clear_duration":0,"exec_run":1791644664,"delay_up_to_timestamp":1791644664,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644663,"alarm_id":1791644651,"alarm_event_id":5,"name":"hs_calc","transition_id":"4634b8cf-f215-427f-883e-54b5e54cb509","when":1791644663,"duration":13,"non_clear_duration":0,"exec_run":1791644663,"delay_up_to_timestamp":1791644663,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644662,"alarm_id":1791644653,"alarm_event_id":4,"name":"hs_avg","transition_id":"f0e35865-ff42-4f57-a482-6d8769bf763d","when":1791644653,"duration":3,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644653,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644661,"alarm_id":1791644652,"alarm_event_id":4,"name":"hs_max","transition_id":"f8557b97-465e-4f06-ab04-2c6c57c88d96","when":1791644651,"duration":1,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644651,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644660,"alarm_id":1791644651,"alarm_event_id":4,"name":"hs_calc","transition_id":"5b5de0c8-04ba-4f20-82f0-2baac4d6f2f7","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644659,"alarm_id":1791644653,"alarm_event_id":3,"name":"hs_avg","transition_id":"45021ae4-d116-47ee-a000-900c1a54f316","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644658,"alarm_id":1791644652,"alarm_event_id":3,"name":"hs_max","transition_id":"abd90ff0-49ee-479a-97b9-28d707b54bac","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644657,"alarm_id":1791644651,"alarm_event_id":3,"name":"hs_calc","transition_id":"98213118-0f99-421a-a0f2-6ae91c6cfb97","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644656,"alarm_id":1791644653,"alarm_event_id":2,"name":"hs_avg","transition_id":"f37d2977-897f-45d4-abd7-9c29fa4a95e5","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644655,"alarm_id":1791644652,"alarm_event_id":2,"name":"hs_max","transition_id":"5f9ed47f-17f4-47dc-a0da-c5bcadece848","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644654,"alarm_id":1791644651,"alarm_event_id":2,"name":"hs_calc","transition_id":"e9c1ad92-df82-450c-afc2-6faaa87daaa0","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644653,"alarm_id":1791644653,"alarm_event_id":1,"name":"hs_avg","transition_id":"39a66492-515b-4ba1-9cea-5d1bf5342a64","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644652,"alarm_id":1791644652,"alarm_event_id":1,"name":"hs_max","transition_id":"609d81e5-9201-4d0c-bb74-225a16e066ab","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644651,"alarm_id":1791644651,"alarm_event_id":1,"name":"hs_calc","transition_id":"5c0821f5-46c0-4eea-9cdb-43ceb0bf84f1","when":1791644650,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644650,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L10": `[{"unique_id":1791644670,"alarm_id":1791644652,"alarm_event_id":7,"name":"hs_avg","transition_id":"ca0be008-171d-4fac-bb03-f79fd1973210","when":1791644696,"duration":10,"non_clear_duration":25,"exec_run":1791644696,"delay_up_to_timestamp":1791644696,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791644669,"alarm_id":1791644651,"alarm_event_id":7,"name":"hs_max","transition_id":"5809f86a-fbe9-4c25-9a5d-6a5144826a10","when":1791644696,"duration":17,"non_clear_duration":32,"exec_run":1791644696,"delay_up_to_timestamp":1791644696,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791644668,"alarm_id":1791644650,"alarm_event_id":7,"name":"hs_calc","transition_id":"25bf326b-1e0f-4c9e-ae82-7e523ce97789","when":1791644693,"duration":15,"non_clear_duration":30,"exec_run":1791644693,"delay_up_to_timestamp":1791644693,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791644667,"alarm_id":1791644652,"alarm_event_id":6,"name":"hs_avg","transition_id":"4d89b465-8a9f-4e87-89c2-1373d9765abf","when":1791644686,"duration":15,"non_clear_duration":15,"exec_run":1791644686,"delay_up_to_timestamp":1791644686,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791644666,"alarm_id":1791644651,"alarm_event_id":6,"name":"hs_max","transition_id":"114fb247-141f-4235-8c8e-238fba0fbe45","when":1791644679,"duration":15,"non_clear_duration":15,"exec_run":1791644679,"delay_up_to_timestamp":1791644679,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791644665,"alarm_id":1791644650,"alarm_event_id":6,"name":"hs_calc","transition_id":"d92cad9d-8361-4d54-9c25-62ae2d394b28","when":1791644678,"duration":15,"non_clear_duration":15,"exec_run":1791644678,"delay_up_to_timestamp":1791644678,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791644664,"alarm_id":1791644652,"alarm_event_id":5,"name":"hs_avg","transition_id":"7b6a4edd-e32c-4c0a-afca-ad1e5a5d67db","when":1791644671,"duration":18,"non_clear_duration":0,"exec_run":1791644671,"delay_up_to_timestamp":1791644671,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644663,"alarm_id":1791644651,"alarm_event_id":5,"name":"hs_max","transition_id":"0401a4d6-18db-40dc-b021-a923ab38b070","when":1791644664,"duration":13,"non_clear_duration":0,"exec_run":1791644664,"delay_up_to_timestamp":1791644664,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644662,"alarm_id":1791644650,"alarm_event_id":5,"name":"hs_calc","transition_id":"b6d4a11d-e3a4-4ebd-97d8-baa0719d6126","when":1791644663,"duration":14,"non_clear_duration":0,"exec_run":1791644663,"delay_up_to_timestamp":1791644663,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791644661,"alarm_id":1791644652,"alarm_event_id":4,"name":"hs_avg","transition_id":"d7ca2bc0-2ee8-43f3-a57b-83b40a603111","when":1791644653,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644653,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644660,"alarm_id":1791644651,"alarm_event_id":4,"name":"hs_max","transition_id":"2236a836-af98-400a-b28d-d0f594f4f7a2","when":1791644651,"duration":2,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644651,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644659,"alarm_id":1791644650,"alarm_event_id":4,"name":"hs_calc","transition_id":"58b47e64-7bff-4eca-ad68-c44fc60a1bfb","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791644658,"alarm_id":1791644652,"alarm_event_id":3,"name":"hs_avg","transition_id":"985f5383-51db-482b-b5f7-24cc1798cd3c","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644657,"alarm_id":1791644651,"alarm_event_id":3,"name":"hs_max","transition_id":"24e450c8-8e35-454b-9130-f26888df084d","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644656,"alarm_id":1791644650,"alarm_event_id":3,"name":"hs_calc","transition_id":"8d73f5f8-ea81-4fe9-984d-58e61ca16bc7","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644655,"alarm_id":1791644652,"alarm_event_id":2,"name":"hs_avg","transition_id":"edcfe462-85da-4185-941b-c3b4b20b4809","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644654,"alarm_id":1791644651,"alarm_event_id":2,"name":"hs_max","transition_id":"b84b62d3-6056-4703-a7c5-6fee953d8eec","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644653,"alarm_id":1791644650,"alarm_event_id":2,"name":"hs_calc","transition_id":"cfd95f3b-6cb1-4874-8896-08f1fb35810d","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791644652,"alarm_id":1791644652,"alarm_event_id":1,"name":"hs_avg","transition_id":"b5e82d15-3c0b-4853-881a-4020b6648a09","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644651,"alarm_id":1791644651,"alarm_event_id":1,"name":"hs_max","transition_id":"ff92dd58-1b8e-4e29-87d4-498874001954","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791644650,"alarm_id":1791644650,"alarm_event_id":1,"name":"hs_calc","transition_id":"46cbffcb-41de-444d-ba0f-99a591015b51","when":1791644649,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791644649,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
}
