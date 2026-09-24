// SPDX-License-Identifier: GPL-3.0-or-later

package corpus

import (
	"reflect"
	"slices"
	"strings"
	"testing"
)

// TestCorpusProfilesAreValid keeps every not-applicable entry tied to a manifest scope with a structural reason.
func TestCorpusProfilesAreValid(t *testing.T) {
	if n := len(corpusProfiles["dbengine"].notApplicable); n != 0 {
		t.Errorf("the dbengine profile is the reference and has %d not-applicable entries", n)
	}
	for name, p := range corpusProfiles {
		if p.Name != name {
			t.Errorf("profile %q is named %q", name, p.Name)
		}
		if p.StorageTiers < 1 {
			t.Errorf("profile %q has %d storage tiers", name, p.StorageTiers)
		}
		whole := map[string]bool{}
		for scope := range p.notApplicable {
			if scope.component == "" {
				whole[scope.contract] = true
			}
		}
		for scope, reason := range p.notApplicable {
			mc, ok := manifest[scope.contract]
			switch {
			case !ok:
				t.Errorf("profile %q: %q is not a manifest contract", name, scope.contract)
			case scope.component != "" && !slices.Contains(mc.Components, scope.component):
				t.Errorf("profile %q: contract %q declares no component %q", name, scope.contract, scope.component)
			case scope.component != "" && whole[scope.contract]:
				t.Errorf("profile %q: %q is not applicable as a whole and per component", name, scope.contract)
			}
			if strings.TrimSpace(reason) == "" {
				t.Errorf("profile %q: %v has no reason", name, scope)
			}
		}
	}

	// Decision D25 counts: 118 whole contracts and 7 component scopes.
	whole, components := 0, 0
	for scope := range corpusProfiles["ram"].notApplicable {
		if scope.component == "" {
			whole++
		} else {
			components++
		}
	}
	if whole != 118 || components != 7 {
		t.Errorf("ram profile: %d whole contracts and %d component scopes not applicable, want 118 and 7",
			whole, components)
	}
}

func TestContractLedgerReportsNotApplicableScopes(t *testing.T) {
	cases := map[string]ManifestCase{
		"whole":     {},
		"composite": {Components: []string{"first", "second"}},
		"plain":     {},
	}
	p := corpusProfile{Name: "synthetic", StorageTiers: 1, notApplicable: map[contractScope]string{
		{"whole", ""}:           "reason w",
		{"composite", "second"}: "reason c",
	}}

	ledger := newContractLedger()
	ledger.record("composite", "first", true, false)
	ledger.record("plain", defaultContractComponent, true, false)
	// Whatever a not-applicable scope recorded, it is neither evaluated nor broken.
	ledger.record("whole", defaultContractComponent, false, false)
	ledger.record("composite", "second", false, false)

	got := ledger.summarizeProfile(cases, p)
	want := contractRunSummary{
		evaluated: 2,
		profile:   "synthetic",
		notApplicable: []string{
			"composite/second  n/a (profile synthetic: reason c)",
			"whole  n/a (profile synthetic: reason w)",
		},
		wholeNA: 1,
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("summarizeProfile() = %#v, want %#v", got, want)
	}

	report, complete := formatContractSummary(got, len(cases), true)
	if !complete {
		t.Fatalf("a run of every applicable scope is incomplete:\n%s", report)
	}
	for _, line := range []string{
		"query contract corpus: profile synthetic: 1 contract(s) and 1 component scope(s) not applicable\n",
		"  N/A      composite/second  n/a (profile synthetic: reason c)\n",
		"  N/A      whole  n/a (profile synthetic: reason w)\n",
		"query contract corpus: all 2 applicable contracts hold\n",
	} {
		if !strings.Contains(report, line) {
			t.Errorf("report lacks %q:\n%s", line, report)
		}
	}
	if short, _ := formatContractSummary(got, len(cases), false); strings.Contains(short, "  N/A ") {
		t.Errorf("a partial run lists not-applicable scopes:\n%s", short)
	}

	// Without the profile the same ledger is incomplete and broken, as before profiles existed.
	plain := ledger.summarize(cases)
	if plain.wholeNA != 0 || plain.notApplicable != nil || len(plain.broken) != 2 {
		t.Fatalf("summarize() = %#v, want two broken contracts and no not-applicable scopes", plain)
	}
}
