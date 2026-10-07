// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"slices"
	"strings"
	"testing"
)

// testDashNormInfo pins runR's list (infoV2RunR, D228) on the `db_size` tiers two C daemons printed for the runR
// fixture (H31's probe, C against C, pair p1, 2026-10-06): tier 0's sample count 106565 higher on the first,
// everything else equal. The list hides that count and nothing more; the fresh dbengine's list does not hide it.
func testDashNormInfo(t *testing.T) {
	const (
		tier0 = `{"tier":0,"granularity":"1s","metrics":116,"samples":SAMPLES0,"disk_used":1819108,` +
			`"disk_max":26214400,"disk_percent":6.94,"from":1789980541,"to":1791327866,"retention":1347325,` +
			`"retention_human":"15d15h","requested_retention":0,"requested_retention_human":"off",` +
			`"expected_retention":19415733,"expected_retention_human":"7mo15d"}`
		tier1 = `{"tier":1,"granularity":"1m","metrics":110,"samples":66860,"disk_used":782564,` +
			`"disk_max":26214400,"disk_percent":2.99,"from":1789980600,"to":1791327866,"retention":1347266,` +
			`"retention_human":"15d15h","requested_retention":0,"requested_retention_human":"off",` +
			`"expected_retention":45130838,"expected_retention_human":"1y5mo8d"}`
		tier2 = `{"tier":2,"granularity":"1h","metrics":20,"samples":0,"disk_used":36864,"disk_max":26214400,` +
			`"disk_percent":0.14,"from":1789981200,"to":1791327866,"retention":1346666,"retention_human":"15d15h",` +
			`"requested_retention":0,"requested_retention_human":"off","expected_retention":788400000,` +
			`"expected_retention_human":"25y"}`
		info = `{"agents":[{"db_size":[` + tier0 + `,` + tier1 + `,` + tier2 + `]}]}`
	)
	high := strings.Replace(info, "SAMPLES0", "5294079", 1)
	usual := strings.Replace(info, "SAMPLES0", "5187514", 1)
	parse := func(t *testing.T, s string) Value {
		t.Helper()
		v, err := ParseJSON([]byte(s))
		if err != nil {
			t.Fatalf("%v: %s", err, s)
		}
		return v
	}
	for name, c := range map[string]struct {
		candidate string
		masks     []Mask
		want      []string
	}{
		"runR hides tier 0's samples": {usual, infoV2RunR, nil},
		"fresh-dbengine reports them": {usual, infoV2Fresh, []string{"$.agents[0].db_size[0].samples"}},
		"runR keeps tier 0's metrics": {strings.Replace(usual, `"metrics":116,`, `"metrics":117,`, 1),
			infoV2RunR, []string{"$.agents[0].db_size[0].metrics"}},
		"runR keeps tier 0's disk_used": {strings.Replace(usual, `"disk_used":1819108,`, `"disk_used":1819109,`, 1),
			infoV2RunR, []string{"$.agents[0].db_size[0].disk_used"}},
		"runR keeps tier 1's samples": {strings.Replace(usual, `"samples":66860,`, `"samples":66861,`, 1),
			infoV2RunR, []string{"$.agents[0].db_size[1].samples"}},
	} {
		t.Run(name, func(t *testing.T) {
			fam := infoV2Family(c.masks...)
			o, cand := ApplyMasks(parse(t, high), fam.masks), ApplyMasks(parse(t, c.candidate), fam.masks)
			var got []string
			for _, d := range Compare(o, cand, fam.unordered...) {
				got = append(got, d.Path)
			}
			if !slices.Equal(got, c.want) {
				t.Errorf("differences %q, want %q", got, c.want)
			}
		})
	}
}
