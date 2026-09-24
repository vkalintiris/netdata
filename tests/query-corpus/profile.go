// SPDX-License-Identifier: GPL-3.0-or-later

package corpus

import (
	"fmt"
	"sort"
	"strings"
	"testing"
)

// contractScope names a whole contract (component "") or one of its components.
type contractScope struct{ contract, component string }

// corpusProfile is how the shared daemon stores child data, and which contract scopes cannot hold that way.
// QUERY_CORPUS_PROFILE selects one; the default is dbengine with 3 tiers.
type corpusProfile struct {
	Name             string
	StreamMemoryMode string // daemon.Options.StreamMemoryMode; "" is dbengine
	StorageTiers     int
	// notApplicable gives a structural reason per scope: why the contract cannot hold under this profile. It is
	// never a list of known-broken contracts.
	notApplicable map[contractScope]string
}

var corpusProfiles = map[string]corpusProfile{
	"dbengine": {Name: "dbengine", StorageTiers: 3},
	// ram is the slice-1 reference profile of the Rust agent: ram children, one storage tier.
	"ram": {Name: "ram", StreamMemoryMode: "ram", StorageTiers: 1, notApplicable: ramNotApplicable},
}

var activeProfile = corpusProfiles["dbengine"]

func resolveCorpusProfile(name string) (corpusProfile, error) {
	if name == "" {
		return corpusProfiles["dbengine"], nil
	}
	if p, ok := corpusProfiles[name]; ok {
		return p, nil
	}
	names := make([]string, 0, len(corpusProfiles))
	for n := range corpusProfiles {
		names = append(names, n)
	}
	sort.Strings(names)
	return corpusProfile{}, fmt.Errorf("query contract corpus: unknown QUERY_CORPUS_PROFILE %q (choose one of %s)",
		name, strings.Join(names, ", "))
}

// notApplicableReason looks up the whole contract first, then the component.
func (p corpusProfile) notApplicableReason(contract, component string) (string, bool) {
	if reason, ok := p.notApplicable[contractScope{contract, ""}]; ok {
		return reason, true
	}
	reason, ok := p.notApplicable[contractScope{contract, component}]
	return reason, ok
}

// filterTiers keeps the tiers the profile stores.
func (p corpusProfile) filterTiers(tiers []int) []int {
	var out []int
	for _, tier := range tiers {
		if tier < p.StorageTiers {
			out = append(out, tier)
		}
	}
	return out
}

// noTierReads is the expected per-tier presence of a query that reads no tier.
func noTierReads() []bool { return make([]bool, activeProfile.StorageTiers) }

// skipNotApplicable is called by every registration helper: a not-applicable scope skips before it is recorded.
func skipNotApplicable(t *testing.T, contract, component string) {
	t.Helper()
	if reason, ok := activeProfile.notApplicableReason(contract, component); ok {
		t.Skipf("n/a (profile %s: %s)", activeProfile.Name, reason)
	}
}

// skipIfNotApplicable guards a test that does shared work (restarts, dedicated daemons, fixtures) before its first
// registration: it skips when every listed scope is not applicable, returns when none is, and fails when the list
// is mixed (the test must then register the applicable scopes first).
func skipIfNotApplicable(t *testing.T, scopes ...contractScope) {
	t.Helper()
	na := 0
	var reason string
	for _, s := range scopes {
		component := s.component
		if component == "" {
			component = defaultContractComponent
		}
		if r, ok := activeProfile.notApplicableReason(s.contract, component); ok {
			na++
			reason = r
		}
	}
	switch {
	case na == 0:
		return
	case na == len(scopes):
		t.Skipf("n/a (profile %s: %s)", activeProfile.Name, reason)
	default:
		t.Fatalf("profile %s: %d of %d guarded scopes are not applicable; register the applicable ones first",
			activeProfile.Name, na, len(scopes))
	}
}

