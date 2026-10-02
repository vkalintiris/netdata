//! Shared substrate for the treight-vs-roaring blog benchmarks: turning a
//! captured real-log corpus into the exact posting lists the production SFST
//! indexer would build from it, plus the on-disk cache format the derive bin
//! writes and the bench suite reads.
//!
//! # The production path this mirrors (traced 2026-10-02, see
//! posts/treight-bitmap/evidence/bench/TRACE.md)
//!
//! 1. `otel-streams` sources convert each raw event to an OTLP `LogRecord`
//!    (`Source::event_to_log_record`, runner.rs) and batch them into
//!    `ExportLogsServiceRequest`s (`otel.rs::build_export_request`, default
//!    100 records/request, resource `service.name=<source>`, no namespace).
//! 2. Ingest normalizes + flattens each request — `ng_flatten::prepare_log_frame`
//!    (`normalize_log_request` then `flatten_log_request`) — and bincode-encodes
//!    it into a flattened WAL frame (`LOG_FRAME_PAYLOAD_FORMAT` = 1).
//! 3. Seal decodes each frame and interns every entry — resource ++ scope ++
//!    record — rendering `key=value` via `ng_flatten::build_kv`
//!    (`ng_index::sfst_build::populate_row_index`), then feeds one row per
//!    record to `sfst::RowIndex::row(ts, &tokens)`.
//! 4. `RowIndex::row` appends the row at the next insertion-order position and
//!    `RoaringBitmap::insert`s that position into the per-`key=value` bitmap:
//!    THE posting lists this crate reconstructs.
//! 5. At write time `sfst::build::remap_one_bitmap` remaps each bitmap from
//!    insertion order to chronological order (the `TimeOrder` permutation from
//!    the per-row timestamps), complements above 50% density, and encodes it
//!    as a treight `Bitmap` (bitset path at ≥ max(universe/64, 256) positions).
//!
//! This crate calls the production functions for steps 1–3 (otel-streams'
//! `Source` impls, `build_export_request`, `prepare_log_frame`,
//! `decode_log_frame`, `build_kv`) and mirrors only the trivial step-4
//! accumulation (`HashMap<key=value, ascending positions>`, semantically
//! identical to the interner + per-slot roaring inserts) and the step-5
//! permutation (`build_time_order` mirrored from sfst's row_index.rs).
//!
//! Determinism: `event_to_log_record` stamps `observed_time_unix_nano` from
//! the wall clock, but both sources always set `time_unix_nano` from event
//! data (certstream `seen`, jetstream `time_us`), so the resolved row `ts`
//! — the only wall-clock-independent input to `TimeOrder` — is deterministic.
//! `observed_ts` is a stored column, never a posting-list input.

use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use ng_flatten::FlattenedLogRequest;
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use otel_streams::certstream::{self, Certstream};
use otel_streams::jetstream::{self, Jetstream};
use otel_streams::otel::build_export_request;
use otel_streams::Source;

/// Derived-cache format version — bump on any layout change to
/// `timestamps.bin` / `to_sorted.bin` / `postings.bin` / `meta.json`.
pub const CACHE_FORMAT_VERSION: u32 = 1;

/// Which pipeline a corpus line belongs to. Single-source corpora carry no
/// envelope (the source is a property of the file); the mixed corpus wraps
/// each line in `{"_capture_src": ..., "event": {...}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTag {
    Certstream,
    Jetstream,
}

