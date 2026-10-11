// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
	"slices"
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The closer rows of `api.v2-alerts` (milestone 10 commit 4's coverage limits): rows on the `alerts` case's fixture
// (alertsV2CloserRows and the search row alertsV2CloserSearchRows, asked after its own), and the cases `two-charts`
// (alertsV2TwoChartsCase) and `stock-rules` (alertsV2StockRulesCase).

// alertsV2ValuesRe is an instance's last evaluation as the plain form prints it by its short or its long key
// (api_v2_contexts_alerts.c:375-377), a number or a date.
var alertsV2ValuesRe = regexp.MustCompile(`"(t|last_updated_timestamp)":(\s*)(\d+|"[^"\\]*")`)

// alertsV2RenderValues is alertsV2Render for a body without a global id, as `values` without `instances` answers
// (api_v2_contexts_alerts.c:321-383, :330-350, :374-378): an instance is its index, its host's index, its name, its
// chart's id and name and its last value and evaluation, so there is no entry of the alert log to read it against.
// Its last evaluation (`t`, by its long key `last_updated_timestamp`) reads T where it is a second of the request's
// flight or of the healthBound seconds before it, as alertsV2Render reads it in an instance that has a global id; a
// date (`rfc3339`) has its mark end alertsV2Dated. A 0 (never evaluated) and any other second stay as the agent wrote
// them, and so does a body of the MCP form (alertsV2RenderMCP's) or with a global id. It returns the seconds it
// replaced, in the body's order.
func alertsV2RenderValues(flight [2]int64, body []byte) ([]byte, []int64) {
	if alertsV2ItemRe.Match(body) || alertsV2McpHeaderRe.Match(body) {
		return body, nil
	}
	var clocks []int64
	out := alertsV2ValuesRe.ReplaceAllStringFunc(string(body), func(m string) string {
		g := alertsV2ValuesRe.FindStringSubmatch(m)
		v, date, ok := alertsV2Second(g[3])
		if !ok || v == 0 || v < flight[0]-healthBound || v > flight[1] {
			return m
		}
		mark := "T"
		if date {
			mark += alertsV2Dated
		}
		clocks = append(clocks, v)
		return `"` + g[1] + `":` + g[2] + `"` + mark + `"`
	})
	return []byte(out), clocks
}

// alertsV2McpHeaderRe is a header of the MCP form, which has headers and rows where the plain form has keys
// (api_v2_contexts_alerts.c:434-619).
var alertsV2McpHeaderRe = regexp.MustCompile(`"(all_alerts|alert_instances)_header"`)

// alertsV2CloserAlerts asks the closer rows of the `alerts` case once its own were compared.
func alertsV2CloserAlerts(t *testing.T, h *healthPair) {
	t.Helper()
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2CloserRows {
		compareV2(t, h.p, req, fam)
	}
	for _, req := range alertsV2CloserSearchRows {
		compareV2(t, h.p, req, searchFamily)
	}
}

// alertsV2CloserRows are the closer rows of the `alerts` case, asked after its own rows on the same state (a = 70:
// ha_low WARNING, ha_mid and ha_high CLEAR, hm_plain UNINITIALIZED). Their guards read what C answers (probes C
// against C, SA-C's hand-back):
//   - `values` without `instances` lists the instances it keeps, each with its index (`ati` only with `summary`),
//     its host's index, its name, its chart's id and name, and its last value and evaluation: no global id, no
//     status, no context (api_v2_contexts_alerts.c:321-383: :330, :333, :336, :344, :347-350, :352, :374-378; the
//     list is made and printed with either option, :156, :693-695). hm_plain was never evaluated: `t` 0 and `v`
//     null (`values-uninitialized`); the three others' `t` is a second of the request's flight
//     (alertsV2RenderValues: `values-alone`, and beside `summary`, `values-summary`);
//   - a status list is split on a comma, a blank and a `|` (web/api/maps/contexts_alert_statuses.c:24), read from the
//     decoded query, where a `+` is a blank (libnetdata/url/url.c:229): `clear` and `warning` keep the three alerts
//     with a status (`status-pipe`, `status-blank`, `status-plus`); a word is compared exactly (:31: `RAISED` is no
//     word, so no filter: all four, `status-upper`); `active` is `raised` and is echoed so, the first word of its bit
//     (:9-17, :41-55: `active-debug`), and a list of four words echoes each bit once, in the table's order
//     (`debug-statuses`, which keeps all four alerts);
//   - the plain form reads no `cardinality`: only the MCP writer cuts its lists (:435, :469, :556; :621-695 has no
//     limit): all four alerts and instances, nothing cut (`plain-cardinality`);
//   - the echo's window under `rfc3339`: a 0 is null, -600, a relative time, a number
//     (database/contexts/api_v2_contexts.c:1427-1428; libnetdata/buffer/buffer.h:1119-1128: `debug-rfc3339`);
//   - the MCP instance rows by one option alone: with `instances` 18 columns, with `values` 5 (:510-605:
//     `mcp-instances-only`, `mcp-values-only`; the last evaluation the row's last item);
//   - `alert=` is a pattern that tells the case (database/contexts/api_v2_contexts.c:1320; string_to_simple_pattern,
//     libnetdata/simple_pattern/simple_pattern.h:59): `HA_LOW|ha_mid` keeps ha_mid alone (`alert-case`);
//   - `/api/v2/alerts` is `/api/v3/alerts` (web/api/web_api_v2.c:53, web_api_v3.c:80: one callback): the MCP form of
//     everything, minified (`v2-mcp`).
var alertsV2CloserRows = func() []v2Req {
	at := func(query string) string { return alertsV2Route + "?" + query }
	names := func(items ...string) alertsV2Rows {
		return alertsV2Rows{member: "alerts", keys: []string{"nm"}, items: items}
	}
	instances := func(items ...string) alertsV2Rows {
		return alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st"}, items: items}
	}
	recipients := func(items ...string) alertsV2Rows {
		return alertsV2Rows{member: "alerts_by_recipient", keys: alertsV2RecipientKeys, items: items}
	}
	modules := func(items ...string) alertsV2Rows {
		return alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: items}
	}
	// the groupings by type, component and classification: hm_plain's rule alone has them
	groupings := func(running string) []alertsV2Rows {
		return []alertsV2Rows{
			{member: "alerts_by_type", keys: alertsV2GroupKeys, items: []string{"Parity Check " + running + " " + running + " 1"}},
			{member: "alerts_by_component", keys: alertsV2GroupKeys, items: []string{"Fixture Part " + running + " " + running + " 1"}},
			{member: "alerts_by_classification", keys: alertsV2GroupKeys, items: []string{"Workload " + running + " " + running + " 1"}},
		}
	}
	// an instance with `values` alone: what it has, and `-` for what it must not
	valuesKeys := []string{"ni", "nm", "ch", "ch_n", "v", "t", "ati?", "gi?", "st?", "ctx?"}
	const (
		silent = "silent 0 0 0 0 0 1"
		// the three alerts with a status, by their status
		raised = "root 0 1 2 3 0 3"
		// the request as `options=debug` echoes it, from its scope on, with no selector
		echo = `"scope":{"scope_nodes":null,"scope_contexts":null},"selectors":{"nodes":null,"contexts":null,` +
			`"alerts":{"status":[%s],"alert":null,"transition":null}},"filters":%s}`
		noWindow = `{"after":0,"before":0}`
	)
	listed := []alertsV2Rows{names("ha_low", "ha_mid", "ha_high"),
		instances("ha_low WARNING", "ha_mid CLEAR", "ha_high CLEAR"),
		recipients(raised, silent), modules("parity.mod 0 1 2 3 0")}
	listed = append(listed, groupings("0")...)
	every := append([]alertsV2Rows{names("hm_plain", "ha_low", "ha_mid", "ha_high"),
		instances("hm_plain UNINITIALIZED", "ha_low WARNING", "ha_mid CLEAR", "ha_high CLEAR"),
		recipients("silent 0 0 0 1 1 1", raised), modules("[none] 0 0 0 1 1", "parity.mod 0 1 2 3 0")}, groupings("1")...)
	mcpInstances := `["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units",` +
		`"Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source",` +
		`"Recipients","Type","Component","Classification"]`
	mcpValues := `["Alert Name","Hostname","Instance Name","Last Updated Value","Last Updated Timestamp"]`
	return []v2Req{
		{name: "values-uninitialized", target: at("options=values,minify&status=uninitialized"), status: "200",
			guard: dashGuard([]dashFact{dashKeys("api nodes alert_instances timings"), alertsV2Guard(alertsV2Nodes(alertsV2Parent),
				alertsV2Rows{member: "alert_instances", keys: valuesKeys, items: []string{"0 hm_plain hsig.plain hsig.plain null 0 - - - -"}})})},
		{name: "values-alone", target: at("options=values,minify"), status: "200",
			guard: dashGuard([]dashFact{dashKeys("api nodes alert_instances timings"), alertsV2Guard(
				alertsV2Rows{member: "alert_instances", keys: valuesKeys, items: []string{"0 hm_plain hsig.plain hsig.plain null 0 - - - -",
					"0 ha_low hsig.values hsig.values 70 T - - - -", "0 ha_mid hsig.values hsig.values 70 T - - - -",
					"0 ha_high hsig.values hsig.values 70 T - - - -"}})})},
		{name: "values-summary", target: at("options=summary,values,minify&status=raised"), status: "200",
			guard: alertsV2Guard(names("ha_low"),
				alertsV2Rows{member: "alert_instances", keys: []string{"ati", "ni", "nm", "ch", "ch_n", "v", "t", "gi?", "st?"},
					items: []string{"0 0 ha_low hsig.values hsig.values 70 T - -"}})},

		// the status list
		{name: "status-pipe", target: at("options=summary,instances,minify&status=clear%7Cwarning"), status: "200",
			guard: alertsV2Guard(listed...)},
		{name: "status-blank", target: at("options=summary,instances,minify&status=clear%20warning"), status: "200",
			guard: alertsV2Guard(listed...)},
		{name: "status-plus", target: at("options=summary,instances,minify&status=clear+warning"), status: "200",
			guard: alertsV2Guard(listed...)},
		{name: "status-upper", target: at("options=summary,instances,minify&status=RAISED"), status: "200",
			guard: alertsV2Guard(every...)},
		{name: "active-debug", target: at("options=summary,debug&status=active"), status: "200",
			guard: dashGuard([]dashFact{
				dashIs(`{"mode":["nodes","alerts"],"options":["debug","summary"],`+fmt.Sprintf(echo, `"raised"`, noWindow), "request"),
				alertsV2Guard(names("ha_low"), alertsV2Rows{member: "alert_instances"}, recipients("root 0 1 0 1 0 3", silent),
					modules("parity.mod 0 1 0 1 0")),
			})},
		{name: "debug-statuses", target: at("options=debug,summary&status=uninitialized,undefined,clear,active"),
			status: "200",
			guard: dashGuard([]dashFact{
				dashIs(`{"mode":["nodes","alerts"],"options":["debug","summary"],`+
					fmt.Sprintf(echo, `"uninitialized","undefined","clear","raised"`, noWindow), "request"),
				alertsV2Guard(names("hm_plain", "ha_low", "ha_mid", "ha_high"), alertsV2Rows{member: "alert_instances"},
					recipients("silent 0 0 0 1 1 1", raised)),
			})},

		// the plain form and the limit
		{name: "plain-cardinality", target: at("options=summary,instances,minify&cardinality=1"), status: "200",
			guard: dashGuard([]dashFact{dashKeys("api nodes alerts alerts_by_type alerts_by_component alerts_by_classification " +
				"alerts_by_recipient alerts_by_module alert_instances timings"), alertsV2Guard(every...)})},
		// the echo's window under `rfc3339`
		{name: "debug-rfc3339", target: at("options=debug,rfc3339&after=-600"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("api request nodes timings"),
				dashIs(`{"mode":["nodes","alerts"],"options":["debug","rfc3339"],`+
					fmt.Sprintf(echo, "", `{"after":-600,"before":null}`), "request"),
				alertsV2Guard(alertsV2Nodes(alertsV2Parent)),
			})},

		// the MCP instance rows by one option alone
		{name: "mcp-instances-only", target: at("options=mcp,instances"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes alert_instances_header alert_instances"), dashIs(mcpInstances, "alert_instances_header"),
				alertsV2Guard(alertsV2Rows{member: "alert_instances",
					keys: []string{"[0]", "[1]", "[2]", "[3]", "[4]", "[10]", "[11]", "[14]", "[15]", "[16]", "[17]", "[18]?"},
					items: []string{"hm_plain parity-parent hsig.ctx hsig.plain UNINITIALIZED null WHEN silent Parity Check Fixture Part Workload -",
						"ha_low parity-parent hsig.ctx hsig.values WARNING 70 WHEN root    -",
						"ha_mid parity-parent hsig.ctx hsig.values CLEAR 10 WHEN root    -",
						"ha_high parity-parent hsig.ctx hsig.values CLEAR 10 WHEN root    -"}}),
			})},
		{name: "mcp-values-only", target: at("options=mcp,values"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes alert_instances_header alert_instances"), dashIs(mcpValues, "alert_instances_header"),
				alertsV2Guard(alertsV2Rows{member: "alert_instances", keys: []string{"[0]", "[1]", "[2]", "[3]", "[4]", "[5]?"},
					items: []string{"hm_plain parity-parent hsig.plain null 0 -", "ha_low parity-parent hsig.values 70 T -",
						"ha_mid parity-parent hsig.values 70 T -", "ha_high parity-parent hsig.values 70 T -"}}),
			})},

		// a name in another case, and the v2 route
		{name: "alert-case", target: at("options=summary,instances,minify&alert=HA_LOW%7Cha_mid"), status: "200",
			guard: alertsV2Guard(names("ha_mid"), instances("ha_mid CLEAR"), recipients("root 0 0 1 1 0 3", silent),
				modules("parity.mod 0 0 1 1 0"))},
		{name: "v2-mcp", target: "/api/v2/alerts?options=mcp,summary,instances,values,minify", status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes all_alerts_header all_alerts alert_instances_header alert_instances"),
				alertsV2Guard(
					alertsV2Rows{member: "all_alerts", keys: []string{"[0]", "[8]", "[9]", "[11]"},
						items: []string{"hm_plain 0 0 1", "ha_low 1 0 1", "ha_mid 0 1 1", "ha_high 0 1 1"}},
					alertsV2Rows{member: "alert_instances", keys: []string{"[0]", "[4]", "[11]", "[18]", "[19]", "[20]?"},
						items: []string{"hm_plain UNINITIALIZED WHEN null 0 -", "ha_low WARNING WHEN 70 T -", "ha_mid CLEAR WHEN 70 T -",
							"ha_high CLEAR WHEN 70 T -"}}),
			})},
	}
}()