// pinCorpusProfile runs a daemon-free guard under a fixed profile (top-level tests run sequentially).
func pinCorpusProfile(t *testing.T, name string) {
	t.Helper()
	p, err := resolveCorpusProfile(name)
	if err != nil {
		t.Fatal(err)
	}
	saved := activeProfile
	activeProfile = p
	t.Cleanup(func() { activeProfile = saved })
}

// The structural reasons of the ram profile.
const (
	naTier1        = "reads storage tier 1 or above, which a one-tier profile does not store"
	naMultiTier    = "boots dedicated multi-tier dbengine daemons"
	naLargeFixture = "its fixture is larger than the ram ring and the same contract reads higher tiers"
	naEndpoint     = "the endpoint is outside slice 1 (the ram reference profile covers the data APIs)"
	naCadence      = "an update_every change flushes a ram ring (rrddim_store_metric_change_collection_frequency)"
	naRestart      = "ram keeps nothing across a daemon restart"
	naReplication  = "a ram child caps replication at entries x update_every, so old fixture rows never replicate"
	naDBEngine     = "needs dbengine storage"
)

func wholeContracts(reason string, names ...string) map[contractScope]string {
	m := make(map[contractScope]string, len(names))
	for _, n := range names {
		m[contractScope{n, ""}] = reason
	}
	return m
}

func components(reason, contract string, names ...string) map[contractScope]string {
	m := make(map[contractScope]string, len(names))
	for _, n := range names {
		m[contractScope{contract, n}] = reason
	}
	return m
}

func mergeScopes(parts ...map[contractScope]string) map[contractScope]string {
	out := make(map[contractScope]string)
	for _, part := range parts {
		for k, v := range part {
			out[k] = v
		}
	}
	return out
}

