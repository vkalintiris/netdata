// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strings"
	"testing"
)

// TestDashNorm (check `dash.norm`) pins the dashboard checks' masks and normalisers on answers recorded from C, with
// no agent started: each family's cases live beside its check (dash_norm_<family>_test.go).
func TestDashNorm(t *testing.T) {
	t.Run("contexts", testDashNormContexts)
	t.Run("alerts", testDashNormAlerts)
	t.Run("alerts-rows", testDashNormAlertsRows)
	t.Run("transitions", testDashNormTransitions)
	t.Run("rules-order", testDashNormRulesOrder)
	t.Run("weights", testDashNormWeights)
	t.Run("weights-rows", testDashNormWeightsRows)
	t.Run("web", testDashNormWeb)
	t.Run("info", testDashNormInfo)
	t.Run("nodes", testDashNormNodes)
	t.Run("contexts-rows", testDashNormContextsRows)
	t.Run("search", testDashNormSearch)
	t.Run("settings", testDashNormSettings)
	t.Run("tight", testDashNormTight)
	t.Run("wiring", testDashNormWiring)
}

// dashNormDiffs are the paths where two recorded answers differ as a family compares them: each side's body
// normalised by fam (0 the oracle's, both in the seconds of flight, which are the agent's clock's too), then masks
// applied and fam's unordered paths left unordered.
func dashNormDiffs(t *testing.T, fam v2Family, masks []Mask, flight [2]int64, o, c string) string {
	t.Helper()
	var docs [2]Value
	for i, b := range []string{o, c} {
		body := fam.normalise(i, flight, flight, []byte(b))
		v, err := ParseJSON(body)
		if err != nil {
			t.Fatalf("%v: %s", err, body)
		}
		docs[i] = v
	}
	var out []string
	for _, d := range Compare(ApplyMasks(docs[0], masks), ApplyMasks(docs[1], masks), fam.unordered...) {
		out = append(out, d.Path)
	}
	return strings.Join(out, " ")
}