// alertsV2CloserSearchRows are the closer rows of the `alerts` case on the search (searchFamily): `q=ha_low`, an
// alert's name, finds no context, for C does not search the alerts (database/contexts/api_v2_contexts.c:187-267: "We
// don't check alerts anymore", :265); the host is listed (no context pattern, no window: :658), and the texts tested
// are hsig.ctx's: its id, family, title and units, its two charts' ids and their two dimensions' ids, whose names are
// their ids (:196-240), 8.
var alertsV2CloserSearchRows = []v2Req{
	{name: "q-alert", target: "/api/v2/q?q=ha_low", status: "200",
		guard: dashGuard([]dashFact{
			dashKeys("api nodes contexts searches versions agents timings"), dashIs("{}", "contexts"),
			dashIs(`{"strings":8,"char":0,"total":8}`, "searches"), alertsV2Guard(alertsV2Nodes(alertsV2Parent)),
		})},
}

// alertsV2NamedEmit defines the `two-charts` case's chart with a name of its own, never collected: hcl.named, named
// `shown` (C names a chart `<type>.<name>`, database/rrdset-index-name.c:13-16), on a context of its own.
const alertsV2NamedEmit = "CHART hcl.named 'shown' 'title' 'units' 'family' 'hcl.alone' line 1000 1 '' '' ''\n" +
	"DIMENSION b '' absolute 1 1\n"

