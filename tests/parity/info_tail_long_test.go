// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The long pair of `api.v1-info-tail` (milestone 10 commit 14, D251 F6 A): the tail of `/api/v1/info` on agents
// whose localhost has a plugin's charts, before and after C's ANALYTICS thread made its first mutable gather. The
// thread (daemon/analytics.c:655-697) starts once the agent is ready (main.c:1395-1413), gathers what does not change
// after its 10th second and the rest after its 121st (the localhost charts and metrics the viewers see, :471-511, then
// the notification methods, :395-442, :603-635), then every 6 h.
//
// The notification methods are what `<primary plugins dir>/alarm-notify.sh dump_methods` prints, one a line: the
// names of the shell variables `SEND_*` that are `YES` once the script has sourced the stock and then the user
// `health_alarm_notify.conf` (the installed script, :519-526, :903-911). C runs it through `/bin/sh -c` on the plugins
// spawn server (spawn_popen.c:135-141), reads its output with `fgets(line, 200)`, cuts each piece at its newline and
// joins the pieces with `|` (analytics.c:413-437). In the harness the primary plugins directory is the oracle's
// installed one, so both agents run the installed script, steered by the run directory's own configuration
// (infoTailNotifyLines): see RECIPES "Running parity checks", the D251 safety bullet.

// infoTailNotifyLines are the run directory's user `health_alarm_notify.conf` (`<run>/etc`, which the agents export as
// NETDATA_USER_CONFIG_DIR: environment.c:76): plain assignments of literal values and nothing else, as the safety rule
// says (infoTailNotifyLineRe refuses anything else). The methods that depend on the machine are turned off
// (`SEND_EMAIL` is AUTO in the stock file and YES where curl and sendmail exist; `SEND_SMS` and `SEND_AWSSNS` are YES
// where sendsms and aws exist; `SEND_SYSLOG`, NO in the stock file, would need logger: alarm-notify.sh:696-702,
// :858-890), and the script prints exactly the names set to YES here, in bash's order of `${!SEND_@}` (sorted
// bytewise, as the recorded C runs show): two short names, one of 198 bytes (whole: fgets reads 199 bytes and the
// newline fits), one of 199 (199 bytes, then an empty piece holding the newline: `name|`) and one of 250 (199, then
// 51), and one name set to NO, which is not printed.
var infoTailNotifyLines = []string{
	`SEND_EMAIL="NO"`,
	`SEND_AWSSNS="NO"`,
	`SEND_SMS="NO"`,
	`SEND_SYSLOG="NO"`,
	`SEND_PARITY_A="YES"`,
	`SEND_PARITY_B="YES"`,
	`SEND_PARITY_C="NO"`,
	infoTailLongName("SEND_PARITY_L", 198) + `="YES"`,
	infoTailLongName("SEND_PARITY_M", 199) + `="YES"`,
	infoTailLongName("SEND_PARITY_N", 250) + `="YES"`,
}

// infoTailLongName is prefix padded with x to n bytes.
func infoTailLongName(prefix string, n int) string {
	return prefix + strings.Repeat("x", n-len(prefix))
}

// infoTailNotifyLineRe is the one form a line of the run directory's health_alarm_notify.conf may take: a method of
// the check's own (`SEND_PARITY_*`, letters, digits and underscores) given the literal YES or NO, or another method
// `SEND_*` (capitals, digits, underscores) turned off with NO, so no real sender is ever turned on. The script executes
// the file as shell code; nothing else (a command, a substitution, a function, a host, an address, a recipient) may be
// written.
var infoTailNotifyLineRe = regexp.MustCompile(`^SEND_(PARITY_[A-Za-z0-9_]+="(YES|NO)"|[A-Z0-9_]+="NO")$`)

// infoTailNotifyConf is the file's text, or an error naming the first line that is not a plain assignment.
func infoTailNotifyConf(lines []string) (string, error) {
	for _, l := range lines {
		if !infoTailNotifyLineRe.MatchString(l) {
			return "", fmt.Errorf("harness: %q is not a plain SEND_*=YES|NO assignment: the script would execute it", l)
		}
	}
	return strings.Join(lines, "\n") + "\n", nil
}

