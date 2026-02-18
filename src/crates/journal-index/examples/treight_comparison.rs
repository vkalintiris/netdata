//! Compare roaring bitmap vs treight for journal file indexing.
//!
//! Accepts a single journal file or a directory (recursively discovers journal files).
//! Measures construction time, serialized size, and heap allocation for both.
//! Benchmarks both treight::RawBitmap and treight::Bitmap (high-level wrapper).
//!
//! Usage:
//!     cargo run --release --example treight_comparison --features allocative -p journal-index \
//!         -- /var/log/journal/ --max-files 20

use clap::Parser;
use journal_common::Seconds;
use journal_core::file::{JournalFile, Mmap};
use journal_index::{Bitmap, FieldName, FileIndexer, IndexingLimits};
use journal_registry::File;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(about = "Compare roaring bitmap vs treight for journal file indexing")]
struct Args {
    /// Path to a journal file or directory (recursively discovers journal files)
    path: PathBuf,

    /// Maximum number of files to process
    #[arg(long, short = 'n')]
    max_files: Option<usize>,

    /// Maximum unique values per field for indexing
    #[arg(long, default_value_t = 1000000)]
    max_unique_values: usize,

    /// Maximum field payload size in bytes
    #[arg(long, default_value_t = 512)]
    max_payload_size: usize,
}

fn discover_journal_files(path: &Path) -> Vec<File> {
    if path.is_file() {
        return File::from_path(path).into_iter().collect();
    }
    let mut files = Vec::new();
    walk_dir(path, &mut files);
    files.sort_by(|a, b| a.path().cmp(b.path()));
    files
}

fn walk_dir(dir: &Path, files: &mut Vec<File>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, files);
        } else if let Some(f) = File::from_path(&path) {
            files.push(f);
        }
    }
}

/// Given a sorted, deduplicated slice of set values and a universe size,
/// return the sorted complement: all values in `0..universe_size` NOT in `values`.
fn sorted_complement(values: &[u32], universe_size: u32) -> Vec<u32> {
    let mut result = Vec::with_capacity(universe_size as usize - values.len());
    let mut vi = 0;
    for v in 0..universe_size {
        if vi < values.len() && values[vi] == v {
            vi += 1;
        } else {
            result.push(v);
        }
    }
    result
}

/// Batch size for AND/OR set operation benchmarks.
/// Each batch folds independently to avoid degeneration toward empty/full.
const SET_OPS_BATCH: usize = 8;

/// Benchmark alternating AND/OR on roaring bitmaps in batches.
fn bench_set_ops_roaring(bitmaps: &[Bitmap]) -> u64 {
    let t0 = Instant::now();
    for chunk in bitmaps.chunks(SET_OPS_BATCH) {
        let mut acc = chunk[0].clone();
        for (i, bm) in chunk[1..].iter().enumerate() {
            if i % 2 == 0 {
                acc &= bm;
            } else {
                acc |= bm;
            }
        }
        black_box(acc.len());
    }
    t0.elapsed().as_micros() as u64
}

/// Benchmark alternating AND/OR on treight::RawBitmap in batches.
fn bench_set_ops_raw(bitmaps: &[treight::RawBitmap]) -> u64 {
    let t0 = Instant::now();
    for chunk in bitmaps.chunks(SET_OPS_BATCH) {
        let mut acc = chunk[0].clone();
        for (i, bm) in chunk[1..].iter().enumerate() {
            acc = if i % 2 == 0 { &acc & bm } else { &acc | bm };
        }
        black_box(acc.len());
    }
    t0.elapsed().as_micros() as u64
}

/// Benchmark alternating AND/OR on treight::Bitmap in batches.
fn bench_set_ops_bitmap(bitmaps: &[treight::Bitmap]) -> u64 {
    let t0 = Instant::now();
    for chunk in bitmaps.chunks(SET_OPS_BATCH) {
        let mut acc = chunk[0].clone();
        for (i, bm) in chunk[1..].iter().enumerate() {
            acc = if i % 2 == 0 { &acc & bm } else { &acc | bm };
        }
        black_box(acc.len());
    }
    t0.elapsed().as_micros() as u64
}

/// Benchmark range_cardinality on the middle 50% of the universe.
fn bench_range_cardinality_roaring(bitmaps: &[Bitmap], universe_size: u32) -> u64 {
    let lo = universe_size / 4;
    let hi = universe_size * 3 / 4;
    let t0 = Instant::now();
    for bm in bitmaps {
        black_box(bm.range_cardinality(lo..hi));
    }
    t0.elapsed().as_micros() as u64
}

