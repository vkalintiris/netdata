# Corpus capture manifest — treight bitmap blog

Three frozen, real-internet log corpora captured through otel-streams' production source
modules for the treight-vs-roaring benchmark series. This manifest records exactly how each
corpus was produced, its fingerprints, and its validation results, so any number in the blog
post can be traced back to bytes on disk.

- Capture tool: `src/crates/otel-streams/src/bin/capture.rs` (bin `capture`; registered in
  `src/crates/otel-streams/Cargo.toml`). It reuses the library's `certstream::connect` and
  `jetstream::connect` — the identical WebSocket→JSON→`serde_json::Value` parsing path the
  production OTLP bridges use — and serializes each raw event value to one JSONL line,
  gzip-compressed (level 6, no timestamp in the gzip header).
- Toolchain: rustc 1.95.0 (59807616e 2026-04-14), cargo 1.95.0.
- Machine: AMD Ryzen 9 5900X, 31 GiB RAM (the same machine the benchmarks run on).
- Corpus files live in gitignored `data/corpus/` (public-internet data containing personal
  identifiers — certificate subject names, DIDs, handles — so they stay local; only the
  aggregates in this directory are committed).

## Sources

| Source | Endpoint | Notes |
|---|---|---|
| certstream | `ws://127.0.0.1:8080/` | local CertStream relay docker (user-operated), fed by Certificate Transparency |
| jetstream | `wss://jetstream2.us-east.bsky.network/subscribe` | Bluesky Jetstream firehose |

Fallback if `jetstream2.us-east` is unreachable: `wss://jetstream.us-west.bsky.network/subscribe`
(the `/subscribe` path must be part of the base URL — `jetstream::build_jetstream_url` only
appends `wantedCollections` query parameters).

## Captures

All commands run from the repository root; the binary is built with
`cargo build --release -p otel-streams --bin capture` (from `src/crates/`).

| Corpus | Command | UTC window | Duration | Events | Uncompressed B | Compressed B |
|---|---|---|---|---|---|---|
| certstream-250k | `src/crates/target/release/capture --source certstream --out data/corpus/certstream-250k.jsonl.gz --max-events 250000` | 2026-10-02T17:39:59Z → 17:44:57Z | 298 s (838 ev/s) | 250,000 | 413,067,184 | 63,839,734 |
| jetstream-250k | `src/crates/target/release/capture --source jetstream --out data/corpus/jetstream-250k.jsonl.gz --max-events 250000` | 2026-10-02T17:39:59Z → 17:49:30Z | 571 s (437 ev/s) | 250,000 | 150,391,274 | 41,886,887 |
| mixed-250k | `src/crates/target/release/capture --source mixed --out data/corpus/mixed-250k.jsonl.gz --max-events 250000 --per-source 125000` | 2026-10-02T17:45:07Z → 17:49:53Z | 286 s (874 ev/s) | 250,000 (125,000 + 125,000) | 294,877,447 | 56,151,327 |

SHA256 fingerprints of the frozen files: see `sha256sums.txt` (paths relative to the repository
root). A re-capture can never be byte-identical — these are live streams — so the hashes pin
exactly the instance every benchmark in this series ran against.

## Line formats (agreed with bench-eng, task-2)

One JSON object per line, gzip-compressed JSONL. `serde_json` serializes its `Value` map with
sorted keys, so captured lines are canonical; this is byte-order-only — the production pipeline
also goes text → `Value` → conversion, so semantics are untouched.

- `certstream-250k.jsonl.gz`: the raw CertStream message per line, unchanged:
  `{"message_type":"certificate_update","data":{...}}`.
- `jetstream-250k.jsonl.gz`: the raw Jetstream event per line, unchanged:
  `{"did":...,"time_us":...,"kind":"commit"|"identity"|"account","commit"?:{...},"identity"?:{...},"account"?:{...}}`.
- `mixed-250k.jsonl.gz`: both sources connected concurrently, lines merged in arrival order at
  the writer (per-source interleaving follows live timing; per-source counts are fixed at
  125,000 each for determinism), each line tagged:
  `{"_capture_src":"certstream"|"jetstream","event":{...the same raw object as above...}}`.

Only events that deserialize through the source modules are captured — heartbeats, unknown
`message_type`s, and JSON that fails `serde` parsing are skipped inside `connect()`, exactly as
in production. One such production skip was observed live: Jetstream `account` events with
status `"takendown"` fail `AccountStatus` deserialization (the enum expects `taken_down`), so
they are absent from the corpora. This mirrors the production bridge's behavior faithfully
(fixing it is a library change, out of scope for the capture tool; flagged to the Lead).

## Validation performed (all passing)

- `gzip -t` integrity on all three files.
- Exact line counts: `zcat | wc -l` = 250,000 per corpus; mixed per-source tag counts = 125,000
  `certstream` + 125,000 `jetstream` (grep over the `_capture_src` tag).
- Replay through the real parsing path: the env-gated test
  `src/crates/otel-streams/tests/replay_corpus.rs` (`replay_captured_corpus`) converts a
  1,000-line sample of each corpus to OTel `LogRecord`s via the public `Source` trait — the
  same `event_to_log_record` call `runner::run` makes per event:
  `CORPUS_REPLAY_FILE=data/corpus/<corpus>.jsonl.gz CORPUS_REPLAY_LIMIT=1000 cargo test -p otel-streams --test replay_corpus replay_captured_corpus`
  (run from `src/crates/`). 1,000/1,000 lines converted per corpus.
- Cross-check: `stats.json`'s per-corpus `uncompressed_bytes` equals the capture log's written
  byte count exactly (413,067,184 / 150,391,274 / 294,877,447).

## Restart safety

- Transient source disconnects are tolerated: each source reconnects with exponential backoff
  (1 s → 60 s cap), mirroring `runner::run`; captures above ran without intervention.
- A stall (`--stall-timeout-secs`, default 300 s with no events) or a killed process aborts the
  capture. Output is written to `<out>.tmp` and renamed only on clean completion, so the final
  path always holds a finished gzip stream; discard the `.tmp` and redo the capture.
- Resume-from-offset is deliberately not implemented: both sources are firehoses, a redo costs
  minutes, and a same-stream resume would silently introduce a gap. Counts, not continuity,
  carry the corpora's determinism.

## Regenerating the stats

From the repository root:

```sh
python3 posts/treight-bitmap/evidence/corpora/corpus_stats.py data/corpus/<corpus>.jsonl.gz
```

emits the same aggregate JSON found in `stats.json` (rendered in `stats.md`): line counts,
byte-size histograms and percentiles, message-type distributions, and top-level key
frequencies. The script counts structural fields only — no personal data is emitted.
