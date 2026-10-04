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

// The clock values of /api/v1/alarm_variables (health/rrdvar.c:159-323): the window of the read (`after` is the
// second before `before`, rrdvar.c:179-180: `before` prints as its distance from `after`) and the times of the last
// collection.
var (
	healthVarsWindowRe = regexp.MustCompile(`"after":(\d+),(\s*)"before":(\d+)`)
	healthVarsRe       = regexp.MustCompile(`"([a-z0-9_]*last_collected_t)":\d+`)
)

// healthVars masks the clock values of an /api/v1/alarm_variables view.
func healthVars(view string) string {
	view = healthVarsWindowRe.ReplaceAllStringFunc(view, func(m string) string {
		g := healthVarsWindowRe.FindStringSubmatch(m)
		after, _ := strconv.ParseInt(g[1], 10, 64)
		before, _ := strconv.ParseInt(g[3], 10, 64)
		return fmt.Sprintf(`"after":T,%s"before":T%+d`, g[2], before-after)
	})
	return healthVarsRe.ReplaceAllString(view, `"${1}":T`)
}

var healthHashRe = regexp.MustCompile(`"config_hash_id": "([0-9a-f-]{36})",\s*"name": "ha_low"`)

// TestHealthAPI (check `health.api`, M9 commit 0, D183): the v1 alert endpoints and the alert members of the endpoints
// the Rust agent already serves, at a settled state: one alert WARNING, two CLEAR. Each answer's status, content type
// and body are compared (the hand-built bodies' bytes, healthNorm.json); the oracle must answer each with 200.
func TestHealthAPI(t *testing.T) {
	want := map[string]string{"ha_low": "WARNING", "ha_mid": "CLEAR", "ha_high": "CLEAR"}
	ok := func(view string) error {
		if !strings.HasPrefix(view, fmt.Sprintf("HTTP %d, ", http.StatusOK)) {
			return fmt.Errorf("answered %q", strings.SplitN(view, "\n", 2)[0])
		}
		return nil
	}
	holds := func(parts ...string) func(string) error {
		return func(view string) error {
			if err := ok(view); err != nil {
				return err
			}
			for _, p := range parts {
				if !strings.Contains(view, p) {
					return fmt.Errorf("the answer does not hold %q", p)
				}
			}
			return nil
		}
	}
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
				// the chart's variables: the lookup window and the collection times are the clock's
				vars := "/api/v1/alarm_variables?chart=hsig.values"
				h.compareNow(t, vars, func(i int) string { return healthVars(h.get(i, vars)) },
					holds(`"chart":"hsig.values"`, `"ha_low":{`, `"after":T,`, `"before":T+1,`))
				one := "/api/v1/variable?chart=hsig.values&variable=a"
				h.compareNow(t, one, get(one), holds(`"variable":"a"`, `"found":true`, `"value":70`))
				// after=<id>: the entries logged after it, each side's own first id
				h.compareNow(t, "/api/v1/alarm_log?after=<the first unique id>", func(i int) string {
					return h.get(i, fmt.Sprintf("/api/v1/alarm_log?after=%d", h.n[i].uBase+1))
				}, holds(`"unique_id":u+2`))
				// the alert members of the endpoints that answer without health too
				h.compareNow(t, "/api/v1/info's alarms", func(i int) string { return h.member(i, "/api/v1/info", "alarms") },
					holds(`"warning":1`))
				h.compareNow(t, "/api/v1/chart's alarms", func(i int) string {
					return h.member(i, "/api/v1/chart?chart=hsig.values", "alarms")
				}, holds("ha_low"))
			},
		},
	})
}

// member is side i's view of one top-level member of a JSON answer, as the agent's order and values give it (the
// ids rebased, the clock masked).
func (h *healthPair) member(i int, path, name string) string {
	r := healthGet(h.p.Each()[i].Daemon, path)
	v, err := ParseJSON(r.Body)
	if err != nil {
		return healthView(r, err.Error()+": "+string(r.Body))
	}
	for _, m := range v.Members {
		if m.Key == name {
			return healthView(r, h.n[i].json(m.Value.String()))
		}
	}
	return healthView(r, "no member "+name)
}
