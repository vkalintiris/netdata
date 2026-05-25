//! Phase 2 (Writing) of the split-FST indexing pipeline.
//!
//! Transforms the in-memory data structures built during Phase 1 into the
//! on-disk split-FST format described in `sfst/FORMAT.md`.
//!
//! The pipeline steps are:
//!
//! 1. **Cardinality classification** — group key=value pairs by field name,
//!    classify each field as low / mid / high cardinality.
//! 2. **Primary FST** — low-card `key=value` entries with bitmaps.
//! 3. **Secondary chunks** — mid-card fields → per-field FST; high-card
//!    fields → bincode + zstd blob.
//! 4. **Tier-aligned ID assignment** — sequential file IDs in FST key order
//!    across low → mid → high tiers.
//! 5. **Stream derivation** — cross-join service.name / service.namespace
//!    bitmaps to identify (namespace, name) streams.
//! 6. **Per-stream log entries** — for each stream, translate interner IDs
//!    to file IDs and serialize in time-sorted order.
//! 7. **Metadata + write** — assemble field table, histogram, and write all
//!    sections to disk.

use std::path::Path;
use std::time::Instant;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use treight::Bitmap;

use crate::bitset::Bitset;
use crate::kv_interner::KeyValueId;
use crate::wal_index::{ServiceStream, SparseHistogram, TimeOrder, WalIndex};

/// A tier-aligned ID assigned during writing. Sequential within each
/// cardinality tier, ordered by FST key. Stored on disk in log entries.
///
/// Not to be confused with [`KeyValueId`] (assigned during reading by the
/// string interner, in insertion order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileId(pub u32);

impl FileId {
    #[inline]
    pub fn idx(self) -> usize {
        self.0 as usize
    }
}

/// Value type for FST entries (primary and mid-card secondary FSTs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitmapValue {
    pub desc: Bitmap,
    pub data: Vec<u8>,
}

/// Cardinality tier for a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldTier {
    Low,
    Mid,
    High,
}

/// An entry in the field table: field name, cardinality, and tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldEntry {
    pub name: String,
    pub cardinality: u32,
    pub tier: FieldTier,
}

/// Heavy query-time metadata stored in the META chunk.
///
/// Holds the data the query engine needs once it has decided to scan a file:
/// the sparse timestamp histogram and the cardinality-tier ID ranges. The
/// cheap-to-read summary fields (min/max timestamp, total log count, stream
/// list) live in their own [`sfst::FileSummary`] in the SUMR chunk and on
/// the registry entry; readers that only need those fields should call
/// [`sfst::Reader::summary`] instead of opening this chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub histogram: SparseHistogram,
    pub id_ranges: IdRanges,
}

/// Contiguous ID ranges for the three cardinality tiers.
///
/// File IDs are assigned sequentially: `0..low_end` for low-card,
/// `low_end..mid_end` for mid-card, `mid_end..high_end` for high-card.
/// The reader uses these ranges to determine which section (primary FST,
/// secondary FST, or HC chunk) to consult for a given ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdRanges {
    pub low_end: FileId,
    pub mid_end: FileId,
    pub high_end: FileId,
}

/// Build tier-aligned file ID translation table.
///
/// Uses [`WalIndex::tier_assignment`] to get the canonical ordering,
/// then maps each [`KeyValueId`] to its sequential [`FileId`].
fn build_id_translation(wal_index: &WalIndex) -> (Vec<FileId>, IdRanges) {
    let [low_kv_ids, mid_kv_ids, high_kv_ids] = wal_index.tier_assignment();

    let total_kv_ids = low_kv_ids.len() + mid_kv_ids.len() + high_kv_ids.len();
    let mut table = vec![FileId(0); total_kv_ids];

    let mut curr_file_id = 0u32;
    for &kv_id in low_kv_ids
        .iter()
        .chain(mid_kv_ids.iter())
        .chain(high_kv_ids.iter())
    {
        table[kv_id.idx()] = FileId(curr_file_id);
        curr_file_id += 1;
    }

    let low_end = FileId(low_kv_ids.len() as u32);
    let mid_end = FileId(low_end.0 + mid_kv_ids.len() as u32);
    let high_end = FileId(mid_end.0 + high_kv_ids.len() as u32);

    tracing::debug!(
        "file ID ranges: {} total (low 0..{}, mid {}..{}, high {}..{})",
        high_end.0,
        low_end.0,
        low_end.0,
        mid_end.0,
        mid_end.0,
        high_end.0,
    );

    (
        table,
        IdRanges {
            low_end,
            mid_end,
            high_end,
        },
    )
}