// alertsV2TwoChartsConf are the `two-charts` case's rules: hcl_named on the named chart, recipient `silent`; two
// templates on the context of the two collected charts, each one value for both (`$a > 0`): hcl_up WARNING, to
// `silent sysadmin`, with a class, a type and a component, and hcl_flat CLEAR, to `silent`; and hcl_off, with a class,
// a type, a component and a recipient of its own, which the case's `enabled alarms` leaves out
// (alertsV2TwoChartsExtra).
const alertsV2TwoChartsConf = `   alarm: hcl_named
      on: hcl.named
    calc: $b
   every: 1s
    warn: $this > 1000
   units: things
      to: silent
    info: b of the named chart

template: hcl_up
      on: hcl.two
    calc: $a > 0
   every: 1s
    warn: $this > 0
   units: things
   class: Utilization
    type: Closer Check
component: Two Charts
      to: silent sysadmin
    info: both charts are up

template: hcl_flat
      on: hcl.two
    calc: $a > 0
   every: 1s
    warn: $this > 5
   units: things
      to: silent
    info: both charts are flat

   alarm: hcl_off
      on: hcl.values
    calc: $a
   every: 1s
    warn: $this > 1000
   units: things
   class: Off Class
    type: Off Kind
component: Off Part
      to: offline
    info: a rule the enabled alarms pattern leaves out
`

