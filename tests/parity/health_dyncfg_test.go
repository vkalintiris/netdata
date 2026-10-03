// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net/http"
	"strings"
	"testing"
	"time"
)

// TestHealthDynCfg (check `health.dyncfg`, M9 commit 0, D183): health's DynCfg nodes for the config check's user file
// (health_dyncfg.c:886-943): the tree under /health, each job's `get` (its rules as JSON, the source and the notifier
// under the side's run directory), the template's schema and a job's, as the admin; then, both stopped, the DynCfg
// records.
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
	runHealthCases(t, map[string]healthCase{
		"user": {
			conf:  healthCfgConf,
			extra: healthCfgExtra,
			play: func(t *testing.T, h *healthPair) {
				config := func(query string) func(i int) string {
					return func(i int) string { return h.config(i, query, started) }
				}
				var jobs []string
				for _, n := range names {
					jobs = append(jobs, `"`+healthJobPrefix+":"+n+`":`)
				}
				h.compareNow(t, "the DynCfg tree under /health", config("action=tree&path=/health"), ok(jobs...))
				for _, n := range names {
					q := "action=get&id=" + healthJobPrefix + ":" + n
					h.compareNow(t, q, config(q), ok(n, "{run}/etc/health.d/"+healthConfFile))
				}
				for _, id := range []string{healthJobPrefix, healthJobPrefix + ":hc_alarm"} {
					q := "action=schema&id=" + id
					h.compareNow(t, q, config(q), ok(`"jsonSchema"`))
				}
			},
			after: func(t *testing.T, h *healthPair) {
				h.compareLines(t, "the DynCfg records", func(i int) []string { return dcRecords(t, h.p.Each()[i].Daemon) },
					func([]string) error { return nil })
			},
		},
	})
}
