//! Derive posting lists from a captured corpus — the `treight-bench` half of
//! the blog's reproducibility story:
//!
//! ```text
//! cargo run -p treight-bench --bin treight-derive -- \
//!     --corpus data/corpus/certstream-250k.jsonl.gz \
//!     --out data/derived/certstream-250k [--batch-size 100] [--max-lines N]
//! ```
//!
//! Writes the derived cache (meta.json / timestamps.bin / to_sorted.bin /
//! postings.bin — see `treight_bench`'s lib docs for the format) plus
//! stats.json: the corpus-level numbers the blog's narrative leans on
//! (field cardinalities and tiers, posting-list density and cardinality
//! histograms, top lists, complement candidates). See TRACE.md in the
//! evidence directory for why this mirror is faithful to the production
//! pipeline.

use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::Parser;

use treight_bench::{CACHE_FORMAT_VERSION, CacheMeta, Derivation};

/// Field-cardinality tier boundaries, mirrored from the production indexer:
/// `CARDINALITY_THRESHOLD = 100` (ng-index/sfst_build.rs) → low `< 100`
/// distinct values, mid `[100, 1000)`, high `≥ 1000` (KeyValueInterner).
const CARDINALITY_THRESHOLD: usize = 100;

/// Density buckets for the posting-list histogram (density = cardinality /
/// universe). Log-spaced at the sparse end because real corpora are
/// dominated by near-unique values, plus the top bucket covering the
/// production complement candidates. Buckets are half-open [lo, hi); the
/// top bucket is [0.5, ∞) and labeled `>=50%` — note the ENCODER's
/// complement rule is the STRICT `card > universe/2`, so an exactly-50%
/// list lands in this bucket yet is still encoded direct (the per-list
/// `complemented` flag in totals.json carries the encoder's actual choice).
const DENSITY_BUCKETS: [(&str, f64, f64); 7] = [
    ("<=0.001%", 0.0, 0.00001),
    ("0.001-0.01%", 0.00001, 0.0001),
    ("0.01-0.1%", 0.0001, 0.001),
    ("0.1-1%", 0.001, 0.01),
    ("1-10%", 0.01, 0.1),
    ("10-50%", 0.1, 0.5),
    (">=50%", 0.5, f64::INFINITY),
];

/// Cardinality buckets (log2-spaced members per posting list).
fn card_bucket(card: usize) -> &'static str {
    match card {
        1 => "1",
        2..=4 => "2-4",
        5..=16 => "5-16",
        17..=64 => "17-64",
        65..=256 => "65-256",
        257..=1024 => "257-1k",
        1025..=4096 => "1k-4k",
        4097..=16384 => "4k-16k",
        16385..=65536 => "16k-64k",
        _ => ">64k",
    }
}

#[derive(Parser)]
struct Args {
    /// Corpus file (.jsonl or .jsonl.gz)
    #[arg(long)]
    corpus: std::path::PathBuf,

    /// Output directory for the derived cache (created)
    #[arg(long)]
    out: std::path::PathBuf,

    /// Records per ExportLogsServiceRequest — mirrors otel-streams'
    /// production default (runner/sender batching).
    #[arg(long, default_value_t = 100)]
    batch_size: usize,

    /// Stop after N lines (smoke tests on the hand-made samples)
    #[arg(long)]
    max_lines: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let corpus = std::fs::canonicalize(&args.corpus)?;
    println!("deriving from {}", corpus.display());

    let mut d = Derivation::new();
    let started = std::time::Instant::now();
    let (rows, lines_seen) = d.run(&corpus, args.batch_size, args.max_lines)?;
    let derive_secs = started.elapsed().as_secs_f64();
    println!(
        "rows={rows} lines={lines_seen} distinct_kvs={} total_postings={} ({derive_secs:.1}s)",
        d.kv_strings.len(),
        d.postings.iter().map(|p| p.len() as u64).sum::<u64>(),
    );

    let universe = rows as u64;
    let sha = Derivation::corpus_sha256(&corpus)?;
    let total_postings: u64 = d.postings.iter().map(|p| p.len() as u64).sum();

    let meta = CacheMeta {
        format_version: CACHE_FORMAT_VERSION,
        corpus_path: corpus.display().to_string(),
        corpus_sha256: sha,
        rows,
        lines_seen,
        distinct_kvs: d.kv_strings.len(),
        total_postings,
        batch_size: args.batch_size,
        rows_per_source: d.rows_per_source,
        derived_at_utc: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string(),
    };

    // ---- stats.json ----