impl SourceTag {
    /// The exact `service.name` / scope identity otel-streams' runner stamps
    /// on requests from this source (`Source::{SERVICE_NAME, SCOPE_NAME,
    /// SCOPE_VERSION}`, runner.rs) — what `build_export_request` receives.
    pub fn export_identity(self) -> (&'static str, &'static str, &'static str) {
        match self {
            SourceTag::Certstream => (
                Certstream::SERVICE_NAME,
                Certstream::SCOPE_NAME,
                Certstream::SCOPE_VERSION,
            ),
            SourceTag::Jetstream => (
                Jetstream::SERVICE_NAME,
                Jetstream::SCOPE_NAME,
                Jetstream::SCOPE_VERSION,
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SourceTag::Certstream => "certstream",
            SourceTag::Jetstream => "jetstream",
        }
    }
}

/// One parsed corpus line: its source and the production `LogRecord`.
pub fn line_to_record(line: &str, file_source: Option<SourceTag>) -> Result<(SourceTag, LogRecord)> {
    let value: serde_json::Value =
        serde_json::from_str(line).context("corpus line is not valid JSON")?;
    let (tag, event_value) = match file_source {
        Some(tag) => (tag, value),
        None => {
            // Mixed envelope: {"_capture_src": "...", "event": {...}}.
            let tag = value
                .get("_capture_src")
                .and_then(|v| v.as_str())
                .with_context(|| "mixed-corpus line lacks _capture_src")?;
            let event = value
                .get("event")
                .cloned()
                .with_context(|| "mixed-corpus line lacks event")?;
            let tag = match tag {
                "certstream" => SourceTag::Certstream,
                "jetstream" => SourceTag::Jetstream,
                other => bail!("unknown _capture_src {other:?}"),
            };
            (tag, event)
        }
    };
    let record = match tag {
        SourceTag::Certstream => {
            let message: certstream::Message =
                serde_json::from_value(event_value.clone()).context("parse certstream message")?;
            match message {
                certstream::Message::CertificateUpdate(data) => {
                    Certstream::event_to_log_record(&data, &event_value)
                }
                // The capture tool skips non-certificate_update messages at
                // capture time (same filter as production connect()); a line
                // that still parses to Other is a corpus-format violation.
                certstream::Message::Other => bail!("certstream line is not a certificate_update"),
            }
        }
        SourceTag::Jetstream => {
            let event: jetstream::Event =
                serde_json::from_value(event_value.clone()).context("parse jetstream event")?;
            Jetstream::event_to_log_record(&event, &event_value)
        }
    };
    Ok((tag, record))
}

/// Open a `.jsonl` or `.jsonl.gz` corpus as a line iterator (gzip
/// decompression is by extension).
pub fn corpus_lines(path: &Path) -> Result<Box<dyn Iterator<Item = Result<String>>>> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let inner: Box<dyn Read> = if ext == "gz" || ext == "gzip" {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    Ok(Box::new(
        std::io::BufReader::new(inner)
            .lines()
            .filter(|l| match l {
                Ok(s) => !s.trim().is_empty(),
                Err(_) => true,
            })
            .map(|l| l.context("read corpus line")),
    ))
}

/// Detect the corpus kind from the first non-empty line: a mixed envelope
/// (`_capture_src` key) means per-line tagging; otherwise the whole file is
/// one source (certstream lines have `message_type`, jetstream lines `did`).
fn detect_source(first_line: &str) -> Result<Option<SourceTag>> {
    let value: serde_json::Value = serde_json::from_str(first_line)?;
    if value.get("_capture_src").is_some() {
        return Ok(None);
    }
    if value.get("message_type").is_some() {
        Ok(Some(SourceTag::Certstream))
    } else if value.get("did").is_some() {
        Ok(Some(SourceTag::Jetstream))
    } else {
        bail!("cannot detect source of corpus line: no message_type / did / _capture_src")
    }
}

/// The derivation accumulator — a semantic mirror of `sfst::RowIndex` +
/// `KeyValueInterner` restricted to what the benchmarks need: the per-
/// `key=value` posting lists (insertion-order positions, ascending because
/// rows arrive 0,1,2,… exactly as `RowIndex::row` numbers them) and the
/// per-row timestamps feeding the `TimeOrder` remap.
///
/// Interning here is a `HashMap<full key=value string, slot>`; production's
/// interner dedups by the same full string (the xxhash64 fast path re-checks
/// the string on every hit — `populate_row_index`'s `intern_one`), so slot
/// identity is equal by construction. What is intentionally NOT mirrored:
/// the arena, the id-translation canonical order, and the FST/tier chunk
/// layout — none of them observe the *contents* of a posting list.
pub struct Derivation {
    /// slot → full `key=value` string.
    pub kv_strings: Vec<String>,
    /// kv string → slot (the interner).
    lookup: HashMap<String, u32>,
    /// slot → row positions carrying it, ascending insertion order
    /// (the `kv_bitmaps` analog before the time-order remap).
    pub postings: Vec<Vec<u32>>,
    /// Resolved `ts` per row, insertion order (the `timestamps` analog).
    pub timestamps: Vec<i64>,
    /// Rows per source tag (mixed corpora interleave two pipelines).
    pub rows_per_source: [u64; 2],
}

impl Default for Derivation {
    fn default() -> Self {
        Self::new()
    }
}

impl Derivation {
    pub fn new() -> Self {
        Self {
            kv_strings: Vec::new(),
            lookup: HashMap::new(),
            postings: Vec::new(),
            timestamps: Vec::new(),
            rows_per_source: [0, 0],
        }
    }

