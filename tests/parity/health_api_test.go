// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"
)

// healthAPIConf are three alerts on the last value of `a`, with thresholds apart: at 70 one is WARNING, two CLEAR.
// All three read the value at the same pass, so their entries are logged in one order.
const healthAPIConf = `# the endpoint check's alerts on hsig.values
 alarm: ha_low
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 50
  crit: $this > 500
 units: things
  info: a above 50

 alarm: ha_mid
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 100
 units: things
  info: a above 100

 alarm: ha_high
    on: hsig.values
  calc: $a
 every: 1s
  warn: $this > 200
 units: things
  info: a above 200
`

// The clock values of /api/v1/alarm_variables (health/rrdvar.c:159-323): the second of the read (`after` is the
// second before `before` and `now`, rrdvar.c:179-181: both print as their distance from `after`) and the seconds of
// the last collection, the chart's and each dimension's; and of a variable's trace (/api/v1/variable,
// health_variable.c:547-564) when the variable is the wall clock (:356-362), the chart's last collection (:420-426)
// or a dimension's (:102-104).
var (
	healthVarsWindowRe = regexp.MustCompile(`"after":(\d+),(\s*)"before":(\d+),(\s*)"now":(\d+)`)
	healthVarsRe       = regexp.MustCompile(`"([^"]*last_collected_t)":(\d+)`)
	healthTraceRe      = regexp.MustCompile(`^(\{\s*"variable":"(?:now|(?:[^"]*_)?last_collected_t)",(?s:.*?)"found":true,\s*"value":)(\d+)`)
)

// healthVars masks the clock values of an /api/v1/alarm_variables body: the read's second as `T` (`before` and `now`
// by their distance from `after`), a last collection's second as `T` when set (0, never collected, is kept). It
// returns the seconds it masked, in the body's order, for the bound beside the mask (healthClocksNear).
func healthVars(body string) (string, []int64) {
	var clocks []int64
	body = healthVarsWindowRe.ReplaceAllStringFunc(body, func(m string) string {
		g := healthVarsWindowRe.FindStringSubmatch(m)
		after, _ := strconv.ParseInt(g[1], 10, 64)
		before, _ := strconv.ParseInt(g[3], 10, 64)
		now, _ := strconv.ParseInt(g[5], 10, 64)
		clocks = append(clocks, after)
		return fmt.Sprintf(`"after":T,%s"before":T%+d,%s"now":T%+d`, g[2], before-after, g[4], now-after)
	})
	body = healthVarsRe.ReplaceAllStringFunc(body, func(m string) string {
		g := healthVarsRe.FindStringSubmatch(m)
		if g[2] == "0" {
			return m
		}
		v, _ := strconv.ParseInt(g[2], 10, 64)
		clocks = append(clocks, v)
		return `"` + g[1] + `":T`
	})
	return body, clocks
}

// healthTrace masks the value of a variable's trace when the variable is a clock's second: `T` when set, 0 kept. It
// returns the second it masked.
func healthTrace(body string) (string, []int64) {
	g := healthTraceRe.FindStringSubmatch(body)
	if g == nil || g[2] == "0" {
		return body, nil
	}
	v, _ := strconv.ParseInt(g[2], 10, 64)
	return g[1] + "T" + strings.TrimPrefix(body, g[0]), []int64{v}
}

// vars is side i's view of an /api/v1/alarm_variables answer: the status, the content type and the body's bytes with
// the clock values masked (healthVars); the masked seconds are kept for the bound (near).
func (h *healthPair) vars(i int, path string) string {
	r := healthGet(h.p.Each()[i].Daemon, path)
	body, clocks := healthVars(string(r.Body))
	return h.keepClocks(i, healthView(r, body), clocks)
}

// trace is side i's view of a variable's trace, /api/v1/variable or /api/v3/variable: the status, the content type and
// the body's bytes, a clock's second masked (healthTrace) and kept for the bound (near).
func (h *healthPair) trace(i int, path string) string {
	r := healthGet(h.p.Each()[i].Daemon, path)
	body, clocks := healthTrace(string(r.Body))
	return h.keepClocks(i, healthView(r, body), clocks)
}

