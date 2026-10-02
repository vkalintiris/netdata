# Reproducing the treight-vs-roaring benchmarks

This README reproduces every measurement quoted in the accompanying blog post (`draft.md`).
Every number in the post traces to an artifact under `posts/treight-bitmap/evidence/`; nothing is
quoted from memory.

## Prerequisites

- Rust (stable for benches; the fuzz target pins its own nightly via
  `src/crates/treight/fuzz/rust-toolchain.toml`). The capture toolchain of record is
  rustc/cargo 1.95.0 (`evidence/corpora/manifest.md`).
- Python 3 (aggregate corpus statistics).
- ~2 GB free disk: the three corpora are 161,877,948 bytes gzipped (858,335,905 uncompressed),
  plus derived posting-list data.
- A CertStream relay reachable over WebSocket — the capture of record used a local docker relay
  at `ws://127.0.0.1:8080/` (user-operated). The Jetstream endpoint is
  `wss://jetstream2.us-east.bsky.network/subscribe`, fallback
  `wss://jetstream.us-west.bsky.network/subscribe`; the `/subscribe` path must be part of the
  base URL (`jetstream::build_jetstream_url` only appends `wantedCollections` query parameters).

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
`otel-streams` capture tool (`src/crates/otel-streams/src/bin/capture.rs`, bin `capture`). It
reuses the library's `certstream::connect` and `jetstream::connect` — the identical
WebSocket→JSON→`serde_json::Value` parsing path the production OTLP bridges use — and writes each
raw event as one canonical JSONL line, gzip level 6, no timestamp in the gzip header.

| corpus | source | events | uncompressed B | compressed B |
|---|---|---|---|---|
| `certstream-250k` | Certificate Transparency via local CertStream relay | 250,000 | 413,067,184 | 63,839,734 |
| `jetstream-250k` | Bluesky Jetstream firehose | 250,000 | 150,391,274 | 41,886,887 |
| `mixed-250k` | both, interleaved by arrival, `_capture_src`-tagged | 250,000 (125,000 + 125,000) | 294,877,447 | 56,151,327 |

Commands of record (from `src/crates/`, binary invoked from the repository root):

```sh
cd src/crates && CCACHE_DIR=/tmp/nd-bp-ccache cargo build --release -p otel-streams --bin capture

src/crates/target/release/capture --source certstream --out data/corpus/certstream-250k.jsonl.gz --max-events 250000
src/crates/target/release/capture --source jetstream  --out data/corpus/jetstream-250k.jsonl.gz  --max-events 250000
src/crates/target/release/capture --source mixed      --out data/corpus/mixed-250k.jsonl.gz      --max-events 250000 --per-source 125000
```

Each capture writes to `<out>.tmp` and renames only on clean completion — a stall (300 s default
without events) or a kill aborts it; discard the `.tmp` and redo. Sources reconnect with
exponential backoff (1 s → 60 s cap), mirroring `runner::run`. Resume-from-offset is deliberately
not implemented: both are firehoses, and a same-stream resume would silently introduce a gap.
A re-capture can never be byte-identical — these are live streams — so the committed SHA256s pin
exactly the instance every published benchmark ran against.

The corpora are public internet data and contain personal data (DIDs, handles, certificate
subject names). They stay local in gitignored `data/corpus/`; nothing under `data/` is committed,
and committed evidence holds fingerprints and aggregate-only statistics. Only events that
deserialize through the source modules are captured (heartbeats, unknown `message_type`s, serde
failures are skipped inside `connect()`), exactly as in production; one such skip is documented in
the manifest (Jetstream `account` events with status `"takendown"` fail `AccountStatus`
deserialization and are absent from the corpora).

## 2. Verify the capture

```sh
# gzip integrity
gzip -t data/corpus/*.jsonl.gz

# fingerprints against the committed evidence (paths relative to the repo root)
sha256sum data/corpus/*.jsonl.gz | diff - posts/treight-bitmap/evidence/corpora/sha256sums.txt

# exact line counts (250,000 per corpus; mixed = 125,000 + 125,000 by _capture_src tag)
zcat data/corpus/certstream-250k.jsonl.gz | wc -l
zcat data/corpus/mixed-250k.jsonl.gz | grep -c '"_capture_src":"certstream"'
```

Replay a 1,000-line sample of each corpus through the real parsing path — the env-gated test
converts lines to OTel `LogRecord`s via the public `Source` trait, the same
`event_to_log_record` call `runner::run` makes per event (from `src/crates/`):

