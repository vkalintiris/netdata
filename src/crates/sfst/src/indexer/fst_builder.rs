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
use treight::Bitmap;

use super::bitset::Bitset;
use super::kv_interner::KvSlot;
use super::wal_index::{TimeOrder, WalIndex};
use crate::{
    BitmapValue, FieldEntry, FieldTier, IdRanges, IndexError, KvId, Metadata, ServiceStream,
};

/// Build tier-aligned key=value ID translation table.
///
/// Uses [`WalIndex::tier_assignment`] to get the canonical ordering,
/// then maps each [`KvSlot`] to its sequential [`KvId`].
fn build_id_translation(wal_index: &WalIndex) -> (Vec<KvId>, IdRanges) {
    let [low_kv_slots, mid_kv_slots, high_kv_slots] = wal_index.tier_assignment();

    let total_kv_slots = low_kv_slots.len() + mid_kv_slots.len() + high_kv_slots.len();
    let mut table = vec![KvId(0); total_kv_slots];

    let mut curr_id = 0u32;
    for &kv_slot in low_kv_slots
        .iter()
        .chain(mid_kv_slots.iter())
        .chain(high_kv_slots.iter())
    {
        table[kv_slot.idx()] = KvId(curr_id);
        curr_id += 1;
    }

    let low_end = KvId(low_kv_slots.len() as u32);
    let mid_end = KvId(low_end.0 + mid_kv_slots.len() as u32);
    let high_end = KvId(mid_end.0 + high_kv_slots.len() as u32);

    tracing::debug!(
        "kv id ranges: {} total (low 0..{}, mid {}..{}, high {}..{})",
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

/// Build the file's stream-batch chunks in chronological order.
///
/// Materialises every log's `KvId` list in chronological order, splits
/// the result into [`num_stream_batches`](crate::num_stream_batches)
/// slices of `batch_size` entries each, and packs each slice into its
/// own zstd blob.
///
/// `total_logs == 0` is handled explicitly: a single empty batch is
/// emitted so the file always carries at least one `SB{i}` chunk.
fn build_stream_batches(
    log_entries: &[Vec<KvSlot>],
    time_order: &TimeOrder,
    kv_to_file: &[KvId],
    total_logs: u32,
    writer: &mut crate::Writer,
) -> Result<usize, IndexError> {
    let entries: Vec<Vec<KvId>> = time_order
        .iter_by_time()
        .map(|ins| {
            log_entries[ins as usize]
                .iter()
                .map(|&kv_slot| kv_to_file[kv_slot.idx()])
                .collect()
        })
        .collect();

    let mut total_packed = 0usize;
    if entries.is_empty() {
        // num_stream_batches(0) == 1: emit a single empty batch so the
        // file's chunk layout is always valid.
        let packed = crate::pack(&crate::StreamBatch::for_write(&[]), crate::ZSTD_LEVEL_DEFAULT)?;
        total_packed += packed.len();
        writer.add_stream_batch(packed);
    } else {
        let batch_size = crate::stream_batch_size(total_logs) as usize;
        for batch in entries.chunks(batch_size) {
            let packed =
                crate::pack(&crate::StreamBatch::for_write(batch), crate::ZSTD_LEVEL_DEFAULT)?;
            total_packed += packed.len();
            writer.add_stream_batch(packed);
        }
    }

    Ok(total_packed)
}

/// Build the primary FST: low-card `key=value` entries with bitmaps.
fn build_primary_fst(
    wal_index: &WalIndex,
    time_order: &TimeOrder,
    writer: &mut crate::Writer,
) -> Result<(), IndexError> {
    let t = Instant::now();
    let mut entries: Vec<(&str, BitmapValue)> = Vec::new();

    let low = wal_index.low_fields();
    for (_, kv_slots) in &low {
        entries.reserve(kv_slots.len());

        for &kv_slot in *kv_slots {
            let kv_pair = wal_index.resolve(kv_slot);
            let (desc, data) = remap_one_bitmap(wal_index.bitmap(kv_slot), time_order);
            entries.push((kv_pair, BitmapValue { desc, data }));
        }
    }

    let fst: fst_index::FstIndex<BitmapValue> = fst_index::FstIndex::build(entries)?;
    let packed = crate::pack(&fst, crate::ZSTD_LEVEL_FST)?;
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
    writer: &mut crate::Writer,
) -> Result<(), IndexError> {
    let t = Instant::now();
    let mut total_kb = 0usize;

    let mut entries: Vec<(&str, BitmapValue)> = Vec::new();

    let mid = wal_index.mid_fields();
    for &(_, kv_slots) in &mid {
        entries.clear();

        for &kv_slot in kv_slots {
            let kv_pair = wal_index.resolve(kv_slot);
            let (desc, data) = remap_one_bitmap(wal_index.bitmap(kv_slot), time_order);
            entries.push((kv_pair, BitmapValue { desc, data }));
        }

        let fst: fst_index::FstIndex<BitmapValue> = fst_index::FstIndex::build(entries.drain(..))?;
        let packed = crate::pack(&fst, crate::ZSTD_LEVEL_FST)?;
        total_kb += packed.len() / 1024;
        writer.add_mid_field(packed);
    }

    tracing::debug!(
        "mid-card FSTs built: {} fields, {} KB, {}ms",
        mid.len(),
        total_kb,
        t.elapsed().as_millis(),
    );

    Ok(())
}

#[cfg(test)]
mod tests;

/// Build high-cardinality field chunks (bincode + zstd).
///
/// Each chunk is a [`crate::HighField`] — a string arena of sorted
/// `key=value` keys plus their per-key `u8` batch-mask. Bit `b` of the mask
/// is set iff the value appears in stream batch `b`. Batch boundaries
/// are defined by `batch_size` over time-sorted positions;
/// `time_order` translates each insertion-order position from the
/// roaring bitmap into its chronological position before bucketing.
fn build_high_card_chunks(
    wal_index: &WalIndex,
    time_order: &TimeOrder,
    batch_size: u32,
    writer: &mut crate::Writer,
) -> Result<(), IndexError> {
    let t = Instant::now();
    let mut total_kb = 0usize;

    let mut paired: Vec<(&str, u8)> = Vec::new();

    let high = wal_index.high_fields();
    for &(_, slots) in &high {
        paired.clear();
        for &slot in slots {
            let key = wal_index.resolve(slot);
            let mask = batch_mask(wal_index.bitmap(slot), time_order, batch_size);
            paired.push((key, mask));
        }
        paired.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

        // Transpose to parallel columns, then pack as the arena layout.
        let (keys, masks): (Vec<&str>, Vec<u8>) = paired.iter().copied().unzip();
        let high = crate::HighField::for_write(&keys, masks);

        let packed = crate::pack(&high, crate::ZSTD_LEVEL_DEFAULT)?;
        total_kb += packed.len() / 1024;
        writer.add_high_field(packed);
    }

    tracing::debug!(
        "high-card chunks built: {} fields, {} KB, {}ms",
        high.len(),
        total_kb,
        t.elapsed().as_millis(),
    );

    Ok(())
}

/// Compute the per-value batch-membership mask for a high-card value.
///
/// Walks the roaring bitmap's insertion-order positions, remaps each
/// through `time_order` to its chronological position, divides by
/// `batch_size` to get the batch index, and sets the corresponding bit
/// in the returned `u8`.
fn batch_mask(rb: &RoaringBitmap, time_order: &TimeOrder, batch_size: u32) -> u8 {
    debug_assert!(
        batch_size > 0,
        "batch_size must be > 0 when high-card values exist"
    );
    let mut mask: u8 = 0;
    for ins_pos in rb.iter() {
        let sorted_pos = time_order.to_sorted(ins_pos);
        let bit = (sorted_pos / batch_size) as u8;
        debug_assert!(bit < crate::MAX_STREAM_BATCHES, "batch index out of range");
        mask |= 1u8 << bit;
    }
    mask
}

/// Resolve and write the file's single stream.
///
/// Each SFST file is required to contain exactly one `(namespace, name)`
/// pair — the WAL writer partitions frames by `ns_hash`, and the ingestor
/// rejects writes whose `(namespace, name)` doesn't match the canonical
/// pair for an `ns_hash`. If multiple distinct values are seen for either
/// key, [`WalIndex::service_stream`] surfaces the offenders via
/// [`IndexError::MultipleStreams`] and we fail the index build.
fn build_streams(
    wal_index: &WalIndex,
    time_order: &TimeOrder,
    kv_to_file: &[KvId],
    total_logs: u32,
    writer: &mut crate::Writer,
) -> Result<ServiceStream, IndexError> {
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
    tracing::debug!(
        "stream {namespace}/{name}: {} logs, {} batches",
        wal_index.num_logs(),
        crate::num_stream_batches(total_logs),
    );

    let t = Instant::now();
    let stream_bytes = build_stream_batches(
        &wal_index.log_entries,
        time_order,
        kv_to_file,
        total_logs,
        writer,
    )?;
    tracing::debug!(
        "stream batches built: {} KB total, {}ms",
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
/// Returns both the cheap-to-read [`crate::Summary`] (which the registry
/// stores inline) and the heavier [`Metadata`] (only needed for query
/// planning and execution).
pub fn build_and_write(
    wal_index: &WalIndex,
    out_path: &Path,
) -> Result<(crate::Summary, Metadata), IndexError> {
    let t_start = Instant::now();

    let (writer, summary, metadata) = build(wal_index)?;

    let t = Instant::now();
    let tmp_path = out_path.with_extension("sfst.tmp");
    let file = std::fs::File::create(&tmp_path)?;
    let mut buf = std::io::BufWriter::new(file);
    writer.write_to(&mut buf)?;
    let file = buf.into_inner().map_err(|e| e.into_error())?;
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

/// Phase 2 proper: consume a [`WalIndex`] into an in-memory
/// [`crate::Writer`] plus the [`crate::Summary`] / [`Metadata`] it
/// carries. Shared by [`build_and_write`] (which then writes the writer
/// to disk) and the in-memory range index
/// ([`index_range`](super::index_range), which serializes it to a
/// `Vec<u8>`). No I/O happens here.
pub(super) fn build(
    wal_index: &WalIndex,
) -> Result<(crate::Writer, crate::Summary, Metadata), IndexError> {
    let mut writer = crate::Writer::new();

    let t = Instant::now();

    // Build time order
    let time_order = wal_index.time_order();
    tracing::debug!("time order built: {}ms", t.elapsed().as_millis());

    // Total log count drives the stream-batch partitioning.
    let total_logs = wal_index.num_logs() as u32;
    let batch_size = crate::stream_batch_size(total_logs);

    // Build low/mid-cardinality FSTs and high-cardinality chunks
    build_primary_fst(wal_index, &time_order, &mut writer)?;
    build_mid_card_chunks(wal_index, &time_order, &mut writer)?;
    build_high_card_chunks(wal_index, &time_order, batch_size, &mut writer)?;

    let (kv_to_file, id_ranges) = build_id_translation(wal_index);
    let stream = build_streams(wal_index, &time_order, &kv_to_file, total_logs, &mut writer)?;

    // Per-log timestamps in chronological order, parallel-indexed to
    // the stream-log-entries chunk.
    let timestamps_chronological: Vec<i64> = time_order
        .iter_by_time()
        .map(|ins| wal_index.timestamps[ins as usize])
        .collect();
    writer.set_timestamps(crate::pack(
        &timestamps_chronological,
        crate::ZSTD_LEVEL_DEFAULT,
    )?);

    // Field table, ordered low → mid → high (each tier sorted by name).
    let fields: crate::FieldTable = wal_index
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

    // Compute histogram once; reused by both the summary (for min/max
    // derivation) and the heavy metadata.
    let histogram = wal_index.sparse_histogram(&time_order);

    let summary = crate::Summary {
        min_timestamp_s: histogram.timestamps.first().copied().unwrap_or(0),
        max_timestamp_s: histogram.timestamps.last().copied().unwrap_or(0),
        total_logs: wal_index.num_logs() as u32,
        stream: crate::ServiceStream {
            namespace: stream.namespace.clone(),
            name: stream.name.clone(),
        },
    };
    writer.set_summary(crate::pack(&summary, crate::ZSTD_LEVEL_DEFAULT)?);

    let metadata = Metadata {
        histogram,
        id_ranges,
        fields,
    };
    writer.set_metadata(crate::pack(&metadata, crate::ZSTD_LEVEL_DEFAULT)?);

    Ok((writer, summary, metadata))
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