/// Build the file's single log-entries chunk in chronological order.
///
/// Iterates [`TimeOrder::iter_by_time`] to walk insertion-order positions
/// in chronological order, translates each log's [`KeyValueId`]s to
/// [`FileId`]s, and serializes the result with bincode + zstd.
fn build_stream_entries(
    log_entries: &[Vec<KeyValueId>],
    time_order: &TimeOrder,
    kv_to_file: &[FileId],
    writer: &mut sfst::Writer,
) -> Result<usize, Box<dyn std::error::Error>> {
    let entries: Vec<Vec<FileId>> = time_order
        .iter_by_time()
        .map(|ins| {
            log_entries[ins as usize]
                .iter()
                .map(|&kv_id| kv_to_file[kv_id.idx()])
                .collect()
        })
        .collect();

    let raw = bincode::serde::encode_to_vec(&entries, bincode::config::standard())?;
    let packed = zstd::encode_all(&raw[..], 1)?;
    let len = packed.len();
    writer.add_chunk(packed);

    Ok(len)
}

/// Build the primary FST: low-card `key=value` entries with bitmaps.
fn build_primary_fst(
    wal_index: &WalIndex,
    time_order: &TimeOrder,
    writer: &mut sfst::Writer,
) -> Result<(), Box<dyn std::error::Error>> {
    let t = Instant::now();
    let mut entries: Vec<(&str, BitmapValue)> = Vec::new();

    let low = wal_index.low_fields();
    for (_, kv_ids) in &low {
        entries.reserve(kv_ids.len());

        for &kv_id in *kv_ids {
            let kv_pair = wal_index.resolve(kv_id);
            let (desc, data) = remap_one_bitmap(wal_index.bitmap(kv_id), time_order);
            entries.push((kv_pair, BitmapValue { desc, data }));
        }
    }

    let fst: fst_index::FstIndex<BitmapValue> = fst_index::FstIndex::build(entries)?;
    let packed = sfst::pack(&fst, 3)?;
    tracing::debug!(
        "primary FST built: {} fields, {} KB, {}ms",
        low.len(),
        packed.len() / 1024,
        t.elapsed().as_millis(),
    );
    writer.set_primary(packed);
    Ok(())
}

/// Build secondary FST chunks for mid-cardinality fields.
fn build_mid_card_chunks(
    wal_index: &WalIndex,
    time_order: &TimeOrder,
    writer: &mut sfst::Writer,
) -> Result<(), Box<dyn std::error::Error>> {
    let t = Instant::now();
    let mut total_kb = 0usize;

    let mut entries: Vec<(&str, BitmapValue)> = Vec::new();

    let mid = wal_index.mid_fields();
    for &(_, kv_ids) in &mid {
        entries.clear();

        for &kv_id in kv_ids {
            let kv_pair = wal_index.resolve(kv_id);
            let (desc, data) = remap_one_bitmap(wal_index.bitmap(kv_id), time_order);
            entries.push((kv_pair, BitmapValue { desc, data }));
        }

        let fst: fst_index::FstIndex<BitmapValue> = fst_index::FstIndex::build(entries.drain(..))?;
        let packed = sfst::pack(&fst, 3)?;
        total_kb += packed.len() / 1024;
        writer.add_chunk(packed);
    }

    tracing::debug!(
        "mid-card FSTs built: {} fields, {} KB, {}ms",
        mid.len(),
        total_kb,
        t.elapsed().as_millis(),
    );

    Ok(())
}

