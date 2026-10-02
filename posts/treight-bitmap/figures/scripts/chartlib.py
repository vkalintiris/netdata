#!/usr/bin/env python3
"""Shared helpers for the treight blog data charts (diagram-eng).

Design contract for every chart script in this directory:

- Reads REAL evidence from posts/treight-bitmap/evidence/bench/ when
  present; otherwise generates clearly-marked SYNTHETIC sample data so the
  scripts can be developed and layout-checked before bench-eng's artifacts
  land (the schema is evidence/bench/schema.md). Synthetic mode stamps the
  plot title AND prints SYNTHETIC to stdout; --strict turns absence into an
  error instead.
- Okabe-Ito palette only; every color distinction is doubled by a hatch
  pattern and/or marker shape so charts survive greyscale printing and
  colorblind readers.
- Exports SVG (primary, text kept as text via svg.fonttype=none so the user
  can hand-edit labels in Inkscape/Illustrator) + PNG at 150 dpi.
- Run with: MPLCONFIGDIR=/tmp/nd-bp-mpl python3 scripts/<chart>.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

# Okabe-Ito
ORANGE = "#e69f00"
SKY = "#56b4e9"
GREEN = "#009e73"
YELLOW = "#f0e442"
BLUE = "#0072b2"
VERMIL = "#d55e00"
PURPLE = "#cc79a7"
GRAY = "#9a9a9a"
INK = "#1a1a1a"

CORPORA = ["certstream-250k", "jetstream-250k", "mixed-250k"]
CORPUS_COLORS = {  # corpus -> (color, hatch)
    "certstream-250k": (BLUE, "///"),
    "jetstream-250k": (GREEN, "\\\\\\"),
    "mixed-250k": (ORANGE, "xxx"),
}
IMPL_STYLE = {  # impl -> (color, hatch)
    "treight": (VERMIL, "///"),
    "roaring": (BLUE, ""),
}
DENSITY_BUCKETS = [
    "<=0.001%",
    "0.001-0.01%",
    "0.01-0.1%",
    "0.1-1%",
    "1-10%",
    "10-50%",
    ">50%",
]
OPS_ORDER = [
    "bulk_build",
    "insert_arrival",
    "insert_shuffled",
    "contains_hit",
    "contains_miss",
    "len_min_max",
    "iterate",
    "and",
    "or",
    "and_not",
    "range_count",
]
BUCKETS = ["lt1pct", "1to50pct", "gt50pct"]


def repo_figures_dir() -> Path:
    """posts/treight-bitmap/figures/, located relative to this file."""
    return Path(__file__).resolve().parent.parent


def evidence_dir(explicit: str | None = None) -> Path:
    if explicit:
        return Path(explicit).resolve()
    return repo_figures_dir().parent / "evidence" / "bench"


def load_json(path: Path):
    with open(path) as f:
        return json.load(f)


def find_stats(edir: Path, corpus: str) -> Path | None:
    """Locate a corpus's stats.json under the evidence dir.

    schema.md says 'one file per corpus' without fixing the layout; try the
    plausible shapes, newest first. Update here once bench-eng pins it.
    """
    candidates = [
        edir / corpus / "stats.json",
        edir / "stats" / f"{corpus}.json",
        edir / "stats" / corpus / "stats.json",
        edir / f"{corpus}-stats.json",
        edir / f"stats-{corpus}.json",
        edir / "derived" / corpus / "stats.json",
    ]
    for c in candidates:
        if c.is_file():
            return c
    return None


def available_corpora(edir: Path) -> list[str]:
    out = []
    for c in CORPORA:
        if find_stats(edir, c):
            out.append(c)
    return out


def synthetic_stamp(fig, text: str = "SYNTHETIC SAMPLE DATA — NOT MEASUREMENTS") -> None:
    fig.text(0.5, 0.5, text, fontsize=26, color=VERMIL, alpha=0.55,
             ha="center", va="center", rotation=18, weight="bold", zorder=1000)


def style_setup() -> None:
    plt.rcParams.update({
        "font.family": "DejaVu Sans",
        "font.size": 9,
        "text.color": INK,
        "axes.edgecolor": INK,
        "axes.labelcolor": INK,
        "axes.titlesize": 10,
        "axes.titleweight": "bold",
        "axes.grid": True,
        "grid.color": "#dddddd",
        "grid.linewidth": 0.6,
        "xtick.color": INK,
        "ytick.color": INK,
        "legend.frameon": False,
        "figure.facecolor": "white",
        "svg.fonttype": "none",  # keep text as text for hand-editing
        "axes.formatter.use_mathtext": True,
    })


def save_both(fig, chart: str) -> None:
    """Save fig to figures/<chart>/<chart>.{svg,png}."""
    outdir = out_dir(chart)
    svg = outdir / f"{chart}.svg"
    png = outdir / f"{chart}.png"
    fig.savefig(svg, bbox_inches="tight")
    fig.savefig(png, dpi=150, bbox_inches="tight")
    print(f"wrote {svg}")
    print(f"wrote {png}")


def out_dir(chart: str) -> Path:
    d = repo_figures_dir() / chart
    d.mkdir(parents=True, exist_ok=True)
    return d


def die_synth(chart: str) -> None:
    print(
        f"{chart}: no evidence found — SYNTHETIC SAMPLE MODE (plots are "
        f"layout placeholders, NOT measurements). Pass --strict to fail instead.",
        file=sys.stderr,
    )
