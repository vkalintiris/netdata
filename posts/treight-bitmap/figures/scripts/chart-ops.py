#!/usr/bin/env python3
"""chart-ops — per-op CPU medians: treight vs roaring (criterion).

Grouped bars per operation (log y — ops span ns to ms). Faceted by density
bucket (lt1pct / 1to50pct / gt50pct); default corpus = mixed-250k, override
with --corpus. Bars show the criterion median (median_ns); the per-op
normalization actually used for the blog's per-position/per-list claims is
printed to stdout (from per_position_ns / per_list_ns) since mixing
normalizations on one axis would be misleading.

Input: evidence/bench/results.json → corpora.<name>.buckets.<bucket>.ops
(schema.md §3). Synthetic fallback otherwise.

Usage:
  MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-ops.py [--corpus mixed-250k] [--strict]
"""
from __future__ import annotations

import sys

import matplotlib.pyplot as plt
import numpy as np

from chartlib import (
    BUCKETS, CORPORA, IMPL_STYLE, OPS_ORDER, die_synth, evidence_dir,
    load_json, save_both, style_setup, synthetic_stamp,
)

SYNTH_NS = {
    "lt1pct": {
        "bulk_build": {"treight": 410.0, "roaring": 940.0},
        "insert_arrival": {"treight": 96.0, "roaring": 51.0},
        "insert_shuffled": {"treight": 610.0, "roaring": 58.0},
        "contains_hit": {"treight": 31.0, "roaring": 12.0},
        "contains_miss": {"treight": 18.0, "roaring": 11.0},
        "len_min_max": {"treight": 44.0, "roaring": 8.0},
        "iterate": {"treight": 6.2, "roaring": 3.1},
        "and": {"treight": 590.0, "roaring": 240.0},
        "or": {"treight": 640.0, "roaring": 280.0},
        "and_not": {"treight": 610.0, "roaring": 260.0},
        "range_count": {"treight": 290.0, "roaring": 15.0},
    },
    "1to50pct": {
        "bulk_build": {"treight": 9_800.0, "roaring": 21_000.0},
        "insert_arrival": {"treight": 120.0, "roaring": 48.0},
        "insert_shuffled": {"treight": 1_450.0, "roaring": 60.0},
        "contains_hit": {"treight": 44.0, "roaring": 14.0},
        "contains_miss": {"treight": 26.0, "roaring": 12.0},
        "len_min_max": {"treight": 1_800.0, "roaring": 9.0},
        "iterate": {"treight": 9.4, "roaring": 2.6},
        "and": {"treight": 14_000.0, "roaring": 5_100.0},
        "or": {"treight": 16_500.0, "roaring": 6_200.0},
        "and_not": {"treight": 15_200.0, "roaring": 5_800.0},
        "range_count": {"treight": 6_400.0, "roaring": 18.0},
    },
    "gt50pct": {
        "bulk_build": {"treight": 210_000.0, "roaring": 480_000.0},
        "insert_arrival": {"treight": 210.0, "roaring": 44.0},
        "insert_shuffled": {"treight": 4_800.0, "roaring": 70.0},
        "contains_hit": {"treight": 88.0, "roaring": 15.0},
        "contains_miss": {"treight": 52.0, "roaring": 13.0},
        "len_min_max": {"treight": 96_000.0, "roaring": 10.0},
        "iterate": {"treight": 12.8, "roaring": 2.2},
        "and": {"treight": 310_000.0, "roaring": 120_000.0},
        "or": {"treight": 340_000.0, "roaring": 135_000.0},
        "and_not": {"treight": 320_000.0, "roaring": 128_000.0},
        "range_count": {"treight": 140_000.0, "roaring": 22.0},
    },
}


def main() -> int:
    corpus = "mixed-250k"
    if "--corpus" in sys.argv:
        corpus = sys.argv[sys.argv.index("--corpus") + 1]
    strict = "--strict" in sys.argv
    edir = evidence_dir()
    results_path = edir / "results.json"
    synthetic = not results_path.is_file()
    if synthetic and strict:
        print("chart-ops: --strict and no evidence found", file=sys.stderr)
        return 1
    if synthetic:
        die_synth("chart-ops")
        buckets = {b: SYNTH_NS[b] for b in BUCKETS}
    else:
        results = load_json(results_path)
        if corpus not in results.get("corpora", {}):
            print(f"chart-ops: corpus {corpus!r} not in results.json", file=sys.stderr)
            return 1
        rbuckets = results["corpora"][corpus]["buckets"]
        buckets = {}
        for b in BUCKETS:
            ops = rbuckets.get(b, {}).get("ops", {})
            m = {}
            for op in OPS_ORDER:
                row = {}
                for impl in ("treight", "roaring"):
                    e = ops.get(f"{op}/{impl}")
                    if e is not None:
                        row[impl] = float(e["median_ns"])
                        if e.get("per_position_ns") is not None:
                            print(f"  note {corpus}/{b}/{op}/{impl}: "
                                  f"per_position_ns={e['per_position_ns']:.3g} "
                                  f"per_list_ns={e.get('per_list_ns')}")
                        elif e.get("per_list_ns") is not None:
                            print(f"  note {corpus}/{b}/{op}/{impl}: "
                                  f"per_list_ns={e['per_list_ns']:.3g}")
                if row:
                    m[op] = row
            buckets[b] = m

    style_setup()
    fig, axes = plt.subplots(1, 3, figsize=(10.4, 4.0), sharey=True)
    ops = [o for o in OPS_ORDER if any(o in buckets[b] for b in BUCKETS)]
    x = np.arange(len(ops))
    for ax, b in zip(axes, BUCKETS):
        for i, impl in enumerate(("treight", "roaring")):
            color, hatch = IMPL_STYLE[impl]
            vals = [buckets[b].get(o, {}).get(impl, np.nan) for o in ops]
            ax.bar(x + (i - 0.5) * 0.38, vals, width=0.36, color=color,
                   hatch=hatch, edgecolor="#1a1a1a", linewidth=0.7, label=impl)
        ax.set_title(f"{b} lists", fontsize=9)
        ax.set_yscale("log")
        ax.set_xticks(x)
        ax.set_xticklabels(ops, rotation=52, ha="right", fontsize=6.8)
    axes[0].set_ylabel("criterion median per iteration (ns, log)")
    axes[0].legend(loc="upper right", fontsize=8)
    fig.suptitle(f"CPU per operation — treight vs roaring ({corpus})",
                 fontsize=10, weight="bold")
    if synthetic:
        synthetic_stamp(fig)
    fig.tight_layout(rect=(0, 0, 1, 0.93))
    save_both(fig, "chart-ops")
    return 0


if __name__ == "__main__":
    sys.exit(main())
