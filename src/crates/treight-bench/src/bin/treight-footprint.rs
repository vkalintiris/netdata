//! Index-footprint totals: per corpus, the exact sum of treight bytes vs
//! roaring bytes over the derived posting lists — the blog's headline
//! storage numbers.
//!
//! ```text
//! cargo run -p treight-bench --bin treight-footprint -- \
//!     --derived data/derived/certstream-250k \
//!     --json posts/treight-bitmap/evidence/bench/totals-certstream.json \
//!     --md   posts/treight-bitmap/evidence/bench/totals-certstream.md
//! ```
//!
//! Every posting list is encoded BOTH ways on identical data, mirroring the
//! production encoder's choices exactly (`sfst::build::remap_one_bitmap`:
//! insertion→time-order remap, complement above 50% density, bitset path at
//! ≥ max(universe/64, 256) positions — the last affects the encode route,
//! not the bytes). Totals are scoped two ways:
//!
//! - `sfst_materialized`: low+mid cardinality-tier lists — the ones the real
//!   SFST stores as treight bitmaps (high-card fields carry batch masks +
//!   stream batches instead; no per-value bitmap exists in that design).
//! - `all_lists`: every posting list, the pure data-structure comparison
//!   "if you stored a bitmap per value regardless of design".
//!
//! Roaring side: the workspace pin (netdata/roaring-rs, branch allocative),
//! built from the same time-ordered values; `serialized_size()` for the wire
//! form and `allocative::size_of` for the in-memory deep size. Treight side:
//! tree-blob bytes (`data.len()`), plus `+5` per list for the `BitmapValue`
//! descriptor (universe u32 + levels u8 + inverted bool) when named
//! `*_with_desc` / `*_heap`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use roaring::RoaringBitmap;
use treight_bench::DerivedCache;

/// Field-cardinality tier boundaries (production: CARDINALITY_THRESHOLD=100).
const CARDINALITY_THRESHOLD: usize = 100;

/// Row cap for the per-list scatter CSV (stride-sampled above it; the JSON
/// aggregates remain exact over every list).
const PER_LIST_ROW_CAP: usize = 200_000;

#[derive(Parser)]
struct Args {
    /// Derived cache directory (from treight-derive)
    #[arg(long)]
    derived: PathBuf,

    /// Output JSON path (totals.json shape — see evidence/bench/schema.md)
    #[arg(long)]
    json: PathBuf,

    /// Output markdown path
    #[arg(long)]
    md: PathBuf,
}

#[derive(Default, Clone, Copy)]
struct Scope {
    lists: u64,
    treight_blob: u64,
    roaring_serialized: u64,
    roaring_heap: u64,
}

impl Scope {
    fn add(&mut self, other: &Self) {
        self.lists += other.lists;
        self.treight_blob += other.treight_blob;
        self.roaring_serialized += other.roaring_serialized;
        self.roaring_heap += other.roaring_heap;
    }

    fn json(self, with_desc: u64) -> serde_json::Value {
        serde_json::json!({
            "lists": self.lists,
            "treight_blob_bytes": self.treight_blob,
            "treight_with_desc_bytes": with_desc,
            "roaring_serialized_bytes": self.roaring_serialized,
            "roaring_heap_bytes": self.roaring_heap,
            "treight_heap_bytes": with_desc,
        })
    }
}