// infoTailNotifyEnv is the first variable of env (KEY=value) named SEND_*: the script prints an exported one too
// (`${!SEND_@}`, alarm-notify.sh:903-911), and both agents inherit the harness's environment (daemon.go's launch), so
// such a variable would change the methods the row is held to.
func infoTailNotifyEnv(env []string) string {
	for _, e := range env {
		if strings.HasPrefix(e, "SEND_") {
			name, _, _ := strings.Cut(e, "=")
			return name
		}
	}
	return ""
}

// infoTailLongCharts are the fake plugin's charts on localhost, defined in one write, in this order (C lists a chart's
// plugin and module pair once, at its first chart in creation order that the viewers see: api_v1_info.c:5-35;
// rrdset.h:324-329): c1 with no plugin and no module words (the plugin is the file, `difftest.plugin`, the module ""
// : pluginsd_parser.c:522; string.c:326), c2 (two metrics) and c3 (one: its obsolete dimension is none,
// analytics.c:496) of one plugin with two modules, c4 (two metrics) a second chart of c2's pair, c5 hidden, c6
// without a dimension, c7 obsolete (none of the three is seen), c8 with a hidden dimension (seen; the dimension is no
// metric), c9 and c10 whose keys `a:b:c` collide (the key is `plugin:module`, api_v1_info.c:18), c11 a module without
// a plugin word.
const infoTailLongCharts = "CHART parity.c1 '' 'c1' 'units' 'fam' '' line 1000 1 '' '' ''\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"CHART parity.c2 '' 'c2' 'units' 'fam' '' line 1001 1 '' 'parity' 'm1'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"DIMENSION z '' absolute 1 1\n" +
	"CHART parity.c3 '' 'c3' 'units' 'fam' '' line 1002 1 '' 'parity' 'm2'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"DIMENSION o '' absolute 1 1 'obsolete'\n" +
	"CHART parity.c4 '' 'c4' 'units' 'fam' '' line 1003 1 '' 'parity' 'm1'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"DIMENSION w '' absolute 1 1\n" +
	"CHART parity.c5 '' 'c5' 'units' 'fam' '' line 1004 1 'hidden' 'hid' 'h'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"CHART parity.c6 '' 'c6' 'units' 'fam' '' line 1005 1 '' 'nodims' 'n'\n" +
	"CHART parity.c7 '' 'c7' 'units' 'fam' '' line 1006 1 'obsolete' 'obs' 'o'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"CHART parity.c8 '' 'c8' 'units' 'fam' '' line 1007 1 '' 'parity' 'm3'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"DIMENSION y '' absolute 1 1 'hidden'\n" +
	"CHART parity.c9 '' 'c9' 'units' 'fam' '' line 1008 1 '' 'a:b' 'c'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"CHART parity.c10 '' 'c10' 'units' 'fam' '' line 1009 1 '' 'a' 'b:c'\n" +
	"DIMENSION x '' absolute 1 1\n" +
	"CHART parity.c11 '' 'c11' 'units' 'fam' '' line 1010 1 '' '' 'm4'\n" +
	"DIMENSION x '' absolute 1 1\n"

// infoTailLongObsolete is the file the check releases (plugin.Layout.Release) once `child` was asked: the plugin then
// defines c2 again as obsolete (infoTailLongObsoleteC2), so c2's pair is next listed at c4, its first chart the
// viewers still see (the walk reads the flags as they are: api_v1_info.c:14-16; the flag is set before the parser's
// next line, pluginsd_parser.c:528-531, rrdset.c:121-125).
const (
	infoTailLongObsolete   = "c2-obsolete"
	infoTailLongObsoleteC2 = "CHART parity.c2 '' 'c2' 'units' 'fam' '' line 1001 1 'obsolete' 'parity' 'm1'\n"
)

// infoTailLongScenario is the fake plugin's one start: the charts, then a chart of module `live` collected every
// second until the agent stops (the plugin stays), whose second phase begins with c2's obsolete definition.
var infoTailLongScenario = plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Emit: infoTailLongCharts},
	{Values: &plugin.Values{Chart: "parity.live", Module: "live", Dims: []string{"v"},
		Phases: []plugin.Phase{{Set: map[string]int64{"v": 1}, Until: infoTailLongObsolete},
			{Set: map[string]int64{"v": 1}, Emit: infoTailLongObsoleteC2}}}}}}}}