// alertsV2TwoChartsExtra is the `two-charts` case's [health] line: hcl_off is left out (health/health_prototypes.c:
// 525-528: its prototype is read, and matches no host).
const alertsV2TwoChartsExtra = "    enabled alarms = !hcl_off *\n"

// alertsV2TwoChartsScenario is the `two-charts` case's plugin: at the `create` release the named chart's definition,
// then hcl.two collected each second in the background (values counting from 1: plugin.Collect, its context its id),
// then, once its definition went out, hcl.values on the context hcl.two, with a module, collected each second at 10:
// both sides create the three charts in one order and collect the two at the same seconds.
func alertsV2TwoChartsScenario() *plugin.Scenario {
	v := &plugin.Values{Chart: "hcl.values", Context: "hcl.two", Module: "closers", Dims: []string{"a"},
		Phases: []plugin.Phase{{Set: map[string]int64{"a": 10}}}}
	return &plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
		{WaitFile: "create"},
		{Emit: alertsV2NamedEmit},
		{Collect: &plugin.Collect{Chart: "hcl.two", Dims: []string{"a"}, N: 590, Background: true}},
		{SleepMs: 200},
		{Values: v},
	}}}}
}

// alertsV2TwoChartsCase is the case `two-charts` of TestAlertsV2.
func alertsV2TwoChartsCase() healthCase {
	return healthCase{conf: alertsV2TwoChartsConf, extra: alertsV2TwoChartsExtra, sc: alertsV2TwoChartsScenario(),
		play: alertsV2PlayTwoCharts}
}

