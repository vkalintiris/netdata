// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import "testing"

// TestDashNorm (check `dash.norm`) pins the dashboard checks' masks and normalisers on answers recorded from C, with
// no agent started: each family's cases live beside its check (dash_norm_<family>_test.go).
func TestDashNorm(t *testing.T) {
	t.Run("contexts", testDashNormContexts)
	t.Run("alerts", testDashNormAlerts)
	t.Run("weights", testDashNormWeights)
	t.Run("web", testDashNormWeb)
	t.Run("info", testDashNormInfo)
}
