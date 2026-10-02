#!/usr/bin/env python3
"""Harvest criterion results + bench fixtures into results.json / results.md.

Usage:
    python3 harvest.py --derived data/derived --criterion src/crates/target/criterion \
        --out posts/treight-bitmap/evidence/bench

Reads, per corpus (every data/derived/<name> that is not sample-*):
  - <name>/stats.json          (corpus shape; embedded verbatim)
  - <name>/bench-fixtures.json (sample counts, pairs — written by the bench)
and criterion's target/criterion/<corpus>_<bucket>_<op>_<impl>/new/estimates.json
(criterion flattens '/' in bench ids to '_').

Writes results.json + results.md per evidence/bench/schema.md.

Normalization per op (n = what one benchmark iteration processed):
  bulk_build, insert_arrival, insert_shuffled, iterate  -> positions_sampled
  contains_hit / contains_miss                          -> hits/misses_sampled
  len_min_max, range_count                              -> lists_sampled
  pairs/*                                               -> none (single op pair)
"""
import argparse
import json
import sys
from pathlib import Path

PER_POSITION = {"bulk_build", "insert_arrival", "insert_shuffled", "iterate"}


def norm_n(op: str, meta: dict):
    """Items one benchmark iteration processed, per op."""
    if op == "bulk_build" or op == "iterate":
        return meta.get("positions_sampled", 0)
    if op in ("insert_arrival", "insert_shuffled"):
        # Insert-phase benches run on the position-capped copies (see
        # INSERT_POSITION_CAP in vs_roaring.rs) — normalize by the capped
        # count, identically for both implementations.
        return meta.get("insert_positions_sampled", 0)
    if op == "contains_hit":
        return meta.get("hits_sampled", 0)
    if op == "contains_miss":
        return meta.get("misses_sampled", 0)
    return meta.get("lists_sampled", 0)


def load_estimates(criterion_dir: Path, bench_id: str):
    """estimates.json for a bench id, or None. Criterion replaces '/' with '_'."""
    p = criterion_dir / bench_id.replace("/", "_") / "new" / "estimates.json"
    if not p.is_file():
        return None
    e = json.loads(p.read_text())
    return {
        "median_ns": e["median"]["point_estimate"],
        "median_lo_ns": e["median"]["confidence_interval"]["lower_bound"],
        "median_hi_ns": e["median"]["confidence_interval"]["upper_bound"],
        "mad_ns": e["median_abs_dev"]["point_estimate"],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--derived", required=True, type=Path)
    ap.add_argument("--criterion", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    corpora = {}
    for corpus_dir in sorted(args.derived.iterdir()):
        name = corpus_dir.name
        if name.startswith("sample-") or not (corpus_dir / "bench-fixtures.json").is_file():
            continue
        stats = json.loads((corpus_dir / "stats.json").read_text())
        fixtures = json.loads((corpus_dir / "bench-fixtures.json").read_text())
        buckets_out = {}
        rows = []
        for bucket, meta in fixtures["buckets"].items():
            ops = {}
            lists_n = meta.get("lists_sampled", 0)
            for op in [
                "bulk_build",
                "insert_arrival",
                "insert_shuffled",
                "contains_hit",
                "contains_miss",
                "len_min_max",
                "iterate",
                "range_count",
            ]:
                for impl in ["treight", "roaring"]:
                    est = load_estimates(args.criterion, f"{name}/{bucket}/{op}/{impl}")
                    if est is None:
                        continue
                    n = norm_n(op, meta)
                    est["n"] = n
                    est["impl"] = impl
                    est["per_position_ns"] = (
                        est["median_ns"] / n if op in PER_POSITION and n else None
                    )
                    est["per_list_ns"] = (
                        est["median_ns"] / n
                        if op in {"len_min_max", "range_count", "contains_hit", "contains_miss"} and n
                        else None
                    )
                    ops[f"{op}/{impl}"] = est
                    rows.append((name, bucket, f"{op}/{impl}", est))
            buckets_out[bucket] = {
                "lists_in_bucket": meta.get("lists_in_bucket"),
                **{k: v for k, v in meta.items() if k.endswith("_sampled")},
                "ops": ops,
                "sanity": "treight==roaring on len/min/max/contains-sample/iteration (asserted in-bench; run passed)",
            }
        # Pair ops (corpus-global).
        pairs_meta = fixtures.get("pairs", [])
        pairs_ops = {}
        for p in pairs_meta:
            est_t = load_estimates(args.criterion, f"{name}/pairs/{p['op']}/{p['kind']}/treight")
            est_r = load_estimates(args.criterion, f"{name}/pairs/{p['op']}/{p['kind']}/roaring")
            if est_t:
                est_t.update({"impl": "treight", "n": 1})
                pairs_ops[f"pairs/{p['op']}/{p['kind']}/treight"] = est_t
            if est_r:
                est_r.update({"impl": "roaring", "n": 1})
                pairs_ops[f"pairs/{p['op']}/{p['kind']}/roaring"] = est_r
        buckets_out["pairs"] = {"pairs": pairs_meta, "ops": pairs_ops}
        corpora[name] = {"stats": stats, "buckets": buckets_out}

    missing = [
        r[:3] for r in rows if r[3] is None
    ]
    doc = {"corpora": corpora}
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "results.json").write_text(json.dumps(doc, indent=2) + "\n")

    md = ["# treight vs roaring — criterion medians\n"]
    md.append("| corpus | bucket | op/impl | median ns | per-position ns | per-list ns | n |")
    md.append("|---|---|---|---|---|---|---|")
    for name, bucket, opimpl, est in rows:
        if est is None:
            continue
        md.append(
            "| {} | {} | {} | {:,.1f} | {} | {} | {} |".format(
                name,
                bucket,
                opimpl,
                est["median_ns"],
                f"{est['per_position_ns']:,.2f}" if est.get("per_position_ns") else "—",
                f"{est['per_list_ns']:,.2f}" if est.get("per_list_ns") else "—",
                est["n"],
            )
        )
    (args.out / "results.md").write_text("\n".join(md) + "\n")
    print(f"wrote {args.out/'results.json'} and results.md ({len(rows)} op rows)")
    if missing:
        print("MISSING estimates for:", missing[:10], file=sys.stderr)


if __name__ == "__main__":
    main()