fn bench_range_cardinality_raw(bitmaps: &[treight::RawBitmap], universe_size: u32) -> u64 {
    let lo = universe_size / 4;
    let hi = universe_size * 3 / 4;
    let t0 = Instant::now();
    for bm in bitmaps {
        black_box(bm.range_cardinality(lo..hi));
    }
    t0.elapsed().as_micros() as u64
}

/// Benchmark remove_range: restrict each bitmap to the middle 50% of the universe.
/// Mirrors the matching_indices_in_range() pattern used in query filtering.
fn bench_remove_range_roaring(bitmaps: &[Bitmap], universe_size: u32) -> u64 {
    let lo = universe_size / 4;
    let hi = universe_size * 3 / 4;
    let t0 = Instant::now();
    for bm in bitmaps {
        let mut b = bm.clone();
        b.0.remove_range(..lo);
        b.0.remove_range(hi..);
        black_box(b.len());
    }
    t0.elapsed().as_micros() as u64
}

fn bench_remove_range_raw(bitmaps: &[treight::RawBitmap], universe_size: u32) -> u64 {
    let lo = universe_size / 4;
    let hi = universe_size * 3 / 4;
    let t0 = Instant::now();
    for bm in bitmaps {
        let mut b = bm.clone();
        b.remove_range(..lo);
        b.remove_range(hi..);
        black_box(b.len());
    }
    t0.elapsed().as_micros() as u64
}

/// Benchmark full iteration over every bitmap.
fn bench_iter_roaring(bitmaps: &[Bitmap]) -> u64 {
    let t0 = Instant::now();
    for bm in bitmaps {
        let s: u64 = bm.iter().map(|v| v as u64).sum();
        black_box(s);
    }
    t0.elapsed().as_micros() as u64
}

fn bench_iter_raw(bitmaps: &[treight::RawBitmap]) -> u64 {
    let t0 = Instant::now();
    for bm in bitmaps {
        let s: u64 = bm.iter().map(|v| v as u64).sum();
        black_box(s);
    }
    t0.elapsed().as_micros() as u64
}

/// Hybrid benchmarks: convert treight::RawBitmap → roaring on-the-fly, then operate.
/// The conversion cost is included since that's what a real hybrid approach would pay.

fn bench_set_ops_hybrid(bitmaps: &[treight::RawBitmap]) -> u64 {
    let t0 = Instant::now();
    for chunk in bitmaps.chunks(SET_OPS_BATCH) {
        let mut acc = roaring::RoaringBitmap::from(&chunk[0]);
        for (i, bm) in chunk[1..].iter().enumerate() {
            let rb = roaring::RoaringBitmap::from(bm);
            if i % 2 == 0 {
                acc &= &rb;
            } else {
                acc |= &rb;
            }
        }
        black_box(acc.len());
    }
    t0.elapsed().as_micros() as u64
}

fn bench_range_cardinality_hybrid(bitmaps: &[treight::RawBitmap], universe_size: u32) -> u64 {
    let lo = universe_size / 4;
    let hi = universe_size * 3 / 4;
    let t0 = Instant::now();
    for bm in bitmaps {
        let rb = roaring::RoaringBitmap::from(bm);
        black_box(rb.range_cardinality(lo..hi));
    }
    t0.elapsed().as_micros() as u64
}

fn bench_remove_range_hybrid(bitmaps: &[treight::RawBitmap], universe_size: u32) -> u64 {
    let lo = universe_size / 4;
    let hi = universe_size * 3 / 4;
    let t0 = Instant::now();
    for bm in bitmaps {
        let mut rb = roaring::RoaringBitmap::from(bm);
        rb.remove_range(..lo);
        rb.remove_range(hi..);
        black_box(rb.len());
    }
    t0.elapsed().as_micros() as u64
}

fn bench_iter_hybrid(bitmaps: &[treight::RawBitmap]) -> u64 {
    let t0 = Instant::now();
    for bm in bitmaps {
        let rb = roaring::RoaringBitmap::from(bm);
        let s: u64 = rb.iter().map(|v| v as u64).sum();
        black_box(s);
    }
    t0.elapsed().as_micros() as u64
}

