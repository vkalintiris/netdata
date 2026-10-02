#!/usr/bin/env python3
"""chart-density — posting-list density histogram per corpus.

One grouped bar per density bucket (log x-axis semantics: the buckets span
decades, from <=0.001% to >50%); the >50% bucket is the band the encoder
complements (stores holes) — outlined + annotated.

Input: evidence/bench/<corpus>/stats.json → posting_density_histogram
(see evidence/bench/schema.md §1). Falls back to clearly-marked synthetic
sample data when evidence is absent.

Usage:
  MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-density.py [--strict]
"""
from __future__ import annotations

import sys

import matplotlib.pyplot as plt
import numpy as np

from chartlib import (
    CORPORA, CORPUS_COLORS, DENSITY_BUCKETS, VERMIL, available_corpora,
    die_synth, evidence_dir, find_stats, load_json, save_both, style_setup,
    synthetic_stamp,
)

SYNTH = {
    "certstream-250k": [152_000, 8_400, 610, 240, 180, 42, 9],
    "jetstream-250k": [98_000, 12_500, 1_900, 700, 310, 66, 14],
    "mixed-250k": [244_000, 20_600, 2_500, 930, 480, 105, 22],
}


def hist_from_stats(stats: dict) -> list[int]:
    got = {row["bucket"]: int(row["lists"]) for row in stats["posting_density_histogram"]}
    return [got[b] for b in DENSITY_BUCKETS]


def main() -> int:
    strict = "--strict" in sys.argv
    edir = evidence_dir()
    corpora = available_corpora(edir)
    synthetic = not corpora
    if synthetic and strict:
        print("chart-density: --strict and no evidence found", file=sys.stderr)
        return 1
    if synthetic:
        die_synth("chart-density")
        corpora = CORPORA
        hists = {c: SYNTH[c] for c in corpora}
    else:
        hists = {c: hist_from_stats(load_json(find_stats(edir, c))) for c in corpora}

    style_setup()
    fig, ax = plt.subplots(figsize=(9.2, 4.4))
    n = len(corpora)
    width = 0.8 / n
    x = np.arange(len(DENSITY_BUCKETS))
    for i, corpus in enumerate(corpora):
        color, hatch = CORPUS_COLORS[corpus]
        ax.bar(
            x + (i - (n - 1) / 2) * width,
            [max(v, 1) for v in hists[corpus]],
            width=width,
            color=color,
            hatch=hatch,
            edgecolor="#1a1a1a",
            linewidth=0.7,
            label=corpus,
        )

    ax.set_yscale("log")
    ax.set_xticks(x)
    ax.set_xticklabels(DENSITY_BUCKETS, rotation=28, ha="right", fontsize=8)
    ax.set_xlabel("posting-list density (|posting list| / universe)")
    ax.set_ylabel("posting lists (count, log scale)")
    ax.set_title("Posting-list density per corpus — where the complement band starts")
    ax.axvspan(len(DENSITY_BUCKETS) - 1.5, len(DENSITY_BUCKETS) - 0.5,
               color=VERMIL, alpha=0.10, zorder=0)
    ax.annotate(
        "bucket [0.5, 1.0]:\nencoder stores the holes only when\ndensity is strictly > 50%;\n= 50% stays direct",
        xy=(len(DENSITY_BUCKETS) - 1, max(max(h) for h in hists.values())),
        xytext=(-190, -8), textcoords="offset points",
        fontsize=7.4, color=VERMIL,
        arrowprops=dict(arrowstyle="->", color=VERMIL, lw=1.0),
    )
    ax.legend(loc="upper right", fontsize=8)
    if synthetic:
        synthetic_stamp(fig)
    fig.tight_layout()
    save_both(fig, "chart-density")
    return 0


if __name__ == "__main__":
    sys.exit(main())