// ramNotApplicable: 118 whole contracts and 7 component scopes (spec-query §12.3, decision D25).
var ramNotApplicable = mergeScopes(
	wholeContracts(naTier1,
		"L2/tier1-complete", "L2/tier1-interior-gaps", "L2/tier1-anomaly-rate", "L2/tier1-reset-flags",
		"L2/tier1-float32-fields", "L2/partial-wide-point", "L2/partial-wide-point-values",
		"L2/tier-rollup-original-values", "L2/update-every-5",
		"L4/family-tier-source", "L4/family-tier-grid", "L4/family-tier-values", "L4/family-tier-anomaly-rates",
		"L4/family-tier-annotations",
		"CASE-023/tier-estimation-source", "CASE-023/tier-estimation-percentage-of-time",
		"CASE-023/tier-estimation-number-of-flaps", "CASE-023/tier-estimation-number-of-times",
		"CASE-023/tier-estimation-percentage-of-samples",
		"CASE-025/anomaly-bit-not-blended",
		"L2/whole-chart-absence", "L2/tier2", "L2/update-every-sweep",
		"CASE-017/tier-boundary-absorption",
		"L3/anomaly-bit-tier-rates", "CASE-023/tier-anomaly-bit",
		"L4/auto-tier-choice", "L4/auto-tier-grid", "L4/auto-tier-values", "L4/auto-tier-anomaly-rates",
		"L4/auto-tier-annotations",
		"CASE-023/redelivery-samples-everywhere", "CASE-023/redelivery-counted-once",
		"CASE-023/redelivery-zero-not-empty", "CASE-023/reset-counted-once",
		"CASE-023/previous-survives-redelivery", "CASE-023/previous-drop-at-every-zoom",
		"CASE-023/tier-wide-point-source", "CASE-023/tier-wide-point-time-share",
		"CASE-023/tier-wide-point-number-of-times", "CASE-023/tier-wide-point-number-of-flaps",
		"L10/buckets-finer-than-stored-data-answer", "L10/counts-do-not-inflate-with-zoom",
		"L10/time-shares-stable-across-zoom", "L10/queries-are-deterministic",
	),
	wholeContracts(naMultiTier,
		"L2/historical-tier-grouping", "L2/v1-rollup-count-65536",
		"L4/plan-switching", "L4/three-tier-join-grid", "L4/three-tier-condition-groupings",
		"CASE-026/totals-survive-a-plan-switch", "CASE-026/partial-evidence-survives-a-plan-switch",
		"CASE-031/rate-volume-across-an-automatic-seam", "CASE-036/absolute-across-plan-seam",
		"CASE-038/higher-tier-only-rate-volume", "CASE-038/higher-tier-only-rate-partial-evidence",
	),
	wholeContracts(naLargeFixture,
		"CASE-023/tier-resolution-source", "CASE-023/tier-resolution-percentage-of-time",
		"CASE-023/tier-resolution-percentage-of-samples", "CASE-023/tier-resolution-number-of-times",
		"CASE-023/tier-resolution-number-of-flaps",
		"CASE-028/rate-with-gaps-totals-what-was-measured", "CASE-028/partial-and-off-grid-rate-windows",
		"CASE-029/tier0-slow-metric-totals-at-every-zoom",
	),
	wholeContracts(naEndpoint,
		"CASE-020/badge-rate-sum-value", "CASE-020/badge-gauge-sum-value", "CASE-020/badge-rate-sum-units",
		"CASE-020/badge-gauge-sum-units", "CASE-020/badge-mixed-algorithm-sum-units", "CASE-023/badge-invalid-options",
		"CASE-023/mcp-protocol-lifecycle", "CASE-023/mcp-query-tool-schema", "CASE-023/mcp-valid-result-schema",
		"CASE-023/mcp-valid-query-units", "CASE-023/mcp-valid-query-echo", "CASE-023/mcp-valid-query-timestamps",
		"CASE-023/mcp-valid-query-values", "CASE-023/mcp-valid-query-anomaly-rates",
		"CASE-023/mcp-valid-query-annotations", "CASE-023/mcp-invalid-options", "CASE-023/mcp-default-zero-options",
		"CASE-023/weights-invalid-options", "API/fallback-unknown-weights-method",
		"W/value", "W/anomaly-rate-per-metric-values", "W/anomaly-rate-per-metric-nonzero-default",
		"W/anomaly-rate-multidim", "W/volume-equal-baseline-skip", "W/volume-formula", "W/ks2-raw-endpoints",
		"W/ks2-spread-normalization",
		"W/limit-aliases", "W/limit-boundaries", "W/limit-invalid", "W/limit-ranking", "W/limit-summaries",
		"W/limit-legacy", "W/limit-grouped", "W/limit-complete-groups", "W/limit-thousand", "W/limit-hierarchy",
		"W/limit-node-ties", "W/limit-mcp",
	),
	wholeContracts(naCadence,
		"CASE-023/cadence-change-availability-tier0", "CASE-023/cadence-change-availability-higher-tiers",
		"CASE-023/historical-gap-slots-after-cadence-change",
		"CASE-030/interval-change-slowing-down", "CASE-030/interval-change-speeding-up",
		"CASE-035/completed-rollup-keeps-original-cadence", "CASE-035/transition-volume-slowing-down",
		"CASE-035/transition-volume-speeding-up", "CASE-035/tier0-page-boundary-keeps-every-sample",
		"CASE-037/rate-volume-across-three-tier-cadence-query",
	),
	wholeContracts(naRestart, "L0/restart", "L1/gap-states", "CASE-016/fresh-host-forgotten-on-restart"),
	wholeContracts(naReplication, "L0/replication", "CASE-015/replication-disconnect-discard"),
	components(naDBEngine, "L1/storage-backend-gap-state",
		"dbengine-gorilla-hot", "dbengine-gorilla-restart", "dbengine-raw-hot", "dbengine-raw-restart"),
	components(naTier1, "L4/minmax-absolute-semantics", "tier1-min", "tier1-max"),
	components(naMultiTier, "CASE-033/anomaly-rate-counts-samples-in-the-row", "plan-seam-source"),
)