    // Field = part before the first '=' (KeyValueInterner::track_field).
    let mut field_slots: HashMap<&str, Vec<u32>> = HashMap::new();
    for (slot, kv) in d.kv_strings.iter().enumerate() {
        let field = kv.split('=').next().unwrap_or(kv);
        field_slots.entry(field).or_default().push(slot as u32);
    }
    let tiers = |n: usize| {
        if n < CARDINALITY_THRESHOLD {
            "low"
        } else if n < CARDINALITY_THRESHOLD * 10 {
            "mid"
        } else {
            "high"
        }
    };
    let mut fields: Vec<(&str, usize, &str)> = field_slots
        .iter()
        .map(|(&f, slots)| (f, slots.len(), tiers(slots.len())))
        .collect();
    fields.sort_by_key(|&(f, _, _)| f);

    let mut tier_postings = [0u64; 3]; // low, mid, high — postings covered by each tier
    let mut tier_lists = [0u64; 3];
    for slots in field_slots.values() {
        let t = tiers(slots.len());
        let idx = if t == "low" { 0 } else if t == "mid" { 1 } else { 2 };
        for &s in slots {
            tier_postings[idx] += d.postings[s as usize].len() as u64;
            tier_lists[idx] += 1;
        }
    }

    let mut density_hist = [0u64; DENSITY_BUCKETS.len()];
    let mut card_hist: HashMap<&str, u64> = HashMap::new();
    let mut over_half = 0u64;
    let mut at_bitset_threshold = 0u64; // ≥ max(universe/64, 256) — remap's bitset path
    let bitset_threshold = (rows / 64).max(256);
    for positions in &d.postings {
        let card = positions.len();
        let density = card as f64 / universe as f64;
        for (i, (_, lo, hi)) in DENSITY_BUCKETS.iter().enumerate() {
            if density >= *lo && density < *hi {
                density_hist[i] += 1;
                break;
            }
        }
        *card_hist.entry(card_bucket(card)).or_default() += 1;
        if density > 0.5 {
            over_half += 1;
        }
        if card >= bitset_threshold {
            at_bitset_threshold += 1;
        }
    }

    let mut by_card: Vec<(usize, usize)> = d
        .postings
        .iter()
        .enumerate()
        .map(|(slot, p)| (p.len(), slot))
        .collect();
    by_card.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let top20: Vec<serde_json::Value> = by_card
        .iter()
        .take(20)
        .map(|&(card, slot)| {
            serde_json::json!({
                "kv": d.kv_strings[slot],
                "cardinality": card,
                "density": (card as f64 / universe as f64),
            })
        })
        .collect();

    let mut card_hist_order: Vec<(&str, u64)> = card_hist.into_iter().collect();
    card_hist_order.sort_by_key(|(b, _)| {
        // Order by the bucket's lower bound so the JSON reads naturally.
        match *b {
            "1" => 1,
            "2-4" => 2,
            "5-16" => 5,
            "17-64" => 17,
            "65-256" => 65,
            "257-1k" => 257,
            "1k-4k" => 1025,
            "4k-16k" => 4097,
            "16k-64k" => 16385,
            _ => 65537,
        }
    });

    let stats = serde_json::json!({
        "universe": universe,
        "rows": rows,
        "lines_seen": lines_seen,
        "rows_per_source": {
            "certstream": d.rows_per_source[0],
            "jetstream": d.rows_per_source[1],
        },
        "distinct_fields": fields.len(),
        "distinct_kvs": d.kv_strings.len(),
        "total_postings": total_postings,
        "avg_entries_per_row": total_postings as f64 / rows as f64,
        "cardinality_threshold": CARDINALITY_THRESHOLD,
        "tier_summary": {
            "low":  {"fields": fields.iter().filter(|f| f.2=="low").count(),
                     "lists": tier_lists[0], "postings": tier_postings[0]},
            "mid":  {"fields": fields.iter().filter(|f| f.2=="mid").count(),
                     "lists": tier_lists[1], "postings": tier_postings[1]},
            "high": {"fields": fields.iter().filter(|f| f.2=="high").count(),
                     "lists": tier_lists[2], "postings": tier_postings[2]},
        },
        "fields": fields.iter().map(|&(f, n, t)| serde_json::json!({
            "field": f, "distinct_values": n, "tier": t,
        })).collect::<Vec<_>>(),
        "posting_density_histogram": DENSITY_BUCKETS.iter().zip(density_hist)
            .map(|((name, _, _), n)| serde_json::json!({"bucket": name, "lists": n}))
            .collect::<Vec<_>>(),
        "posting_cardinality_histogram": card_hist_order.iter()
            .map(|&(b, n)| serde_json::json!({"bucket": b, "lists": n}))
            .collect::<Vec<_>>(),
        "complement_candidates_gt50pct": over_half,
        "at_bitset_threshold": {"threshold": bitset_threshold, "lists": at_bitset_threshold},
        "top20_by_cardinality": top20,
        "derive_seconds": derive_secs,
    });

    d.save(&args.out, &meta)
        .with_context(|| format!("save derived cache to {}", args.out.display()))?;
    std::fs::write(
        args.out.join("stats.json"),
        serde_json::to_string_pretty(&stats)? + "\n",
    )?;
    println!("wrote {} and stats.json", args.out.display());
    Ok(())
}