// infoTailBeforeAge is how long after the oracle was ready row `before` asks, and infoTailOldAge how long after the
// later agent was ready row `old` asks: before and past both agents' first mutable gather, the 121st tick of a thread
// started a fraction of a second before the agent answers ready (a tick lands on the next second plus a fixed offset
// of 150 to 500 ms: clocks.c:273-274, :344, :357-358, :395; so 120.15 to 121.5 s after the thread started). `old` is
// asked again for infoTailOldSettle while the answers differ: the script runs for tens of milliseconds on each side.
const (
	infoTailBeforeAge = 105 * time.Second
	infoTailOldAge    = 123 * time.Second
	infoTailOldSettle = 15 * time.Second
)

// infoTailOldFamily is infoTailFamily, asked again each second while the answers differ (the two sides' gathers and
// scripts end at their own times).
var infoTailOldFamily = func() v2Family {
	fam := infoTailFamily
	fam.settle = infoTailOldSettle
	return fam
}()

// infoTailLongCollectors are the pairs C lists for localhost's charts (infoTailLongCharts, then parity.live), and
// infoTailOldCollectors once c2 is obsolete (c2's pair after c3's).
const (
	infoTailLongCollectors = `[{"plugin":"difftest.plugin","module":""},{"plugin":"parity","module":"m1"},` +
		`{"plugin":"parity","module":"m2"},{"plugin":"parity","module":"m3"},{"plugin":"a:b","module":"c"},` +
		`{"plugin":"difftest.plugin","module":"m4"},{"plugin":"difftest.plugin","module":"live"}]`
	infoTailOldCollectors = `[{"plugin":"difftest.plugin","module":""},{"plugin":"parity","module":"m2"},` +
		`{"plugin":"parity","module":"m1"},{"plugin":"parity","module":"m3"},{"plugin":"a:b","module":"c"},` +
		`{"plugin":"difftest.plugin","module":"m4"},{"plugin":"difftest.plugin","module":"live"}]`
)

// The guards of the long pair's rows (the oracle's answer), each on top of infoTailFixed:
var (
	// infoTailChartsGuard (`charts`, 12 s): localhost's pairs listed, nothing gathered yet.
	infoTailChartsGuard = dashGuard(infoTailFixed, dashMembers(nil, "collectors", infoTailLongCollectors,
		"memory-mode", `"dbengine"`, "notification-methods", "null", "charts-count", "0", "metrics-count", "0"))
	// infoTailChildGuard (`child`): the routed child's own pairs and memory mode, the agent's counts.
	infoTailChildGuard = dashGuard(infoTailFixed, dashMembers(nil, "collectors",
		`[{"plugin":"fixture-pusher","module":"corpus"}]`, "memory-mode", `"ram"`, "notification-methods", "null",
		"charts-count", "0", "metrics-count", "0"))
	// infoTailBeforeGuard (`before`): c2 obsolete (the pairs are a live walk), nothing gathered yet (the counts are
	// the gather's, not the request's).
	infoTailBeforeGuard = dashGuard(infoTailFixed, dashMembers(nil, "collectors", infoTailOldCollectors,
		"memory-mode", `"dbengine"`, "notification-methods", "null", "charts-count", "0", "metrics-count", "0"))
	// infoTailOldGuard (`old`): c2 obsolete, and the first mutable gather made: localhost's charts and metrics the
	// viewers see (eight charts, nine metrics), and the methods the script printed, cut and joined as C does.
	infoTailOldGuard = dashGuard(infoTailFixed, dashMembers(nil, "collectors", infoTailOldCollectors,
		"memory-mode", `"dbengine"`, "notification-methods", infoTailOldMethods, "charts-count", "8",
		"metrics-count", "9"))
	// infoTailOldChildGuard (`old-child`): the routed child's pairs with the agent's (localhost's) gathered members.
	infoTailOldChildGuard = dashGuard(infoTailFixed, dashMembers(nil, "collectors",
		`[{"plugin":"fixture-pusher","module":"corpus"}]`, "memory-mode", `"ram"`, "notification-methods",
		infoTailOldMethods, "charts-count", "8", "metrics-count", "9"))
)

// infoTailOldMethods is the stored text of the methods (infoTailNotifyLines' YES names in bytewise order, cut at 199
// bytes a piece and joined with `|`), as the JSON string C prints.
var infoTailOldMethods = func() string {
	l := infoTailLongName("SEND_PARITY_L", 198)
	m := infoTailLongName("SEND_PARITY_M", 199)
	n := infoTailLongName("SEND_PARITY_N", 250)
	return `"` + strings.Join([]string{"SEND_PARITY_A", "SEND_PARITY_B", l, m, "", n[:199], n[199:]}, "|") + `"`
}()

