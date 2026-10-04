// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"testing"
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
