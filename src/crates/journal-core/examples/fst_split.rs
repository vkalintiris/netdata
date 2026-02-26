//! Split FST prototype using gix-chunk.
//!
//! Splits the unified FST into:
//! - A **primary chunk** (always loaded): field metadata + low-cardinality bitmaps
//! - **Per-field chunks** (loaded on demand): high-cardinality field=value → entry count
//!
//! All packaged in a single file using `gix-chunk` for random access via mmap.
//!
//! File layout:
//! ```text
//! [Header: 12 bytes]          magic "SFST" + version u32 + num_chunks u32
//! [TOC]                       written by gix-chunk (12 bytes × (num_chunks + 1))
//! [Primary FST chunk]         chunk ID: b"PRIM"
//! [High-card field 0 chunk]   chunk ID: [b'H', b'C', hi, lo]
//! [High-card field 1 chunk]   ...
//! ```
//!
//! Usage:
//!   cargo run --release -p journal-core --example fst_split -- <journal-file> [--max-cardinality N]

use journal_core::file::file::JournalFile;
use journal_core::file::mmap::Mmap;
use journal_core::file::HashableObject;
use journal_registry::repository::File;
use serde::{Deserialize, Serialize};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::num::NonZeroU64;
use std::time::Instant;

const MAGIC: &[u8; 4] = b"SFST";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 12; // magic(4) + version(4) + num_chunks(4)
const ZSTD_LEVEL: i32 = 1;

const CHUNK_PRIMARY: gix_chunk::Id = *b"PRIM";

fn hc_chunk_id(index: u16) -> gix_chunk::Id {
    [b'H', b'C', (index >> 8) as u8, (index & 0xff) as u8]
}

