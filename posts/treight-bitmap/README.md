# Reproducing the treight-vs-roaring benchmarks

This README reproduces every measurement quoted in the accompanying blog post (`draft.md`).
Every number in the post traces to an artifact under `posts/treight-bitmap/evidence/`; nothing is
quoted from memory.

## Prerequisites

- Rust (stable for benches; the fuzz target pins its own nightly via
  `src/crates/treight/fuzz/rust-toolchain.toml`).
- Python 3 (aggregate corpus statistics).
- ~2 GB free disk for the three corpora.
- A `certstream-server` WebSocket endpoint if you want to re-capture the CT corpus live
  ({{PLACEHOLDER: corpus-eng#certstream-endpoint-doc}} — the capture used for the published
  numbers ran against a local docker instance).

> **Note on this dev box:** `cc` is wrapped by ccache (`/usr/local/bin/cc -> /usr/bin/ccache`) and
> ccache's default cache directory is read-only under the session sandbox. Prefix every
> `cargo build`/`cargo bench`/`cargo test` invocation as shown below, or set `CCACHE_DIR` once in
> your shell. This is an environment artifact of the machine the published numbers were produced
> on, not a project requirement.

```sh
export CCACHE_DIR=/tmp/nd-bp-ccache
```

## 1. Re-capture the corpora

The three corpora are frozen gzipped JSONL captures of real public event streams, taken with the
`otel-streams` capture tool (a dev bin in `src/crates/otel-streams`, reusing the library's live
source modules — no agent surface involved):

| corpus | source | events |
|---|---|---|
| `certstream-250k` | Certificate Transparency log entries (certstream WebSocket) | ~250K |
| `jetstream-250k` | Bluesky Jetstream firehose (jetstream2 relay) | ~250K |
| `mixed-250k` | both of the above, interleaved by arrival, per-line `_capture_src` tag | ~250K |

{{PLACEHOLDER: corpus-eng#capture-commands — exact `capture` invocations (source flags, count
limits, output paths), to be relayed when the tool settles.}}

Output lands in `data/corpus/` (gitignored):

```
data/corpus/certstream-250k.jsonl.gz
data/corpus/jetstream-250k.jsonl.gz
data/corpus/mixed-250k.jsonl.gz
```

The corpora are public internet data and contain personal data (DIDs, handles, certificate
subject names). They stay local; nothing under `data/` is committed. Committed evidence holds
fingerprints and aggregate-only statistics.

## 2. Verify the capture

```sh
# fingerprints + line counts against the committed evidence
sha256sum data/corpus/*.jsonl.gz     # compare: posts/treight-bitmap/evidence/corpora/
zcat data/corpus/certstream-250k.jsonl.gz | wc -l
```

{{PLACEHOLDER: corpus-eng#fingerprints-file — committed SHA256s/line-count file name.}}

Regenerate the aggregate-only corpus statistics (structural keys only — no personal identifiers
are emitted):

```sh
python3 posts/treight-bitmap/evidence/corpora/corpus_stats.py \
    data/corpus/certstream-250k.jsonl.gz --label certstream-250k
```

## 3. Derive posting lists

Posting lists are derived from each corpus the same way the production indexer derives them
(following the verified production flatten path — {{PLACEHOLDER: bench-eng#flatten-path-note}}).

{{PLACEHOLDER: bench-eng#derivation-command — tool name + invocation producing per-corpus posting
lists and the density/cardinality histograms under evidence/bench/.}}

## 4. Run the benchmarks

Criterion suite in `src/crates/treight/benches/` ({{PLACEHOLDER: bench-eng#bench-target-name}}),
following the env-var corpus pattern of `src/crates/sfst/benches/decode.rs` — the suite skips
quietly when the env var is unset:

```sh
CCACHE_DIR=/tmp/nd-bp-ccache \
{{PLACEHOLDER: bench-eng#bench-env-and-command — env var name + cargo bench invocation}} \
    > evidence/bench/<corpus>.txt
```

The roaring side of every bench is the workspace's pinned `netdata/roaring-rs` fork (branch
`allocative`, `src/crates/Cargo.toml`) — the same implementation the differential fuzzer uses.

Methodology constants for the published numbers: AMD Ryzen 9 5900X, 31 GB RAM;
rustc {{PLACEHOLDER: evidence/bench/env#rustc-version}}. Raw criterion output is committed as JSON
+ markdown under `evidence/bench/`.

## 5. Correctness checks behind the post's claims

```sh
# differential fuzzing vs roaring (nightly pinned by the fuzz workspace)
cd src/crates/treight/fuzz
cargo fuzz run against_roaring

# in-crate proptest twin + unit tests
cd .. && CCACHE_DIR=/tmp/nd-bp-ccache cargo test --features roaring
```

The fuzz target replays random op sequences against a roaring oracle and closes with the
canonical-size check (`estimate_data_size` must equal the final blob length). The proptest twin
(`src/crates/treight/src/tests_roaring.rs`) does exhaustive membership checks up to 4,096-element
universes, all inverted-flag combinations included.

## Evidence layout

```
posts/treight-bitmap/
├── draft.md                 the post (numbers cite the files below)
├── outline.md               approved section plan
├── README.md                this file
└── evidence/
    ├── corpora/             capture commands, SHA256s, aggregate-only stats
    │   └── corpus_stats.py  aggregate statistics tool (no personal data out)
    └── bench/               criterion raw output, footprint totals, histograms
```

{{PLACEHOLDER: bench-eng#evidence-file-names — final artifact names as landed.}}