fn density_bucket(d: f64) -> &'static str {
    // Half-open [lo, hi); the top bucket is [0.5, ∞) — see treight-derive's
    // DENSITY_BUCKETS note on the strict encoder complement rule.
    match d {
        x if x < 0.00001 => "<=0.001%",
        x if x < 0.0001 => "0.001-0.01%",
        x if x < 0.001 => "0.01-0.1%",
        x if x < 0.01 => "0.1-1%",
        x if x < 0.1 => "1-10%",
        x if x < 0.5 => "10-50%",
        _ => ">=50%",
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cache = DerivedCache::load(&args.derived)?;
    let universe = cache.meta.rows as u32;
    anyhow::ensure!(universe > 0, "empty derived cache");
    let to_sorted = &cache.to_sorted;
    let half = universe as u64 / 2;

    let mut sfst_scope = Scope::default();
    let mut all_scope = Scope::default();
    let mut complemented = 0u64;
    let mut direct = 0u64;
    let mut per_bucket: std::collections::BTreeMap<&'static str, Scope> =
        std::collections::BTreeMap::new();
    let mut bucket_stats: std::collections::BTreeMap<&'static str, (u64, u64, f64)> =
        std::collections::BTreeMap::new();
    // Per-list rows go to a compact gzipped CSV (the size-vs-density
    // scatter source), NOT into the JSON: pretty-printed per-list JSON for
    // 2.4M lists is hundreds of MB. Deterministic stride keeps the file
    // bounded (every row when under the cap; the stride + full count go in
    // the header comment).
    let mut per_list: Vec<(usize, f64, &str, u64, u64, u64, bool)> =
        Vec::with_capacity(cache.postings.len().min(PER_LIST_ROW_CAP));

    // Field → distinct-value count, once (KeyValueInterner::track_field's
    // grouping; the tier inputs). Doing this per list would be quadratic.
    let mut field_distinct: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for (kv, _) in &cache.postings {
        let field = kv.split('=').next().unwrap_or(kv);
        *field_distinct.entry(field).or_default() += 1;
    }

    for (kv, positions) in &cache.postings {
        let card = positions.len();
        let density = card as f64 / universe as f64;
        let field = kv.split('=').next().unwrap_or(kv);
        let distinct = field_distinct[field];
        let tier = if distinct < CARDINALITY_THRESHOLD {
            "low"
        } else if distinct < CARDINALITY_THRESHOLD * 10 {
            "mid"
        } else {
            "high"
        };

        // Time-ordered values — the exact input remap_one_bitmap produces.
        let mut sorted: Vec<u32> = positions.iter().map(|&p| to_sorted[p as usize]).collect();
        sorted.sort_unstable();

        // Production encode choice: complement above half the universe
        // (remap_one_bitmap). Both encode routes there (bitset scan for
        // card >= max(universe/64, 256), sparse sort) emit identical bytes,
        // so one bitset-scan path covers the totals; the bench suite times
        // the routes separately.
        let is_complement = card as u64 > half;
        let mut blob = Vec::new();
        if is_complement {
            complemented += 1;
            let mut bits = treight_bench::Bitset::new(universe);
            for &v in &sorted {
                bits.set(v);
            }
            treight::Bitmap::from_sorted_iter_complemented(bits.iter_zeros(), universe, &mut blob);
        } else {
            direct += 1;
            treight::Bitmap::from_sorted_iter(sorted.iter().copied(), universe, &mut blob);
        }

        let rb_time: RoaringBitmap = RoaringBitmap::from_sorted_iter(sorted.iter().copied())
            .expect("ascending time-ordered positions");
        let roaring_serialized = rb_time.serialized_size() as u64;
        // `size_of_unique` = deep size of the bitmap's own allocations
        // (containers + Vec header), excluding shared/static data.
        let roaring_heap = allocative::size_of_unique(&rb_time) as u64;

        let scope = Scope {
            lists: 1,
            treight_blob: blob.len() as u64,
            roaring_serialized,
            roaring_heap,
        };
        all_scope.add(&scope);
        if tier != "high" {
            sfst_scope.add(&scope);
        }
        let b = per_bucket.entry(density_bucket(density)).or_default();
        b.add(&scope);
        let e = bucket_stats.entry(density_bucket(density)).or_default();
        *e = (e.0 + 1, e.1 + card as u64, e.2 + density);

        per_list.push((
            card,
            density,
            tier,
            blob.len() as u64,
            roaring_serialized,
            roaring_heap,
            is_complement,
        ));
    }

    let json = serde_json::json!({
        "universe": universe,
        "lists_total": cache.postings.len() as u64,
        "bitsets": {
            "sfst_materialized": sfst_scope.json(sfst_scope.treight_blob + sfst_scope.lists * 5),
            "all_lists": all_scope.json(all_scope.treight_blob + all_scope.lists * 5),
        },
        "complement": { "complemented_lists": complemented, "direct_lists": direct },
        "per_density_bucket": per_bucket.iter().map(|(&b, s)| {
            let &(n, card_sum, dsum) = &bucket_stats[b];
            serde_json::json!({
                "bucket": b, "lists": s.lists,
                "treight_blob_bytes": s.treight_blob,
                "roaring_serialized_bytes": s.roaring_serialized,
                "avg_cardinality": if n > 0 { card_sum as f64 / n as f64 } else { 0.0 },
                "avg_density": if n > 0 { dsum / n as f64 } else { 0.0 },
            })
        }).collect::<Vec<_>>(),
        "notes": [
            "treight bytes mirror sfst::build::remap_one_bitmap exactly (time-order remap, complement >50% density)",
            "roaring is the workspace pin (netdata/roaring-rs branch allocative); serialized_size on time-ordered values",
            "roaring_heap_bytes = allocative::size_of_unique deep size of the same bitmap (containers + Vec)",
            "sfst_materialized = low+mid tier (threshold 100); production stores NO per-value bitmap for high-card fields",
        ],
    });

    std::fs::write(&args.json, serde_json::to_string_pretty(&json)? + "\n")?;

    // Per-list CSV.gz: card,density,tier,treight_blob,roaring_serialized,
    // roaring_heap,complemented. Stride-sampled to PER_LIST_ROW_CAP rows
    // (deterministic; aggregates in the JSON stay exact over ALL lists).
    let csv_path = args
        .json
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(
            "per-list-{}.csv.gz",
            args.json
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("corpus")
                .trim_start_matches("totals-")
        ));
    let stride = per_list.len().div_ceil(PER_LIST_ROW_CAP).max(1);
    let csv_file = std::fs::File::create(&csv_path)?;
    let encoder = flate2::GzBuilder::new()
        .comment(format!(
            "card,density,tier,treight_blob_bytes,roaring_serialized_bytes,roaring_heap_bytes,complemented | stride {} of {} lists (aggregates in totals json are exact over all)",
            stride,
            cache.postings.len()
        ))
        .write(csv_file, flate2::Compression::default());
    let mut w = std::io::BufWriter::new(encoder);
    writeln!(w, "card,density,tier,treight_blob_bytes,roaring_serialized_bytes,roaring_heap_bytes,complemented")?;
    for row in per_list.iter().step_by(stride) {
        writeln!(
            w,
            "{},{:.6},{},{},{},{},{}",
            row.0, row.1, row.2, row.3, row.4, row.5, row.6
        )?;
    }
    w.flush()?;

    // Markdown summary.
    let ratio = |a: u64, b: u64| if b > 0 { a as f64 / b as f64 } else { 0.0 };
    let mut md = String::new();
    md.push_str("# Index-footprint totals — treight vs roaring\n\n");
    md.push_str(&format!(
        "- corpus: `{}` (universe {} rows, {} posting lists, sha256 {}…)\n",
        cache.meta.corpus_path,
        universe,
        cache.postings.len(),
        &cache.meta.corpus_sha256[..16.min(cache.meta.corpus_sha256.len())]
    ));
    md.push_str(&format!(
        "- complement: {} lists encoded complemented (>50% density), {} direct\n",
        complemented, direct
    ));
    for (name, s) in [("SFST-materialized (low+mid)", &sfst_scope), ("all lists", &all_scope)] {
        md.push_str(&format!(
            "\n## {}\n\n| metric | treight | roaring | t/r ratio |\n|---|---|---|---|\n",
            name
        ));
        md.push_str(&format!(
            "| serialized bytes | {} | {} | {:.3} |\n",
            s.treight_blob,
            s.roaring_serialized,
            ratio(s.treight_blob, s.roaring_serialized)
        ));
        md.push_str(&format!(
            "| + 5B descriptor per list | {} | — | — |\n",
            s.treight_blob + s.lists * 5
        ));
        md.push_str(&format!(
            "| in-memory heap | {} | {} | {:.3} |\n",
            s.treight_blob + s.lists * 5,
            s.roaring_heap,
            ratio(s.treight_blob + s.lists * 5, s.roaring_heap)
        ));
        md.push_str(&format!("| lists | {} | {} | — |\n", s.lists, s.lists));
    }
    md.push_str("\n## Per density bucket\n\n| bucket | lists | treight B | roaring B | t/r |\n|---|---|---|---|---|\n");
    for (&b, s) in &per_bucket {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {:.3} |\n",
            b,
            s.lists,
            s.treight_blob,
            s.roaring_serialized,
            ratio(s.treight_blob, s.roaring_serialized)
        ));
    }
    std::fs::write(&args.md, md)?;
    println!(
        "wrote {} and {} (sfst-materialized treight {} B vs roaring {} B over {} lists)",
        args.json.display(),
        args.md.display(),
        sfst_scope.treight_blob,
        sfst_scope.roaring_serialized,
        sfst_scope.lists
    );
    Ok(())
}