/// Value type for the primary FST.
///
/// Two kinds of keys coexist in the primary FST:
/// - Bare `FIELD` keys (no `=`) → `Field` variant with cardinality and optional HC chunk index
/// - `FIELD=value` keys → `Bitmap` variant (low-cardinality fields only)
#[derive(Debug, Clone, Serialize, Deserialize)]
enum PrimaryValue {
    /// Bare FIELD key: cardinality + optional chunk index for high-card lookup.
    Field {
        cardinality: u64,
        chunk_index: Option<u16>,
    },
    /// Low-cardinality FIELD=value: bitmap of entry indices.
    Bitmap(treight::Bitmap),
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: {} <journal-file> [--max-cardinality N]",
            args[0]
        );
        std::process::exit(1);
    }

    let journal_path = &args[1];
    let max_cardinality: usize = args
        .iter()
        .position(|a| a == "--max-cardinality")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    let file = File::from_str(journal_path).unwrap_or_else(|| {
        eprintln!("Failed to parse journal file path: {}", journal_path);
        std::process::exit(1);
    });

    let window_size = 32 * 1024 * 1024;
    let journal_file = JournalFile::<Mmap>::open(&file, window_size).unwrap_or_else(|e| {
        eprintln!("Failed to open journal file: {:#?}", e);
        std::process::exit(1);
    });

    let header = journal_file.journal_header_ref();
    let tail_object_offset = header
        .tail_object_offset
        .expect("missing tail_object_offset");

    println!("=== Journal File Info ===");
    println!("Path:       {}", journal_path);
    println!("Objects:    {}", header.n_objects);
    println!("Entries:    {}", header.n_entries);
    println!(
        "Arena size: {} bytes ({:.1} MiB)",
        header.arena_size,
        header.arena_size as f64 / (1024.0 * 1024.0)
    );
    println!();

    // ── Step 1: Build entry offset → index mapping ──────────────────────
    let mut entry_offsets: Vec<NonZeroU64> = Vec::new();
    journal_file
        .entry_offsets(&mut entry_offsets)
        .expect("failed to load entry offsets");
    entry_offsets.retain(|o| *o <= tail_object_offset);

    let universe_size = entry_offsets.len() as u32;
    let entry_offset_index: HashMap<NonZeroU64, u32> = entry_offsets
        .iter()
        .enumerate()
        .map(|(idx, offset)| (*offset, idx as u32))
        .collect();

    println!(
        "Entry offsets loaded: {} (universe_size for bitmaps)",
        universe_size
    );
    println!();

    // ── Step 2: Collect field names ─────────────────────────────────────
    let mut field_names: Vec<String> = Vec::new();
    for field_result in journal_file.fields() {
        let field_guard = match field_result {
            Ok(g) => g,
            Err(e) => {
                eprintln!("Error reading field object: {:#?}", e);
                continue;
            }
        };
        if let Ok(name) = std::str::from_utf8(field_guard.raw_payload()) {
            field_names.push(name.to_string());
        }
    }

    println!("Fields found: {}", field_names.len());

    // ── Steps 3-6: Single-pass field processing ───────────────────────
    // Instead of separate passes for cardinality counting (old Step 3),
    // classification (old Step 4), bitmap building (old Step 5b), and
    // HC entry counting (old Step 6), we process each field in one iteration:
    //   - Iterate data objects once, collecting key + n_entries + inlined_cursor
    //   - Classify by cardinality (= number of collected items)
    //   - Low-card: build bitmaps via collect_offsets
    //   - High-card: use header.n_entries directly (no chain traversal)
    let t_build = Instant::now();

    let mut primary_entries: Vec<(String, PrimaryValue)> = Vec::new();
    let mut hc_field_data: Vec<(u16, String, Vec<(String, u64)>)> = Vec::new();
    let mut next_hc_index: u16 = 0;
    let mut scratch_offsets: Vec<NonZeroU64> = Vec::new();
    let mut scratch_indices: Vec<u32> = Vec::new();
    let mut low_card_fields: Vec<String> = Vec::new();
    let mut high_card_count: usize = 0;

    for field_name in &field_names {
        let field_data_iter = match journal_file.field_data_objects(field_name.as_bytes()) {
            Ok(iter) => iter,
            Err(_) => continue,
        };

        // Single pass: collect key, n_entries, and inlined_cursor from each data object
        let mut collected = Vec::new();
        for data_result in field_data_iter {
            let data_guard = match data_result {
                Ok(g) => g,
                Err(_) => continue,
            };

            let key = if data_guard.is_compressed() {
                let mut buf = Vec::new();
                match data_guard.decompress(&mut buf) {
                    Ok(_) => std::str::from_utf8(&buf).ok().map(|s| s.to_string()),
                    Err(_) => None,
                }
            } else {
                std::str::from_utf8(data_guard.raw_payload())
                    .ok()
                    .map(|s| s.to_string())
            };

            let Some(key) = key else { continue };
            let n_entries = data_guard.header.n_entries.map_or(0, |n| n.get());
            let Some(ic) = data_guard.inlined_cursor() else {
                continue;
            };

            collected.push((key, n_entries, ic));
        }

        let cardinality = collected.len();

        if cardinality <= max_cardinality {
            // Low-cardinality: bare FIELD key + bitmaps for each value
            low_card_fields.push(field_name.clone());
            primary_entries.push((
                field_name.clone(),
                PrimaryValue::Field {
                    cardinality: cardinality as u64,
                    chunk_index: None,
                },
            ));

            for (key, _, inlined_cursor) in collected {
                scratch_offsets.clear();
                if inlined_cursor
                    .collect_offsets(&journal_file, &mut scratch_offsets)
                    .is_err()
                {
                    continue;
                }

                scratch_indices.clear();
                for offset in scratch_offsets
                    .iter()
                    .copied()
                    .filter(|o| *o <= tail_object_offset)
                {
                    if let Some(&idx) = entry_offset_index.get(&offset) {
                        scratch_indices.push(idx);
                    }
                }
                scratch_indices.sort_unstable();

                let bitmap = treight::Bitmap::from_sorted_iter(
                    scratch_indices.iter().copied(),
                    universe_size,
                );

                primary_entries.push((key, PrimaryValue::Bitmap(bitmap)));
            }
        } else {
            // High-cardinality: bare FIELD key + per-field HC FST using n_entries directly
            high_card_count += 1;
            let chunk_idx = next_hc_index;
            next_hc_index += 1;

            primary_entries.push((
                field_name.clone(),
                PrimaryValue::Field {
                    cardinality: cardinality as u64,
                    chunk_index: Some(chunk_idx),
                },
            ));

            // Collect (key, n_entries) pairs — FST build deferred to parallel phase
            let fst_entries: Vec<(String, u64)> = collected
                .into_iter()
                .map(|(key, n_entries, _)| (key, n_entries))
                .collect();

            hc_field_data.push((chunk_idx, field_name.clone(), fst_entries));
        }
    }

    println!(
        "Low-cardinality fields:  {} (cardinality <= {})",
        low_card_fields.len(),
        max_cardinality
    );
    println!("High-cardinality fields: {}", high_card_count);
    println!();

    let primary_fst: fst_index::FstIndex<PrimaryValue> =
        fst_index::FstIndex::build(primary_entries).expect("failed to build primary FST");
    let primary_serialized =
        bincode::serialize(&primary_fst).expect("failed to serialize primary FST");
    let primary_compressed =
        zstd::encode_all(&primary_serialized[..], ZSTD_LEVEL).expect("zstd compress primary");

    println!("=== Primary FST ===");
    println!("  Keys:     {}", primary_fst.len());
    println!(
        "  FST size: {} bytes ({:.1} KiB)",
        primary_fst.fst_bytes(),
        primary_fst.fst_bytes() as f64 / 1024.0
    );
    println!(
        "  Bincode:  {} bytes ({:.1} KiB)",
        primary_serialized.len(),
        primary_serialized.len() as f64 / 1024.0
    );
    println!(
        "  Zstd:     {} bytes ({:.1} KiB)  ratio {:.2}x",
        primary_compressed.len(),
        primary_compressed.len() as f64 / 1024.0,
        primary_serialized.len() as f64 / primary_compressed.len() as f64,
    );
    println!();

    // Build HC FSTs in parallel — each field's FST is independent
    let mut hc_chunks: Vec<(u16, String, Vec<u8>, usize)> = hc_field_data
        .into_par_iter()
        .map(|(chunk_idx, field_name, fst_entries)| {
            let hc_fst: fst_index::FstIndex<u64> =
                fst_index::FstIndex::build(fst_entries).expect("failed to build HC FST");
            let serialized = bincode::serialize(&hc_fst).expect("failed to serialize HC FST");
            let raw_size = serialized.len();
            let compressed =
                zstd::encode_all(&serialized[..], ZSTD_LEVEL).expect("zstd compress HC FST");
            (chunk_idx, field_name, compressed, raw_size)
        })
        .collect();

    // Sort by chunk index (par_iter may reorder)
    hc_chunks.sort_by_key(|(idx, _, _, _)| *idx);

    let build_elapsed = t_build.elapsed();

    println!("=== High-Cardinality Chunks ===");
    for (idx, field, compressed, raw_size) in &hc_chunks {
        println!(
            "  HC[{:>3}] {:<40} {} → {} bytes ({:.1} KiB, {:.2}x)",
            idx,
            field,
            raw_size,
            compressed.len(),
            compressed.len() as f64 / 1024.0,
            *raw_size as f64 / compressed.len() as f64,
        );
    }
    println!();

    // ── Step 7: Write the gix-chunk file ────────────────────────────────
    let t_write = Instant::now();

    let num_chunks = 1 + hc_chunks.len(); // primary + per-field
    let output_path = format!("{}.split_fst", journal_path);
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&output_path).expect("failed to create output file"),
    );

    // Write header: magic + version + num_chunks
    out.write_all(MAGIC).expect("write magic");
    out.write_all(&VERSION.to_le_bytes()).expect("write version");
    out.write_all(&(num_chunks as u32).to_le_bytes())
        .expect("write num_chunks");

    // Plan chunks with compressed sizes
    let mut index = gix_chunk::file::Index::for_writing();
    index.plan_chunk(CHUNK_PRIMARY, primary_compressed.len() as u64);
    for (idx, _, compressed, _) in &hc_chunks {
        index.plan_chunk(hc_chunk_id(*idx), compressed.len() as u64);
    }

    // Write TOC, then chunk data
    let mut chunk_writer = index
        .into_write(&mut out, HEADER_SIZE)
        .expect("failed to write TOC");

    // Primary chunk (compressed)
    let id = chunk_writer.next_chunk().expect("expected primary chunk");
    assert_eq!(id, CHUNK_PRIMARY);
    chunk_writer
        .write_all(&primary_compressed)
        .expect("write primary chunk");

    // HC chunks (compressed)
    for (idx, _, compressed, _) in &hc_chunks {
        let id = chunk_writer.next_chunk().expect("expected HC chunk");
        assert_eq!(id, hc_chunk_id(*idx));
        chunk_writer.write_all(compressed).expect("write HC chunk");
    }

    assert!(chunk_writer.next_chunk().is_none(), "unexpected extra chunk");
    chunk_writer.into_inner();
    out.flush().expect("flush");
    drop(out);

    let write_elapsed = t_write.elapsed();

    let file_size = std::fs::metadata(&output_path).expect("stat").len();
    let hc_total_compressed: usize = hc_chunks.iter().map(|(_, _, c, _)| c.len()).sum();
    let hc_total_raw: usize = hc_chunks.iter().map(|(_, _, _, r)| *r).sum();

    println!("=== Written File ===");
    println!("  Path:         {}", output_path);
    println!(
        "  Total size:   {} bytes ({:.1} KiB)",
        file_size,
        file_size as f64 / 1024.0
    );
    println!("  Header:       {} bytes", HEADER_SIZE);
    println!(
        "  TOC:          {} bytes",
        gix_chunk::file::Index::size_for_entries(num_chunks)
    );
    println!(
        "  Primary:      {} → {} bytes ({:.1} KiB)",
        primary_serialized.len(),
        primary_compressed.len(),
        primary_compressed.len() as f64 / 1024.0
    );
    println!(
        "  HC total:     {} → {} bytes ({:.1} KiB)",
        hc_total_raw,
        hc_total_compressed,
        hc_total_compressed as f64 / 1024.0
    );
    println!(
        "  Build time:   {:.1}ms",
        build_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "  Write time:   {:.1}ms",
        write_elapsed.as_secs_f64() * 1000.0
    );
    println!();

    // ── Step 8: Read back via mmap ──────────────────────────────────────
    println!("=== Read-back Verification ===");
    let t_read = Instant::now();

    let read_file = std::fs::File::open(&output_path).expect("open split file");
    let mmap = unsafe { memmap2::Mmap::map(&read_file) }.expect("mmap");
    let file_data = &mmap[..];

    // Parse header
    assert_eq!(&file_data[0..4], MAGIC, "bad magic");
    let version = u32::from_le_bytes(file_data[4..8].try_into().unwrap());
    assert_eq!(version, VERSION, "bad version");
    let num_chunks_read = u32::from_le_bytes(file_data[8..12].try_into().unwrap());

    println!("  Magic:      {:?}", std::str::from_utf8(&file_data[0..4]).unwrap());
    println!("  Version:    {}", version);
    println!("  Num chunks: {}", num_chunks_read);

    // Parse TOC
    let chunk_index = gix_chunk::file::Index::from_bytes(file_data, HEADER_SIZE, num_chunks_read)
        .expect("failed to parse chunk index");

    // Read + decompress primary chunk
    let primary_bytes_compressed = chunk_index
        .data_by_id(file_data, CHUNK_PRIMARY)
        .expect("primary chunk not found");
    let primary_bytes_decompressed =
        zstd::decode_all(primary_bytes_compressed).expect("zstd decompress primary");
    let primary_read: fst_index::FstIndex<PrimaryValue> =
        bincode::deserialize(&primary_bytes_decompressed).expect("failed to deserialize primary FST");

    println!(
        "  Primary FST: {} keys ({} compressed → {} decompressed)",
        primary_read.len(),
        primary_bytes_compressed.len(),
        primary_bytes_decompressed.len(),
    );

    // Demonstrate targeted access: look up a high-card field
    if let Some((idx, field, _, _)) = hc_chunks.first() {
        // Step A: consult primary FST for field metadata
        if let Some(pv) = primary_read.get(field.as_bytes()) {
            println!("  Primary['{}'] = {:?}", field, pv);
        }

        // Step B: load + decompress just that field's HC chunk
        let hc_compressed = chunk_index
            .data_by_id(file_data, hc_chunk_id(*idx))
            .expect("HC chunk not found");
        let hc_decompressed =
            zstd::decode_all(hc_compressed).expect("zstd decompress HC");
        let hc_fst: fst_index::FstIndex<u64> =
            bincode::deserialize(&hc_decompressed).expect("failed to deserialize HC FST");
        println!(
            "  HC[{}] '{}': {} keys, {} compressed → {} decompressed",
            idx,
            field,
            hc_fst.len(),
            hc_compressed.len(),
            hc_decompressed.len(),
        );

        // Step C: sample lookup in HC FST
        let mut sample_shown = 0;
        hc_fst.for_each(|key, count| {
            if sample_shown < 3 {
                if let Ok(k) = std::str::from_utf8(key) {
                    println!("    '{}' → {} entries", k, count);
                }
                sample_shown += 1;
            }
        });
    }

    // Demonstrate targeted access: look up a low-card field=value
    if let Some(field) = low_card_fields.first() {
        let mut sample_shown = 0;
        primary_read.prefix_for_each(format!("{}=", field).as_bytes(), |key, val| {
            if sample_shown < 3 {
                if let Ok(k) = std::str::from_utf8(key) {
                    match val {
                        PrimaryValue::Bitmap(bm) => {
                            println!("  Primary['{}'] = bitmap({} entries)", k, bm.len());
                        }
                        other => {
                            println!("  Primary['{}'] = {:?}", k, other);
                        }
                    }
                }
                sample_shown += 1;
            }
        });
    }

    let read_elapsed = t_read.elapsed();
    println!(
        "  Read time:  {:.1}ms",
        read_elapsed.as_secs_f64() * 1000.0
    );
    println!();

    // ── Step 9: Size comparison ─────────────────────────────────────────
    println!("=== Size Comparison ===");
    println!(
        "  Primary chunk (always loaded): {} bytes ({:.1} KiB) compressed",
        primary_compressed.len(),
        primary_compressed.len() as f64 / 1024.0
    );
    println!(
        "  HC chunks (loaded on demand):  {} bytes ({:.1} KiB) compressed",
        hc_total_compressed,
        hc_total_compressed as f64 / 1024.0
    );
    println!(
        "  Total split file:              {} bytes ({:.1} KiB)",
        file_size,
        file_size as f64 / 1024.0
    );
    println!(
        "  Raw (uncompressed) total:      {} bytes ({:.1} KiB)",
        primary_serialized.len() + hc_total_raw,
        (primary_serialized.len() + hc_total_raw) as f64 / 1024.0,
    );
    println!();
    println!(
        "  For a targeted query on a single field, only the primary chunk"
    );
    println!(
        "  ({:.1} KiB) + one HC chunk need to be loaded, vs the full {:.1} KiB.",
        primary_compressed.len() as f64 / 1024.0,
        file_size as f64 / 1024.0
    );

    // Cleanup
    std::fs::remove_file(&output_path).ok();
}
