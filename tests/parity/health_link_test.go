// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"testing"
)

// TestHealthLink (check `health.link`, D189: the config check's linked-alerts comparison, as M9 commit 0 had it):
// the config check's user file with a chart its rules match (`hcfg.values`, context `hcfg.ctx`, created at the same
// second on both sides). Compared: `/api/v1/alarms?all`, once the alerts are linked and evaluated: each alert with
// its rule's fields and its config hash. hc_alarm, hc_tmpl, hc_bad and hc_chain's first rule are linked; hc_off is
// not (`[health] enabled alarms` leaves it out), and hc_chain's second rule matches no chart.
func TestHealthLink(t *testing.T) {
	runHealthCases(t, map[string]healthCase{
		"user": {
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