// healthHolds is a guard on a view: the answer's status is 200 and the view holds each of the parts.
func healthHolds(parts ...string) func(string) error {
	return func(view string) error {
		if !strings.HasPrefix(view, fmt.Sprintf("HTTP %d, ", http.StatusOK)) {
			return fmt.Errorf("answered %q", strings.SplitN(view, "\n", 2)[0])
		}
		for _, p := range parts {
			if !strings.Contains(view, p) {
				return fmt.Errorf("the answer does not hold %q", p)
			}
		}
		return nil
	}
}

// healthIs is a guard on a view: the answer's status is 200 and its rendered body is exactly `body`.
func healthIs(body string) func(string) error {
	return func(view string) error {
		if err := healthHolds()(view); err != nil {
			return err
		}
		if got := healthBody(view); got != body {
			return fmt.Errorf("the answer is %s, want %s", got, body)
		}
		return nil
	}
}

var healthHashRe = regexp.MustCompile(`"config_hash_id": "([0-9a-f-]{36})",\s*"name": "ha_low"`)

// TestHealthAPI (check `health.api`, M9 commit 0, D183): the v1 alert endpoints and the alert members of the endpoints
// the Rust agent already serves, at a settled state. Each answer's status, content type and body are compared (the
// hand-built bodies' bytes, healthNorm.json); the oracle must answer each with 200. Cases:
//   - `endpoints`: one alert WARNING, two CLEAR, through every endpoint;
//   - `log` (M9 commit 5, D205 F1): the quiet rule set, whose entries carry no notification: `/api/v1/alarm_log` whole
//     and with its two parameters, then `/api/v2/alert_config` and `/api/v3/alert_config` for a template, an alarm and
//     a rule with a lookup, and for what is no rule's hash (healthLogAsks); `log-off`: the alert log and a rule's
//     configuration with health off;
//   - `linked` (D194 F1: `health.link`'s comparison before the linking commit): the config checks' user file with a
//     chart its rules match (`hcfg.values`, context `hcfg.ctx`): `/api/v1/alarms?all` once the alerts are linked and
//     evaluated, each alert with its rule's fields and its config hash. hc_alarm, hc_tmpl, hc_bad and hc_chain's first
//     rule are linked; hc_off is not (`[health] enabled alarms` leaves it out), and hc_chain's second rule matches no
//     chart.
func TestHealthAPI(t *testing.T) {
	want := map[string]string{"ha_low": "WARNING", "ha_mid": "CLEAR", "ha_high": "CLEAR"}
	holds := healthHolds
	runHealthCases(t, map[string]healthCase{
		"endpoints": {
			conf: healthAPIConf,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70}),
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				h.waitOracle(t, "the chart's alerts", func() (string, error) {
					v := h.get(0, "/api/v1/alarms?all")
					return v, healthAll("CLEAR", "ha_low", "ha_mid", "ha_high")(v)
				})
				h.release(t, "p1", 1, healthCalcHold)
				all := h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") }, healthWant(want))
				hash := healthHashRe.FindStringSubmatch(all)
				if hash == nil {
					t.Fatalf("oracle: /api/v1/alarms?all names no config hash for ha_low")
				}
				get := func(path string) func(i int) string { return func(i int) string { return h.get(i, path) } }
				for _, e := range []struct {
					path  string
					guard func(string) error
				}{
					{"/api/v1/alarms", holds(`"name": "ha_low"`)},
					{"/api/v1/alarms_values?all", holds(`"hsig.values.ha_low"`, `"hsig.values.ha_high"`)},
					{"/api/v1/alarm_count", holds("[1]")},
					{"/api/v1/alarm_count?status=CLEAR", holds("[2]")},
					{"/api/v1/alarm_log", holds(`"name":"ha_low"`, `"status":"WARNING"`)},
					{"/api/v2/alert_config?config=" + hash[1], holds("ha_low")},
				} {
					h.compareNow(t, e.path, get(e.path), e.guard)
				}
				// the chart's variables: the read's second and the collection times are the clock's
				vars := "/api/v1/alarm_variables?chart=hsig.values"
				h.compareNow(t, vars, func(i int) string { return h.vars(i, vars) },
					holds(`"chart":"hsig.values"`, `"ha_low":{`, `"after":T,`, `"before":T+1,`, `"now":T+1,`))
				one := "/api/v1/variable?chart=hsig.values&variable=a"
				h.compareNow(t, one, func(i int) string { return h.trace(i, one) },
					holds(`"variable":"a"`, `"found":true`, `"value":70`))
				// after=<id>: the entries logged after it, each side's own first id
				h.compareNow(t, "/api/v1/alarm_log?after=<the first unique id>", func(i int) string {
					return h.get(i, fmt.Sprintf("/api/v1/alarm_log?after=%d", h.n[i].uBase+1))
				}, holds(`"unique_id":u+2`))
				// the alert members of the endpoints that answer without health too
				h.compareNow(t, "/api/v1/info's alarms", func(i int) string { return h.member(i, "/api/v1/info", "alarms") },
					holds(`"warning":1`))
				h.compareNow(t, "/api/v1/chart's alarms", func(i int) string {
					return h.plainMember(i, "/api/v1/chart?chart=hsig.values", "alarms")
				}, holds(`"ha_low":{"id":"ha_low","status":"WARNING","units":"things","duration":1}`))
			},
		},
		"log": {
			conf: healthQuietConf,
			sc:   healthQuietScenario(),
			play: func(t *testing.T, h *healthPair) {
				healthPlayQuiet(t, h)
				healthLogAsks(t, h)
			},
		},
		"log-off": {
			conf: healthQuietConf,
			off:  true,
			sc:   healthQuietScenario(),
			play: func(t *testing.T, h *healthPair) {
				h.createChart(t)
				// an agent that ran health though it is off would have logged its alerts by now
				time.Sleep(healthCalcHold)
				// a host whose health never ran has no limit for its log yet (the query's LIMIT is the host's copy of
				// `in memory max health log entries`, set at its first pass: sqlite_health.c:1074, health_event_loop.c:266):
				// no entry, in the array C opens before it asks for any
				empty := healthIs(healthLogEmpty)
				for _, path := range []string{"/api/v1/alarm_log", "/api/v1/alarm_log?after=1&chart=" + healthQuietChart} {
					h.compareNow(t, path, func(i int) string { return h.get(i, path) }, empty)
				}
				// no rule is read with health off: no hash is known
				config := "/api/v2/alert_config?config=" + healthNoHash
				h.compareNow(t, config, func(i int) string { return h.plain(i, config) }, healthAnswer(http.StatusNotFound, "Config is not found."))
			},
		},
		"linked": {
			conf:  healthCfgConf,
			extra: healthCfgExtra,
			sc:    healthValues("hcfg.values", "hcfg.ctx", []string{"a"}, map[string]int64{"a": 10}),
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
					healthWant(map[string]string{"hc_alarm": "CLEAR", "hc_tmpl": "CLEAR", "hc_chain": "CLEAR", "hc_bad": "CLEAR"}))
			},
		},
	})
}