/// Build high-cardinality field chunks (bincode + zstd).
fn build_high_card_chunks(
    wal_index: &WalIndex,
    time_order: &TimeOrder,
    writer: &mut sfst::Writer,
) -> Result<(), Box<dyn std::error::Error>> {
    let t = Instant::now();
    let mut total_kb = 0usize;

    let mut entries: Vec<(&str, BitmapValue)> = Vec::new();

    let high = wal_index.high_fields();
    for &(_, ids) in &high {
        entries.clear();

        for &id in ids {
            let key = wal_index.resolve(id);
            let (desc, data) = remap_one_bitmap(wal_index.bitmap(id), time_order);
            entries.push((key, BitmapValue { desc, data }));
        }

        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

        let raw = bincode::serde::encode_to_vec(&entries, bincode::config::standard())?;
        let packed = zstd::encode_all(&raw[..], 1)?;
        total_kb += packed.len() / 1024;
        writer.add_chunk(packed);
    }

    tracing::debug!(
        "high-card chunks built: {} fields, {} KB, {}ms",
        high.len(),
        total_kb,
        t.elapsed().as_millis(),
    );

    Ok(())
}

/// Resolve and write the file's single stream.
///
/// Each SFST file is required to contain exactly one `(namespace, name)`
/// pair — the WAL writer partitions frames by `ns_hash`, and the ingestor
/// rejects writes whose `(namespace, name)` doesn't match the canonical
/// pair for an `ns_hash`. If multiple distinct values are seen for either
/// key, [`WalIndex::service_stream`] surfaces the offenders via
/// [`MultipleStreamsError`] and we fail the index build.
fn build_streams(
    wal_index: &WalIndex,
    time_order: &TimeOrder,
    kv_to_file: &[FileId],
    writer: &mut sfst::Writer,
) -> Result<ServiceStream, Box<dyn std::error::Error>> {
    let stream = wal_index.service_stream()?;

    let namespace = if stream.namespace.is_empty() {
        "<none>"
    } else {
        &stream.namespace
    };
    let name = if stream.name.is_empty() {
        "<none>"
    } else {
        &stream.name
    };
    tracing::debug!("stream {namespace}/{name}: {} logs", wal_index.num_logs(),);

    let t = Instant::now();
    let stream_bytes =
        build_stream_entries(&wal_index.log_entries, time_order, kv_to_file, writer)?;
    tracing::debug!(
        "stream log entries built: {} KB, {}ms",
        stream_bytes / 1024,
        t.elapsed().as_millis(),
    );

    Ok(stream)
}

