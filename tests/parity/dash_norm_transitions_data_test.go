// SPDX-License-Identifier: GPL-3.0-or-later

package parity

// The transitions rows of D234 F7 as one C-against-C run answered them (0 the oracle): each row's bodies with the
// side's run directory written `<run>` as v2Round writes it, the seconds each request was in flight, and the
// names of the alert logs the family read for it (dashNormTransitionsLogs: each side's `/api/v1/alarm_log`,
// trimmed to the members the render and the views read; one per case and side: the longest read, which holds the
// entries of the earlier ones); the candidate side's body only where the pins read the pair; and the answers that
// are no JSON whole. Generated from the hooked copy's dumps
// (the harness agent H37's run of 2026-10-08T18:03Z; its hand-back says how to make it again).

// dashNormTransitionsRow is one recorded row.
type dashNormTransitionsRow struct {
	// target is what the row asked: its v2Req.target, which names each side's own id where the row has one
	target string
	// same, when set, is the key of an earlier row of the case whose recorded answer this row's equalled but for
	// its timings: the bodies, the flights and the logs are that row's (dashNormTransitionsRowOf)
	same   string
	bodies [2]string
	flight [2][2]int64
	// logs are the names of each side's alert log
	logs [2]string
}

// dashNormTransitionsRows are the recorded rows by `<case>/<row>`.
var dashNormTransitionsRows = map[string]dashNormTransitionsRow{
	"rules/window": {
		target: "/api/v3/alert_transitions?after=-3600&last=200&options=minify",
		bodies: [2]string{
			"{\"api\":2,\"facets\":[{\"id\":\"f_status\",\"name\":\"Alert Status\",\"order\":1,\"options\":[{\"id\":\"CLEAR\",\"name\":\"CLEAR\",\"count\":3},{\"id\":\"WARNING\",\"name\":\"WARNING\",\"count\":3},{\"id\":\"CRITICAL\",\"name\":\"CRITICAL\",\"count\":1}]},{\"id\":\"f_class\",\"name\":\"Alert Class\",\"order\":4,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"count\":2},{\"id\":\"Errors\",\"name\":\"Errors\",\"count\":3}]},{\"id\":\"f_type\",\"name\":\"Alert Type\",\"order\":2,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Type Two\",\"name\":\"Type Two\",\"count\":2},{\"id\":\"Type One\",\"name\":\"Type One\",\"count\":3}]},{\"id\":\"f_component\",\"name\":\"Alert Component\",\"order\":5,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Part Two\",\"name\":\"Part Two\",\"count\":2},{\"id\":\"Part One\",\"name\":\"Part One\",\"count\":3}]},{\"id\":\"f_role\",\"name\":\"Recipient Role\",\"order\":3,\"options\":[{\"id\":\"root\",\"name\":\"root\",\"count\":2},{\"id\":\"silent\",\"name\":\"silent\",\"count\":2},{\"id\":\"sysadmin webmaster\",\"name\":\"sysadmin webmaster\",\"count\":3}]},{\"id\":\"f_node\",\"name\":\"Alert Node\",\"order\":6,\"options\":[{\"id\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"name\":\"parity-parent\",\"count\":7}]},{\"id\":\"f_alert\",\"name\":\"Alert Name\",\"order\":7,\"options\":[{\"id\":\"hr_plain\",\"name\":\"hr_plain\",\"count\":2},{\"id\":\"hr_two\",\"name\":\"hr_two\",\"count\":2},{\"id\":\"hr_tpl\",\"name\":\"hr_tpl\",\"count\":3}]},{\"id\":\"f_instance\",\"name\":\"Instance Name\",\"order\":8,\"options\":[{\"id\":\"hrul.values\",\"name\":\"hrul.values\",\"count\":7}]},{\"id\":\"f_context\",\"name\":\"Context\",\"order\":9,\"options\":[{\"id\":\"hrul.ctx\",\"name\":\"hrul.ctx\",\"count\":7}]}],\"transitions\":[{\"gi\":1791482494539129,\"alert\":\"hr_plain\",\"transition_id\":\"77468b3b-1800-49a7-a167-a911053f635f\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482494,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}},{\"gi\":1791482494539056,\"alert\":\"hr_two\",\"transition_id\":\"c56d5593-0aaf-434e-9e76-4d2383728bbb\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part Two\",\"classification\":\"Latency_ccccccccccccccccccccccccccccccccccccccc\",\"type\":\"Type Two\",\"when\":1791482494,\"info\":\"two on values\",\"summary\":\"\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"silent\"}},{\"gi\":1791482494533140,\"alert\":\"hr_tpl\",\"transition_id\":\"b8a16b7b-e60e-4d77-acaa-bc9e1230608c\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part One\",\"classification\":\"Errors\",\"type\":\"Type One\",\"when\":1791482494,\"info\":\"the template of fam r\",\"summary\":\"tpl fam r\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"CRITICAL\",\"value\":70,\"duration\":5,\"raised_duration\":10},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"sysadmin webmaster\"}},{\"gi\":1791482489496570,\"alert\":\"hr_plain\",\"transition_id\":\"01b29328-6d4b-4c2a-ad3f-ce1cb6d23135\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482489,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"WARNING\",\"value\":70},\"old\":{\"status\":\"CLEAR\",\"value\":40,\"duration\":9,\"raised_duration\":0},\"notification\":{\"when\":1791482489,\"delay\":0,\"delay_up_to_time\":1791482489,\"flags\":[\"PROCESSED\",\"UPDATED\",\"EXEC_RUN\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}},{\"gi\":1791482489496447,\"alert\":\"hr_two\",\"transition_id\":\"32b5b2dc-e457-4850-847c-19b62d788719\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part Two\",\"classification\":\"Latency_ccccccccccccccccccccccccccccccccccccccc\",\"type\":\"Type Two\",\"when\":1791482489,\"info\":\"two on values\",\"summary\":\"\",\"units\":\"things\",\"new\":{\"status\":\"WARNING\",\"value\":70},\"old\":{\"status\":\"CLEAR\",\"value\":40,\"duration\":9,\"raised_duration\":0},\"notification\":{\"when\":1791482489,\"delay\":0,\"delay_up_to_time\":1791482489,\"flags\":[\"PROCESSED\",\"UPDATED\",\"EXEC_RUN\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"silent\"}},{\"gi\":1791482489488259,\"alert\":\"hr_tpl\",\"transition_id\":\"c1d680e5-18e0-4252-870f-22c218a4ea0e\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part One\",\"classification\":\"Errors\",\"type\":\"Type One\",\"when\":1791482489,\"info\":\"the template of fam r\",\"summary\":\"tpl fam r\",\"units\":\"things\",\"new\":{\"status\":\"CRITICAL\",\"value\":70},\"old\":{\"status\":\"WARNING\",\"value\":40,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":1791482489,\"delay\":0,\"delay_up_to_time\":1791482489,\"flags\":[\"PROCESSED\",\"UPDATED\",\"EXEC_RUN\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"sysadmin webmaster\"}},{\"gi\":1791482484468331,\"alert\":\"hr_tpl\",\"transition_id\":\"c257fa0f-2c92-4971-9392-2c20d865f204\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part One\",\"classification\":\"Errors\",\"type\":\"Type One\",\"when\":1791482484,\"info\":\"the template of fam r\",\"summary\":\"tpl fam r\",\"units\":\"things\",\"new\":{\"status\":\"WARNING\",\"value\":40},\"old\":{\"status\":\"CLEAR\",\"value\":10,\"duration\":4,\"raised_duration\":0},\"notification\":{\"when\":1791482484,\"delay\":0,\"delay_up_to_time\":1791482484,\"flags\":[\"PROCESSED\",\"UPDATED\",\"EXEC_RUN\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"sysadmin webmaster\"}}],\"items\":{\"evaluated\":7,\"matched\":7,\"returned\":7,\"max_to_return\":200,\"before\":0,\"after\":0},\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.712}}",
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/config": {
		target: "/api/v3/alert_transitions?after=-3600&last=3&options=minify,config",
		bodies: [2]string{
			"{\"api\":2,\"facets\":[{\"id\":\"f_status\",\"name\":\"Alert Status\",\"order\":1,\"options\":[{\"id\":\"CLEAR\",\"name\":\"CLEAR\",\"count\":3},{\"id\":\"WARNING\",\"name\":\"WARNING\",\"count\":3},{\"id\":\"CRITICAL\",\"name\":\"CRITICAL\",\"count\":1}]},{\"id\":\"f_class\",\"name\":\"Alert Class\",\"order\":4,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"count\":2},{\"id\":\"Errors\",\"name\":\"Errors\",\"count\":3}]},{\"id\":\"f_type\",\"name\":\"Alert Type\",\"order\":2,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Type Two\",\"name\":\"Type Two\",\"count\":2},{\"id\":\"Type One\",\"name\":\"Type One\",\"count\":3}]},{\"id\":\"f_component\",\"name\":\"Alert Component\",\"order\":5,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Part Two\",\"name\":\"Part Two\",\"count\":2},{\"id\":\"Part One\",\"name\":\"Part One\",\"count\":3}]},{\"id\":\"f_role\",\"name\":\"Recipient Role\",\"order\":3,\"options\":[{\"id\":\"root\",\"name\":\"root\",\"count\":2},{\"id\":\"silent\",\"name\":\"silent\",\"count\":2},{\"id\":\"sysadmin webmaster\",\"name\":\"sysadmin webmaster\",\"count\":3}]},{\"id\":\"f_node\",\"name\":\"Alert Node\",\"order\":6,\"options\":[{\"id\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"name\":\"parity-parent\",\"count\":7}]},{\"id\":\"f_alert\",\"name\":\"Alert Name\",\"order\":7,\"options\":[{\"id\":\"hr_plain\",\"name\":\"hr_plain\",\"count\":2},{\"id\":\"hr_two\",\"name\":\"hr_two\",\"count\":2},{\"id\":\"hr_tpl\",\"name\":\"hr_tpl\",\"count\":3}]},{\"id\":\"f_instance\",\"name\":\"Instance Name\",\"order\":8,\"options\":[{\"id\":\"hrul.values\",\"name\":\"hrul.values\",\"count\":7}]},{\"id\":\"f_context\",\"name\":\"Context\",\"order\":9,\"options\":[{\"id\":\"hrul.ctx\",\"name\":\"hrul.ctx\",\"count\":7}]}],\"transitions\":[{\"gi\":1791482494539129,\"alert\":\"hr_plain\",\"transition_id\":\"77468b3b-1800-49a7-a167-a911053f635f\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482494,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}},{\"gi\":1791482494539056,\"alert\":\"hr_two\",\"transition_id\":\"c56d5593-0aaf-434e-9e76-4d2383728bbb\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part Two\",\"classification\":\"Latency_ccccccccccccccccccccccccccccccccccccccc\",\"type\":\"Type Two\",\"when\":1791482494,\"info\":\"two on values\",\"summary\":\"\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"silent\"}},{\"gi\":1791482494533140,\"alert\":\"hr_tpl\",\"transition_id\":\"b8a16b7b-e60e-4d77-acaa-bc9e1230608c\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part One\",\"classification\":\"Errors\",\"type\":\"Type One\",\"when\":1791482494,\"info\":\"the template of fam r\",\"summary\":\"tpl fam r\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"CRITICAL\",\"value\":70,\"duration\":5,\"raised_duration\":10},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"sysadmin webmaster\"}}],\"configurations\":[{\"name\":\"hr_plain\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"selectors\":{\"type\":\"alarm\",\"on\":\"hrul.values\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":null,\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 50\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"root\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":\"no-clear-notification\"},\"class\":null,\"component\":null,\"type\":null,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\xa9zz\",\"summary\":null},{\"name\":\"hr_two\",\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"selectors\":{\"type\":\"alarm\",\"on\":\"hrul.values\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":\"things\",\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 50\",\"crit\":\"$this > 80\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"silent\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":null},\"class\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"component\":\"Part Two\",\"type\":\"Type Two\",\"info\":\"two on values\",\"summary\":null},{\"name\":null,\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"selectors\":{\"type\":\"template\",\"on\":\"hr_tpl\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":\"things\",\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 30\",\"crit\":\"$this > 60\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"sysadmin webmaster\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":null},\"class\":\"Errors\",\"component\":\"Part One\",\"type\":\"Type One\",\"info\":\"the template of ${family}\",\"summary\":\"tpl ${family}\"}],\"items\":{\"evaluated\":7,\"matched\":7,\"returned\":3,\"max_to_return\":3,\"before\":0,\"after\":4},\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.66}}",
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/config-debug": {
		target: "/api/v3/alert_transitions?after=-3600&last=1&alert=hr_two&options=minify,config,debug",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug","config"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":1,
                "alert":"hr_two",
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
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
                    "id":"CLEAR",
                    "name":"CLEAR",
                    "count":1
                },{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":1
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc",
                    "name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc",
                    "count":2
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"Type Two",
                    "name":"Type Two",
                    "count":2
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"Part Two",
                    "name":"Part Two",
                    "count":2
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"silent",
                    "name":"silent",
                    "count":2
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":2
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hr_two",
                    "name":"hr_two",
                    "count":2
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hrul.values",
                    "name":"hrul.values",
                    "count":2
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hrul.ctx",
                    "name":"hrul.ctx",
                    "count":2
                }]
        }],
    "transitions":[{
            "gi":1791482494539056,
            "alert":"hr_two",
            "transition_id":"c56d5593-0aaf-434e-9e76-4d2383728bbb",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"48ba6102-15cd-4941-a714-3e19471ea1c4",
            "hostname":"parity-parent",
            "instance":"hrul.values",
            "instance_n":"hrul.values",
            "context":"hrul.ctx",
            "component":"Part Two",
            "classification":"Latency_ccccccccccccccccccccccccccccccccccccccc",
            "type":"Type Two",
            "when":1791482494,
            "info":"two on values",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":10
            },
            "old":{
                "status":"WARNING",
                "value":70,
                "duration":5,
                "raised_duration":5
            },
            "notification":{
                "when":1791482494,
                "delay":0,
                "delay_up_to_time":1791482494,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"silent"
            }
        }],
    "configurations":[{
            "name":"hr_two",
            "config_hash_id":"48ba6102-15cd-4941-a714-3e19471ea1c4",
            "selectors":{
                "type":"alarm",
                "on":"hrul.values",
                "families":null,
                "host_labels":null,
                "chart_labels":null
            },
            "value":{
                "units":"things",
                "update_every":1,
                "db":{
                    "after":0,
                    "before":0,
                    "time_group_condition":"=",
                    "time_group_value":0,
                    "dims_group":"sum",
                    "data_source":"samples",
                    "method":null,
                    "dimensions":null,
                    "options":[]
                },
                "calc":"$a"
            },
            "status":{
                "green":null,
                "red":null,
                "warn":"$this > 50",
                "crit":"$this > 80"
            },
            "notification":{
                "type":"agent",
                "exec":null,
                "to":"silent",
                "delay":"multiplier 1.0 ",
                "repeat":null,
                "options":null
            },
            "class":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc",
            "component":"Part Two",
            "type":"Type Two",
            "info":"two on values",
            "summary":null
        }],
    "items":{
        "evaluated":2,
        "matched":2,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":1
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":1
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.743
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/config-mcp": {
		target: "/api/v3/alert_transitions?after=-3600&last=3&options=minify,config,mcp",
		bodies: [2]string{
			"{\"transitions\":[{\"gi\":1791482494539129,\"alert\":\"hr_plain\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482494,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}},{\"gi\":1791482494539056,\"alert\":\"hr_two\",\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part Two\",\"classification\":\"Latency_ccccccccccccccccccccccccccccccccccccccc\",\"type\":\"Type Two\",\"when\":1791482494,\"info\":\"two on values\",\"summary\":\"\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"silent\"}},{\"gi\":1791482494533140,\"alert\":\"hr_tpl\",\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part One\",\"classification\":\"Errors\",\"type\":\"Type One\",\"when\":1791482494,\"info\":\"the template of fam r\",\"summary\":\"tpl fam r\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"CRITICAL\",\"value\":70,\"duration\":5,\"raised_duration\":10},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"sysadmin webmaster\"}}],\"configurations\":[{\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"selectors\":{\"type\":\"alarm\",\"on\":\"hrul.values\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":null,\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 50\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"root\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":\"no-clear-notification\"},\"class\":null,\"component\":null,\"type\":null,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\xa9zz\",\"summary\":null},{\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"selectors\":{\"type\":\"alarm\",\"on\":\"hrul.values\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":\"things\",\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 50\",\"crit\":\"$this > 80\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"silent\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":null},\"class\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"component\":\"Part Two\",\"type\":\"Type Two\",\"info\":\"two on values\",\"summary\":null},{\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"selectors\":{\"type\":\"template\",\"on\":\"hr_tpl\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":\"things\",\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 30\",\"crit\":\"$this > 60\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"sysadmin webmaster\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":null},\"class\":\"Errors\",\"component\":\"Part One\",\"type\":\"Type One\",\"info\":\"the template of ${family}\",\"summary\":\"tpl ${family}\"}],\"items\":{\"evaluated\":7,\"matched\":7,\"returned\":3,\"max_to_return\":3,\"before\":0,\"after\":4}}",
			"{\"transitions\":[{\"gi\":1791482494348731,\"alert\":\"hr_plain\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482494,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}},{\"gi\":1791482494348601,\"alert\":\"hr_two\",\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part Two\",\"classification\":\"Latency_ccccccccccccccccccccccccccccccccccccccc\",\"type\":\"Type Two\",\"when\":1791482494,\"info\":\"two on values\",\"summary\":\"\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"silent\"}},{\"gi\":1791482494342730,\"alert\":\"hr_tpl\",\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":\"Part One\",\"classification\":\"Errors\",\"type\":\"Type One\",\"when\":1791482494,\"info\":\"the template of fam r\",\"summary\":\"tpl fam r\",\"units\":\"things\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"CRITICAL\",\"value\":70,\"duration\":5,\"raised_duration\":10},\"notification\":{\"when\":1791482494,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"EXEC_RUN\",\"EXEC_IN_PROGRESS\",\"SAVED\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"sysadmin webmaster\"}}],\"configurations\":[{\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"selectors\":{\"type\":\"alarm\",\"on\":\"hrul.values\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":null,\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 50\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"root\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":\"no-clear-notification\"},\"class\":null,\"component\":null,\"type\":null,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\xa9zz\",\"summary\":null},{\"config_hash_id\":\"48ba6102-15cd-4941-a714-3e19471ea1c4\",\"selectors\":{\"type\":\"alarm\",\"on\":\"hrul.values\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":\"things\",\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 50\",\"crit\":\"$this > 80\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"silent\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":null},\"class\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"component\":\"Part Two\",\"type\":\"Type Two\",\"info\":\"two on values\",\"summary\":null},{\"config_hash_id\":\"6efe9f28-709f-40b0-9db7-ad7928b11b7a\",\"selectors\":{\"type\":\"template\",\"on\":\"hr_tpl\",\"families\":null,\"host_labels\":null,\"chart_labels\":null},\"value\":{\"units\":\"things\",\"update_every\":1,\"calc\":\"$a\"},\"status\":{\"warn\":\"$this > 30\",\"crit\":\"$this > 60\"},\"notification\":{\"type\":\"agent\",\"exec\":null,\"to\":\"sysadmin webmaster\",\"delay\":\"multiplier 1.0 \",\"repeat\":null,\"options\":null},\"class\":\"Errors\",\"component\":\"Part One\",\"type\":\"Type One\",\"info\":\"the template of ${family}\",\"summary\":\"tpl ${family}\"}],\"items\":{\"evaluated\":7,\"matched\":7,\"returned\":3,\"max_to_return\":3,\"before\":0,\"after\":4}}",
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-class": {
		target: "/api/v3/alert_transitions?after=-3600&last=200&options=minify&f_class=errors",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"WARNING","name":"WARNING","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":2},{"id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","count":2},{"id":"Errors","name":"Errors","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Type Two","name":"Type Two","count":0},{"id":"Type One","name":"Type One","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Part Two","name":"Part Two","count":0},{"id":"Part One","name":"Part One","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0},{"id":"silent","name":"silent","count":0},{"id":"sysadmin webmaster","name":"sysadmin webmaster","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_plain","name":"hr_plain","count":0},{"id":"hr_two","name":"hr_two","count":0},{"id":"hr_tpl","name":"hr_tpl","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.values","name":"hrul.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.ctx","name":"hrul.ctx","count":3}]}],"transitions":[{"gi":1791482494533140,"alert":"hr_tpl","transition_id":"b8a16b7b-e60e-4d77-acaa-bc9e1230608c","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"6efe9f28-709f-40b0-9db7-ad7928b11b7a","hostname":"parity-parent","instance":"hrul.values","instance_n":"hrul.values","context":"hrul.ctx","component":"Part One","classification":"Errors","type":"Type One","when":1791482494,"info":"the template of fam r","summary":"tpl fam r","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":70,"duration":5,"raised_duration":10},"notification":{"when":1791482494,"delay":0,"delay_up_to_time":1791482494,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"sysadmin webmaster"}},{"gi":1791482489488259,"alert":"hr_tpl","transition_id":"c1d680e5-18e0-4252-870f-22c218a4ea0e","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"6efe9f28-709f-40b0-9db7-ad7928b11b7a","hostname":"parity-parent","instance":"hrul.values","instance_n":"hrul.values","context":"hrul.ctx","component":"Part One","classification":"Errors","type":"Type One","when":1791482489,"info":"the template of fam r","summary":"tpl fam r","units":"things","new":{"status":"CRITICAL","value":70},"old":{"status":"WARNING","value":40,"duration":5,"raised_duration":5},"notification":{"when":1791482489,"delay":0,"delay_up_to_time":1791482489,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"sysadmin webmaster"}},{"gi":1791482484468331,"alert":"hr_tpl","transition_id":"c257fa0f-2c92-4971-9392-2c20d865f204","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"6efe9f28-709f-40b0-9db7-ad7928b11b7a","hostname":"parity-parent","instance":"hrul.values","instance_n":"hrul.values","context":"hrul.ctx","component":"Part One","classification":"Errors","type":"Type One","when":1791482484,"info":"the template of fam r","summary":"tpl fam r","units":"things","new":{"status":"WARNING","value":40},"old":{"status":"CLEAR","value":10,"duration":4,"raised_duration":0},"notification":{"when":1791482484,"delay":0,"delay_up_to_time":1791482484,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"sysadmin webmaster"}}],"items":{"evaluated":7,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.308}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-cross": {
		target: "/api/v3/alert_transitions?after=-3600&last=200&options=minify&f_class=Errors&f_type=Type%20Two",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"WARNING","name":"WARNING","count":0},{"id":"CRITICAL","name":"CRITICAL","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","count":2},{"id":"Errors","name":"Errors","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Type Two","name":"Type Two","count":0},{"id":"Type One","name":"Type One","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Part Two","name":"Part Two","count":0},{"id":"Part One","name":"Part One","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0},{"id":"silent","name":"silent","count":0},{"id":"sysadmin webmaster","name":"sysadmin webmaster","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_plain","name":"hr_plain","count":0},{"id":"hr_two","name":"hr_two","count":0},{"id":"hr_tpl","name":"hr_tpl","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.values","name":"hrul.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.ctx","name":"hrul.ctx","count":0}]}],"transitions":[],"items":{"evaluated":7,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.306}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-blank": {
		target: "/api/v3/alert_transitions?after=-3600&last=1&options=minify&f_type=Type+One&f_role=sysadmin%20webmaster",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"WARNING","name":"WARNING","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","count":0},{"id":"Errors","name":"Errors","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Type Two","name":"Type Two","count":0},{"id":"Type One","name":"Type One","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Part Two","name":"Part Two","count":0},{"id":"Part One","name":"Part One","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0},{"id":"silent","name":"silent","count":0},{"id":"sysadmin webmaster","name":"sysadmin webmaster","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_plain","name":"hr_plain","count":0},{"id":"hr_two","name":"hr_two","count":0},{"id":"hr_tpl","name":"hr_tpl","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.values","name":"hrul.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.ctx","name":"hrul.ctx","count":3}]}],"transitions":[{"gi":1791482494533140,"alert":"hr_tpl","transition_id":"b8a16b7b-e60e-4d77-acaa-bc9e1230608c","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"6efe9f28-709f-40b0-9db7-ad7928b11b7a","hostname":"parity-parent","instance":"hrul.values","instance_n":"hrul.values","context":"hrul.ctx","component":"Part One","classification":"Errors","type":"Type One","when":1791482494,"info":"the template of fam r","summary":"tpl fam r","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":70,"duration":5,"raised_duration":10},"notification":{"when":1791482494,"delay":0,"delay_up_to_time":1791482494,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"sysadmin webmaster"}}],"items":{"evaluated":7,"matched":3,"returned":1,"max_to_return":1,"before":0,"after":2},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.297}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-word": {
		target: "/api/v3/alert_transitions?after=-3600&last=200&options=minify&f_role=sysadmin",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"WARNING","name":"WARNING","count":0},{"id":"CRITICAL","name":"CRITICAL","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","count":0},{"id":"Errors","name":"Errors","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Type Two","name":"Type Two","count":0},{"id":"Type One","name":"Type One","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Part Two","name":"Part Two","count":0},{"id":"Part One","name":"Part One","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":2},{"id":"silent","name":"silent","count":2},{"id":"sysadmin webmaster","name":"sysadmin webmaster","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_plain","name":"hr_plain","count":0},{"id":"hr_two","name":"hr_two","count":0},{"id":"hr_tpl","name":"hr_tpl","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.values","name":"hrul.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.ctx","name":"hrul.ctx","count":0}]}],"transitions":[],"items":{"evaluated":7,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.41}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-unknown": {
		target: "/api/v3/alert_transitions?after=-3600&last=1&options=minify&f_class=unknown",
		bodies: [2]string{
			"{\"api\":2,\"facets\":[{\"id\":\"f_status\",\"name\":\"Alert Status\",\"order\":1,\"options\":[{\"id\":\"CLEAR\",\"name\":\"CLEAR\",\"count\":1},{\"id\":\"WARNING\",\"name\":\"WARNING\",\"count\":1},{\"id\":\"CRITICAL\",\"name\":\"CRITICAL\",\"count\":0}]},{\"id\":\"f_class\",\"name\":\"Alert Class\",\"order\":4,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"count\":2},{\"id\":\"Errors\",\"name\":\"Errors\",\"count\":3}]},{\"id\":\"f_type\",\"name\":\"Alert Type\",\"order\":2,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Type Two\",\"name\":\"Type Two\",\"count\":0},{\"id\":\"Type One\",\"name\":\"Type One\",\"count\":0}]},{\"id\":\"f_component\",\"name\":\"Alert Component\",\"order\":5,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Part Two\",\"name\":\"Part Two\",\"count\":0},{\"id\":\"Part One\",\"name\":\"Part One\",\"count\":0}]},{\"id\":\"f_role\",\"name\":\"Recipient Role\",\"order\":3,\"options\":[{\"id\":\"root\",\"name\":\"root\",\"count\":2},{\"id\":\"silent\",\"name\":\"silent\",\"count\":0},{\"id\":\"sysadmin webmaster\",\"name\":\"sysadmin webmaster\",\"count\":0}]},{\"id\":\"f_node\",\"name\":\"Alert Node\",\"order\":6,\"options\":[{\"id\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"name\":\"parity-parent\",\"count\":2}]},{\"id\":\"f_alert\",\"name\":\"Alert Name\",\"order\":7,\"options\":[{\"id\":\"hr_plain\",\"name\":\"hr_plain\",\"count\":2},{\"id\":\"hr_two\",\"name\":\"hr_two\",\"count\":0},{\"id\":\"hr_tpl\",\"name\":\"hr_tpl\",\"count\":0}]},{\"id\":\"f_instance\",\"name\":\"Instance Name\",\"order\":8,\"options\":[{\"id\":\"hrul.values\",\"name\":\"hrul.values\",\"count\":2}]},{\"id\":\"f_context\",\"name\":\"Context\",\"order\":9,\"options\":[{\"id\":\"hrul.ctx\",\"name\":\"hrul.ctx\",\"count\":2}]}],\"transitions\":[{\"gi\":1791482494539129,\"alert\":\"hr_plain\",\"transition_id\":\"77468b3b-1800-49a7-a167-a911053f635f\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482494,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}}],\"items\":{\"evaluated\":7,\"matched\":2,\"returned\":1,\"max_to_return\":1,\"before\":0,\"after\":1},\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.327}}",
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-negative": {
		target: "/api/v3/alert_transitions?after=-3600&last=1&options=minify&f_class=!Errors%7C*",
		bodies: [2]string{
			"{\"api\":2,\"facets\":[{\"id\":\"f_status\",\"name\":\"Alert Status\",\"order\":1,\"options\":[{\"id\":\"CLEAR\",\"name\":\"CLEAR\",\"count\":2},{\"id\":\"WARNING\",\"name\":\"WARNING\",\"count\":2},{\"id\":\"CRITICAL\",\"name\":\"CRITICAL\",\"count\":0}]},{\"id\":\"f_class\",\"name\":\"Alert Class\",\"order\":4,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"count\":2},{\"id\":\"Errors\",\"name\":\"Errors\",\"count\":3}]},{\"id\":\"f_type\",\"name\":\"Alert Type\",\"order\":2,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Type Two\",\"name\":\"Type Two\",\"count\":2},{\"id\":\"Type One\",\"name\":\"Type One\",\"count\":0}]},{\"id\":\"f_component\",\"name\":\"Alert Component\",\"order\":5,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Part Two\",\"name\":\"Part Two\",\"count\":2},{\"id\":\"Part One\",\"name\":\"Part One\",\"count\":0}]},{\"id\":\"f_role\",\"name\":\"Recipient Role\",\"order\":3,\"options\":[{\"id\":\"root\",\"name\":\"root\",\"count\":2},{\"id\":\"silent\",\"name\":\"silent\",\"count\":2},{\"id\":\"sysadmin webmaster\",\"name\":\"sysadmin webmaster\",\"count\":0}]},{\"id\":\"f_node\",\"name\":\"Alert Node\",\"order\":6,\"options\":[{\"id\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"name\":\"parity-parent\",\"count\":4}]},{\"id\":\"f_alert\",\"name\":\"Alert Name\",\"order\":7,\"options\":[{\"id\":\"hr_plain\",\"name\":\"hr_plain\",\"count\":2},{\"id\":\"hr_two\",\"name\":\"hr_two\",\"count\":2},{\"id\":\"hr_tpl\",\"name\":\"hr_tpl\",\"count\":0}]},{\"id\":\"f_instance\",\"name\":\"Instance Name\",\"order\":8,\"options\":[{\"id\":\"hrul.values\",\"name\":\"hrul.values\",\"count\":4}]},{\"id\":\"f_context\",\"name\":\"Context\",\"order\":9,\"options\":[{\"id\":\"hrul.ctx\",\"name\":\"hrul.ctx\",\"count\":4}]}],\"transitions\":[{\"gi\":1791482494539129,\"alert\":\"hr_plain\",\"transition_id\":\"77468b3b-1800-49a7-a167-a911053f635f\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482494,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}}],\"items\":{\"evaluated\":7,\"matched\":4,\"returned\":1,\"max_to_return\":1,\"before\":0,\"after\":3},\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.319}}",
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-long": {
		target: "/api/v3/alert_transitions?after=-3600&last=1&options=minify&f_class=Latency_ccccccccccccccccccccccccccccccccccccccccccccccc",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"WARNING","name":"WARNING","count":1},{"id":"CRITICAL","name":"CRITICAL","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":2},{"id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","count":2},{"id":"Errors","name":"Errors","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Type Two","name":"Type Two","count":2},{"id":"Type One","name":"Type One","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Part Two","name":"Part Two","count":2},{"id":"Part One","name":"Part One","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0},{"id":"silent","name":"silent","count":2},{"id":"sysadmin webmaster","name":"sysadmin webmaster","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":2}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_plain","name":"hr_plain","count":0},{"id":"hr_two","name":"hr_two","count":2},{"id":"hr_tpl","name":"hr_tpl","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.values","name":"hrul.values","count":2}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.ctx","name":"hrul.ctx","count":2}]}],"transitions":[{"gi":1791482494539056,"alert":"hr_two","transition_id":"c56d5593-0aaf-434e-9e76-4d2383728bbb","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"48ba6102-15cd-4941-a714-3e19471ea1c4","hostname":"parity-parent","instance":"hrul.values","instance_n":"hrul.values","context":"hrul.ctx","component":"Part Two","classification":"Latency_ccccccccccccccccccccccccccccccccccccccc","type":"Type Two","when":1791482494,"info":"two on values","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":1791482494,"delay":0,"delay_up_to_time":1791482494,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"silent"}}],"items":{"evaluated":7,"matched":2,"returned":1,"max_to_return":1,"before":0,"after":1},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.309}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/facet-cut": {
		target: "/api/v3/alert_transitions?after=-3600&last=200&options=minify&f_class=Latency_ccccccccccccccccccccccccccccccccccccccc",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"WARNING","name":"WARNING","count":0},{"id":"CRITICAL","name":"CRITICAL","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":2},{"id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","count":2},{"id":"Errors","name":"Errors","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Type Two","name":"Type Two","count":0},{"id":"Type One","name":"Type One","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0},{"id":"Part Two","name":"Part Two","count":0},{"id":"Part One","name":"Part One","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0},{"id":"silent","name":"silent","count":0},{"id":"sysadmin webmaster","name":"sysadmin webmaster","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_plain","name":"hr_plain","count":0},{"id":"hr_two","name":"hr_two","count":0},{"id":"hr_tpl","name":"hr_tpl","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.values","name":"hrul.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.ctx","name":"hrul.ctx","count":0}]}],"transitions":[],"items":{"evaluated":7,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.275}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/context-other": {
		target: "/api/v3/alert_transitions?after=-3600&last=200&options=minify&context=hrul.idle",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[]},{"id":"f_class","name":"Alert Class","order":4,"options":[]},{"id":"f_type","name":"Alert Type","order":2,"options":[]},{"id":"f_component","name":"Alert Component","order":5,"options":[]},{"id":"f_role","name":"Recipient Role","order":3,"options":[]},{"id":"f_node","name":"Alert Node","order":6,"options":[]},{"id":"f_alert","name":"Alert Name","order":7,"options":[]},{"id":"f_instance","name":"Instance Name","order":8,"options":[]},{"id":"f_context","name":"Context","order":9,"options":[]}],"transitions":[],"items":{"evaluated":0,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.265}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/scope-contexts-other": {
		target: "/api/v3/alert_transitions?after=-3600&last=1&options=minify&scope_contexts=hrul.idle",
		bodies: [2]string{
			"{\"api\":2,\"facets\":[{\"id\":\"f_status\",\"name\":\"Alert Status\",\"order\":1,\"options\":[{\"id\":\"CLEAR\",\"name\":\"CLEAR\",\"count\":3},{\"id\":\"WARNING\",\"name\":\"WARNING\",\"count\":3},{\"id\":\"CRITICAL\",\"name\":\"CRITICAL\",\"count\":1}]},{\"id\":\"f_class\",\"name\":\"Alert Class\",\"order\":4,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc\",\"count\":2},{\"id\":\"Errors\",\"name\":\"Errors\",\"count\":3}]},{\"id\":\"f_type\",\"name\":\"Alert Type\",\"order\":2,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Type Two\",\"name\":\"Type Two\",\"count\":2},{\"id\":\"Type One\",\"name\":\"Type One\",\"count\":3}]},{\"id\":\"f_component\",\"name\":\"Alert Component\",\"order\":5,\"options\":[{\"id\":\"unknown\",\"name\":\"unknown\",\"count\":2},{\"id\":\"Part Two\",\"name\":\"Part Two\",\"count\":2},{\"id\":\"Part One\",\"name\":\"Part One\",\"count\":3}]},{\"id\":\"f_role\",\"name\":\"Recipient Role\",\"order\":3,\"options\":[{\"id\":\"root\",\"name\":\"root\",\"count\":2},{\"id\":\"silent\",\"name\":\"silent\",\"count\":2},{\"id\":\"sysadmin webmaster\",\"name\":\"sysadmin webmaster\",\"count\":3}]},{\"id\":\"f_node\",\"name\":\"Alert Node\",\"order\":6,\"options\":[{\"id\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"name\":\"parity-parent\",\"count\":7}]},{\"id\":\"f_alert\",\"name\":\"Alert Name\",\"order\":7,\"options\":[{\"id\":\"hr_plain\",\"name\":\"hr_plain\",\"count\":2},{\"id\":\"hr_two\",\"name\":\"hr_two\",\"count\":2},{\"id\":\"hr_tpl\",\"name\":\"hr_tpl\",\"count\":3}]},{\"id\":\"f_instance\",\"name\":\"Instance Name\",\"order\":8,\"options\":[{\"id\":\"hrul.values\",\"name\":\"hrul.values\",\"count\":7}]},{\"id\":\"f_context\",\"name\":\"Context\",\"order\":9,\"options\":[{\"id\":\"hrul.ctx\",\"name\":\"hrul.ctx\",\"count\":7}]}],\"transitions\":[{\"gi\":1791482494539129,\"alert\":\"hr_plain\",\"transition_id\":\"77468b3b-1800-49a7-a167-a911053f635f\",\"machine_guid\":\"5a1e0000-0000-4000-8000-0000000000aa\",\"config_hash_id\":\"f25a4617-151a-465a-b7e4-ad2b8a36e309\",\"hostname\":\"parity-parent\",\"instance\":\"hrul.values\",\"instance_n\":\"hrul.values\",\"context\":\"hrul.ctx\",\"component\":null,\"classification\":null,\"type\":null,\"when\":1791482494,\"info\":\"iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii\xc3\",\"summary\":\"\",\"units\":\"units\",\"new\":{\"status\":\"CLEAR\",\"value\":10},\"old\":{\"status\":\"WARNING\",\"value\":70,\"duration\":5,\"raised_duration\":5},\"notification\":{\"when\":0,\"delay\":0,\"delay_up_to_time\":1791482494,\"flags\":[\"PROCESSED\",\"SAVED\",\"NO_CLEAR_NOTIFICATION\"],\"exec\":\"<run>/notify/stub\",\"exec_code\":0,\"to\":\"root\"}}],\"items\":{\"evaluated\":7,\"matched\":7,\"returned\":1,\"max_to_return\":1,\"before\":0,\"after\":6},\"timings\":{\"routing_ms\":0,\"node_max_ms\":0,\"total_ms\":0.317}}",
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/alert-context": {
		target: "/api/v3/alert_transitions?after=-3600&last=200&options=minify&alert=hr_two&context=hrul.ctx",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","name":"Latency_ccccccccccccccccccccccccccccccccccccccccccccccc","count":2}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"Type Two","name":"Type Two","count":2}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"Part Two","name":"Part Two","count":2}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"silent","name":"silent","count":2}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":2}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_two","name":"hr_two","count":2}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.values","name":"hrul.values","count":2}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.ctx","name":"hrul.ctx","count":2}]}],"transitions":[{"gi":1791482494539056,"alert":"hr_two","transition_id":"c56d5593-0aaf-434e-9e76-4d2383728bbb","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"48ba6102-15cd-4941-a714-3e19471ea1c4","hostname":"parity-parent","instance":"hrul.values","instance_n":"hrul.values","context":"hrul.ctx","component":"Part Two","classification":"Latency_ccccccccccccccccccccccccccccccccccccccc","type":"Type Two","when":1791482494,"info":"two on values","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"WARNING","value":70,"duration":5,"raised_duration":5},"notification":{"when":1791482494,"delay":0,"delay_up_to_time":1791482494,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"silent"}},{"gi":1791482489496447,"alert":"hr_two","transition_id":"32b5b2dc-e457-4850-847c-19b62d788719","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"48ba6102-15cd-4941-a714-3e19471ea1c4","hostname":"parity-parent","instance":"hrul.values","instance_n":"hrul.values","context":"hrul.ctx","component":"Part Two","classification":"Latency_ccccccccccccccccccccccccccccccccccccccc","type":"Type Two","when":1791482489,"info":"two on values","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":40,"duration":9,"raised_duration":0},"notification":{"when":1791482489,"delay":0,"delay_up_to_time":1791482489,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"silent"}}],"items":{"evaluated":2,"matched":2,"returned":2,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.373}}`,
			``,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"rules/idle": {
		target: "/api/v3/alert_transitions?options=minify,config&transition=<hr_idle's last link>",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNINITIALIZED","name":"UNINITIALIZED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_idle","name":"hr_idle","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.plain","name":"hrul.plain","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.idle","name":"hrul.idle","count":1}]}],"transitions":[{"gi":1791482480422007,"alert":"hr_idle","transition_id":"bc104341-5b3f-4b39-98f8-2a08aa506949","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"3507064c-baec-42ec-98a3-553d645fb9b8","hostname":"parity-parent","instance":"hrul.plain","instance_n":"hrul.plain","context":"hrul.idle","component":null,"classification":null,"type":null,"when":1791482480,"info":"idle","summary":"","units":"idles","new":{"status":"UNINITIALIZED","value":0},"old":{"status":"REMOVED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791482480,"flags":["PROCESSED","RECURRING","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"configurations":[{"name":"hr_idle","config_hash_id":"3507064c-baec-42ec-98a3-553d645fb9b8","selectors":{"type":"alarm","on":"hrul.plain","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"idles","update_every":1,"calc":"$b"},"status":{"warn":"$this > 1000"},"notification":{"type":"agent","exec":null,"to":"root","delay":"up 60s down 120s multiplier 1.5 max 3600s","repeat":"warning 120s critical 30s","options":"no-clear-notification"},"class":null,"component":null,"type":null,"info":"idle","summary":null}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.326}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"UNINITIALIZED","name":"UNINITIALIZED","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hr_idle","name":"hr_idle","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hrul.plain","name":"hrul.plain","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hrul.idle","name":"hrul.idle","count":1}]}],"transitions":[{"gi":1791482481275785,"alert":"hr_idle","transition_id":"c5c5982a-ff10-4142-b39d-a753cb69a399","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"3507064c-baec-42ec-98a3-553d645fb9b8","hostname":"parity-parent","instance":"hrul.plain","instance_n":"hrul.plain","context":"hrul.idle","component":null,"classification":null,"type":null,"when":1791482481,"info":"idle","summary":"","units":"idles","new":{"status":"UNINITIALIZED","value":0},"old":{"status":"REMOVED","value":0,"duration":0,"raised_duration":0},"notification":{"when":0,"delay":0,"delay_up_to_time":1791482481,"flags":["PROCESSED","RECURRING","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"configurations":[{"name":"hr_idle","config_hash_id":"3507064c-baec-42ec-98a3-553d645fb9b8","selectors":{"type":"alarm","on":"hrul.plain","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"idles","update_every":1,"calc":"$b"},"status":{"warn":"$this > 1000"},"notification":{"type":"agent","exec":null,"to":"root","delay":"up 60s down 120s multiplier 1.5 max 3600s","repeat":"warning 120s critical 30s","options":"no-clear-notification"},"class":null,"component":null,"type":null,"info":"idle","summary":null}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.29}}`,
		},
		flight: [2][2]int64{{1791482497, 1791482497}, {1791482497, 1791482497}},
		logs:   [2]string{"L01", "L02"},
	},
	"transitions/facets": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_status=warning&f_alert=hs_calc%7Chs_max",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":2},{"id":"CRITICAL","name":"CRITICAL","count":2},{"id":"WARNING","name":"WARNING","count":2}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":2}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":2}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":2}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":2}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":2}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":1},{"id":"hs_max","name":"hs_max","count":1},{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":2}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":2}]}],"transitions":[{"gi":1791482549495618,"alert":"hs_max","transition_id":"ed13ea3a-d0ae-43fd-93b3-3c40eb031d0d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482549,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":13,"raised_duration":0},"notification":{"when":1791482549,"delay":0,"delay_up_to_time":1791482549,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482548470354,"alert":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":2,"returned":2,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.364}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":2},{"id":"CRITICAL","name":"CRITICAL","count":2},{"id":"WARNING","name":"WARNING","count":2}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":2}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":2}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":2}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":2}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":2}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":1},{"id":"hs_max","name":"hs_max","count":1},{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":2}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":2}]}],"transitions":[{"gi":1791482549288728,"alert":"hs_max","transition_id":"9f8530e3-19b8-4a79-a16d-7471340fe033","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482549,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":13,"raised_duration":0},"notification":{"when":1791482549,"delay":0,"delay_up_to_time":1791482549,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482548257223,"alert":"hs_calc","transition_id":"c96e70db-5812-4270-9da6-8e0d694f3bc4","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":13,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":2,"returned":2,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.281}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-select": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_alert=hs_calc",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482578873602,"alert":"hs_calc","transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482578,"info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":15,"raised_duration":30},"notification":{"when":1791482578,"delay":0,"delay_up_to_time":1791482578,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482563759497,"alert":"hs_calc","transition_id":"f582cc59-232d-4b28-99ba-d932d005c3de","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482563,"info":"the last value of a","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":70,"duration":15,"raised_duration":15},"notification":{"when":1791482563,"delay":0,"delay_up_to_time":1791482563,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482548470354,"alert":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.435}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-reject": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_status=nothing",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":0},{"id":"hs_max","name":"hs_max","count":0},{"id":"hs_calc","name":"hs_calc","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":0}]}],"transitions":[],"items":{"evaluated":9,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.376}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-negative-or": {
		target: "/api/v2/alert_transitions?after=-3600&last=2&options=minify&f_alert=!hs_calc%7C*",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":2},{"id":"CRITICAL","name":"CRITICAL","count":2},{"id":"WARNING","name":"WARNING","count":2}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":6}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":6}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":6}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":6}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482581900591,"alert":"hs_max","transition_id":"3e98bbba-770d-4057-86db-0cb8b202b413","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":17,"raised_duration":32},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":6,"returned":2,"max_to_return":2,"before":0,"after":4},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.382}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-negative": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_alert=!hs_calc",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"CRITICAL","name":"CRITICAL","count":0},{"id":"WARNING","name":"WARNING","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":0}]}],"transitions":[],"items":{"evaluated":9,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.26}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-wildcard": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&f_alert=hs_*c",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482578873602,"alert":"hs_calc","transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482578,"info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":15,"raised_duration":30},"notification":{"when":1791482578,"delay":0,"delay_up_to_time":1791482578,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":3,"returned":1,"max_to_return":1,"before":0,"after":2},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.278}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-star": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&f_alert=*",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":0,"after":8},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.274}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-wordless": {target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&f_alert=,%7C", same: "transitions/facet-star"},
	"transitions/facet-case": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_status=WARNING&f_alert=HS_CALC",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":1},{"id":"hs_max","name":"hs_max","count":1},{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":1791482548470354,"alert":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":1,"returned":1,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.257}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-blank": {target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_alert=hs_calc%20hs_max", same: "transitions/facet-negative"},
	"transitions/facet-repeat": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&f_alert=hs_calc&f_alert=hs_max",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482581900591,"alert":"hs_max","transition_id":"3e98bbba-770d-4057-86db-0cb8b202b413","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":17,"raised_duration":32},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":3,"returned":1,"max_to_return":1,"before":0,"after":2},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.267}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-two-reject": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_status=nothing&f_alert=nothing",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"CRITICAL","name":"CRITICAL","count":0},{"id":"WARNING","name":"WARNING","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":0},{"id":"hs_max","name":"hs_max","count":0},{"id":"hs_calc","name":"hs_calc","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":0}]}],"transitions":[],"items":{"evaluated":9,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.382}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-third": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_status=warning&f_alert=hs_calc&f_context=nothing",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"CRITICAL","name":"CRITICAL","count":0},{"id":"WARNING","name":"WARNING","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":0},{"id":"hs_max","name":"hs_max","count":0},{"id":"hs_calc","name":"hs_calc","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[],"items":{"evaluated":9,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.333}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-node": {target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&f_node=5A1E0000-0000-4000-8000-0000000000AA", same: "transitions/facet-star"},
	"transitions/facet-node-name": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&f_node=parity-parent",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":0},{"id":"CRITICAL","name":"CRITICAL","count":0},{"id":"WARNING","name":"WARNING","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":0},{"id":"hs_max","name":"hs_max","count":0},{"id":"hs_calc","name":"hs_calc","count":0}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":0}]}],"transitions":[],"items":{"evaluated":9,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.332}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/facet-values": {target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&f_instance=hsig.values&f_context=hsig.ctx&f_role=root&f_class=unknown&f_type=unknown&f_component=unknown", same: "transitions/facet-star"},
	"transitions/config": {
		target: "/api/v2/alert_transitions?after=-3600&last=4&options=minify,config",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482581900591,"alert":"hs_max","transition_id":"3e98bbba-770d-4057-86db-0cb8b202b413","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":17,"raised_duration":32},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482578873602,"alert":"hs_calc","transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482578,"info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":15,"raised_duration":30},"notification":{"when":1791482578,"delay":0,"delay_up_to_time":1791482578,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482571804750,"alert":"hs_avg","transition_id":"86adb823-e084-4817-91dc-a7f3f6c62902","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482571,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":85,"duration":15,"raised_duration":15},"notification":{"when":1791482571,"delay":0,"delay_up_to_time":1791482571,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"configurations":[{"name":"hs_avg","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-5,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"average","dimensions":"a","options":[]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the average of a over 5 aligned seconds","summary":null},{"name":"hs_max","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-3,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"max","dimensions":"a","options":["unaligned"]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the maximum of a over 3 seconds","summary":null},{"name":"hs_calc","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"calc":"$a"},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the last value of a","summary":null}],"items":{"evaluated":9,"matched":9,"returned":4,"max_to_return":4,"before":0,"after":5},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.537}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/config-last2": {
		target: "/api/v2/alert_transitions?after=-3600&last=2&options=minify,config",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482581900591,"alert":"hs_max","transition_id":"3e98bbba-770d-4057-86db-0cb8b202b413","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":17,"raised_duration":32},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"configurations":[{"name":"hs_avg","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-5,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"average","dimensions":"a","options":[]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the average of a over 5 aligned seconds","summary":null},{"name":"hs_max","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-3,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"max","dimensions":"a","options":["unaligned"]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the maximum of a over 3 seconds","summary":null}],"items":{"evaluated":9,"matched":9,"returned":2,"max_to_return":2,"before":0,"after":7},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.467}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/config-debug": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify,config,debug&alert=hs_calc",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug","config"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":1,
                "alert":"hs_calc",
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
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
                    "id":"CLEAR",
                    "name":"CLEAR",
                    "count":1
                },{
                    "id":"CRITICAL",
                    "name":"CRITICAL",
                    "count":1
                },{
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
                    "count":3
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":3
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":3
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":3
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":3
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hs_calc",
                    "name":"hs_calc",
                    "count":3
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hsig.values",
                    "name":"hsig.values",
                    "count":3
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hsig.ctx",
                    "name":"hsig.ctx",
                    "count":3
                }]
        }],
    "transitions":[{
            "gi":1791482578873602,
            "alert":"hs_calc",
            "transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482578,
            "info":"the last value of a",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":10
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":15,
                "raised_duration":30
            },
            "notification":{
                "when":1791482578,
                "delay":0,
                "delay_up_to_time":1791482578,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "configurations":[{
            "name":"hs_calc",
            "config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "selectors":{
                "type":"alarm",
                "on":"hsig.values",
                "families":null,
                "host_labels":null,
                "chart_labels":null
            },
            "value":{
                "units":"things",
                "update_every":1,
                "db":{
                    "after":0,
                    "before":0,
                    "time_group_condition":"=",
                    "time_group_value":0,
                    "dims_group":"sum",
                    "data_source":"samples",
                    "method":null,
                    "dimensions":null,
                    "options":[]
                },
                "calc":"$a"
            },
            "status":{
                "green":null,
                "red":null,
                "warn":"$this > 50",
                "crit":"$this > 90"
            },
            "notification":{
                "type":"agent",
                "exec":null,
                "to":"root",
                "delay":"multiplier 1.0 ",
                "repeat":null,
                "options":null
            },
            "class":null,
            "component":null,
            "type":null,
            "info":"the last value of a",
            "summary":null
        }],
    "items":{
        "evaluated":3,
        "matched":3,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":2
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":2
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.697
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/config-debug-rfc3339": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify,config,debug,rfc3339",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug","config","rfc3339"],
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
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
            "before":null
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
                    "id":"CLEAR",
                    "name":"CLEAR",
                    "count":3
                },{
                    "id":"CRITICAL",
                    "name":"CRITICAL",
                    "count":3
                },{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":3
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":9
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":9
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hs_avg",
                    "name":"hs_avg",
                    "count":3
                },{
                    "id":"hs_max",
                    "name":"hs_max",
                    "count":3
                },{
                    "id":"hs_calc",
                    "name":"hs_calc",
                    "count":3
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hsig.values",
                    "name":"hsig.values",
                    "count":9
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hsig.ctx",
                    "name":"hsig.ctx",
                    "count":9
                }]
        }],
    "transitions":[{
            "gi":1791482581900830,
            "alert":"hs_avg",
            "transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":"2026-10-08T18:03:01Z",
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":"2026-10-08T18:03:01Z",
                "delay":0,
                "delay_up_to_time":"2026-10-08T18:03:01Z",
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "configurations":[{
            "name":"hs_avg",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "selectors":{
                "type":"alarm",
                "on":"hsig.values",
                "families":null,
                "host_labels":null,
                "chart_labels":null
            },
            "value":{
                "units":"things",
                "update_every":1,
                "db":{
                    "after":-5,
                    "before":null,
                    "time_group_condition":"=",
                    "time_group_value":0,
                    "dims_group":"sum",
                    "data_source":"samples",
                    "method":"average",
                    "dimensions":"a",
                    "options":[]
                },
                "calc":null
            },
            "status":{
                "green":null,
                "red":null,
                "warn":"$this > 50",
                "crit":"$this > 90"
            },
            "notification":{
                "type":"agent",
                "exec":null,
                "to":"root",
                "delay":"multiplier 1.0 ",
                "repeat":null,
                "options":null
            },
            "class":null,
            "component":null,
            "type":null,
            "info":"the average of a over 5 aligned seconds",
            "summary":null
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":8
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":8
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.691
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/config-mcp": {
		target: "/api/v2/alert_transitions?after=-3600&last=3&options=minify,config,mcp",
		bodies: [2]string{
			`{"transitions":[{"gi":1791482581900830,"alert":"hs_avg","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482581900591,"alert":"hs_max","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":17,"raised_duration":32},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482578873602,"alert":"hs_calc","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482578,"info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":15,"raised_duration":30},"notification":{"when":1791482578,"delay":0,"delay_up_to_time":1791482578,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"configurations":[{"config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-5,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"average","dimensions":"a","options":[]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the average of a over 5 aligned seconds","summary":null},{"config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-3,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"max","dimensions":"a","options":["unaligned"]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the maximum of a over 3 seconds","summary":null},{"config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"calc":"$a"},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the last value of a","summary":null}],"items":{"evaluated":9,"matched":9,"returned":3,"max_to_return":3,"before":0,"after":6}}`,
			`{"transitions":[{"gi":1791482581670355,"alert":"hs_avg","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482581670185,"alert":"hs_max","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":17,"raised_duration":32},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482578611186,"alert":"hs_calc","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482578,"info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":15,"raised_duration":30},"notification":{"when":1791482578,"delay":0,"delay_up_to_time":1791482578,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"configurations":[{"config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-5,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"average","dimensions":"a","options":[]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the average of a over 5 aligned seconds","summary":null},{"config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"db":{"after":-3,"before":0,"time_group_condition":"=","time_group_value":0,"dims_group":"sum","data_source":"samples","method":"max","dimensions":"a","options":["unaligned"]}},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the maximum of a over 3 seconds","summary":null},{"config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"calc":"$a"},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the last value of a","summary":null}],"items":{"evaluated":9,"matched":9,"returned":3,"max_to_return":3,"before":0,"after":6}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/context": {
		target: "/api/v2/alert_transitions?after=-3600&last=2&options=minify&context=hsig.ctx",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482581900591,"alert":"hs_max","transition_id":"3e98bbba-770d-4057-86db-0cb8b202b413","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":17,"raised_duration":32},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":2,"max_to_return":2,"before":0,"after":7},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.424}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/alert": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&alert=hs_calc",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482578873602,"alert":"hs_calc","transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482578,"info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":15,"raised_duration":30},"notification":{"when":1791482578,"delay":0,"delay_up_to_time":1791482578,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482563759497,"alert":"hs_calc","transition_id":"f582cc59-232d-4b28-99ba-d932d005c3de","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482563,"info":"the last value of a","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":70,"duration":15,"raised_duration":15},"notification":{"when":1791482563,"delay":0,"delay_up_to_time":1791482563,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482548470354,"alert":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":3,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.286}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/alert-context": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&alert=hs_calc&context=hsig.ctx",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1},{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482578873602,"alert":"hs_calc","transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482578,"info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"CRITICAL","value":95,"duration":15,"raised_duration":30},"notification":{"when":1791482578,"delay":0,"delay_up_to_time":1791482578,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":3,"matched":3,"returned":1,"max_to_return":1,"before":0,"after":2},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.3}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/alert-pattern": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&alert=hs_*",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[]},{"id":"f_class","name":"Alert Class","order":4,"options":[]},{"id":"f_type","name":"Alert Type","order":2,"options":[]},{"id":"f_component","name":"Alert Component","order":5,"options":[]},{"id":"f_role","name":"Recipient Role","order":3,"options":[]},{"id":"f_node","name":"Alert Node","order":6,"options":[]},{"id":"f_alert","name":"Alert Name","order":7,"options":[]},{"id":"f_instance","name":"Instance Name","order":8,"options":[]},{"id":"f_context","name":"Context","order":9,"options":[]}],"transitions":[],"items":{"evaluated":0,"matched":0,"returned":0,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.355}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/alert-case":          {target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&alert=HS_CALC", same: "transitions/alert-pattern"},
	"transitions/context-pattern":     {target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&context=hsig.*", same: "transitions/alert-pattern"},
	"transitions/contexts-other":      {target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&contexts=other.ctx", same: "transitions/alert-pattern"},
	"transitions/context-later":       {target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&contexts=other.ctx&context=hsig.ctx", same: "transitions/facet-star"},
	"transitions/contexts-later":      {target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&context=hsig.ctx&contexts=other.ctx", same: "transitions/alert-pattern"},
	"transitions/scope-contexts-none": {target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&scope_contexts=nothing*", same: "transitions/alert-pattern"},
	"transitions/nodes-none":          {target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&nodes=nothing*", same: "transitions/alert-pattern"},
	"transitions/unread":              {target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&status=critical&cardinality=1", same: "transitions/facet-star"},
	"transitions/window-abs": {
		target: "/api/v2/alert_transitions?last=200&options=minify&after=1791482560&before=1791482575",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CRITICAL","name":"CRITICAL","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":1},{"id":"hs_max","name":"hs_max","count":1},{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482571804750,"alert":"hs_avg","transition_id":"86adb823-e084-4817-91dc-a7f3f6c62902","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482571,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":85,"duration":15,"raised_duration":15},"notification":{"when":1791482571,"delay":0,"delay_up_to_time":1791482571,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482564783809,"alert":"hs_max","transition_id":"098284ca-b490-4f1a-a06c-95e043eff5c2","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482564,"info":"the maximum of a over 3 seconds","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":70,"duration":15,"raised_duration":15},"notification":{"when":1791482564,"delay":0,"delay_up_to_time":1791482564,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}},{"gi":1791482563759497,"alert":"hs_calc","transition_id":"f582cc59-232d-4b28-99ba-d932d005c3de","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482563,"info":"the last value of a","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":70,"duration":15,"raised_duration":15},"notification":{"when":1791482563,"delay":0,"delay_up_to_time":1791482563,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":3,"matched":3,"returned":3,"max_to_return":200,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.433}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/window-after": {
		target: "/api/v2/alert_transitions?last=1&options=minify&after=1791482560",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":6}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":6}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":2},{"id":"hs_max","name":"hs_max","count":2},{"id":"hs_calc","name":"hs_calc","count":2}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":6}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":6}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":6,"returned":1,"max_to_return":1,"before":0,"after":5},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.292}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/window-before": {
		target: "/api/v2/alert_transitions?last=1&options=minify&before=1791482560",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":1},{"id":"hs_max","name":"hs_max","count":1},{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482556736124,"alert":"hs_avg","transition_id":"87213cbb-9c18-41ae-9971-91062bfe6bf3","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482556,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":46,"duration":18,"raised_duration":0},"notification":{"when":1791482556,"delay":0,"delay_up_to_time":1791482556,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":3,"matched":3,"returned":1,"max_to_return":1,"before":0,"after":2},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.275}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/window-flipped": {
		target: "/api/v2/alert_transitions?last=1&options=minify&after=1791482575&before=1791482560",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CRITICAL","name":"CRITICAL","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":3}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":3}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":3}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":1},{"id":"hs_max","name":"hs_max","count":1},{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":3}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":3}]}],"transitions":[{"gi":1791482571804750,"alert":"hs_avg","transition_id":"86adb823-e084-4817-91dc-a7f3f6c62902","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482571,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":85,"duration":15,"raised_duration":15},"notification":{"when":1791482571,"delay":0,"delay_up_to_time":1791482571,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":3,"matched":3,"returned":1,"max_to_return":1,"before":0,"after":2},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.264}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/no-window": {target: "/api/v2/alert_transitions?last=200&options=minify", same: "transitions/alert-pattern"},
	"transitions/no-last":   {target: "/api/v2/alert_transitions?after=-3600&options=minify", same: "transitions/facet-star"},
	"transitions/anchor-above": {
		target: "/api/v2/alert_transitions?after=-3600&last=200&options=minify&anchor_gi=9999999999999999",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[],"items":{"evaluated":9,"matched":9,"returned":0,"max_to_return":200,"before":9,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.333}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/anchor-last1": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify&anchor_gi=1791482563000000",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":3,"after":5},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.297}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/anchor-zero": {target: "/api/v2/alert_transitions?after=-3600&last=2&options=minify&anchor_gi=0", same: "transitions/context"},
	"transitions/dashboard":   {target: "/api/v2/alert_transitions?after=-3600&last=2&anchor_gi=&options=minify&scope_nodes=*", same: "transitions/context"},
	"transitions/debug": {
		target: "/api/v2/alert_transitions?after=-3600&last=4&options=debug&anchor_gi=1791482563000000",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":1791482563000000,
                "last":4,
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
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
                    "id":"CLEAR",
                    "name":"CLEAR",
                    "count":3
                },{
                    "id":"CRITICAL",
                    "name":"CRITICAL",
                    "count":3
                },{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":3
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":9
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":9
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hs_avg",
                    "name":"hs_avg",
                    "count":3
                },{
                    "id":"hs_max",
                    "name":"hs_max",
                    "count":3
                },{
                    "id":"hs_calc",
                    "name":"hs_calc",
                    "count":3
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hsig.values",
                    "name":"hsig.values",
                    "count":9
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hsig.ctx",
                    "name":"hsig.ctx",
                    "count":9
                }]
        }],
    "transitions":[{
            "gi":1791482581900830,
            "alert":"hs_avg",
            "transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        },{
            "gi":1791482581900591,
            "alert":"hs_max",
            "transition_id":"3e98bbba-770d-4057-86db-0cb8b202b413",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the maximum of a over 3 seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":10
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":17,
                "raised_duration":32
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        },{
            "gi":1791482578873602,
            "alert":"hs_calc",
            "transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482578,
            "info":"the last value of a",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":10
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":15,
                "raised_duration":30
            },
            "notification":{
                "when":1791482578,
                "delay":0,
                "delay_up_to_time":1791482578,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        },{
            "gi":1791482571804750,
            "alert":"hs_avg",
            "transition_id":"86adb823-e084-4817-91dc-a7f3f6c62902",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482571,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CRITICAL",
                "value":95
            },
            "old":{
                "status":"WARNING",
                "value":85,
                "duration":15,
                "raised_duration":15
            },
            "notification":{
                "when":1791482571,
                "delay":0,
                "delay_up_to_time":1791482571,
                "flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":4,
        "max_to_return":4,
        "before":3,
        "after":2
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":3,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":3,
        "skips_after":2
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.316
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/debug-all": {
		target: "/api/v2/alert_transitions?after=-3600&last=4&options=debug&scope_nodes=*&nodes=parity*&scope_contexts=hsig*&context=hsig.ctx&alert=hs_calc&f_status=warning&f_class=unknown&f_type=unknown&f_component=unknown&f_role=root&f_node=5a1e0000-0000-4000-8000-0000000000aa&f_alert=hs_calc&f_instance=hsig.values&f_context=hsig.ctx&timeout=30000&cardinality=3",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug"],
        "scope":{
            "scope_nodes":"*"
        },
        "selectors":{
            "nodes":"parity*",
            "alerts":{
                "context":"hsig.ctx",
                "anchor_gi":0,
                "last":4,
                "alert":"hs_calc",
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
            "before":0
        },
        "facets":{
            "f_status":"warning",
            "f_class":"unknown",
            "f_type":"unknown",
            "f_component":"unknown",
            "f_role":"root",
            "f_node":"5a1e0000-0000-4000-8000-0000000000aa",
            "f_alert":"hs_calc",
            "f_instance":"hsig.values",
            "f_context":"hsig.ctx"
        }
    },
    "facets":[{
            "id":"f_status",
            "name":"Alert Status",
            "order":1,
            "options":[{
                    "id":"CLEAR",
                    "name":"CLEAR",
                    "count":1
                },{
                    "id":"CRITICAL",
                    "name":"CRITICAL",
                    "count":1
                },{
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
            "gi":1791482548470354,
            "alert":"hs_calc",
            "transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482548,
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
                "when":1791482548,
                "delay":0,
                "delay_up_to_time":1791482548,
                "flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":3,
        "matched":1,
        "returned":1,
        "max_to_return":4,
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
        "total_ms":0.318
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/debug-options": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=debug,instances,minify,summary,values",
		bodies: [2]string{
			`{
    "api":2,
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["minify","debug","values","summary"],
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
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
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
                    "id":"CLEAR",
                    "name":"CLEAR",
                    "count":3
                },{
                    "id":"CRITICAL",
                    "name":"CRITICAL",
                    "count":3
                },{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":3
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":9
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":9
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hs_avg",
                    "name":"hs_avg",
                    "count":3
                },{
                    "id":"hs_max",
                    "name":"hs_max",
                    "count":3
                },{
                    "id":"hs_calc",
                    "name":"hs_calc",
                    "count":3
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hsig.values",
                    "name":"hsig.values",
                    "count":9
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hsig.ctx",
                    "name":"hsig.ctx",
                    "count":9
                }]
        }],
    "transitions":[{
            "gi":1791482581900830,
            "alert":"hs_avg",
            "transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":8
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":8
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.303
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/debug-mcp": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=debug,mcp",
		bodies: [2]string{
			`{
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug","mcp"],
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
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
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
    "transitions":[{
            "gi":1791482581900830,
            "alert":"hs_avg",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":8
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":8
    }
}
`,
			`{
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug","mcp"],
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
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
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
    "transitions":[{
            "gi":1791482581670355,
            "alert":"hs_avg",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":8
    },
    "stats":{
        "first":1,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":8
    }
}
`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/last-negative": {
		target: "/api/v2/alert_transitions?after=-3600&options=debug,mcp&f_alert=nothing&last=-1",
		bodies: [2]string{
			`{
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug","mcp"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":4294967295,
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":"nothing",
            "f_instance":null,
            "f_context":null
        }
    },
    "transitions":[],
    "items":{
        "evaluated":9,
        "matched":0,
        "returned":0,
        "max_to_return":4294967295,
        "before":0,
        "after":0
    },
    "stats":{
        "first":0,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/last-wrap": {
		target: "/api/v2/alert_transitions?after=-3600&options=debug,mcp&f_alert=nothing&last=4294967296",
		bodies: [2]string{
			`{
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug","mcp"],
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
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":"nothing",
            "f_instance":null,
            "f_context":null
        }
    },
    "transitions":[],
    "items":{
        "evaluated":9,
        "matched":0,
        "returned":0,
        "max_to_return":1,
        "before":0,
        "after":0
    },
    "stats":{
        "first":0,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/last-plus": {
		target: "/api/v2/alert_transitions?after=-3600&options=debug,mcp&f_alert=nothing&last=%2B5",
		bodies: [2]string{
			`{
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug","mcp"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":5,
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":"nothing",
            "f_instance":null,
            "f_context":null
        }
    },
    "transitions":[],
    "items":{
        "evaluated":9,
        "matched":0,
        "returned":0,
        "max_to_return":5,
        "before":0,
        "after":0
    },
    "stats":{
        "first":0,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/last-octal": {
		target: "/api/v2/alert_transitions?after=-3600&options=debug,mcp&f_alert=nothing&last=010",
		bodies: [2]string{
			`{
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug","mcp"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":8,
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":"nothing",
            "f_instance":null,
            "f_context":null
        }
    },
    "transitions":[],
    "items":{
        "evaluated":9,
        "matched":0,
        "returned":0,
        "max_to_return":8,
        "before":0,
        "after":0
    },
    "stats":{
        "first":0,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/last-binary": {
		target: "/api/v2/alert_transitions?after=-3600&options=debug,mcp&f_alert=nothing&last=0b11",
		bodies: [2]string{
			`{
    "request":{
        "mode":["nodes","alert_transitions"],
        "options":["debug","mcp"],
        "scope":{
            "scope_nodes":null
        },
        "selectors":{
            "nodes":null,
            "alerts":{
                "context":null,
                "anchor_gi":0,
                "last":3,
                "alert":null,
                "transition":null
            }
        },
        "filters":{
            "after":-3600,
            "before":0
        },
        "facets":{
            "f_status":null,
            "f_class":null,
            "f_type":null,
            "f_component":null,
            "f_role":null,
            "f_node":null,
            "f_alert":"nothing",
            "f_instance":null,
            "f_context":null
        }
    },
    "transitions":[],
    "items":{
        "evaluated":9,
        "matched":0,
        "returned":0,
        "max_to_return":3,
        "before":0,
        "after":0
    },
    "stats":{
        "first":0,
        "prepend":0,
        "append":0,
        "backwards":0,
        "forwards":0,
        "shifts":0,
        "skips_before":0,
        "skips_after":0
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/anchor-wrap": {target: "/api/v2/alert_transitions?after=-3600&options=debug,mcp&last=1&anchor_gi=18446744073709551616", same: "transitions/debug-mcp"},
	"transitions/long": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=long-json-keys,minify",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"global_id":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":0,"after":8},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.407}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"global_id":1791482581670355,"alert":"hs_avg","transition_id":"8550e59e-b2ea-4dac-9c88-c1091f3512ed","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482581,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":1791482581,"delay":0,"delay_up_to_time":1791482581,"flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":0,"after":8},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.271}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/rfc3339": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=minify,rfc3339",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"gi":1791482581900830,"alert":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"2026-10-08T18:03:01Z","info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":"2026-10-08T18:03:01Z","delay":0,"delay_up_to_time":"2026-10-08T18:03:01Z","flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":0,"after":8},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.303}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":3},{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":9}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":9}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":9}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":3},{"id":"hs_max","name":"hs_max","count":3},{"id":"hs_calc","name":"hs_calc","count":3}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":9}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":9}]}],"transitions":[{"gi":1791482581670355,"alert":"hs_avg","transition_id":"8550e59e-b2ea-4dac-9c88-c1091f3512ed","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"2026-10-08T18:03:01Z","info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":"2026-10-08T18:03:01Z","delay":0,"delay_up_to_time":"2026-10-08T18:03:01Z","flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":0,"after":8},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.287}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/mcp": {
		target: "/api/v2/alert_transitions?after=-3600&last=2&options=mcp",
		bodies: [2]string{
			`{
    "transitions":[{
            "gi":1791482581900830,
            "alert":"hs_avg",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        },{
            "gi":1791482581900591,
            "alert":"hs_max",
            "config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the maximum of a over 3 seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":10
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":17,
                "raised_duration":32
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":2,
        "max_to_return":2,
        "before":0,
        "after":7
    }
}
`,
			`{
    "transitions":[{
            "gi":1791482581670355,
            "alert":"hs_avg",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        },{
            "gi":1791482581670185,
            "alert":"hs_max",
            "config_hash_id":"f090fc72-965e-4e84-a2e4-99a07b484c62",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the maximum of a over 3 seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":10
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":17,
                "raised_duration":32
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":2,
        "max_to_return":2,
        "before":0,
        "after":7
    }
}
`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/mcp-rfc3339": {
		target: "/api/v2/alert_transitions?after=-3600&last=1&options=mcp,minify,rfc3339",
		bodies: [2]string{
			`{"transitions":[{"gi":1791482581900830,"alert":"hs_avg","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"2026-10-08T18:03:01Z","info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":"2026-10-08T18:03:01Z","delay":0,"delay_up_to_time":"2026-10-08T18:03:01Z","flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":0,"after":8}}`,
			`{"transitions":[{"gi":1791482581670355,"alert":"hs_avg","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"2026-10-08T18:03:01Z","info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CLEAR","value":44},"old":{"status":"CRITICAL","value":95,"duration":10,"raised_duration":25},"notification":{"when":"2026-10-08T18:03:01Z","delay":0,"delay_up_to_time":"2026-10-08T18:03:01Z","flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":9,"matched":9,"returned":1,"max_to_return":1,"before":0,"after":8}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/pretty": {
		target: "/api/v3/alert_transitions?after=-3600&last=1",
		bodies: [2]string{
			`{
    "api":2,
    "facets":[{
            "id":"f_status",
            "name":"Alert Status",
            "order":1,
            "options":[{
                    "id":"CLEAR",
                    "name":"CLEAR",
                    "count":3
                },{
                    "id":"CRITICAL",
                    "name":"CRITICAL",
                    "count":3
                },{
                    "id":"WARNING",
                    "name":"WARNING",
                    "count":3
                }]
        },{
            "id":"f_class",
            "name":"Alert Class",
            "order":4,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_type",
            "name":"Alert Type",
            "order":2,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_component",
            "name":"Alert Component",
            "order":5,
            "options":[{
                    "id":"unknown",
                    "name":"unknown",
                    "count":9
                }]
        },{
            "id":"f_role",
            "name":"Recipient Role",
            "order":3,
            "options":[{
                    "id":"root",
                    "name":"root",
                    "count":9
                }]
        },{
            "id":"f_node",
            "name":"Alert Node",
            "order":6,
            "options":[{
                    "id":"5a1e0000-0000-4000-8000-0000000000aa",
                    "name":"parity-parent",
                    "count":9
                }]
        },{
            "id":"f_alert",
            "name":"Alert Name",
            "order":7,
            "options":[{
                    "id":"hs_avg",
                    "name":"hs_avg",
                    "count":3
                },{
                    "id":"hs_max",
                    "name":"hs_max",
                    "count":3
                },{
                    "id":"hs_calc",
                    "name":"hs_calc",
                    "count":3
                }]
        },{
            "id":"f_instance",
            "name":"Instance Name",
            "order":8,
            "options":[{
                    "id":"hsig.values",
                    "name":"hsig.values",
                    "count":9
                }]
        },{
            "id":"f_context",
            "name":"Context",
            "order":9,
            "options":[{
                    "id":"hsig.ctx",
                    "name":"hsig.ctx",
                    "count":9
                }]
        }],
    "transitions":[{
            "gi":1791482581900830,
            "alert":"hs_avg",
            "transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af",
            "machine_guid":"5a1e0000-0000-4000-8000-0000000000aa",
            "config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1",
            "hostname":"parity-parent",
            "instance":"hsig.values",
            "instance_n":"hsig.values",
            "context":"hsig.ctx",
            "component":null,
            "classification":null,
            "type":null,
            "when":1791482581,
            "info":"the average of a over 5 aligned seconds",
            "summary":"",
            "units":"things",
            "new":{
                "status":"CLEAR",
                "value":44
            },
            "old":{
                "status":"CRITICAL",
                "value":95,
                "duration":10,
                "raised_duration":25
            },
            "notification":{
                "when":1791482581,
                "delay":0,
                "delay_up_to_time":1791482581,
                "flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],
                "exec":"<run>/notify/stub",
                "exec_code":0,
                "to":"root"
            }
        }],
    "items":{
        "evaluated":9,
        "matched":9,
        "returned":1,
        "max_to_return":1,
        "before":0,
        "after":8
    },
    "timings":{
        "routing_ms":0,
        "node_max_ms":0,
        "total_ms":0.389
    }
}
`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/one-text": {
		target: "/api/v3/alert_transitions?options=minify&transition=x",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[]},{"id":"f_class","name":"Alert Class","order":4,"options":[]},{"id":"f_type","name":"Alert Type","order":2,"options":[]},{"id":"f_component","name":"Alert Component","order":5,"options":[]},{"id":"f_role","name":"Recipient Role","order":3,"options":[]},{"id":"f_node","name":"Alert Node","order":6,"options":[]},{"id":"f_alert","name":"Alert Name","order":7,"options":[]},{"id":"f_instance","name":"Instance Name","order":8,"options":[]},{"id":"f_context","name":"Context","order":9,"options":[]}],"transitions":[],"items":{"evaluated":0,"matched":0,"returned":0,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.035}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/timeout-no-host": {target: "/api/v3/alert_transitions?timeout=-1&options=minify&scope_nodes=nothing*", same: "transitions/one-text"},
	"transitions/one-bare": {
		target: "/api/v3/alert_transitions?options=minify&transition=<one-bare>",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":1791482548470354,"alert":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.163}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/one-appended": {target: "/api/v3/alert_transitions?options=minify&transition=<one-appended>", same: "transitions/one-bare"},
	"transitions/one-last2": {
		target: "/api/v3/alert_transitions?options=minify&transition=<one-last2>&last=2",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":1791482548470354,"alert":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":2,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.162}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/one-filters": {target: "/api/v3/alert_transitions?options=minify&transition=<one-filters>&after=-3600&alert=hs_max&context=other.ctx&scope_nodes=nothing*&nodes=nothing*", same: "transitions/one-bare"},
	"transitions/one-facet": {
		target: "/api/v3/alert_transitions?options=minify&transition=<one-facet>&f_alert=hs_max",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":0}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":0}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":0}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":0}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":0}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":0}]}],"transitions":[],"items":{"evaluated":1,"matched":0,"returned":0,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.128}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/one-anchor": {
		target: "/api/v3/alert_transitions?options=minify&transition=<one-anchor>&anchor_gi=9999999999999999",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[],"items":{"evaluated":1,"matched":1,"returned":0,"max_to_return":1,"before":1,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.113}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/one-config": {
		target: "/api/v3/alert_transitions?options=minify&transition=<one-config>&options=config",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"WARNING","name":"WARNING","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":1791482548470354,"alert":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"configurations":[{"name":"hs_calc","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","selectors":{"type":"alarm","on":"hsig.values","families":null,"host_labels":null,"chart_labels":null},"value":{"units":"things","update_every":1,"calc":"$a"},"status":{"warn":"$this > 50","crit":"$this > 90"},"notification":{"type":"agent","exec":null,"to":"root","delay":"multiplier 1.0 ","repeat":null,"options":null},"class":null,"component":null,"type":null,"info":"the last value of a","summary":null}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.344}}`,
			``,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/one-mcp": {
		target: "/api/v3/alert_transitions?options=minify&transition=<one-mcp>&options=mcp",
		bodies: [2]string{
			`{"transitions":[{"gi":1791482548470354,"alert":"hs_calc","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":14,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0}}`,
			`{"transitions":[{"gi":1791482548257223,"alert":"hs_calc","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482548,"info":"the last value of a","summary":"","units":"things","new":{"status":"WARNING","value":70},"old":{"status":"CLEAR","value":10,"duration":13,"raised_duration":0},"notification":{"when":1791482548,"delay":0,"delay_up_to_time":1791482548,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/first-rfc3339": {
		target: "/api/v3/alert_transitions?options=minify&transition=<first-rfc3339>&options=rfc3339",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":1791482534427571,"alert":"hs_calc","transition_id":"63608a16-2fd1-4bd7-8eca-d605650a04b9","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"2026-10-08T18:02:14Z","info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},"notification":{"when":null,"delay":0,"delay_up_to_time":"2026-10-08T18:02:14Z","flags":["PROCESSED","UPDATED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.14}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CLEAR","name":"CLEAR","count":1}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":1}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":1}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":1}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_calc","name":"hs_calc","count":1}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":1}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":1}]}],"transitions":[{"gi":1791482535196266,"alert":"hs_calc","transition_id":"ccc083ae-0521-4245-b9d6-3e8aa5934965","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"cbc27ceb-dc89-46d5-9e2c-d04f869ddbda","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":"2026-10-08T18:02:15Z","info":"the last value of a","summary":"","units":"things","new":{"status":"CLEAR","value":10},"old":{"status":"UNINITIALIZED","value":0,"duration":0,"raised_duration":0},"notification":{"when":null,"delay":0,"delay_up_to_time":"2026-10-08T18:02:15Z","flags":["PROCESSED","UPDATED","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":1,"matched":1,"returned":1,"max_to_return":1,"before":0,"after":0},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.123}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
	"transitions/window-ago": {
		target: "/api/v2/alert_transitions?last=1&options=minify&before=-9",
		bodies: [2]string{
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":6}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":6}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":2},{"id":"hs_max","name":"hs_max","count":2},{"id":"hs_calc","name":"hs_calc","count":2}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":6}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":6}]}],"transitions":[{"gi":1791482571804750,"alert":"hs_avg","transition_id":"86adb823-e084-4817-91dc-a7f3f6c62902","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482571,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":85,"duration":15,"raised_duration":15},"notification":{"when":1791482571,"delay":0,"delay_up_to_time":1791482571,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":6,"returned":1,"max_to_return":1,"before":0,"after":5},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.299}}`,
			`{"api":2,"facets":[{"id":"f_status","name":"Alert Status","order":1,"options":[{"id":"CRITICAL","name":"CRITICAL","count":3},{"id":"WARNING","name":"WARNING","count":3}]},{"id":"f_class","name":"Alert Class","order":4,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_type","name":"Alert Type","order":2,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_component","name":"Alert Component","order":5,"options":[{"id":"unknown","name":"unknown","count":6}]},{"id":"f_role","name":"Recipient Role","order":3,"options":[{"id":"root","name":"root","count":6}]},{"id":"f_node","name":"Alert Node","order":6,"options":[{"id":"5a1e0000-0000-4000-8000-0000000000aa","name":"parity-parent","count":6}]},{"id":"f_alert","name":"Alert Name","order":7,"options":[{"id":"hs_avg","name":"hs_avg","count":2},{"id":"hs_max","name":"hs_max","count":2},{"id":"hs_calc","name":"hs_calc","count":2}]},{"id":"f_instance","name":"Instance Name","order":8,"options":[{"id":"hsig.values","name":"hsig.values","count":6}]},{"id":"f_context","name":"Context","order":9,"options":[{"id":"hsig.ctx","name":"hsig.ctx","count":6}]}],"transitions":[{"gi":1791482571585739,"alert":"hs_avg","transition_id":"cb817544-211c-412c-86c6-85007e98db9b","machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","config_hash_id":"674523c2-cd53-486b-b06c-8abd3d0a83d1","hostname":"parity-parent","instance":"hsig.values","instance_n":"hsig.values","context":"hsig.ctx","component":null,"classification":null,"type":null,"when":1791482571,"info":"the average of a over 5 aligned seconds","summary":"","units":"things","new":{"status":"CRITICAL","value":95},"old":{"status":"WARNING","value":85,"duration":15,"raised_duration":15},"notification":{"when":1791482571,"delay":0,"delay_up_to_time":1791482571,"flags":["PROCESSED","UPDATED","EXEC_RUN","SAVED"],"exec":"<run>/notify/stub","exec_code":0,"to":"root"}}],"items":{"evaluated":6,"matched":6,"returned":1,"max_to_return":1,"before":0,"after":5},"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.318}}`,
		},
		flight: [2][2]int64{{1791482584, 1791482584}, {1791482584, 1791482584}},
		logs:   [2]string{"L03", "L04"},
	},
}

// dashNormTransitionsRaw are the recorded rows whose answer is no JSON, by `<case>/<row>`: each side's answer as it
// was read.
var dashNormTransitionsRaw = map[string][2]string{
	"transitions/timeout": {
		"HTTP/1.1 504 Gateway Timeout\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Length: 13\r\nX-Transaction-ID: 4c3d1aa3cd5d4bb79e33753ec47998a5\r\n\r\nquery timeout",
		"HTTP/1.1 504 Gateway Timeout\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Length: 13\r\nX-Transaction-ID: 5f654ecca74f4970a8a86bddbaa373cf\r\n\r\nquery timeout",
	},
	"transitions/timeout-v2": {
		"HTTP/1.1 504 Gateway Timeout\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Length: 13\r\nX-Transaction-ID: ad09422959e346e6bc85d8ad97f25a14\r\n\r\nquery timeout",
		"HTTP/1.1 504 Gateway Timeout\r\nConnection: close\r\nServer: Netdata Embedded HTTP Server v2.11.0-458-g1e97a0fc9e\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\nDate: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-cache, no-store, must-revalidate\r\nPragma: no-cache\r\nExpires: Thu, 08 Oct 2026 18:03:04 GMT\r\nContent-Length: 13\r\nX-Transaction-ID: e90b4acf13614647b0a3796c59de6d2e\r\n\r\nquery timeout",
	},
}

// dashNormTransitionsLogs are the recorded alert logs by name.
var dashNormTransitionsLogs = map[string]string{
	"L01": `[{"unique_id":1791482502,"alarm_id":1791482484,"alarm_event_id":6,"name":"hr_plain","transition_id":"77468b3b-1800-49a7-a167-a911053f635f","when":1791482494,"duration":5,"non_clear_duration":5,"exec_run":0,"delay_up_to_timestamp":1791482494,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791482501,"alarm_id":1791482483,"alarm_event_id":6,"name":"hr_two","transition_id":"c56d5593-0aaf-434e-9e76-4d2383728bbb","when":1791482494,"duration":5,"non_clear_duration":5,"exec_run":1791482494,"delay_up_to_timestamp":1791482494,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791482500,"alarm_id":1791482482,"alarm_event_id":7,"name":"hr_tpl","transition_id":"b8a16b7b-e60e-4d77-acaa-bc9e1230608c","when":1791482494,"duration":5,"non_clear_duration":10,"exec_run":1791482494,"delay_up_to_timestamp":1791482494,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482499,"alarm_id":1791482484,"alarm_event_id":5,"name":"hr_plain","transition_id":"01b29328-6d4b-4c2a-ad3f-ce1cb6d23135","when":1791482489,"duration":9,"non_clear_duration":0,"exec_run":1791482489,"delay_up_to_timestamp":1791482489,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482498,"alarm_id":1791482483,"alarm_event_id":5,"name":"hr_two","transition_id":"32b5b2dc-e457-4850-847c-19b62d788719","when":1791482489,"duration":9,"non_clear_duration":0,"exec_run":1791482489,"delay_up_to_timestamp":1791482489,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482497,"alarm_id":1791482482,"alarm_event_id":6,"name":"hr_tpl","transition_id":"c1d680e5-18e0-4252-870f-22c218a4ea0e","when":1791482489,"duration":5,"non_clear_duration":5,"exec_run":1791482489,"delay_up_to_timestamp":1791482489,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482496,"alarm_id":1791482482,"alarm_event_id":5,"name":"hr_tpl","transition_id":"c257fa0f-2c92-4971-9392-2c20d865f204","when":1791482484,"duration":4,"non_clear_duration":0,"exec_run":1791482484,"delay_up_to_timestamp":1791482484,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482495,"alarm_id":1791482484,"alarm_event_id":4,"name":"hr_plain","transition_id":"91732c5a-2359-4a19-b5d6-4d09c3837edb","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482494,"alarm_id":1791482483,"alarm_event_id":4,"name":"hr_two","transition_id":"3c3e042a-5a7d-4431-a05c-f51f5cf89c5f","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482493,"alarm_id":1791482482,"alarm_event_id":4,"name":"hr_tpl","transition_id":"7cc1cb83-c58a-4684-89b5-f3d487a3449b","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482492,"alarm_id":1791482484,"alarm_event_id":3,"name":"hr_plain","transition_id":"20117199-eaeb-4c40-814d-75bd08279073","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482491,"alarm_id":1791482483,"alarm_event_id":3,"name":"hr_two","transition_id":"d2132c2c-412d-495a-8e01-028e914b6c75","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482490,"alarm_id":1791482482,"alarm_event_id":3,"name":"hr_tpl","transition_id":"197e0c93-86e7-46f1-a39e-27030371a336","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482489,"alarm_id":1791482484,"alarm_event_id":2,"name":"hr_plain","transition_id":"88671860-20d1-482b-8292-cf7d60e7656b","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482488,"alarm_id":1791482483,"alarm_event_id":2,"name":"hr_two","transition_id":"8c64bf40-1960-4d2f-bd0f-672092c9837b","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482487,"alarm_id":1791482482,"alarm_event_id":2,"name":"hr_tpl","transition_id":"f2a3fdc7-17ec-4e87-8582-a0e36b1547d0","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482486,"alarm_id":1791482481,"alarm_event_id":3,"name":"hr_idle","transition_id":"bc104341-5b3f-4b39-98f8-2a08aa506949","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482485,"alarm_id":1791482481,"alarm_event_id":2,"name":"hr_idle","transition_id":"ebe15cf4-9b9b-40e1-936f-da25fdff4798","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482484,"alarm_id":1791482484,"alarm_event_id":1,"name":"hr_plain","transition_id":"a3f115f3-42e0-4eea-9642-161b7d1ba79e","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482483,"alarm_id":1791482483,"alarm_event_id":1,"name":"hr_two","transition_id":"4891d79b-7a8a-400c-b9e7-9d993c6797fb","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482482,"alarm_id":1791482482,"alarm_event_id":1,"name":"hr_tpl","transition_id":"0393a71c-96ca-45a0-bee9-ec7325baf728","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482481,"alarm_id":1791482481,"alarm_event_id":1,"name":"hr_idle","transition_id":"54d05129-e7d8-48fb-8597-dba1c94a840b","when":1791482480,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482480,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L02": `[{"unique_id":1791482503,"alarm_id":1791482485,"alarm_event_id":6,"name":"hr_plain","transition_id":"b7985916-8dc4-4e6c-ad65-edf20956a252","when":1791482494,"duration":5,"non_clear_duration":5,"exec_run":0,"delay_up_to_timestamp":1791482494,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791482502,"alarm_id":1791482484,"alarm_event_id":6,"name":"hr_two","transition_id":"7bc0da93-0dd1-4c23-9936-1e4f472cca98","when":1791482494,"duration":5,"non_clear_duration":5,"exec_run":1791482494,"delay_up_to_timestamp":1791482494,"status":"CLEAR","old_status":"WARNING"},{"unique_id":1791482501,"alarm_id":1791482483,"alarm_event_id":7,"name":"hr_tpl","transition_id":"4b653737-7e70-4f3a-8dc9-cf89af95c167","when":1791482494,"duration":5,"non_clear_duration":10,"exec_run":1791482494,"delay_up_to_timestamp":1791482494,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482500,"alarm_id":1791482485,"alarm_event_id":5,"name":"hr_plain","transition_id":"8a40860b-c938-44bd-aaf4-697d4f9e0d9f","when":1791482489,"duration":8,"non_clear_duration":0,"exec_run":1791482489,"delay_up_to_timestamp":1791482489,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482499,"alarm_id":1791482484,"alarm_event_id":5,"name":"hr_two","transition_id":"fd7e4792-b3b5-46ae-ac06-26ec70742138","when":1791482489,"duration":8,"non_clear_duration":0,"exec_run":1791482489,"delay_up_to_timestamp":1791482489,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482498,"alarm_id":1791482483,"alarm_event_id":6,"name":"hr_tpl","transition_id":"ec6ff23c-b64d-4239-98ab-70e2bf7a4229","when":1791482489,"duration":5,"non_clear_duration":5,"exec_run":1791482489,"delay_up_to_timestamp":1791482489,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482497,"alarm_id":1791482483,"alarm_event_id":5,"name":"hr_tpl","transition_id":"8b57db08-e28f-40c3-a40f-12ab79ddcfe7","when":1791482484,"duration":3,"non_clear_duration":0,"exec_run":1791482484,"delay_up_to_timestamp":1791482484,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482496,"alarm_id":1791482485,"alarm_event_id":4,"name":"hr_plain","transition_id":"da8e8f1f-159e-4e62-8d22-ad9de69c259d","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482495,"alarm_id":1791482484,"alarm_event_id":4,"name":"hr_two","transition_id":"7ca4c8cd-3ed2-451d-8a59-8ab66e374624","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482494,"alarm_id":1791482483,"alarm_event_id":4,"name":"hr_tpl","transition_id":"11503ab9-2bf7-416b-8f5c-5a41346901fd","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482493,"alarm_id":1791482485,"alarm_event_id":3,"name":"hr_plain","transition_id":"ce92697a-4271-4822-b6ae-6f0eb8f54c37","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482492,"alarm_id":1791482484,"alarm_event_id":3,"name":"hr_two","transition_id":"2e711411-ba8b-4015-b24f-bfb10912f862","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482491,"alarm_id":1791482483,"alarm_event_id":3,"name":"hr_tpl","transition_id":"24cb4090-a04f-4d1e-b94e-eed5e50e410b","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482490,"alarm_id":1791482485,"alarm_event_id":2,"name":"hr_plain","transition_id":"4792b0b6-6990-470f-b9bd-f9878dd30279","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482489,"alarm_id":1791482484,"alarm_event_id":2,"name":"hr_two","transition_id":"93c32b62-a1e2-42c2-94ec-53f0cba3ec51","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482488,"alarm_id":1791482483,"alarm_event_id":2,"name":"hr_tpl","transition_id":"e1569b95-01c2-4d2f-97d2-8c1643dc9f3b","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482487,"alarm_id":1791482482,"alarm_event_id":3,"name":"hr_idle","transition_id":"c5c5982a-ff10-4142-b39d-a753cb69a399","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482486,"alarm_id":1791482482,"alarm_event_id":2,"name":"hr_idle","transition_id":"bbf98558-2050-4f79-be9e-c9dcbbb038cc","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482485,"alarm_id":1791482485,"alarm_event_id":1,"name":"hr_plain","transition_id":"6831f14f-43a8-4974-bf13-4e9a1f02798c","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482484,"alarm_id":1791482484,"alarm_event_id":1,"name":"hr_two","transition_id":"d222a486-6d6b-4a27-b94d-26a0f262af5f","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482483,"alarm_id":1791482483,"alarm_event_id":1,"name":"hr_tpl","transition_id":"abc6a6c1-9ef4-4156-bad3-c7c730401f91","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482482,"alarm_id":1791482482,"alarm_event_id":1,"name":"hr_idle","transition_id":"106a5c3d-7ca7-4bf7-ad2d-c8f642929f66","when":1791482481,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482481,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L03": `[{"unique_id":1791482555,"alarm_id":1791482537,"alarm_event_id":7,"name":"hs_avg","transition_id":"53ff6f7e-ac58-44d4-b4ce-6ed69071f5af","when":1791482581,"duration":10,"non_clear_duration":25,"exec_run":1791482581,"delay_up_to_timestamp":1791482581,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482554,"alarm_id":1791482536,"alarm_event_id":7,"name":"hs_max","transition_id":"3e98bbba-770d-4057-86db-0cb8b202b413","when":1791482581,"duration":17,"non_clear_duration":32,"exec_run":1791482581,"delay_up_to_timestamp":1791482581,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482553,"alarm_id":1791482535,"alarm_event_id":7,"name":"hs_calc","transition_id":"07c00ca8-6738-4572-832f-af6e49d3b689","when":1791482578,"duration":15,"non_clear_duration":30,"exec_run":1791482578,"delay_up_to_timestamp":1791482578,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482552,"alarm_id":1791482537,"alarm_event_id":6,"name":"hs_avg","transition_id":"86adb823-e084-4817-91dc-a7f3f6c62902","when":1791482571,"duration":15,"non_clear_duration":15,"exec_run":1791482571,"delay_up_to_timestamp":1791482571,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482551,"alarm_id":1791482536,"alarm_event_id":6,"name":"hs_max","transition_id":"098284ca-b490-4f1a-a06c-95e043eff5c2","when":1791482564,"duration":15,"non_clear_duration":15,"exec_run":1791482564,"delay_up_to_timestamp":1791482564,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482550,"alarm_id":1791482535,"alarm_event_id":6,"name":"hs_calc","transition_id":"f582cc59-232d-4b28-99ba-d932d005c3de","when":1791482563,"duration":15,"non_clear_duration":15,"exec_run":1791482563,"delay_up_to_timestamp":1791482563,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482549,"alarm_id":1791482537,"alarm_event_id":5,"name":"hs_avg","transition_id":"87213cbb-9c18-41ae-9971-91062bfe6bf3","when":1791482556,"duration":18,"non_clear_duration":0,"exec_run":1791482556,"delay_up_to_timestamp":1791482556,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482548,"alarm_id":1791482536,"alarm_event_id":5,"name":"hs_max","transition_id":"ed13ea3a-d0ae-43fd-93b3-3c40eb031d0d","when":1791482549,"duration":13,"non_clear_duration":0,"exec_run":1791482549,"delay_up_to_timestamp":1791482549,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482547,"alarm_id":1791482535,"alarm_event_id":5,"name":"hs_calc","transition_id":"bdb2f940-87e3-4894-a14d-63fd29940b1d","when":1791482548,"duration":14,"non_clear_duration":0,"exec_run":1791482548,"delay_up_to_timestamp":1791482548,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482546,"alarm_id":1791482537,"alarm_event_id":4,"name":"hs_avg","transition_id":"6d7252da-3e83-4e7f-bc11-87c74de86faf","when":1791482538,"duration":4,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482538,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482545,"alarm_id":1791482536,"alarm_event_id":4,"name":"hs_max","transition_id":"2533a135-22b3-420d-a13d-318b933a506e","when":1791482536,"duration":2,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482536,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482544,"alarm_id":1791482535,"alarm_event_id":4,"name":"hs_calc","transition_id":"63608a16-2fd1-4bd7-8eca-d605650a04b9","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482543,"alarm_id":1791482537,"alarm_event_id":3,"name":"hs_avg","transition_id":"f9716e63-0566-443d-a26c-9379d0aea087","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482542,"alarm_id":1791482536,"alarm_event_id":3,"name":"hs_max","transition_id":"d817603b-c630-461a-99d6-3696bf35cc50","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482541,"alarm_id":1791482535,"alarm_event_id":3,"name":"hs_calc","transition_id":"899b3b95-d41a-4341-824c-eb4a3036e902","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482540,"alarm_id":1791482537,"alarm_event_id":2,"name":"hs_avg","transition_id":"6f8a5175-23c1-447f-92df-448fec6f6111","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482539,"alarm_id":1791482536,"alarm_event_id":2,"name":"hs_max","transition_id":"bdc3d7cf-5cd6-4db6-ad20-251f08813af2","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482538,"alarm_id":1791482535,"alarm_event_id":2,"name":"hs_calc","transition_id":"ff2780e8-3dc1-4dc7-bdcc-cc9fbff04e9e","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482537,"alarm_id":1791482537,"alarm_event_id":1,"name":"hs_avg","transition_id":"bdc1ee52-f71e-4774-861c-bddee2e0f27c","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482536,"alarm_id":1791482536,"alarm_event_id":1,"name":"hs_max","transition_id":"351d177a-cfd8-4774-8334-3d2d6c51349a","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482535,"alarm_id":1791482535,"alarm_event_id":1,"name":"hs_calc","transition_id":"f78312cd-18c4-4008-999a-3f2e559257ae","when":1791482534,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482534,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
	"L04": `[{"unique_id":1791482556,"alarm_id":1791482538,"alarm_event_id":7,"name":"hs_avg","transition_id":"8550e59e-b2ea-4dac-9c88-c1091f3512ed","when":1791482581,"duration":10,"non_clear_duration":25,"exec_run":1791482581,"delay_up_to_timestamp":1791482581,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482555,"alarm_id":1791482537,"alarm_event_id":7,"name":"hs_max","transition_id":"dadb6aef-765b-479e-8cf8-0ff11439e1bf","when":1791482581,"duration":17,"non_clear_duration":32,"exec_run":1791482581,"delay_up_to_timestamp":1791482581,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482554,"alarm_id":1791482536,"alarm_event_id":7,"name":"hs_calc","transition_id":"5a1dce60-0a63-4f1f-b099-fdbe3b04b8cb","when":1791482578,"duration":15,"non_clear_duration":30,"exec_run":1791482578,"delay_up_to_timestamp":1791482578,"status":"CLEAR","old_status":"CRITICAL"},{"unique_id":1791482553,"alarm_id":1791482538,"alarm_event_id":6,"name":"hs_avg","transition_id":"cb817544-211c-412c-86c6-85007e98db9b","when":1791482571,"duration":15,"non_clear_duration":15,"exec_run":1791482571,"delay_up_to_timestamp":1791482571,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482552,"alarm_id":1791482537,"alarm_event_id":6,"name":"hs_max","transition_id":"526f7c71-c27d-4d9c-b16f-16129ce1cae6","when":1791482564,"duration":15,"non_clear_duration":15,"exec_run":1791482564,"delay_up_to_timestamp":1791482564,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482551,"alarm_id":1791482536,"alarm_event_id":6,"name":"hs_calc","transition_id":"1e777aef-8f19-4c9d-833f-5efa3804f730","when":1791482563,"duration":15,"non_clear_duration":15,"exec_run":1791482563,"delay_up_to_timestamp":1791482563,"status":"CRITICAL","old_status":"WARNING"},{"unique_id":1791482550,"alarm_id":1791482538,"alarm_event_id":5,"name":"hs_avg","transition_id":"cbbaf78c-7960-4d93-81e7-55c4b1e839ec","when":1791482556,"duration":18,"non_clear_duration":0,"exec_run":1791482556,"delay_up_to_timestamp":1791482556,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482549,"alarm_id":1791482537,"alarm_event_id":5,"name":"hs_max","transition_id":"9f8530e3-19b8-4a79-a16d-7471340fe033","when":1791482549,"duration":13,"non_clear_duration":0,"exec_run":1791482549,"delay_up_to_timestamp":1791482549,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482548,"alarm_id":1791482536,"alarm_event_id":5,"name":"hs_calc","transition_id":"c96e70db-5812-4270-9da6-8e0d694f3bc4","when":1791482548,"duration":13,"non_clear_duration":0,"exec_run":1791482548,"delay_up_to_timestamp":1791482548,"status":"WARNING","old_status":"CLEAR"},{"unique_id":1791482547,"alarm_id":1791482538,"alarm_event_id":4,"name":"hs_avg","transition_id":"d9873658-2fff-4a1d-b58f-6476b219421e","when":1791482538,"duration":3,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482538,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482546,"alarm_id":1791482537,"alarm_event_id":4,"name":"hs_max","transition_id":"e4c1698a-8023-4cd4-8b04-1e85f219e768","when":1791482536,"duration":1,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482536,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482545,"alarm_id":1791482536,"alarm_event_id":4,"name":"hs_calc","transition_id":"ccc083ae-0521-4245-b9d6-3e8aa5934965","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"CLEAR","old_status":"UNINITIALIZED"},{"unique_id":1791482544,"alarm_id":1791482538,"alarm_event_id":3,"name":"hs_avg","transition_id":"892a5a75-1c6a-4422-8ab9-a7e7e14c3bcb","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482543,"alarm_id":1791482537,"alarm_event_id":3,"name":"hs_max","transition_id":"96556ec7-ac21-4bdd-acb2-0fdf5a391b1f","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482542,"alarm_id":1791482536,"alarm_event_id":3,"name":"hs_calc","transition_id":"238583b0-9c3b-421f-b093-656ea39a3286","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482541,"alarm_id":1791482538,"alarm_event_id":2,"name":"hs_avg","transition_id":"4e3eb67d-2084-43b3-ac13-ebbcae193c03","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482540,"alarm_id":1791482537,"alarm_event_id":2,"name":"hs_max","transition_id":"23eb68fe-9c28-4869-aa0b-c66fdf8de036","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482539,"alarm_id":1791482536,"alarm_event_id":2,"name":"hs_calc","transition_id":"432a303c-ba1f-4f9d-86b0-9338b1a70132","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"REMOVED","old_status":"UNINITIALIZED"},{"unique_id":1791482538,"alarm_id":1791482538,"alarm_event_id":1,"name":"hs_avg","transition_id":"07b425f5-c8ed-4153-ac58-3bf0cae88925","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482537,"alarm_id":1791482537,"alarm_event_id":1,"name":"hs_max","transition_id":"e749369e-a8f8-42b4-995d-5219394dba8f","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"UNINITIALIZED","old_status":"REMOVED"},{"unique_id":1791482536,"alarm_id":1791482536,"alarm_event_id":1,"name":"hs_calc","transition_id":"464cf2ed-1ba7-4ba9-8197-98d97907fc95","when":1791482535,"duration":0,"non_clear_duration":0,"exec_run":0,"delay_up_to_timestamp":1791482535,"status":"UNINITIALIZED","old_status":"REMOVED"}]`,
}
