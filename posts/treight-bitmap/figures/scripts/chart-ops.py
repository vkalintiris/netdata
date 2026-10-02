#!/usr/bin/env python3
"""chart-ops — per-op CPU medians: treight vs roaring (criterion).

Top row: the eight per-list ops, faceted by density bucket (lt1pct /
1to50pct / gt50pct), log y, bars = criterion median per benchmark
iteration. Bottom row (separate axis — different measurement unit): the
three set-op PAIR groups from `buckets.pairs.ops` — per-OPERATION medians
over one measured operand pair each (n = 1): `and` on same-field-disjoint
operands, `or` on cross-field operands, `and_not` dense-vs-sparse. These
must not share an axis with the per-position/per-list world, hence the
visually separated panel.

The per-op normalizations actually used for the blog's per-position /
per-list claims (per_position_ns / per_list_ns) are printed to stdout —
mixing normalizations on one axis would mislead.

Default corpus = mixed-250k; override with --corpus.

Input: evidence/bench/results.json (schema.md §3 + the flat pair-group
keys under buckets.pairs.ops). Synthetic fallback otherwise.

Usage:
  MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/chart-ops.py [--corpus mixed-250k] [--strict]
"""
from __future__ import annotations

import sys

import matplotlib.pyplot as plt
import numpy as np

from chartlib import (
    BUCKETS, IMPL_STYLE, OPS_ORDER, PAIR_GROUPS, die_synth, evidence_dir,
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
        "range_count": {"treight": 140_000.0, "roaring": 22.0},
    },
}

SYNTH_PAIRS = {  # (op, kind) -> {impl: median_ns per pair operation}
    ("and", "same-field-disjoint"): {"treight": 44_700.0, "roaring": 17_400.0},
    ("or", "cross-field"): {"treight": 4.8, "roaring": 2_814.0},
    ("and_not", "dense-vs-sparse"): {"treight": 36.5, "roaring": 694.0},
}


def load_pairs(results: dict, corpus: str) -> dict:
    """{((op, kind)): {impl: median_ns}} from buckets.pairs.ops flat keys."""
    ops = results["corpora"][corpus]["buckets"]["pairs"]["ops"]
    out = {}
    for op, kind in PAIR_GROUPS:
        row = {}
        for impl in ("treight", "roaring"):
            e = ops.get(f"pairs/{op}/{kind}/{impl}")
            if e is not None:
                row[impl] = float(e["median_ns"])
        if row:
            out[(op, kind)] = row
    return out


def fmt_ns(v: float) -> str:
    if v >= 10_000:
        return f"{v / 1000:.0f} µs"
    if v >= 1_000:
        return f"{v / 1000:.1f} µs"
    return f"{v:.1f} ns"


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
        buckets = {b: {op: dict(SYNTH_NS[b][op]) for op in SYNTH_NS[b]} for b in BUCKETS}
        pairs = {g: dict(v) for g, v in SYNTH_PAIRS.items()}
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
        pairs = load_pairs(results, corpus)

    style_setup()
    fig, axes = plt.subplot_mosaic(
        [["lt1pct", "1to50pct", "gt50pct"], ["pairs", "pairs", "pairs"]],
        figsize=(10.4, 6.8),
        gridspec_kw={"height_ratios": [3.0, 1.9]},
    )

    # --- top row: per-list ops per density bucket (median per iteration)
    ops = [o for o in OPS_ORDER if any(o in buckets[b] for b in BUCKETS)]
    x = np.arange(len(ops))
    for b in BUCKETS:
        ax = axes[b]
        for i, impl in enumerate(("treight", "roaring")):
            color, hatch = IMPL_STYLE[impl]
            vals = [buckets[b].get(o, {}).get(impl, np.nan) for o in ops]
            ax.bar(x + (i - 0.5) * 0.38, vals, width=0.36, color=color,
                   hatch=hatch, edgecolor="#1a1a1a", linewidth=0.7, label=impl)
        ax.set_title(f"{b} lists", fontsize=9)
        ax.set_yscale("log")
        ax.set_xticks(x)
        ax.set_xticklabels(ops, rotation=52, ha="right", fontsize=6.8)
    axes["lt1pct"].set_ylabel("median per iteration (ns, log)")
    axes["lt1pct"].legend(loc="upper right", fontsize=8)

    # --- bottom row: set-op pair groups (median per PAIR OPERATION — own axis)
    axp = axes["pairs"]
    groups = [g for g in PAIR_GROUPS if g in pairs]
    xp = np.arange(len(groups))
    for i, impl in enumerate(("treight", "roaring")):
        color, hatch = IMPL_STYLE[impl]
        vals = [pairs[g].get(impl, np.nan) for g in groups]
        bars = axp.bar(xp + (i - 0.5) * 0.34, vals, width=0.32, color=color,
                       hatch=hatch, edgecolor="#1a1a1a", linewidth=0.7, label=impl)
        for rect, v in zip(bars, vals):
            if np.isfinite(v):
                axp.annotate(fmt_ns(v), (rect.get_x() + rect.get_width() / 2, v),
                             textcoords="offset points", xytext=(0, 3),
                             ha="center", fontsize=7)
    axp.set_yscale("log")
    axp.set_xticks(xp)
    axp.set_xticklabels([f"{op}\n{kind}" for op, kind in groups], fontsize=7.6)
    axp.set_ylabel("median per pair op (ns, log)")
    axp.set_title(
        "set operations on the measured operand pairs — per-operation medians "
        "(one operand pair per group; separate axis: NOT per-position)",
        fontsize=7.8, style="italic")
    # ratio annotation per group (treight/roaring)
    for xi, g in zip(xp, groups):
        t, r = pairs[g].get("treight"), pairs[g].get("roaring")
        if t and r:
            ratio = t / r
            word = "faster" if ratio < 1 else "slower"
            axp.annotate(
                f"treight {word}\n{max(ratio, 1 / ratio):.1f}×",
                (xi, max(t, r)), textcoords="offset points", xytext=(0, 14),
                ha="center", fontsize=6.8,
                color="#0072b2" if ratio < 1 else "#d55e00")

    fig.suptitle(f"CPU per operation — treight vs roaring ({corpus})",
                 fontsize=10, weight="bold")
    if synthetic:
        synthetic_stamp(fig)
    fig.tight_layout(rect=(0, 0, 1, 0.94))
    save_both(fig, "chart-ops")
    return 0


if __name__ == "__main__":
    sys.exit(main())