fn fmt_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn fmt_us(us: u64) -> String {
    if us < 1000 {
        format!("{us} us")
    } else if us < 1_000_000 {
        format!("{:.1} ms", us as f64 / 1000.0)
    } else {
        format!("{:.2} s", us as f64 / 1_000_000.0)
    }
}

/// Format a ratio as a percentage: 0.49 → "-51%", 1.25 → "+25%".
fn fmt_pct(num: f64, den: f64) -> String {
    if den == 0.0 {
        "-".to_string()
    } else {
        let pct = (num / den - 1.0) * 100.0;
        format!("{:+.0}%", pct)
    }
}

#[derive(Default)]
struct Totals {
    files: usize,
    entries: usize,
    bitmaps: usize,
    roaring_serial: u64,
    roaring_heap: u64,
    raw_data: u64,
    raw_heap: u64,
    bm_data: u64,
    bm_heap: u64,
    roaring_build_us: u64,
    treight_raw_build_us: u64,
    treight_bm_build_us: u64,
    roaring_ops_us: u64,
    raw_ops_us: u64,
    bm_ops_us: u64,
    hybrid_ops_us: u64,
    roaring_range_card_us: u64,
    raw_range_card_us: u64,
    hybrid_range_card_us: u64,
    roaring_remove_range_us: u64,
    raw_remove_range_us: u64,
    hybrid_remove_range_us: u64,
    roaring_iter_us: u64,
    raw_iter_us: u64,
    hybrid_iter_us: u64,
    index_us: u64,
}

// Per-file table columns:
//   Roaring Ser   - roaring serialized size (wire/disk format)
//   Roaring Heap  - roaring heap allocation (via allocative crate)
//   Raw Data      - treight::RawBitmap data payload bytes (always normal representation)
//   Raw Heap      - treight::RawBitmap heap allocation (via allocative crate)
//   Bitmap Data   - treight::Bitmap data payload bytes (normal or complemented)
//   Bitmap Heap   - treight::Bitmap heap allocation (via allocative crate)
//   Size %        - Bitmap Data vs Roaring Ser as % change (negative = treight smaller)
//   Build Roar    - time to build all roaring bitmaps (from_sorted_iter + optimize)
//   Build T8      - time to build all treight::Bitmap   (from_sorted_iter, high-level wrapper)
//   Time %        - Build T8 vs Build Roar as % change (negative = treight faster)

fn print_row(label: &str, entries: usize, bitmaps: usize, t: &Totals) {
    println!(
        "{:<60} {:>8} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7} {:>12} {:>12} {:>7}",
        label,
        entries,
        bitmaps,
        fmt_bytes(t.roaring_serial),
        fmt_bytes(t.roaring_heap),
        fmt_bytes(t.raw_data),
        fmt_bytes(t.raw_heap),
        fmt_bytes(t.bm_data),
        fmt_bytes(t.bm_heap),
        fmt_pct(t.bm_data as f64, t.roaring_serial as f64),
        fmt_us(t.roaring_build_us),
        fmt_us(t.treight_bm_build_us),
        fmt_pct(t.treight_bm_build_us as f64, t.roaring_build_us as f64),
    );
}

