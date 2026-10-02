#!/usr/bin/env python3
"""chart-size-vs-density — per-list size ratio vs density, by complement choice.

One panel per corpus; x = posting-list density (log), y = treight /
roaring serialized-size ratio (log). Below 1.0 ⇒ treight smaller. Color AND
marker encode the encoder's representation choice: direct (members) =
filled blue circles, complemented (holes stored) = open vermillion
triangles; the parity line (ratio = 1) and the >50%-density complement
region are marked.

Input: evidence/bench/totals.json → corpora.<name>.per_list (schema.md §2).
Synthetic fallback otherwise.

Usage:
  MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-size-vs-density.py [--strict]
"""
from __future__ import annotations

import sys

import matplotlib.pyplot as plt
import numpy as np

from chartlib import (
    BLUE, CORPORA, VERMIL, die_synth, evidence_dir, iter_per_list,
    load_totals_summary, save_both, style_setup, synthetic_stamp,
)


def synth_lists(rng: np.random.Generator) -> list[dict]:
    rows = []
    for _ in range(900):
        density = 10 ** rng.uniform(-6, -0.2)
        card = max(1, int(density * 262_144))
        # toy size model: treight ~ card * 6 * (1 - locality), roaring ~
        # header-per-container floor + 2 B/member
        treight = card * rng.uniform(3.0, 6.5)
        roaring = max(card * 2.0, rng.uniform(24, 48))
        if density > 0.5:
            treight = (1 - density) * 262_144 * rng.uniform(3.0, 6.0)
            roaring = (1 - density) * 262_144 * rng.uniform(1.4, 2.2)
        rows.append(dict(density=density, complemented=density > 0.5,
                         ratio=max(treight / roaring, 1e-3)))
    return rows


def main() -> int:
    strict = "--strict" in sys.argv
    edir = evidence_dir()
    corpora = [c for c in CORPORA if load_totals_summary(edir, c)]
    synthetic = not corpora
    if synthetic and strict:
        print("chart-size-vs-density: --strict and no evidence found", file=sys.stderr)
        return 1
    if synthetic:
        die_synth("chart-size-vs-density")
        rng = np.random.default_rng(20261002)
        corpora = CORPORA
        per_list = {c: synth_lists(rng) for c in corpora}
    else:
        per_list = {}
        for c in corpora:
            rows = list(iter_per_list(edir, c))
            per_list[c] = rows
            print(f"  {c}: {len(rows)} plottable lists "
                  f"({sum(1 for r in rows if r['complemented'])} complemented)")

    style_setup()
    fig, axes = plt.subplots(1, len(corpora), figsize=(10.4, 3.8), sharey=True)
    if len(corpora) == 1:
        axes = [axes]
    for ax, corpus in zip(axes, corpora):
        rows = per_list[corpus]
        direct = [r for r in rows if not r["complemented"]]
        comp = [r for r in rows if r["complemented"]]
        ax.scatter([r["density"] for r in direct], [r["ratio"] for r in direct],
                   s=7, c=BLUE, marker="o", edgecolors="none", alpha=0.45,
                   rasterized=True, label=f"direct ({len(direct)})")
        ax.scatter([r["density"] for r in comp], [r["ratio"] for r in comp],
                   s=12, facecolors="none", edgecolors=VERMIL, marker="^",
                   linewidths=0.8, alpha=0.8, rasterized=True,
                   label=f"complemented ({len(comp)})")
        ax.axhline(1.0, color="#1a1a1a", linewidth=0.9, linestyle="--")
        ax.axvspan(0.5, 1.0, color=VERMIL, alpha=0.08, zorder=0)
        ax.set_xscale("log")
        ax.set_yscale("log")
        ax.set_title(corpus.replace("-250k", ""), fontsize=9)
        ax.set_xlabel("density (log)")
    axes[0].set_ylabel("treight / roaring serialized size (log)")
    axes[0].annotate("parity", xy=(0.02, 1.0), xycoords=("axes fraction", "data"),
                     xytext=(2, 3), textcoords="offset points", fontsize=7)
    axes[0].annotate(">50%: complemented", xy=(0.86, 0.03),
                     xycoords="axes fraction", fontsize=7, color=VERMIL)
    axes[0].legend(loc="lower left", fontsize=7)
    fig.suptitle("Per-posting-list size: treight vs roaring serialized bytes",
                 fontsize=10, weight="bold")
    fig.text(0.5, 0.005,
             "below the dashed line treight is smaller; counts in parentheses; "
             "complemented lists store holes (their blob excludes descriptor bytes)",
             ha="center", fontsize=7, color="#9a9a9a")
    if synthetic:
        synthetic_stamp(fig)
    fig.tight_layout(rect=(0, 0.02, 1, 0.93))
    save_both(fig, "chart-size-vs-density")
    return 0


if __name__ == "__main__":
    sys.exit(main())