// alertsV2LogTimes is a guard on a transitions view (healthPair.transitions): each of the lines is in it n times.
func alertsV2LogTimes(n int, lines ...string) func(string) error {
	return func(view string) error {
		got := strings.Split(view, "\n")
		for _, l := range lines {
			if k := len(slices.DeleteFunc(slices.Clone(got), func(g string) bool { return g != l })); k != n {
				return fmt.Errorf("%q %d times, want %d", l, k, n)
			}
		}
		return nil
	}
}

// alertsV2StockRulesCase is the case `stock-rules` of TestAlertsV2: the installed rules on (healthCase.stock: each
// side reads its own stock directory, which is the oracle's for a candidate built with the oracle's paths, so both
// read one directory in one kernel order) and no chart, so no alert: what a summary's groupings hold of the rules
// alone.
func alertsV2StockRulesCase() healthCase {
	return healthCase{stock: true, play: alertsV2PlayStockRules}
}

// alertsV2PlayStockRules plays `stock-rules`: its one row.
func alertsV2PlayStockRules(t *testing.T, h *healthPair) {
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2StockRulesRows() {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2StockRulesRows are the `stock-rules` case's rows: `groupings`, the summary with no alert. C reads the rule
// files in the order the kernel lists the directory (libnetdata/paths/paths.c:238, :281: readdir, nothing sorted;
// health/health_prototypes.c:509-514), keeps a prototype per alert name in the order first read with the rules of a
// later file after its first (health/health_prototypes.c:247-259), and counts each name once under its first rule's
// type, component, class and recipient (health_prototypes.c:743-749; api_v2_contexts_alerts.c:384-392, :685), never
// by module: the groupings are the stock directory's names in that order, each `running` 0. The guard holds the 30
// types, the 8 classes and the 6 recipients (a rule without one is `root`'s) in C's order with their counts, and the
// 139 components' count and first twelve.
func alertsV2StockRulesRows() []v2Req {
	group := func(member string, items ...string) alertsV2Rows {
		return alertsV2Rows{member: member, keys: alertsV2RecipientKeys, items: items}
	}
	types := []string{"System 0 0 0 0 0 281", "Web Server 0 0 0 0 0 53", "Kubernetes 0 0 0 0 0 34", "Storage 0 0 0 0 0 118",
		"Power Supply 0 0 0 0 0 7", "Other 0 0 0 0 0 207", "DNS 0 0 0 0 0 3", "Containers 0 0 0 0 0 21", "Database 0 0 0 0 0 204",
		"Certificates 0 0 0 0 0 5", "Netdata 0 0 0 0 0 9", "Messaging 0 0 0 0 0 83", "Data Sharing 0 0 0 0 0 2", "DHCP 0 0 0 0 0 2",
		"SearchEngine 0 0 0 0 0 5", "Network 0 0 0 0 0 17", "Ad Filtering 0 0 0 0 0 1", "ServiceMesh 0 0 0 0 0 12", "GPU 0 0 0 0 0 5",
		"Virtual Machine 0 0 0 0 0 19", "KV Storage 0 0 0 0 0 7", "NetworkDevice 0 0 0 0 0 27", "Switch 0 0 0 0 0 7",
		"Application Server 0 0 0 0 0 7", "Linux 0 0 0 0 0 11", "Message Queue 0 0 0 0 0 3", "Streaming 0 0 0 0 0 2",
		"Cgroups 0 0 0 0 0 4", "Computing 0 0 0 0 0 4", "ethereum_node 0 0 0 0 0 1"}
	components := []string{"Network", "Web log", "AKS", "ScaleIO", "UPS", "UPS device", "TCP endpoint", "API Management",
		"API Server", "Ceph", "Azure VMSS", "Memory"}
	// the components: 139 entries, the first in this order
	first := func(v Value) error {
		m, err := dashMember(v, "alerts_by_component")
		if err != nil {
			return err
		}
		var got []string
		for _, item := range m.Items {
			name, err := dashMember(item, "name")
			if err != nil {
				return err
			}
			got = append(got, name.Text)
		}
		if len(got) != 139 || !slices.Equal(got[:len(components)], components) {
			return fmt.Errorf("alerts_by_component holds %d names, first %q; want 139, first %q", len(got),
				got[:min(len(got), len(components))], components)
		}
		return nil
	}
	return []v2Req{
		{name: "groupings", target: alertsV2Route + "?options=summary,minify", status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("api nodes alerts alerts_by_type alerts_by_component alerts_by_classification alerts_by_recipient " +
					"alerts_by_module timings"),
				alertsV2Guard(alertsV2Nodes(alertsV2Parent), alertsV2Rows{member: "alerts", keys: []string{"nm"}, items: []string{}},
					group("alerts_by_type", types...),
					group("alerts_by_classification", "Errors 0 0 0 0 0 561", "Workload 0 0 0 0 0 108", "Latency 0 0 0 0 0 126",
						"Utilization 0 0 0 0 0 300", "Availability 0 0 0 0 0 54", "Error 0 0 0 0 0 5", "Performance 0 0 0 0 0 5",
						"Backup 0 0 0 0 0 2"),
					group("alerts_by_recipient", "silent 0 0 0 0 0 139", "root 0 0 0 0 0 31", "webmaster 0 0 0 0 0 27",
						"sysadmin 0 0 0 0 0 788", "sitemgr 0 0 0 0 0 13", "dba 0 0 0 0 0 177"),
					alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{}}),
				first,
			})},
	}
}