const SEP_WIDTH: usize = 198;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut files = discover_journal_files(&args.path);
    if files.is_empty() {
        eprintln!("No journal files found at: {}", args.path.display());
        std::process::exit(1);
    }

    let total_discovered = files.len();
    if let Some(max) = args.max_files {
        files.truncate(max);
    }

    eprintln!(
        "Discovered {} journal file(s), processing {}\n",
        total_discovered,
        files.len()
    );

    let mut indexer = FileIndexer::new(IndexingLimits {
        max_unique_values_per_field: args.max_unique_values,
        max_field_payload_size: args.max_payload_size,
    });
    let bucket_duration = Seconds(600);

    println!(
        "{:<60} {:>8} {:>8} {:>12} {:>12} {:>12} {:>12} {:>12} {:>12} {:>7} {:>12} {:>12} {:>7}",
        "FILE",
        "ENTRIES",
        "BITMAPS",
        "Roaring Ser",
        "Roaring Heap",
        "Raw Data",
        "Raw Heap",
        "Bitmap Data",
        "Bitmap Heap",
        "Size %",
        "Build Roar",
        "Build T8",
        "Time %",
    );
    println!("{}", "-".repeat(SEP_WIDTH));

    let mut grand = Totals::default();

    for file in &files {
        let display_path = file.path();
        let label = if display_path.len() > 58 {
            format!("...{}", &display_path[display_path.len() - 55..])
        } else {
            display_path.to_string()
        };

        // Discover fields.
        let journal_file = match JournalFile::<Mmap>::open(file, 32 * 1024 * 1024) {
            Ok(jf) => jf,
            Err(e) => {
                eprintln!("  skip {label}: {e}");
                continue;
            }
        };
        let field_map = journal_file.load_fields()?;
        drop(journal_file);
        let field_names: Vec<FieldName> =
            field_map.keys().filter_map(|k| FieldName::new(k)).collect();

        // Index (uses roaring internally).
        let t0 = Instant::now();
        let file_index = indexer.index(file, None, &field_names, bucket_duration)?;
        let index_us = t0.elapsed().as_micros() as u64;

        let total_entries = file_index.total_entries();
        let universe_size = total_entries as u32;
        let bitmaps = file_index.bitmaps();
        let bitmap_count = bitmaps.len();

        // Extract sorted values from each bitmap (raw input for all constructions).
        let raw_data: Vec<Vec<u32>> = bitmaps.values().map(|bm| bm.iter().collect()).collect();

        // Benchmark roaring construction (from_sorted_iter + optimize).
        let t0 = Instant::now();
        let roaring_bitmaps: Vec<Bitmap> = raw_data
            .iter()
            .map(|values| {
                let mut bm = Bitmap::from_sorted_iter(values.iter().copied(), universe_size);
                bm.optimize();
                bm
            })
            .collect();
        let roaring_build_us = t0.elapsed().as_micros() as u64;

        // Benchmark treight::RawBitmap construction.
        let t0 = Instant::now();
        let treight_raw_bitmaps: Vec<treight::RawBitmap> = raw_data
            .iter()
            .map(|values| {
                treight::RawBitmap::from_sorted_iter(values.iter().copied(), universe_size)
            })
            .collect();
        let treight_raw_build_us = t0.elapsed().as_micros() as u64;

        // Benchmark treight::Bitmap construction (high-level wrapper).
        // Choose normal vs complemented representation based on density:
        // when more than half the bits are set, store the complement instead.
        let t0 = Instant::now();
        let treight_bitmaps: Vec<treight::Bitmap> = raw_data
            .iter()
            .map(|values| {
                if (values.len() as u64) * 2 > universe_size as u64 {
                    let complement = sorted_complement(values, universe_size);
                    treight::Bitmap::from_sorted_iter_complemented(
                        complement.into_iter(),
                        universe_size,
                    )
                } else {
                    treight::Bitmap::from_sorted_iter(values.iter().copied(), universe_size)
                }
            })
            .collect();
        let treight_bm_build_us = t0.elapsed().as_micros() as u64;

        // Measure sizes.
        let roaring_serial: u64 = roaring_bitmaps
            .iter()
            .map(|bm| bm.0.serialized_size() as u64)
            .sum();
        let roaring_heap: u64 = roaring_bitmaps
            .iter()
            .map(|bm| allocative::size_of_unique_allocated_data(bm) as u64)
            .sum();
        let raw_data: u64 = treight_raw_bitmaps
            .iter()
            .map(|bm| bm.data().len() as u64)
            .sum();
        let raw_heap: u64 = treight_raw_bitmaps
            .iter()
            .map(|bm| allocative::size_of_unique_allocated_data(bm) as u64)
            .sum();
        let bm_data: u64 = treight_bitmaps
            .iter()
            .map(|bm| bm.raw().data().len() as u64)
            .sum();
        let bm_heap: u64 = treight_bitmaps
            .iter()
            .map(|bm| allocative::size_of_unique_allocated_data(bm) as u64)
            .sum();

        // Benchmark set operations (alternating AND/OR in batches).
        let roaring_ops_us = bench_set_ops_roaring(&roaring_bitmaps);
        let raw_ops_us = bench_set_ops_raw(&treight_raw_bitmaps);
        let bm_ops_us = bench_set_ops_bitmap(&treight_bitmaps);
        let hybrid_ops_us = bench_set_ops_hybrid(&treight_raw_bitmaps);

        // Benchmark range_cardinality (middle 50% of universe).
        let roaring_range_card_us =
            bench_range_cardinality_roaring(&roaring_bitmaps, universe_size);
        let raw_range_card_us = bench_range_cardinality_raw(&treight_raw_bitmaps, universe_size);
        let hybrid_range_card_us =
            bench_range_cardinality_hybrid(&treight_raw_bitmaps, universe_size);

        // Benchmark remove_range (restrict to middle 50% of universe).
        let roaring_remove_range_us = bench_remove_range_roaring(&roaring_bitmaps, universe_size);
        let raw_remove_range_us = bench_remove_range_raw(&treight_raw_bitmaps, universe_size);
        let hybrid_remove_range_us = bench_remove_range_hybrid(&treight_raw_bitmaps, universe_size);

        // Benchmark full iteration.
        let roaring_iter_us = bench_iter_roaring(&roaring_bitmaps);
        let raw_iter_us = bench_iter_raw(&treight_raw_bitmaps);
        let hybrid_iter_us = bench_iter_hybrid(&treight_raw_bitmaps);

        let row = Totals {
            files: 1,
            entries: total_entries,
            bitmaps: bitmap_count,
            roaring_serial,
            roaring_heap,
            raw_data,
            raw_heap,
            bm_data,
            bm_heap,
            roaring_build_us,
            treight_raw_build_us,
            treight_bm_build_us,
            roaring_ops_us,
            raw_ops_us,
            bm_ops_us,
            hybrid_ops_us,
            roaring_range_card_us,
            raw_range_card_us,
            hybrid_range_card_us,
            roaring_remove_range_us,
            raw_remove_range_us,
            hybrid_remove_range_us,
            roaring_iter_us,
            raw_iter_us,
            hybrid_iter_us,
            index_us,
        };

        print_row(&label, total_entries, bitmap_count, &row);

        grand.files += 1;
        grand.entries += total_entries;
        grand.bitmaps += bitmap_count;
        grand.roaring_serial += roaring_serial;
        grand.roaring_heap += roaring_heap;
        grand.raw_data += raw_data;
        grand.raw_heap += raw_heap;
        grand.bm_data += bm_data;
        grand.bm_heap += bm_heap;
        grand.roaring_build_us += roaring_build_us;
        grand.treight_raw_build_us += treight_raw_build_us;
        grand.treight_bm_build_us += treight_bm_build_us;
        grand.roaring_ops_us += roaring_ops_us;
        grand.raw_ops_us += raw_ops_us;
        grand.bm_ops_us += bm_ops_us;
        grand.hybrid_ops_us += hybrid_ops_us;
        grand.roaring_range_card_us += roaring_range_card_us;
        grand.raw_range_card_us += raw_range_card_us;
        grand.hybrid_range_card_us += hybrid_range_card_us;
        grand.roaring_remove_range_us += roaring_remove_range_us;
        grand.raw_remove_range_us += raw_remove_range_us;
        grand.hybrid_remove_range_us += hybrid_remove_range_us;
        grand.roaring_iter_us += roaring_iter_us;
        grand.raw_iter_us += raw_iter_us;
        grand.hybrid_iter_us += hybrid_iter_us;
        grand.index_us += index_us;
    }

    if grand.files > 1 {
        println!("{}", "-".repeat(SEP_WIDTH));
        print_row("TOTAL", grand.entries, grand.bitmaps, &grand);
    }

    // Summary.
    println!();
    println!(
        "Summary ({} files, {} entries, {} bitmaps)",
        grand.files, grand.entries, grand.bitmaps
    );
    println!();

    println!("  Storage (Raw = treight::RawBitmap, T8 = treight::Bitmap, RB = roaring):");
    println!(
        "    RB serialized:    {:>10}",
        fmt_bytes(grand.roaring_serial)
    );
    println!(
        "    RB heap (alloc):  {:>10}",
        fmt_bytes(grand.roaring_heap)
    );
    println!("    Raw data:         {:>10}", fmt_bytes(grand.raw_data));
    println!("    Raw heap (alloc): {:>10}", fmt_bytes(grand.raw_heap));
    println!("    T8 data:          {:>10}", fmt_bytes(grand.bm_data));
    println!("    T8 heap (alloc):  {:>10}", fmt_bytes(grand.bm_heap));
    println!(
        "    Raw vs RB ser:    {:>10}  (negative = treight smaller)",
        fmt_pct(grand.raw_data as f64, grand.roaring_serial as f64)
    );
    println!(
        "    T8 vs RB ser:     {:>10}",
        fmt_pct(grand.bm_data as f64, grand.roaring_serial as f64)
    );
    println!(
        "    T8 vs Raw data:   {:>10}  (negative = complementing helps)",
        fmt_pct(grand.bm_data as f64, grand.raw_data as f64)
    );
    println!(
        "    T8 vs RB heap:    {:>10}",
        fmt_pct(grand.bm_heap as f64, grand.roaring_heap as f64)
    );

    println!();
    println!("  Construction (from_sorted_iter, all bitmaps):");
    println!(
        "    RB (+ optimize):  {:>9}",
        fmt_us(grand.roaring_build_us)
    );
    println!(
        "    T8 RawBitmap:     {:>9}  ({})",
        fmt_us(grand.treight_raw_build_us),
        fmt_pct(
            grand.treight_raw_build_us as f64,
            grand.roaring_build_us as f64
        )
    );
    println!(
        "    T8 Bitmap:        {:>9}  ({})  (negative = T8 faster)",
        fmt_us(grand.treight_bm_build_us),
        fmt_pct(
            grand.treight_bm_build_us as f64,
            grand.roaring_build_us as f64
        )
    );

    println!();
    println!("  Set operations (alternating AND/OR, batches of {SET_OPS_BATCH}):");
    println!("    RB:               {:>9}", fmt_us(grand.roaring_ops_us));
    println!(
        "    T8 RawBitmap:     {:>9}  ({})",
        fmt_us(grand.raw_ops_us),
        fmt_pct(grand.raw_ops_us as f64, grand.roaring_ops_us as f64)
    );
    println!(
        "    T8 Bitmap:        {:>9}  ({})",
        fmt_us(grand.bm_ops_us),
        fmt_pct(grand.bm_ops_us as f64, grand.roaring_ops_us as f64)
    );
    println!(
        "    Hybrid (T8→RB):   {:>9}  ({})  (negative = faster than RB)",
        fmt_us(grand.hybrid_ops_us),
        fmt_pct(grand.hybrid_ops_us as f64, grand.roaring_ops_us as f64)
    );

    println!();
    println!("  Range cardinality (middle 50% of universe, all bitmaps):");
    println!(
        "    RB:               {:>9}",
        fmt_us(grand.roaring_range_card_us)
    );
    println!(
        "    T8 RawBitmap:     {:>9}  ({})",
        fmt_us(grand.raw_range_card_us),
        fmt_pct(
            grand.raw_range_card_us as f64,
            grand.roaring_range_card_us as f64
        )
    );
    println!(
        "    Hybrid (T8→RB):   {:>9}  ({})  (negative = faster than RB)",
        fmt_us(grand.hybrid_range_card_us),
        fmt_pct(
            grand.hybrid_range_card_us as f64,
            grand.roaring_range_card_us as f64
        )
    );

    println!();
    println!("  Remove range (restrict to middle 50%, clone + trim, all bitmaps):");
    println!(
        "    RB:               {:>9}",
        fmt_us(grand.roaring_remove_range_us)
    );
    println!(
        "    T8 RawBitmap:     {:>9}  ({})",
        fmt_us(grand.raw_remove_range_us),
        fmt_pct(
            grand.raw_remove_range_us as f64,
            grand.roaring_remove_range_us as f64
        )
    );
    println!(
        "    Hybrid (T8→RB):   {:>9}  ({})  (negative = faster than RB)",
        fmt_us(grand.hybrid_remove_range_us),
        fmt_pct(
            grand.hybrid_remove_range_us as f64,
            grand.roaring_remove_range_us as f64
        )
    );

    println!();
    println!("  Iteration (full scan, all bitmaps):");
    println!("    RB:               {:>9}", fmt_us(grand.roaring_iter_us));
    println!(
        "    T8 RawBitmap:     {:>9}  ({})",
        fmt_us(grand.raw_iter_us),
        fmt_pct(grand.raw_iter_us as f64, grand.roaring_iter_us as f64)
    );
    println!(
        "    Hybrid (T8→RB):   {:>9}  ({})  (negative = faster than RB)",
        fmt_us(grand.hybrid_iter_us),
        fmt_pct(grand.hybrid_iter_us as f64, grand.roaring_iter_us as f64)
    );

    println!();
    println!("  Indexing (file I/O + field discovery + bitmap construction):");
    println!("    Total:            {:>9}", fmt_us(grand.index_us));

    Ok(())
}
