// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import "testing"

// TestDashNormClosers (check `dash.norm`) pins the masks, renders, guards and judges of milestone 10's closer rows on
// answers recorded from C, with no agent started, as TestDashNorm does for the rows before them: each family's cases
// live beside its rows (dash_norm_closers_<family>_test.go).
func TestDashNormClosers(t *testing.T) {
	t.Run("r-spawn", testDashNormClosersRSpawn)
	t.Run("r-settings", testDashNormClosersRSettings)
	t.Run("r-search", testDashNormClosersRSearch)
	t.Run("transitions-l", testDashNormClosersLRows)
	t.Run("alerts-c", testDashNormClosersC)
	t.Run("b", testDashNormClosersB)
	t.Run("f-out-facts", testDashNormClosersFOutFacts)
	t.Run("f-socket", testDashNormClosersFSocket)
	t.Run("f-peer", testDashNormClosersFPeer)
	t.Run("f-ni-stream", testDashNormClosersFNIStream)
	t.Run("f-words", testDashNormClosersFWords)
	t.Run("f-fn-out", testDashNormClosersFFnOut)
	t.Run("f-down", testDashNormClosersFDown)
	t.Run("f-tls", testDashNormClosersFTLS)
	t.Run("f-ephemeral", testDashNormClosersFEph)
	t.Run("f-load", testDashNormClosersFLoad)
	t.Run("f-post", testDashNormClosersFPost)
}
