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
    ">=50%",   # bucket is [0.5, 1.0]; the ENCODER complements only
               # strictly-above-half lists (card > universe/2) — exactly-50%
               # lists sit in this bucket but encode direct (schema.md §1
               # note; per-list `complemented` flag is authoritative)
]
OPS_ORDER = [  # ops under buckets.<b>.ops (certified results.json)
    "bulk_build",
    "insert_arrival",
    "insert_shuffled",
    "contains_hit",
    "contains_miss",
    "len_min_max",
    "iterate",
    "range_count",
]
# Set-op pair groups under buckets.pairs.ops — flat keys
# "pairs/<op>/<kind>/<impl>", one measured operand pair per group (n = 1):
# per-OPERATION medians, not per-position/per-list normalized.
PAIR_GROUPS = [
    ("and", "same-field-disjoint"),
    ("or", "cross-field"),
    ("and_not", "dense-vs-sparse"),
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


def repo_root() -> Path:
    # scripts/chartlib.py → figures/ → treight-bitmap/ → posts/ → repo root
    return Path(__file__).resolve().parents[4]


def find_stats(edir: Path, corpus: str) -> Path | None:
    """Locate a corpus's stats.json.

    bench-eng pins it at data/derived/<corpus>/stats.json (Lead relay,
    2026-10-02; observed for all six derived dirs). The evidence-dir copy
    is kept as a fallback in case one lands there later.
    """
    candidates = [
        repo_root() / "data" / "derived" / corpus / "stats.json",
        edir / corpus / "stats.json",
    ]
    for c in candidates:
        if c.is_file():
            return c
    return None


def totals_files(edir: Path, corpus: str) -> list[Path]:
    """Candidate totals-evidence files for one corpus, in priority order.

    Shipped shape (observed 2026-10-02): per-corpus `totals-<corpus>.json`
    whose root IS the per-corpus object (bitsets/complement/per_list/...).
    schema.md also allows a single `totals.json` with a `corpora` root —
    both are supported; per-corpus wins.
    """
    per_corpus = [
        edir / f"totals-{corpus}.json",
        edir / "totals" / f"{corpus}.json",
    ]
    single = edir / "totals.json"
    return [p for p in per_corpus if p.is_file()] + ([single] if single.is_file() else [])


def load_totals_summary(edir: Path, corpus: str) -> dict | None:
    """Per-corpus totals summary (small since the 22:26 regen dropped the
    per-list rows): bitsets, complement, lists_total, per_density_bucket.
    Accepts the shipped per-corpus shape (per-corpus object at the root)
    and the schema.md single-file shape (`corpora` root)."""
    for path in totals_files(edir, corpus):
        data = load_json(path)
        if "corpora" in data:
            data = data["corpora"].get(corpus, {})
        if data.get("bitsets"):
            return {k: data[k] for k in
                    ("bitsets", "complement", "lists_total", "per_density_bucket",
                     "universe", "notes")
                    if k in data}
    return None


# Provenance of the last-read per-list CSV (gzip comment: stride/full count).
PER_LIST_SAMPLE_INFO: dict[str, str] = {}


def per_list_csv(edir: Path, corpus: str) -> Path | None:
    """The per-list scatter source: per-list-<corpus>.csv.gz (schema.md §2).

    Stride-sampled by bench-eng to ≤ 200,000 rows (stride + full count ride
    in the gzip comment); the totals aggregates stay exact over ALL lists.
    """
    for cand in (edir / f"per-list-{corpus}.csv.gz",
                 edir / "per-list" / f"{corpus}.csv.gz"):
        if cand.is_file():
            return cand
    return None


def gzip_comment(path: Path) -> str:
    """Read a gzip member's FCOMMENT header field (zero-terminated string).

    Python's gzip module parses past it without exposing it; we walk the
    header by hand: magic(2) CM(1) FLG(1) MTIME(4) XFL(1) OS(1), then the
    optional fields in flag order — FEXTRA, FNAME, FCOMMENT.
    """
    with open(path, "rb") as f:
        head = f.read(10)
        if len(head) < 10 or head[:2] != b"\x1f\x8b":
            return ""
        flg = head[3]

        def read_cstr() -> bytes:
            buf = bytearray()
            while True:
                c = f.read(1)
                if not c or c == b"\x00":
                    break
                buf += c
            return bytes(buf)

        if flg & 0x04:  # FEXTRA
            xlen = int.from_bytes(f.read(2), "little")
            f.seek(xlen, 1)
        if flg & 0x08:  # FNAME
            read_cstr()
        if flg & 0x10:  # FCOMMENT
            return read_cstr().decode("utf-8", "replace").strip()
    return ""


def iter_per_list(edir: Path, corpus: str):
    """Yield {density, complemented, ratio, tier} per sampled posting list.

    ratio = treight_blob_bytes / roaring_serialized_bytes; rows with
    roaring_serialized_bytes == 0 are skipped. After iteration,
    PER_LIST_SAMPLE_INFO[corpus] holds the gzip comment (stride + full
    count) for provenance printing.
    """
    import csv
    import gzip
    import io

    path = per_list_csv(edir, corpus)
    if path is None:
        return
    PER_LIST_SAMPLE_INFO[corpus] = gzip_comment(path)
    with gzip.GzipFile(path, "rb") as gz:
        with io.TextIOWrapper(gz, newline="") as f:
            for row in csv.DictReader(f):
                roaring = int(row["roaring_serialized_bytes"])
                if roaring <= 0:
                    continue
                yield dict(
                    density=float(row["density"]),
                    complemented=row["complemented"] == "true",
                    ratio=int(row["treight_blob_bytes"]) / roaring,
                    tier=row.get("tier", ""),
                )


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