    fn intern(&mut self, kv: &str) -> u32 {
        if let Some(&slot) = self.lookup.get(kv) {
            return slot;
        }
        let slot = self.kv_strings.len() as u32;
        self.kv_strings.push(kv.to_string());
        self.lookup.insert(kv.to_string(), slot);
        self.postings.push(Vec::new());
        slot
    }

    /// Feed one flattened request exactly as `populate_row_index` walks it:
    /// intern the resource entries once per group, the scope entries once per
    /// scope, and give every record a row of `resource ++ scope ++ record`
    /// tokens (`row_index.row(ts, &tokens)` → one posting-list hit per token).
    pub fn feed_flattened(&mut self, flattened: &FlattenedLogRequest, tag: SourceTag) {
        let tree = &flattened.tree;
        // populate_row_index resolves each node's path once per frame and
        // renders `key=value` via build_kv; same here.
        let mut paths: Vec<String> = Vec::with_capacity(tree.len());
        for id in 0..tree.len() as u32 {
            paths.push(tree.path(id));
        }
        let mut kv = String::new();
        let mut intern_group = |entries: &[ng_flatten::Entry], d: &mut Derivation| -> Vec<u32> {
            entries
                .iter()
                .map(|e| {
                    ng_flatten::build_kv(&paths[e.node as usize], &e.value, &mut kv);
                    d.intern(&kv)
                })
                .collect()
        };
        for rg in &flattened.resources {
            let resource = intern_group(&rg.resource, self);
            for sg in &rg.scopes {
                let scope = intern_group(&sg.scope, self);
                let prefix_len = resource.len() + scope.len();
                let mut tokens = resource.clone();
                tokens.extend_from_slice(&scope);
                for record in &sg.records {
                    let record_tokens = intern_group(&record.entries, self);
                    // The truncate-extend dance from populate_row_index:
                    // reuse the resource+scope prefix per record.
                    tokens.truncate(prefix_len);
                    tokens.extend_from_slice(&record_tokens);
                    // Bitmap semantics: `RowIndex::row` inserts each token's
                    // position into its roaring bitmap — idempotent for a
                    // duplicated slot (a multi-valued field emitting the same
                    // key=value twice in one row, e.g. a repeated SAN domain).
                    // The Vec mirror must dedupe per row to match.
                    tokens.sort_unstable();
                    tokens.dedup();
                    let pos = self.timestamps.len() as u32;
                    self.timestamps.push(record.ts);
                    self.rows_per_source[tag as usize] += 1;
                    for &slot in &tokens {
                        self.postings[slot as usize].push(pos);
                    }
                }
            }
        }
    }

    /// Build one flattened frame from a batch of records the way the pipeline
    /// does — `build_export_request` (resource/scope identity from the
    /// source's constants) → `prepare_log_frame` (normalize + flatten +
    /// bincode) → `decode_log_frame` (what seal reads back) — and feed it.
    pub fn feed_batch(&mut self, records: Vec<LogRecord>, tag: SourceTag) -> Result<usize> {
        let (service, scope, version) = tag.export_identity();
        let request = build_export_request(records, service, None, scope, version);
        // fallback_base_ns = 0: both sources always set time_unix_nano, so
        // the fallback never fires (recorded; it only matters for records
        // with neither time nor observed time, which these corpora lack).
        let prepared = ng_flatten::prepare_log_frame(request, 0, None)?;
        let n = prepared.records;
        if prepared.ts_range.is_some() {
            let flattened: FlattenedLogRequest = ng_flatten::decode_log_frame(&prepared.data)?;
            self.feed_flattened(&flattened, tag);
        }
        Ok(n)
    }