/// Build and write a split-fst index file.
///
/// This is Phase 2 of the indexing pipeline. Takes the [`WalIndex`] built by
/// Phase 1 and produces a split-fst file with: summary, metadata, primary FST,
/// secondary chunks (mid/high-card), and per-stream log entries.
///
/// Returns both the cheap-to-read [`sfst::FileSummary`] (which the registry
/// stores inline) and the heavier [`IndexMetadata`] (only needed for query
/// planning and execution).
pub fn build_and_write(
    wal_index: &WalIndex,
    out_path: &Path,
) -> Result<(sfst::FileSummary, IndexMetadata), Box<dyn std::error::Error>> {
    let t_start = Instant::now();

    let mut writer = sfst::Writer::new();

    let t = Instant::now();

    // Build time order
    let time_order = wal_index.time_order();
    tracing::debug!("time order built: {}ms", t.elapsed().as_millis());

    // Build low/mid-cardinality FSTs and high-cardinality chunks
    build_primary_fst(wal_index, &time_order, &mut writer)?;
    build_mid_card_chunks(wal_index, &time_order, &mut writer)?;
    build_high_card_chunks(wal_index, &time_order, &mut writer)?;

    let (kv_to_file, id_ranges) = build_id_translation(wal_index);
    let stream = build_streams(wal_index, &time_order, &kv_to_file, &mut writer)?;

    // Field table (FLDS chunk).
    let field_table: Vec<FieldEntry> = wal_index
        .low_fields()
        .iter()
        .map(|(name, ids)| FieldEntry {
            name: name.to_string(),
            cardinality: ids.len() as u32,
            tier: FieldTier::Low,
        })
        .chain(wal_index.mid_fields().iter().map(|(name, ids)| FieldEntry {
            name: name.to_string(),
            cardinality: ids.len() as u32,
            tier: FieldTier::Mid,
        }))
        .chain(
            wal_index
                .high_fields()
                .iter()
                .map(|(name, ids)| FieldEntry {
                    name: name.to_string(),
                    cardinality: ids.len() as u32,
                    tier: FieldTier::High,
                }),
        )
        .collect();
    writer.set_fields(sfst::pack(&field_table, 1)?);

    // Compute histogram once; reused by both the summary (for min/max
    // derivation) and the heavy metadata.
    let histogram = wal_index.sparse_histogram(&time_order);

    let summary = sfst::FileSummary {
        min_timestamp_s: histogram.timestamps.first().copied().unwrap_or(0),
        max_timestamp_s: histogram.timestamps.last().copied().unwrap_or(0),
        total_logs: wal_index.num_logs() as u32,
        stream: sfst::StreamEntry {
            namespace: stream.namespace.clone(),
            name: stream.name.clone(),
        },
    };
    writer.set_summary(sfst::pack(&summary, 1)?);

    let metadata = IndexMetadata {
        histogram,
        id_ranges,
    };
    writer.set_metadata(sfst::pack(&metadata, 1)?);

    let t = Instant::now();
    let tmp_path = out_path.with_extension("sfst.tmp");
    let file = std::fs::File::create(&tmp_path)?;
    let mut buf = std::io::BufWriter::new(file);
    writer.write_to(&mut buf)?;
    let file = buf.into_inner()?;
    file.sync_all()?;
    let file_size = file.metadata()?.len();
    drop(file);

    std::fs::rename(&tmp_path, out_path)?;
    tracing::info!(
        "index written path={} size_kb={} write_ms={} total_ms={}",
        out_path.display(),
        file_size / 1024,
        t.elapsed().as_millis(),
        t_start.elapsed().as_millis(),
    );

    Ok((summary, metadata))
}

/// Remap a single roaring bitmap from insertion order to time-sorted order,
/// then encode as a treight bitmap.
///
/// For dense bitmaps (cardinality > half the universe), stores the complement
/// instead — fewer positions to encode. Uses a bitset for large bitmaps
/// (avoids O(n log n) sort) and a sorted vec for sparse ones.
fn remap_one_bitmap(rb: &RoaringBitmap, time_order: &TimeOrder) -> (Bitmap, Vec<u8>) {
    let universe_size = time_order.len();
    let half = universe_size as u64 / 2;
    let card = rb.len() as u64;
    let mut data = Vec::new();

    if rb.is_empty() {
        let desc = Bitmap::empty(universe_size);
        return (desc, data);
    }

    // Use bitset for dense bitmaps, sort for sparse.
    let bitset_threshold = (universe_size as usize / 64).max(256);

    if rb.len() as usize >= bitset_threshold {
        let mut bitset = Bitset::new(universe_size);
        for v in rb.iter() {
            bitset.set(time_order.to_sorted(v));
        }
        let desc = if card > half {
            Bitmap::from_sorted_iter_complemented(
                bitset.iter_zeros(universe_size),
                universe_size,
                &mut data,
            )
        } else {
            Bitmap::from_sorted_iter(bitset.iter_ones(), universe_size, &mut data)
        };
        (desc, data)
    } else {
        let mut remapped: Vec<u32> = rb.iter().map(|v| time_order.to_sorted(v)).collect();
        remapped.sort_unstable();
        let desc = if card > half {
            // Build complement from the sorted remapped values.
            let mut bitset = Bitset::new(universe_size);
            for &v in &remapped {
                bitset.set(v);
            }
            Bitmap::from_sorted_iter_complemented(
                bitset.iter_zeros(universe_size),
                universe_size,
                &mut data,
            )
        } else {
            Bitmap::from_sorted_iter(remapped.iter().copied(), universe_size, &mut data)
        };
        (desc, data)
    }
}