```sh
CORPUS_REPLAY_FILE=data/corpus/certstream-250k.jsonl.gz CORPUS_REPLAY_LIMIT=1000 \
  CCACHE_DIR=/tmp/nd-bp-ccache cargo test -p otel-streams --test replay_corpus replay_captured_corpus
```

Regenerate the aggregate-only corpus statistics (structural keys only — no personal identifiers
are emitted; `stats.md` in the same directory is the committed render of the capture of record):

```sh
python3 posts/treight-bitmap/evidence/corpora/corpus_stats.py \
    data/corpus/certstream-250k.jsonl.gz --label certstream-250k
```

## 3. Derive posting lists

Posting lists are derived from each corpus the way the production indexer derives them — the
verified production flatten path is documented in `posts/treight-bitmap/evidence/bench/TRACE.md`.
The derivation lives in the `treight-bench` dev crate (`src/crates/treight-bench`); the
derived-cache format is documented in its `src/lib.rs` and version-tagged in each cache's
`meta.json`. Run from the repo root:

```sh
for c in certstream jetstream mixed; do
  CCACHE_DIR=/tmp/nd-bp-ccache cargo run -q -p treight-bench --bin treight-derive -- \
      --corpus data/corpus/$c-250k.jsonl.gz --out data/derived/$c-250k
done
```

Derived caches are gitignored under `data/derived/<corpus>/`.

## 4. Index-footprint totals and CPU benchmarks

Footprint totals — treight vs roaring serialized bytes over all posting lists, per density
bucket, plus allocative in-memory sizes — come from `treight-footprint`:

```sh
for c in certstream jetstream mixed; do
  CCACHE_DIR=/tmp/nd-bp-ccache cargo run -q -p treight-bench --bin treight-footprint -- \
      --derived data/derived/$c-250k \
      --json posts/treight-bitmap/evidence/bench/totals-$c-250k.json \
      --md   posts/treight-bitmap/evidence/bench/totals-$c-250k.md
done
```

CPU benches are the criterion suite `vs_roaring` in `src/crates/treight/benches/`, following the
env-var corpus pattern of `src/crates/sfst/benches/decode.rs` — it skips quietly when the env var
is unset:

```sh
CCACHE_DIR=/tmp/nd-bp-ccache \
TREIGHT_BENCH_DERIVED=$PWD/data/derived \
  cargo bench -p treight --features roaring --bench vs_roaring
```

Harvest criterion medians into the evidence files:

```sh
python3 posts/treight-bitmap/evidence/bench/harvest.py \
    --derived data/derived \
    --criterion src/crates/target/criterion \
    --out posts/treight-bitmap/evidence/bench
```

The roaring side of every measurement is the workspace's pinned `netdata/roaring-rs` fork
(branch `allocative`, commit `3ee69b1db5a2a4fc909076962c0159ddae893f8c` per `Cargo.lock`) — the
same implementation the differential fuzzer uses; roaring's `allocative` feature is enabled for
the in-memory deep-size numbers.

Methodology constants for the published numbers: AMD Ryzen 9 5900X (a shared machine — timings
are criterion medians over 50 samples with 1 s warm-up + 2 s measurement; compare orderings, not
sub-percent deltas), 31 GB RAM, rustc 1.95.0 (59807616e 2026-04-14), criterion 0.5.1. Full
disclosure in `evidence/bench/environment.txt`.

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
├── style-notes.md           prose research input (advisory)
├── figures/                 typst sources + SVG/PNG renders, one dir per figure
└── evidence/
    ├── corpora/             capture manifest, SHA256s, aggregate-only stats
    │   ├── manifest.md      capture tool, commands of record, validation
    │   ├── sha256sums.txt   fingerprints of the frozen corpora
    │   ├── stats.{json,md}  aggregate statistics (no personal data)
    │   └── corpus_stats.py  the tool that computes them
    └── bench/               benchmark evidence
        ├── environment.txt  hardware, toolchain, roaring pin, exact commands
        ├── TRACE.md         the verified production flatten path the derivation mirrors
        ├── schema.md        the data contract for the JSON files here
        ├── harvest.py       criterion-median harvester (step 4)
        ├── totals-<corpus>-250k.{json,md}   footprint totals + per-density buckets
        └── results.json     criterion CPU medians (per corpus × bucket × op × impl)
```