    /// The `TimeOrder` remap mirrored from `sfst::row_index::build_time_order`
    /// (stable sort of insertion positions by timestamp): returns
    /// `to_sorted[insertion_pos]` — the exact permutation
    /// `remap_one_bitmap` applies to a posting list before encoding.
    pub fn time_order(&self) -> Vec<u32> {
        let n = self.timestamps.len();
        let mut insertion_position: Vec<u32> = (0..n as u32).collect();
        insertion_position.sort_by_key(|&i| self.timestamps[i as usize]);
        let mut to_sorted = vec![0u32; n];
        for (sorted_pos, &original_pos) in insertion_position.iter().enumerate() {
            to_sorted[original_pos as usize] = sorted_pos as u32;
        }
        to_sorted
    }

    /// Run the whole corpus: detect the source layout, convert + batch +
    /// flatten + accumulate. `batch_size` defaults to the otel-streams
    /// production default (100 records/request). Batch boundaries do not
    /// affect posting-list contents (rows and interning are order-global);
    /// the batching is mirrored for faithfulness, not because it is
    /// load-bearing. Returns (rows, lines_seen).
    pub fn run(
        &mut self,
        path: &Path,
        batch_size: usize,
        max_lines: Option<usize>,
    ) -> Result<(usize, usize)> {
        let mut rows = 0usize;
        let (_, lines_seen) = replay_requests(path, batch_size, max_lines, |records, tag| {
            rows += self.feed_batch(records, tag)?;
            Ok(())
        })?;
        Ok((rows, lines_seen))
    }
}

/// Replay a corpus through the production conversion + batching: parse each
/// line, run `Source::event_to_log_record`, and invoke `f` per completed
/// per-source batch of `batch_size` records (plus a final partial batch per
/// source). Shared by the derive bin (flatten+accumulate) and the `.sfst`
/// stretch (real WAL writes) so both walk the corpus identically.
pub fn replay_requests(
    path: &Path,
    batch_size: usize,
    max_lines: Option<usize>,
    mut f: impl FnMut(Vec<LogRecord>, SourceTag) -> Result<()>,
) -> Result<(usize, usize)> {
    let mut lines = corpus_lines(path)?;
    let first = lines.next().transpose()?.context("empty corpus file")?;
    let file_source = detect_source(&first)?;
    let mut batches: [Vec<LogRecord>; 2] = [Vec::new(), Vec::new()];
    let mut seen = 0usize;
    for line in std::iter::once(Ok(first)).chain(lines) {
        if let Some(max) = max_lines {
            if seen >= max {
                break;
            }
        }
        let line = line?;
        let (tag, record) = line_to_record(&line, file_source)?;
        seen += 1;
        let batch = &mut batches[tag as usize];
        batch.push(record);
        if batch.len() >= batch_size {
            f(std::mem::take(batch), tag)?;
        }
    }
    for (tag, batch) in batches.into_iter().enumerate() {
        if !batch.is_empty() {
            f(
                batch,
                if tag == 0 {
                    SourceTag::Certstream
                } else {
                    SourceTag::Jetstream
                },
            )?;
        }
    }
    Ok((seen, seen))
}

/// Minimal bitset over a u32 universe — the tool-side twin of
/// `sfst::bitset::Bitset` (crate-private there): `remap_one_bitmap` routes
/// dense-enough encodes through set-bits-then-scan-zeros, so the footprint
/// and bench paths mirror that route with the same primitive. Behavior
/// (ascending `iter_ones`/`iter_zeros`) is identical; it is not timed
/// library surface, just faithful encode-route plumbing.
pub struct Bitset {
    words: Vec<u64>,
    universe: u32,
}

impl Bitset {
    pub fn new(universe: u32) -> Self {
        Self {
            words: vec![0u64; (universe as usize).div_ceil(64)],
            universe,
        }
    }

    pub fn set(&mut self, v: u32) {
        debug_assert!(v < self.universe);
        self.words[(v >> 6) as usize] |= 1u64 << (v & 63);
    }

    pub fn iter_ones(&self) -> impl Iterator<Item = u32> + '_ {
        let universe = self.universe;
        self.words.iter().enumerate().flat_map(move |(w, &bits)| {
            let base = (w as u64) << 6;
            (0..64u32)
                .filter(move |&b| bits & (1u64 << b) != 0)
                .map(move |b| (base + b as u64) as u32)
                .take_while(move |&v| v < universe)
        })
    }