// member is side i's view of one top-level member of a JSON answer, as the agent's order and values give it (the
// ids rebased, the clock masked).
func (h *healthPair) member(i int, path, name string) string {
	return h.memberAs(i, path, name, h.n[i].json)
}

// plainMember is member for a member that holds no id and no clock: its values as they are. A chart's `alarms` is
// one: its `duration` is the rule's update frequency (rrdset2json.c:93), which the clock's masks would take for a
// time.
func (h *healthPair) plainMember(i int, path, name string) string {
	return h.memberAs(i, path, name, func(member string) string { return member })
}

func (h *healthPair) memberAs(i int, path, name string, render func(string) string) string {
	r := healthGet(h.p.Each()[i].Daemon, path)
	v, err := ParseJSON(r.Body)
	if err != nil {
		return healthView(r, err.Error()+": "+string(r.Body))
	}
	for _, m := range v.Members {
		if m.Key == name {
			return healthView(r, render(m.Value.String()))
		}
	}
	return healthView(r, "no member "+name)
}

// healthNoHash is a UUID that is no rule's hash.
const healthNoHash = "5a1e0000-0000-4000-8000-00000000dead"

// healthAnswer is a guard on a view of an answer that is no 200: its status, `text/plain; charset=utf-8` and its text.
func healthAnswer(status int, text string) func(string) error {
	return func(view string) error {
		if want := fmt.Sprintf("HTTP %d, text/plain; charset=utf-8\n%s", status, text); view != want {
			return fmt.Errorf("answered %q, want %q", view, want)
		}
		return nil
	}
}

