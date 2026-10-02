#!/usr/bin/env python3
"""chart-footprint — per-corpus index footprint: treight vs roaring bytes.

Two panels: left = SFST-materialized scope (low+mid tier, what a real SFST
file stores), right = all posting lists (high tier included, the
like-for-like data-structure comparison). Per corpus, three bars:
treight (blob + 5-B descriptor), roaring serialized, roaring heap
(allocative deep size; hatched outline).

Input: evidence/bench/totals.json → corpora.<name>.bitsets.{sfst_materialized,
all_lists} (schema.md §2). Synthetic fallback otherwise.

Usage:
  MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-footprint.py [--strict]
"""
from __future__ import annotations

import sys

import matplotlib.pyplot as plt
import numpy as np

from chartlib import (
    BLUE, CORPORA, GREEN, GRAY, VERMIL, die_synth, evidence_dir,
    load_totals_summary, save_both, style_setup, synthetic_stamp,
)

SYNTH = {
    "certstream-250k": {
        "sfst_materialized": dict(lists=21_481, treight_with_desc_bytes=3_912_004,
                                  roaring_serialized_bytes=5_201_448, roaring_heap_bytes=9_148_020),
        "all_lists": dict(lists=182_081, treight_with_desc_bytes=15_662_044,
                          roaring_serialized_bytes=39_812_006, roaring_heap_bytes=71_240_882),
    },
    "jetstream-250k": {
        "sfst_materialized": dict(lists=26_070, treight_with_desc_bytes=4_806_113,
                                  roaring_serialized_bytes=6_009_921, roaring_heap_bytes=10_402_317),
        "all_lists": dict(lists=113_376, treight_with_desc_bytes=11_988_770,
                          roaring_serialized_bytes=27_664_102, roaring_heap_bytes=48_006_241),
    },
    "mixed-250k": {
        "sfst_materialized": dict(lists=46_928, treight_with_desc_bytes=8_701_220,
                                  roaring_serialized_bytes=11_108_490, roaring_heap_bytes=19_773_406),
        "all_lists": dict(lists=294_880, treight_with_desc_bytes=27_551_004,
                          roaring_serialized_bytes=67_402_779, roaring_heap_bytes=119_112_600),
    },
}

SCOPES = ["sfst_materialized", "all_lists"]
SCOPE_TITLES = {
    "sfst_materialized": "SFST-materialized (low + mid tier — what the file stores)",
    "all_lists": "all posting lists (high tier included — data-structure comparison)",
}


def main() -> int:
    strict = "--strict" in sys.argv
    edir = evidence_dir()
    data = {}
    for c in CORPORA:
        t = load_totals_summary(edir, c)
        if t:
            data[c] = t
    synthetic = not data
    if synthetic and strict:
        print("chart-footprint: --strict and no evidence found", file=sys.stderr)
        return 1
    if synthetic:
        die_synth("chart-footprint")
        data = SYNTH
    corpora = [c for c in CORPORA if c in data]
    bitsets = {c: data[c]["bitsets"] for c in corpora}

    style_setup()
    fig, axes = plt.subplots(1, 2, figsize=(9.6, 4.2), sharey=True)
    n = len(corpora)
    x = np.arange(n)
    series = [
        ("treight (blob + desc)", "treight_with_desc_bytes", VERMIL, "///", None),
        ("roaring serialized", "roaring_serialized_bytes", BLUE, "", None),
        ("roaring heap (in-memory)", "roaring_heap_bytes", GREEN, "\\\\\\",
         "none"),  # faceless = outline-only bar
    ]

    for ax, scope in zip(axes, SCOPES):
        for i, (label, key, color, hatch, face) in enumerate(series):
            vals = [bitsets[c][scope][key] / 1e6 for c in corpora]
            ax.bar(
                x + (i - 1) * 0.26, vals, width=0.25,
                color=color if face != "none" else "white",
                hatch=hatch, edgecolor=color if face == "none" else "#1a1a1a",
                linewidth=1.0 if face == "none" else 0.7,
                label=label,
            )
            for xi, v in zip(x + (i - 1) * 0.26, vals):
                ax.annotate(f"{v:.1f}", (xi, v), textcoords="offset points",
                            xytext=(0, 2), ha="center", fontsize=6.6)
        ax.set_title(SCOPE_TITLES[scope], fontsize=8.6)
        ax.set_xticks(x)
        ax.set_xticklabels([c.replace("-250k", "") for c in corpora], fontsize=8)
    axes[0].set_ylabel("total bytes (MB)")
    axes[0].set_yscale("log")
    axes[0].legend(loc="upper left", fontsize=7.2)
    fig.suptitle("Index footprint over identical posting lists — treight vs roaring",
                 fontsize=10, weight="bold")
    fig.text(0.5, 0.005,
             "treight = tree blob + 5-B descriptor per list (BitmapValue accounting); "
             "roaring heap = allocative deep size of the same bitmaps",
             ha="center", fontsize=7, color=GRAY)
    if synthetic:
        synthetic_stamp(fig)
    fig.tight_layout(rect=(0, 0.02, 1, 0.94))
    save_both(fig, "chart-footprint")
    return 0


if __name__ == "__main__":
    sys.exit(main())
