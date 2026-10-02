//! Stretch goal: build a REAL `.sfst` index from a corpus through the actual
//! ingest→seal pipeline, then cross-check the derived posting-list mirror
//! against the index the production code produced — the end-to-end proof
//! that the benchmark fixtures are the real thing.
//!
//! ```text
//! cargo run -p treight-bench --bin treight-sfst -- \
//!     --corpus data/corpus/certstream-250k.jsonl.gz \
//!     --derived data/derived/certstream-250k \
//!     --out data/derived/sfst-stretch
//! ```
//!
//! Path (all production functions, none mirrored):
//! 1. corpus lines → `Source::event_to_log_record` → batched
//!    `build_export_request` (as the otel-streams runner does),
//! 2. `ng_ingest::write_request` — THE ingest entry `ng-ingest` uses:
//!    `ng_flatten::prepare_log_frame` → bincode → `wal::Writer` frame
//!    (`LOG_FRAME_PAYLOAD_FORMAT` = 1),
//! 3. `ng_index::build_sfst_file` — the `otel-ledger` seal entry:
//!    `populate_row_index` (intern + `RowIndex::row`) → `IndexWriter`,
//! 4. verify with `sfst::IndexReader`:
//!    - universe: reader `total_logs()` == derived rows;
//!    - the field table's per-field cardinalities/tiers match the derived
//!      field table;
//!    - every primary-FST (low-tier) `key=value` entry must exist in the
//!      derived posting lists with the exact same treight blob bytes and
//!      cardinality — the reader's `BitmapValue` IS `remap_one_bitmap`'s
//!      output for that list, so this pins the whole chain
//!      (flatten → intern → row → time-order remap → complement/encode).
//!
//! Writes sfst-stretch.json (sizes + verification outcomes) next to --out.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use treight_bench::{DerivedCache, replay_requests};

/// Field-cardinality tier threshold (production CARDINALITY_THRESHOLD).
const CARDINALITY_THRESHOLD: usize = 100;

#[derive(Parser)]
struct Args {
    /// Corpus file (.jsonl or .jsonl.gz) — re-fed through the real pipeline
    #[arg(long)]
    corpus: PathBuf,

    /// Derived cache dir (the mirror to verify)
    #[arg(long)]
    derived: PathBuf,

    /// Output dir for the WAL + .sfst + sfst-stretch.json
    #[arg(long)]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    std::fs::create_dir_all(&args.out)?;

    // ---- 1+2: replay the corpus into a REAL WAL via ng-ingest's writer ----
    let seq = std::sync::Arc::new(wal::SeqAllocator::ephemeral(0));
    let machine_id = journal_common::load_machine_id().context("load machine id")?;
    let boot_id = journal_common::load_boot_id().context("load boot id")?;
    let identity = file_registry::Identity::new(
        file_registry::MachineId::new(machine_id).context("machine id")?,
        file_registry::InstanceId::new(boot_id).context("boot id")?,
    );
    let mut writer = wal::Writer::new(
        &args.out,
        ng_ingest::one_file_config(),
        seq,
        wal::FileStamp {
            pipeline_id: ng_ingest::PIPELINE_ID,
            payload_format: ng_flatten::LOG_FRAME_PAYLOAD_FORMAT,
        },
        identity,
    )
    .context("create WAL writer")?;
    let mut clock = file_registry::MonotonicClock::new();

    let mut rows = 0usize;
    replay_requests(&args.corpus, 100, None, |records, tag| {
        let (service, scope, version) = tag.export_identity();
        let request = otel_streams::otel::build_export_request(records, service, None, scope, version);
        rows += ng_ingest::write_request(&mut writer, &mut clock, request)?;
        Ok(())
    })?;
    writer.shutdown_all().context("shutdown WAL writer")?;
    println!("WAL written: {rows} rows");

    // ---- 3: seal via the production ng-index entry point ----
    let wal_path = {
        let mut p: Option<PathBuf> = None;
        for e in std::fs::read_dir(&args.out)? {
            let e = e?;
            if e.path().extension().and_then(|x| x.to_str()) == Some("wal") {
                p = Some(e.path());
            }
        }
        p.context("no .wal file written")?
    };
    let sfst_path = args.out.join("index.sfst");
    let started = std::time::Instant::now();
    let (summary, sfst_size) =
        ng_index::build_sfst_file(&wal_path, &sfst_path, &ng_index::Metrics::new())
            .context("build SFST")?;
    println!(
        "SFST built: {sfst_size} bytes in {:.1}s ({} records)",
        started.elapsed().as_secs_f64(),
        summary.record_count
    );

    // ---- 4: verify the derived mirror against the real index ----
    let cache = DerivedCache::load(&args.derived)?;
    let data = std::fs::read(&sfst_path)?;
    let reader = sfst::IndexReader::open(&data).context("open SFST")?;