// healthLogEmpty is the body of an alert log without an entry (the array C opens, closed at once).
const healthLogEmpty = "\n    []\n"

// healthTimes is a guard on a view: it holds the part n times.
func healthTimes(part string, n int) func(string) error {
	return func(view string) error {
		if got := strings.Count(view, part); got != n {
			return fmt.Errorf("the answer holds %q %d times, want %d", part, got, n)
		}
		return nil
	}
}

// healthLacks is a guard on a view: it holds none of the parts.
func healthLacks(parts ...string) func(string) error {
	return func(view string) error {
		for _, p := range parts {
			if strings.Contains(view, p) {
				return fmt.Errorf("the answer holds %q", p)
			}
		}
		return nil
	}
}

// healthLogAsks compares the alert log's endpoint and the rules' once the quiet phases played (healthPlayQuiet): 26
// entries, none with a notification. The alert log comes first: a candidate that serves no alert log fails here.
//   - `/api/v1/alarm_log`: every member of every entry, newest first (sqlite_health.c:1063-1218);
//   - `after=<id>`: the entries logged after it, each side's own id. C reads the number with strtoul(…, 0)
//     (web/api/v1/api_v1_alarms.c:93): `0x…` is hexadecimal and a leading 0 octal, so the same id in each form gives
//     the same entries, where a decimal reading of them would give 0 and every entry. The last `after` wins;
//   - `chart=<id>`: the entries of the chart's alerts; of a chart without alerts, none;
//   - `/api/v2/alert_config` and `/api/v3/alert_config` with `config=<hash>` (api_v2_contexts_alert_config.c:8-140):
//     a template (`name` null and its own name under `on`), an alarm, a rule with a lookup (the `db` object); a hash no
//     rule has (404), a text that is no hash (C parses it and ignores the failure: no row, 404), none (400).
func healthLogAsks(t *testing.T, h *healthPair) {
	t.Helper()
	const entries = healthQuietEntries + 2
	get := func(path string) func(i int) string { return func(i int) string { return h.get(i, path) } }
	// settled: the nine entries no later one replaced before the scan took them are processed (hq_look's third link
	// and the eight statuses; hq_delay's first CLEAR 6 s after it was logged)
	h.compareNow(t, "/api/v1/alarm_log", get("/api/v1/alarm_log"), healthBoth(healthLogEntries(entries),
		healthTimes(`"processed":true,`, 9), healthHolds(`"name":"hq_undef"`, `"status":"UNDEFINED"`, `"name":"hq_gone"`,
			`"status":"REMOVED"`, `"class":"Utilization"`, `"class":"Unknown"`, `"exec_run":0,`, `"delay":6,`,
			`"command":"sudo {run}/etc/edit-config health.d/parity.conf=`),
		healthLacks(`"status":"WARNING"`, `"status":"CRITICAL"`, `"exec_run":T`)))
	// the entries after the 20th: the six newest
	const after = 20
	newest := healthBoth(healthLogEntries(entries-after), healthHolds(fmt.Sprintf(`"unique_id":u+%d,`, after+1)),
		healthLacks(fmt.Sprintf(`"unique_id":u+%d,`, after)))
	for _, form := range []struct{ what, format string }{
		{"/api/v1/alarm_log?after=<the 20th unique id>", "after=%d"},
		{"/api/v1/alarm_log?after=<the same, hexadecimal>", "after=0x%x"},
		{"/api/v1/alarm_log?after=<the same, octal>", "after=0%o"},
		{"/api/v1/alarm_log?after=1&after=<the same>", "after=1&after=%d"},
		{"/api/v1/alarm_log?after=<the same> and an empty one", "after=%d&after=&=3&after"},
	} {
		h.compareNow(t, form.what, func(i int) string {
			return h.get(i, "/api/v1/alarm_log?"+fmt.Sprintf(form.format, h.n[i].uBase+after))
		}, newest)
	}
	h.compareNow(t, "/api/v1/alarm_log?chart="+healthQuietChart, get("/api/v1/alarm_log?chart="+healthQuietChart),
		healthBoth(healthLogEntries(entries-4), healthLacks(`"name":"hq_gone"`)))
	// the obsolete chart's alert: its three links and its removal
	h.compareNow(t, "/api/v1/alarm_log?chart=hq.gone", get("/api/v1/alarm_log?chart=hq.gone"),
		healthBoth(healthLogEntries(4), healthHolds(`"name":"hq_gone"`, `"chart":"hq.gone"`)))
	h.compareNow(t, "/api/v1/alarm_log?chart=hq.none", get("/api/v1/alarm_log?chart=hq.none"), healthIs(healthLogEmpty))
	h.compareNow(t, "/api/v1/alarm_log?chart=hq.gone&after=<the 20th unique id>", func(i int) string {
		return h.get(i, fmt.Sprintf("/api/v1/alarm_log?chart=hq.gone&after=%d", h.n[i].uBase+after))
	}, healthIs(healthLogEmpty))

	// the rules, by the hashes the oracle's log names (a rule's hash is of its fields: the same on both sides)
	log, err := h.entriesAs(h.n[0], 0, "/api/v1/alarm_log")
	if err != nil {
		t.Fatalf("oracle: %v", err)
	}
	plain := func(path string) func(i int) string { return func(i int) string { return h.plain(i, path) } }
	for _, version := range []string{"v2", "v3"} {
		config := "/api/" + version + "/alert_config"
		for _, rule := range []struct {
			name  string
			guard func(string) error
		}{
			// a template's `name` is null and `on` its own name, not its context (api_v2_contexts_alert_config.c:17-26)
			{"hq_tmpl", healthBoth(healthHolds(`"name":null,`, `"type":"template"`, `"on":"hq_tmpl"`, `"class":"Utilization"`), healthLacks(`"db":`))},
			{"hq_undef", healthBoth(healthHolds(`"name":"hq_undef"`, `"type":"alarm"`, `"on":"hq.values"`), healthLacks(`"db":`))},
			{"hq_look", healthHolds(`"name":"hq_look"`, `"db":{`, `"after":-5`)},
		} {
			hash := healthHashOf(log, rule.name)
			if hash == "" {
				t.Fatalf("oracle: /api/v1/alarm_log names no config hash for %s", rule.name)
			}
			h.compareNow(t, config+"?config=<the hash of "+rule.name+">", plain(config+"?config="+hash),
				healthBoth(rule.guard, healthHolds(`"config_hash_id":"`+hash+`"`)))
		}
		hash := healthHashOf(log, "hq_undef")
		missing := healthAnswer(http.StatusNotFound, "Config is not found.")
		for _, e := range []struct {
			what, query string
			guard       func(string) error
		}{
			{"<a hash no rule has>", "?config=" + healthNoHash, missing},
			{"<no hash>", "?config=nonsense", missing},
			// C's parser takes a hash in upper case and one without its dashes
			{"<the hash of hq_undef, upper case>", "?config=" + strings.ToUpper(hash), healthHolds(`"name":"hq_undef"`)},
			{"<the hash of hq_undef, without dashes>", "?config=" + strings.ReplaceAll(hash, "-", ""), healthHolds(`"name":"hq_undef"`)},
			{"<a hash no rule has>&config=<the hash of hq_undef>", "?config=" + healthNoHash + "&config=" + hash, healthHolds(`"name":"hq_undef"`)},
			{"", "", healthAnswer(http.StatusBadRequest, "A config hash ID is required. Add ?config=UUID query param")},
			{"", "?config=", healthAnswer(http.StatusBadRequest, "A config hash ID is required. Add ?config=UUID query param")},
			{"", "?CONFIG=" + hash, healthAnswer(http.StatusBadRequest, "A config hash ID is required. Add ?config=UUID query param")},
		} {
			what := config + e.query
			if e.what != "" {
				what = config + "?config=" + e.what
			}
			h.compareNow(t, what, plain(config+e.query), e.guard)
		}
	}
}
