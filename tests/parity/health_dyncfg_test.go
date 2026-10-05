// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"net/http"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"
)

// healthJobPrefix is the id of health's DynCfg template; a job's id adds `:<alert name>` (health_dyncfg.c:886-943).
const healthJobPrefix = "health:alert:prototype"

var healthJobRe = regexp.MustCompile(`"(` + healthJobPrefix + `[^"]*)":`)

// config is side i's view of a DynCfg answer (`/api/v1/config?<query>`, as the admin: health's nodes are not open to
// anonymous clients, dyncfg.c:395-399): the status, the content type and the body with the side's directories
// replaced and its wall-clock values ranked (dcClock).
func (h *healthPair) config(i int, query string, since time.Time) string {
	r := healthGet(h.p.Each()[i].Daemon, "/api/v1/config?"+query, dcAdmin)
	body := dcClock([]string{h.n[i].paths(string(r.Body))}, since.Add(-time.Minute), time.Now().Add(time.Minute))[0]
	return healthView(r, body)
}

// healthJobs are the ids of health's DynCfg nodes a tree view lists, in the tree's order.
func healthJobs(view string) []string {
	var out []string
	for _, m := range healthJobRe.FindAllStringSubmatch(healthBody(view), -1) {
		out = append(out, m[1])
	}
	return out
}

// TestHealthDynCfg (check `health.dyncfg`, M9 commit 0, D183; `stock-tree` since D189): health's DynCfg nodes
// (health_dyncfg.c:886-943), as the admin. `user`, for the config check's user file: the tree under /health, each
// job's `get` (its rules as JSON, the source and the notifier under the side's run directory), the template's schema
// and a job's; then, both stopped, the DynCfg records. `stock-tree`: the tree under /health of the installed stock
// rules (one job per alert name, in the tree's order). The cases that send commands to the nodes are
// healthDynCfgCases' (health_dyncfg_cases_test.go; M9 commit 8, D212).
func TestHealthDynCfg(t *testing.T) {
	started := time.Now()
	names := []string{"hc_alarm", "hc_tmpl", "hc_chain", "hc_bad", "hc_off"}
	ok := func(parts ...string) func(string) error {
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
	config := func(h *healthPair, query string) func(i int) string {
		return func(i int) string { return h.config(i, query, started) }
	}
	records := func(t *testing.T, h *healthPair) {
		h.compareLines(t, "the DynCfg records", func(i int) []string { return dcRecords(t, h.p.Each()[i].Daemon) },
			func([]string) error { return nil })
	}
	cases := healthDynCfgCases()
	maps.Copy(cases, map[string]healthCase{
		"user": {
			conf:  healthCfgConf,
			extra: healthCfgExtra,
			play: func(t *testing.T, h *healthPair) {
				var jobs []string
				for _, n := range names {
					jobs = append(jobs, `"`+healthJobPrefix+":"+n+`":`)
				}
				h.compareNow(t, "the DynCfg tree under /health", config(h, "action=tree&path=/health"), ok(jobs...))
				for _, n := range names {
					q := "action=get&id=" + healthJobPrefix + ":" + n
					h.compareNow(t, q, config(h, q), ok(n, "{run}/etc/health.d/"+healthConfFile))
				}
				for _, id := range []string{healthJobPrefix, healthJobPrefix + ":hc_alarm"} {
					q := "action=schema&id=" + id
					h.compareNow(t, q, config(h, q), ok(`"jsonSchema"`))
				}
			},
			after: records,
		},
		"stock-tree": {
			stock: true,
			play: func(t *testing.T, h *healthPair) {
				h.compareNow(t, "the DynCfg tree under /health", config(h, "action=tree&path=/health"),
					func(view string) error {
						if err := ok()(view); err != nil {
							return err
						}
						// the template and a job per alert name of the installed rules
						if got := healthJobs(view); !slices.Contains(got, healthJobPrefix) || len(got) < 100+1 {
							return fmt.Errorf("%d health nodes, want the template and at least 100 jobs: %v", len(got), got)
						}
						return nil
					})
			},
			after: records,
		},
	})
	runHealthCases(t, cases)
}