// alertsV2PlayTwoCharts plays `two-charts`: the three charts, then the green anchors (each template's two alerts
// have their first status on both sides, in the alert log and in /api/v1/alarms?all), then the rows.
func alertsV2PlayTwoCharts(t *testing.T, h *healthPair) {
	h.create(t)
	h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
		alertsV2LogTimes(2, "hcl_up: UNINITIALIZED->WARNING 1 things", "hcl_flat: UNINITIALIZED->CLEAR 1 things"))
	h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
		healthWant(map[string]string{"hcl_up": "WARNING", "hcl_flat": "CLEAR"}))
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2TwoChartsRows() {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2TwoChartsRows are the `two-charts` case's rows. Their guards read what C answers (probes C against C):
//   - a summary entry counts its name's alerts by status (api_v2_contexts_alerts.c:183-208, :210-220): hcl_up's two
//     WARNING (`wr` 2) and hcl_flat's two CLEAR (`cl` 2), each on two charts of one context, by one rule (`in` 2, `nd`
//     1, `cfg` 1); hcl_named's UNINITIALIZED alert counts nowhere;
//   - an alert is `running_silent` only when its recipient is exactly `silent`, one interned text compared by its
//     address (:228-247): `silent sysadmin`, hcl_up's, is not, in its recipient's entry and in its type's, component's,
//     class's and module's; hcl_named and hcl_flat are;
//   - a rule `enabled alarms` leaves out is a prototype still (health/health_prototypes.c:525-528: it matches no
//     host): hcl_off has no alert, and its type, component, class and recipient are `available` 1 with `running` 0
//     (api_v2_contexts_alerts.c:384-392, :685);
//   - an instance's `ch` is its chart's id and `ch_n` its name (:347-350, :704-705): hcl.named's is hcl.shown, the
//     CHART line's name after the chart's type (database/rrdset-index-name.c:13-16); the two collected charts have
//     none of their own (`instances`, `values`, and in an MCP row the name alone, `mcp-values`);
//   - the MCP summary rows hold the same counts (api_v2_contexts_alerts.c:461-497: `mcp-summary`).
func alertsV2TwoChartsRows() []v2Req {
	at := func(query string) string { return alertsV2Route + "?" + query }
	return []v2Req{
		{name: "summary", target: at("options=summary,minify"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent),
				alertsV2Rows{member: "alerts", keys: append([]string{"nm", "cr", "wr", "cl", "er", "in", "nd", "cfg"}, alertsV2SetKeys...),
					items: []string{"hcl_named 0 0 0 0 1 1 1 {hcl.alone} {} {} {} {silent}",
						"hcl_up 0 2 0 0 2 1 1 {hcl.two} {Utilization} {Two_Charts} {Closer_Check} {silent_sysadmin}",
						"hcl_flat 0 0 2 0 2 1 1 {hcl.two} {} {} {} {silent}"}},
				alertsV2Rows{member: "alerts_by_type", keys: alertsV2RecipientKeys, items: []string{"Closer Check 0 2 0 2 0 1", "Off Kind 0 0 0 0 0 1"}},
				alertsV2Rows{member: "alerts_by_component", keys: alertsV2RecipientKeys, items: []string{"Two Charts 0 2 0 2 0 1", "Off Part 0 0 0 0 0 1"}},
				alertsV2Rows{member: "alerts_by_classification", keys: alertsV2RecipientKeys,
					items: []string{"Utilization 0 2 0 2 0 1", "Off Class 0 0 0 0 0 1"}},
				alertsV2Rows{member: "alerts_by_recipient", keys: alertsV2RecipientKeys,
					items: []string{"silent 0 0 2 3 3 2", "silent sysadmin 0 2 0 2 0 1", "offline 0 0 0 0 0 1"}},
				alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"[none] 0 1 1 3 2", "closers 0 1 1 2 1"}})},
		{name: "instances", target: at("options=instances,values,minify"), status: "200",
			guard: alertsV2Guard(alertsV2Rows{member: "alert_instances", keys: []string{"nm", "ch", "ch_n", "st", "v", "tr_v", "t"},
				items: []string{"hcl_named hcl.named hcl.shown UNINITIALIZED null null 0", "hcl_up hcl.two hcl.two WARNING 1 1 T",
					"hcl_flat hcl.two hcl.two CLEAR 1 1 T", "hcl_up hcl.values hcl.values WARNING 1 1 T",
					"hcl_flat hcl.values hcl.values CLEAR 1 1 T"}})},
		{name: "values", target: at("options=values,minify"), status: "200",
			guard: alertsV2Guard(alertsV2Rows{member: "alert_instances", keys: []string{"nm", "ch", "ch_n", "v", "t", "gi?"},
				items: []string{"hcl_named hcl.named hcl.shown null 0 -", "hcl_up hcl.two hcl.two 1 T -", "hcl_flat hcl.two hcl.two 1 T -",
					"hcl_up hcl.values hcl.values 1 T -", "hcl_flat hcl.values hcl.values 1 T -"}})},
		{name: "mcp-values", target: at("options=mcp,values"), status: "200",
			guard: alertsV2Guard(alertsV2Rows{member: "alert_instances", keys: []string{"[0]", "[2]", "[3]", "[4]", "[5]?"},
				items: []string{"hcl_named hcl.shown null 0 -", "hcl_up hcl.two 1 T -", "hcl_flat hcl.two 1 T -", "hcl_up hcl.values 1 T -",
					"hcl_flat hcl.values 1 T -"}})},
		{name: "mcp-summary", target: at("options=mcp,summary,minify"), status: "200",
			guard: alertsV2Guard(alertsV2Rows{member: "all_alerts",
				keys: []string{"[0]", "[2]{}", "[3]{}", "[4]{}", "[5]{}", "[6]{}", "[7]", "[8]", "[9]", "[10]", "[11]", "[12]", "[13]"},
				items: []string{"hcl_named hcl.alone null null null silent 0 0 0 0 1 1 1",
					"hcl_up hcl.two Utilization Two_Charts Closer_Check silent_sysadmin 0 2 0 0 2 1 1",
					"hcl_flat hcl.two null null null silent 0 0 2 0 2 1 1"}})},
	}
}