    pub fn iter_zeros(&self) -> impl Iterator<Item = u32> + '_ {
        let universe = self.universe;
        self.words.iter().enumerate().flat_map(move |(w, &bits)| {
            let base = (w as u64) << 6;
            (0..64u32)
                .filter(move |&b| bits & (1u64 << b) == 0)
                .map(move |b| (base + b as u64) as u32)
                .take_while(move |&v| v < universe)
        })
    }
}

// ---------------------------------------------------------------------------
// Derived-cache format (see CACHE_FORMAT_VERSION):
//   meta.json       — provenance + counts (serde_json, human-readable)
//   timestamps.bin  — universe × i64 LE, insertion order
//   to_sorted.bin   — universe × u32 LE, time_order().to_sorted[insertion]
//   postings.bin    — u32 LE count, then per list: u32 LE kv_len, kv bytes,
//                     u32 LE card, card × u32 LE positions (insertion order,
//                     ascending). Slots in first-intern order.
// ---------------------------------------------------------------------------

/// Cache metadata (meta.json).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CacheMeta {
    pub format_version: u32,
    pub corpus_path: String,
    pub corpus_sha256: String,
    pub rows: usize,
    pub lines_seen: usize,
    pub distinct_kvs: usize,
    pub total_postings: u64,
    pub batch_size: usize,
    pub rows_per_source: [u64; 2],
    pub derived_at_utc: String,
}

impl Derivation {
    /// sha256 of the corpus file as captured (streamed, so 250K-line gz
    /// files do not need to fit in memory twice).
    pub fn corpus_sha256(path: &Path) -> Result<String> {
        use sha2::Digest;
        let mut file = std::fs::File::open(path)?;
        let mut hasher = sha2::Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Persist the derived cache under `dir`.
    pub fn save(&self, dir: &Path, meta: &CacheMeta) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("meta.json"),
            serde_json::to_string_pretty(meta)? + "\n",
        )?;
        let mut ts = Vec::with_capacity(self.timestamps.len() * 8);
        for t in &self.timestamps {
            ts.extend_from_slice(&t.to_le_bytes());
        }
        std::fs::write(dir.join("timestamps.bin"), ts)?;
        let mut perm = Vec::with_capacity(self.timestamps.len() * 4);
        for p in self.time_order() {
            perm.extend_from_slice(&p.to_le_bytes());
        }
        std::fs::write(dir.join("to_sorted.bin"), perm)?;
        let mut postings =
            Vec::with_capacity(self.postings.iter().map(|p| p.len() * 4 + 8).sum::<usize>() + 4);
        postings.extend_from_slice(&(self.postings.len() as u32).to_le_bytes());
        for (slot, positions) in self.postings.iter().enumerate() {
            let kv = &self.kv_strings[slot];
            postings.extend_from_slice(&(kv.len() as u32).to_le_bytes());
            postings.extend_from_slice(kv.as_bytes());
            postings.extend_from_slice(&(positions.len() as u32).to_le_bytes());
            for &p in positions {
                postings.extend_from_slice(&p.to_le_bytes());
            }
        }
        std::fs::write(dir.join("postings.bin"), postings)?;
        Ok(())
    }
}

/// A loaded derived cache: everything the bench suite and footprint tooling
/// need, without re-running the flatten pipeline.
pub struct DerivedCache {
    pub meta: CacheMeta,
    pub timestamps: Vec<i64>,
    /// time_order().to_sorted[insertion_pos]
    pub to_sorted: Vec<u32>,
    /// (kv string, insertion-order positions) in first-intern order.
    pub postings: Vec<(String, Vec<u32>)>,
}