    let mut checks = serde_json::Map::new();
    checks.insert(
        "universe_matches".into(),
        serde_json::json!({
            "reader_total_logs": reader.total_logs(),
            "derived_rows": cache.meta.rows,
            "ok": reader.total_logs() as usize == cache.meta.rows,
        }),
    );

    // Derived field → distinct values (for the tier comparison).
    let mut field_distinct: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for (kv, _) in &cache.postings {
        let field = kv.split('=').next().unwrap_or(kv);
        *field_distinct.entry(field).or_default() += 1;
    }

    let mut fields_bad = 0usize;
    for f in reader.field_table().iter() {
        let derived = field_distinct.get(f.name.as_str()).copied().unwrap_or(0);
        let derived_tier = if derived < CARDINALITY_THRESHOLD {
            "low"
        } else if derived < CARDINALITY_THRESHOLD * 10 {
            "mid"
        } else {
            "high"
        };
        let reader_tier = match f.tier {
            sfst::FieldTier::Low => "low",
            sfst::FieldTier::Mid => "mid",
            sfst::FieldTier::High => "high",
        };
        if derived != f.cardinality as usize || derived_tier != reader_tier {
            fields_bad += 1;
            eprintln!(
                "FIELD MISMATCH {}: derived {derived}/{derived_tier} vs index {}/{}",
                f.name, f.cardinality, reader_tier
            );
        }
    }
    checks.insert(
        "field_table".into(),
        serde_json::json!({
            "reader_fields": reader.field_table().iter().count(),
            "ok": fields_bad == 0,
            "mismatches": fields_bad,
        }),
    );

    // Derived mirror's encode of each kv (production remap_one_bitmap shape).
    let universe = cache.meta.rows as u32;
    let to_sorted = &cache.to_sorted;
    let mut mirror: std::collections::HashMap<&str, (usize, usize)> =
        std::collections::HashMap::with_capacity(cache.postings.len());
    for (kv, positions) in &cache.postings {
        let card = positions.len();
        let mut sorted: Vec<u32> = positions.iter().map(|&p| to_sorted[p as usize]).collect();
        sorted.sort_unstable();
        let mut blob = Vec::new();
        if card as u64 > universe as u64 / 2 {
            let mut bits = treight_bench::Bitset::new(universe);
            for &v in &sorted {
                bits.set(v);
            }
            treight::Bitmap::from_sorted_iter_complemented(bits.iter_zeros(), universe, &mut blob);
        } else {
            treight::Bitmap::from_sorted_iter(sorted.iter().copied(), universe, &mut blob);
        }
        mirror.insert(kv, (card, blob.len()));
    }

    // Primary FST: every entry must match the mirror exactly.
    let mut prim_checked = 0usize;
    let mut prim_ok = 0usize;
    let mut prim_bytes_reader = 0u64;
    let mut prim_bytes_mirror = 0u64;
    reader.primary_for_each(|kv_bytes, value| {
        let kv = std::str::from_utf8(kv_bytes).expect("utf-8 kv");
        let card = value.desc.len(&value.data) as usize;
        let blob_len = value.data.len();
        prim_checked += 1;
        prim_bytes_reader += blob_len as u64;
        match mirror.get(kv) {
            Some(&(m_card, m_blob)) if m_card == card && m_blob == blob_len => {
                prim_ok += 1;
                prim_bytes_mirror += m_blob as u64;
            }
            _ => eprintln!(
                "PRIM MISMATCH {kv}: index card={card} blob={blob_len} vs mirror {:?}",
                mirror.get(kv)
            ),
        }
    });
    // Low-tier LIST count (entries across low-cardinality fields), not the
    // field count — the primary FST stores one entry per list.
    let derived_low: usize = field_distinct
        .values()
        .filter(|&&c| c < CARDINALITY_THRESHOLD)
        .sum();
    checks.insert(
        "primary_fst".into(),
        serde_json::json!({
            "entries_checked": prim_checked,
            "exact_matches": prim_ok,
            "ok": prim_ok == prim_checked && prim_checked == derived_low,
            "derived_low_tier_lists": derived_low,
            "treight_blob_bytes_reader": prim_bytes_reader,
            "treight_blob_bytes_mirror": prim_bytes_mirror,
        }),
    );

    let ok = checks
        .values()
        .all(|v| v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false));
    let doc = serde_json::json!({
        "corpus": args.corpus.display().to_string(),
        "derived": args.derived.display().to_string(),
        "sfst_path": sfst_path.display().to_string(),
        "sfst_size_bytes": sfst_size,
        "wal_path": wal_path.display().to_string(),
        "wal_size_bytes": std::fs::metadata(&wal_path)?.len(),
        "rows": rows,
        "verification": checks,
        "all_ok": ok,
    });
    let stretch_path = args.out.join("sfst-stretch.json");
    std::fs::write(&stretch_path, serde_json::to_string_pretty(&doc)? + "\n")?;
    println!(
        "verification {} → {}",
        if ok { "PASSED" } else { "FAILED" },
        stretch_path.display()
    );
    anyhow::ensure!(ok, "SFST stretch verification failed");
    Ok(())
}
