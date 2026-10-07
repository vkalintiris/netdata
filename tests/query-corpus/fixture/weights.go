// SPDX-License-Identifier: GPL-3.0-or-later

package fixture

import (
	"strconv"

	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The weights fixtures (the corpus's S6 and the parity checks of the weights endpoints): WeightsRows points from
// T0+1, a baseline (T0, T0+WeightsSplit] and a highlight [T0+WeightsSplit, T0+WeightsRows].
const (
	WeightsContext    = "fixture.weights"
	WeightsKS2Context = "fixture.weightsks2"
	WeightsRows       = 240
	WeightsSplit      = 120
)

// Weights is the main weights fixture:
//
//	flat:  constant 50 (equal averages → volume skips it);
//	level: 10/11 alternating in baseline, constant 30 in highlight;
//	split: 100/101 alternating in baseline, +3 ramp in highlight;
//	anom:  constant 20, anomalous only in the highlight window.
func Weights() Chart {
	dims := []Dimension{{ID: "flat"}, {ID: "level"}, {ID: "split"}, {ID: "anom"}}
	val := func(id string, i int) string {
		switch id {
		case "flat":
			return "50"
		case "level":
			if i <= WeightsSplit {
				if i%2 == 1 {
					return "10"
				}
				return "11"
			}
			return "30"
		case "split":
			if i <= WeightsSplit {
				if i%2 == 1 {
					return "100"
				}
				return "101"
			}
			return strconv.Itoa(100 + 3*(i-WeightsSplit-1))
		case "anom":
			return "20"
		}
		panic(id)
	}
	for d := range dims {
		for i := 1; i <= WeightsRows; i++ {
			flags := stream.FlagNotAnomalous
			if dims[d].ID == "anom" && i > WeightsSplit {
				flags = stream.FlagAnomalous
			}
			dims[d].Points = append(dims[d].Points, Point{
				T: T0 + int64(i), Collected: val(dims[d].ID, i), Flags: flags,
			})
		}
	}
	return Chart{
		ID: WeightsContext, Title: "weights", Units: "units", Family: "fixture",
		Context: WeightsContext, UpdateEvery: 1,
		Dimensions: dims,
	}
}

// WeightsKS2 is the ks2 endpoints fixture:
//
//	flat2: constant 50 — identical (all-zero) diffs both windows → d=0
//	       → weight exactly 0;
//	jump:  0/1 alternation in baseline (diffs ±1e5), then a -5 ramp in
//	       the highlight (all consecutive diffs +5e5/+6e5, including
//	       the window-boundary pair) — every highlight diff exceeds
//	       every baseline diff → d=1 with n*d^2>=18 → weight exactly 1.
func WeightsKS2() Chart {
	dims := []Dimension{{ID: "flat2"}, {ID: "jump"}}
	val := func(id string, i int) string {
		if id == "flat2" {
			return "50"
		}
		if i <= WeightsSplit {
			return strconv.Itoa(i % 2)
		}
		return strconv.Itoa(-5 * (i - WeightsSplit))
	}
	for d := range dims {
		for i := 1; i <= WeightsRows; i++ {
			dims[d].Points = append(dims[d].Points, Point{
				T: T0 + int64(i), Collected: val(dims[d].ID, i), Flags: stream.FlagNotAnomalous,
			})
		}
	}
	return Chart{
		ID: WeightsKS2Context, Title: "weights ks2", Units: "units", Family: "fixture",
		Context: WeightsKS2Context, UpdateEvery: 1,
		Dimensions: dims,
	}
}