impl DerivedCache {
    pub fn load(dir: &Path) -> Result<Self> {
        let meta: CacheMeta =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json"))?)?;
        anyhow::ensure!(
            meta.format_version == CACHE_FORMAT_VERSION,
            "derived cache {dir:?} has format version {}, expected {CACHE_FORMAT_VERSION}",
            meta.format_version
        );
        let universe = meta.rows;
        let ts_bytes = std::fs::read(dir.join("timestamps.bin"))?;
        anyhow::ensure!(ts_bytes.len() == universe * 8, "timestamps.bin size mismatch");
        let timestamps = ts_bytes
            .chunks_exact(8)
            .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let perm_bytes = std::fs::read(dir.join("to_sorted.bin"))?;
        anyhow::ensure!(perm_bytes.len() == universe * 4, "to_sorted.bin size mismatch");
        let to_sorted = perm_bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let postings_bytes = std::fs::read(dir.join("postings.bin"))?;
        let mut postings = Vec::with_capacity(meta.distinct_kvs);
        let mut rest = postings_bytes.as_slice();
        let count = u32::from_le_bytes(rest[..4].try_into()?) as usize;
        rest = &rest[4..];
        anyhow::ensure!(count == meta.distinct_kvs, "postings.bin count mismatch");
        for _ in 0..count {
            let kv_len = u32::from_le_bytes(rest[..4].try_into()?) as usize;
            rest = &rest[4..];
            let kv = String::from_utf8(rest[..kv_len].to_vec())?;
            rest = &rest[kv_len..];
            let card = u32::from_le_bytes(rest[..4].try_into()?) as usize;
            rest = &rest[4..];
            let mut positions = Vec::with_capacity(card);
            for c in rest[..card * 4].chunks_exact(4) {
                positions.push(u32::from_le_bytes(c.try_into().unwrap()));
            }
            rest = &rest[card * 4..];
            postings.push((kv, positions));
        }
        Ok(Self {
            meta,
            timestamps,
            to_sorted,
            postings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value::Value as Av};
    use opentelemetry_proto::tonic::logs::v1::LogRecord;

    fn record(ts: i64, severity: i32) -> LogRecord {
        LogRecord {
            time_unix_nano: ts as u64,
            severity_number: severity,
            severity_text: "INFO".into(),
            attributes: vec![KeyValue {
                key: "k".into(),
                value: Some(AnyValue {
                    value: Some(Av::IntValue(severity as i64)),
                }),
            }],
            ..Default::default()
        }
    }

    /// The derived-cache round trip pins the format the treight bench reads:
    /// a save→load cycle must reproduce timestamps, the time-order
    /// permutation, and every posting list verbatim.
    #[test]
    fn cache_round_trip() {
        let mut d = Derivation::new();
        // Two out-of-order timestamps exercise the remap; duplicate
        // (field,value) occurrences within one row exercise the per-row
        // dedup (bitmap insert semantics).
        let mut r = record(2_000_000_000, 9);
        r.attributes.push(KeyValue {
            key: "k".into(),
            value: Some(AnyValue {
                value: Some(Av::IntValue(9)),
            }),
        });
        d.feed_batch(vec![r, record(1_000_000_000, 9)], SourceTag::Certstream)
            .unwrap();
        assert_eq!(d.timestamps.len(), 2);
        // time_order: row 1 (ts=1e9) sorts before row 0 → to_sorted = [1, 0].
        assert_eq!(d.time_order(), vec![1, 0]);

        let dir = std::env::temp_dir().join(format!(
            "treight-bench-cache-test-{}",
            std::process::id()
        ));
        let meta = CacheMeta {
            format_version: CACHE_FORMAT_VERSION,
            corpus_path: "test".into(),
            corpus_sha256: "0".into(),
            rows: d.timestamps.len(),
            lines_seen: 2,
            distinct_kvs: d.kv_strings.len(),
            total_postings: d.postings.iter().map(|p| p.len() as u64).sum(),
            batch_size: 100,
            rows_per_source: d.rows_per_source,
            derived_at_utc: "0".into(),
        };
        d.save(&dir, &meta).unwrap();
        let loaded = DerivedCache::load(&dir).unwrap();
        assert_eq!(loaded.timestamps, d.timestamps);
        assert_eq!(loaded.to_sorted, d.time_order());
        assert_eq!(loaded.postings.len(), d.postings.len());
        for ((kv, pos), (lkv, lpos)) in d.kv_strings.iter().zip(&d.postings).zip(loaded.postings.iter()) {
            assert_eq!(kv, lkv);
            assert_eq!(pos, lpos);
        }
        // The duplicated attribute produced ONE posting for its row.
        let dup = loaded
            .postings
            .iter()
            .find(|(kv, _)| kv == "attributes.k=9")
            .expect("attributes.k=9 interned");
        assert_eq!(dup.1.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