// infoTailLongChild is the fixture child's routed `/api/v1/info`.
var infoTailLongChild = "/host/" + childHost.Hostname + "/api/v1/info"

// infoTailLongRow is one row of the long pair: its request (with the oracle's guard) and the family it is compared by.
type infoTailLongRow struct {
	req v2Req
	fam v2Family
}

// infoTailLongRows are the long pair's rows, in the order TestInfoV1TailLong asks them.
var infoTailLongRows = []infoTailLongRow{
	{v2Req{name: "charts", target: "/api/v1/info", status: "200", guard: infoTailChartsGuard}, infoTailFamily},
	{v2Req{name: "child", target: infoTailLongChild, status: "200", guard: infoTailChildGuard}, infoTailFamily},
	{v2Req{name: "before", target: "/api/v1/info", status: "200", guard: infoTailBeforeGuard}, infoTailFamily},
	{v2Req{name: "old", target: "/api/v1/info", status: "200", guard: infoTailOldGuard}, infoTailOldFamily},
	{v2Req{name: "old-child", target: infoTailLongChild, status: "200", guard: infoTailOldChildGuard},
		infoTailOldFamily},
}

// TestInfoV1TailLong (check `api.v1-info-tail`, its long pair; PARITY_LONG, about 135 s): the fake plugin's charts on
// localhost and the run directory's notifier configuration (infoTailNotifyLines) on both sides; then `/api/v1/info`
// at 12 s (`charts`), the fixture child's routed answer once it streamed (`child`), then c2 made obsolete,
// `/api/v1/info` 105 s after the oracle was ready (`before`), and both again 123 s after the later agent was ready
// (`old`, `old-child`): infoTailLongRows.
func TestInfoV1TailLong(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (the pair lives past the ANALYTICS thread's 121st second)")
	}
	if name := infoTailNotifyEnv(os.Environ()); name != "" {
		t.Fatalf("harness: %s is in the environment: both agents would hand it to the notifier's dump", name)
	}
	conf, err := infoTailNotifyConf(infoTailNotifyLines)
	if err != nil {
		t.Fatal(err)
	}
	engine, err := plugin.Engine()
	if err != nil {
		t.Fatal(err)
	}
	// prepare runs for each side right before its start, and the oracle's start returns once it is ready
	var layouts []plugin.Layout
	var prepared []time.Time
	p := startPairWith(t, pluginsOptions(1, nil, nil, ""), parentIdentity, binaries(t), [2]string{},
		[2]Role{Oracle, Candidate}, func(t *testing.T, runDir string) {
			prepared = append(prepared, time.Now())
			etc := filepath.Join(runDir, "etc")
			if err := os.MkdirAll(etc, 0o755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(etc, "health_alarm_notify.conf"), []byte(conf), 0o644); err != nil {
				t.Fatal(err)
			}
			l, err := plugin.Install(runDir, engine, infoTailLongScenario)
			if err != nil {
				t.Fatal(err)
			}
			layouts = append(layouts, l)
		})
	ready, oracleReady := time.Now(), prepared[1]
	ask := func(i int) {
		row := infoTailLongRows[i]
		t.Run(row.req.name, func(t *testing.T) { compareV2(t, p, row.req, row.fam) })
	}
	time.Sleep(time.Until(ready.Add(infoTailAge)))
	ask(0)
	dashChild(t, p, dashBase())
	ask(1)
	for _, l := range layouts {
		if err := l.Release(infoTailLongObsolete); err != nil {
			t.Fatal(err)
		}
	}
	if age := time.Since(p.Oracle.LaunchStartedAt); age > 115*time.Second {
		t.Errorf("harness: charts and child asked until %s after the oracle's launch, past the 12-115 s phase", age)
	}
	time.Sleep(time.Until(oracleReady.Add(infoTailBeforeAge)))
	ask(2)
	if age := time.Since(oracleReady); age > 115*time.Second {
		t.Errorf("harness: before asked %s after the oracle was ready, past the 12-115 s phase", age)
	}
	time.Sleep(time.Until(ready.Add(infoTailOldAge)))
	ask(3)
	ask(4)
}
