#!/usr/bin/env python3
"""Aggregate statistics for corpora captured by otel-streams' `capture` bin.

Reads a gzipped JSONL corpus (raw certstream messages, raw jetstream events,
or the mixed tagged envelope) and prints AGGREGATE-ONLY statistics as JSON:
line counts per source, per-event byte-size histogram and percentiles,
message-type distributions, and top-level JSON key frequencies.

No personal data (DIDs, handles, certificate subject names) is emitted: every
counter keys on structural fields only. Usage:

    corpus_stats.py data/corpus/certstream-250k.jsonl.gz [--label LABEL]
"""

from __future__ import annotations

import argparse
import gzip
import json
import sys
from collections import Counter

# Bucket edges (bytes) for the per-event (per-line) size histogram.
SIZE_EDGES = [256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536]


def bucket(size: int) -> str:
    lo = 0
    for edge in SIZE_EDGES:
        if size < edge:
            return f"{lo}-{edge}" if lo else f"<{edge}"
        lo = edge
    return f">={lo}"


def pct(sorted_sizes: list[int], p: float) -> int:
    if not sorted_sizes:
        return 0
    idx = min(int(len(sorted_sizes) * p), len(sorted_sizes) - 1)
    return sorted_sizes[idx]


def empty_stats() -> dict:
    return {
        "lines": 0,
        "uncompressed_bytes": 0,
        "sizes": [],
        "size_histogram": Counter(),
        "top_level_keys": Counter(),
        "certstream": {
            "update_type": Counter(),
            "leaf_cert_keys": Counter(),
        },
        "jetstream": {
            "kind": Counter(),
            "commit_keys": Counter(),
            "commit_operation": Counter(),
            "commit_collection_top20": Counter(),
            "account_status": Counter(),
        },
    }


def fold_certstream(data: dict, stats: dict) -> None:
    inner = stats["certstream"]
    inner["update_type"][str(data.get("update_type"))] += 1
    leaf = data.get("leaf_cert") or {}
    for key in leaf:
        inner["leaf_cert_keys"][key] += 1


def fold_jetstream(event: dict, stats: dict) -> None:
    inner = stats["jetstream"]
    inner["kind"][str(event.get("kind"))] += 1
    if isinstance(event.get("commit"), dict):
        commit = event["commit"]
        for key in commit:
            inner["commit_keys"][key] += 1
        inner["commit_operation"][str(commit.get("operation"))] += 1
        inner["commit_collection_top20"][str(commit.get("collection"))] += 1
    if isinstance(event.get("account"), dict):
        inner["account_status"][str(event["account"].get("status"))] += 1


def fold_event(event: dict, stats: dict) -> None:
    # `event` is the source event object: the raw line for jetstream, the
    # `data` object for certstream (whose envelope only wraps message_type +
    # data, both always present on captured lines).
    if "message_type" in event:
        data = event.get("data")
        if isinstance(data, dict):
            for key in data:
                stats["top_level_keys"][key] += 1
            fold_certstream(data, stats)
    else:
        for key in event:
            stats["top_level_keys"][key] += 1
        if "kind" in event:
            fold_jetstream(event, stats)


def bucket_lower(name: str) -> int:
    if name.startswith("<"):
        return 0
    digits = name.strip(">=#").split("-")[0]
    return int(digits) if digits.isdigit() else 0


def finalize(stats: dict) -> dict:
    sizes = sorted(stats.pop("sizes"))
    out = {
        "lines": stats["lines"],
        "uncompressed_bytes": stats["uncompressed_bytes"],
        "size_histogram": dict(
            sorted(stats["size_histogram"].items(), key=lambda kv: bucket_lower(kv[0])
        )),
        "size_bytes": {
            "min": sizes[0] if sizes else 0,
            "p50": pct(sizes, 0.50),
            "p95": pct(sizes, 0.95),
            "p99": pct(sizes, 0.99),
            "max": sizes[-1] if sizes else 0,
        },
        "top_level_key_frequency": dict(
            stats["top_level_keys"].most_common()
        ),
    }
    if any(stats["certstream"]["update_type"].values()):
        out["certstream"] = {
            "update_type": dict(stats["certstream"]["update_type"]),
            "leaf_cert_key_frequency": dict(
                stats["certstream"]["leaf_cert_keys"].most_common()
            ),
        }
    if any(stats["jetstream"]["kind"].values()):
        js = stats["jetstream"]
        out["jetstream"] = {
            "kind": dict(js["kind"]),
            "commit_key_frequency": dict(js["commit_keys"].most_common()),
            "commit_operation": dict(js["commit_operation"]),
            "commit_collection_top20": dict(
                js["commit_collection_top20"].most_common(20)
            ),
            "account_status": dict(js["account_status"]),
        }
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("input", help="gzipped JSONL corpus file")
    ap.add_argument("--label", default=None, help="corpus label for the output")
    args = ap.parse_args()

    per_source: dict[str, dict] = {}
    envelope_keys: Counter = Counter()

    with gzip.open(args.input, "rt") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line.strip():
                continue
            obj = json.loads(line)
            if "_capture_src" in obj:
                envelope_keys["_capture_src"] += 1
                envelope_keys["event"] += 1
                src, event = obj["_capture_src"], obj["event"]
            else:
                src, event = detect_source(obj), obj
            stats = per_source.setdefault(src, empty_stats())
            stats["lines"] += 1
            stats["uncompressed_bytes"] += len(line.encode()) + 1
            stats["sizes"].append(len(line.encode()) + 1)
            stats["size_histogram"][bucket(len(line.encode()) + 1)] += 1
            fold_event(event, stats)

    result = {
        "label": args.label or args.input,
        "input": args.input.rsplit("/", 1)[-1],
        "per_source": {
            src: finalize(stats) for src, stats in sorted(per_source.items())
        },
    }
    if envelope_keys:
        result["mixed_envelope_key_frequency"] = dict(envelope_keys)
    json.dump(result, sys.stdout, indent=2, sort_keys=False)
    print()
    return 0


def detect_source(obj: dict) -> str:
    if "message_type" in obj:
        return "certstream"
    if "kind" in obj:
        return "jetstream"
    raise SystemExit(f"cannot detect source for line: {list(obj)[:5]}")


if __name__ == "__main__":
    sys.exit(main())
