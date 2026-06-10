//! WAL → SFST indexing pipeline.
//!
//! Two-phase build:
//!
//! - **Phase 1** (read) — [`decode::decode_frame`] decodes each WAL frame
//!   and streams its rows into the [`WalIndex`] (a
//!   [`decode::KvSink`]), which interns `key=value` attributes and
//!   accumulates the string interner + per-attribute bitmaps + per-log
//!   entries + per-log timestamps.
//! - **Phase 2** (write) — [`build_and_write`] consumes the `WalIndex` and
//!   emits the on-disk SFST file via [`crate::Writer`].
//!
//! The public entry points are [`index`] (defaults) and
//! [`index_with_options`] (cardinality threshold override). The frame
//! decode is public on its own ([`decode`]) so other consumers — e.g. a
//! query-time WAL row scan — share it rather than reimplement it.

mod arrow_columns;
mod bitset;
pub mod decode;
mod fst_builder;
pub mod kv_interner;
mod otap_frame;
pub mod reader;
pub mod wal_index;

pub use decode::{KvSink, decode_frame};
pub use fst_builder::build_and_write;
pub use kv_interner::KvSlot;
pub use reader::{BitmapFilter, IndexReader};

use fst_builder::build;

use std::path::Path;

use bumpalo::Bump;

use crate::{IndexError, Metadata, Summary};
use wal_index::WalIndex;

/// Default cardinality threshold for tier classification (see
/// [`crate::FieldTier`]). Public so every producer of field tables — the
/// indexer here and the WAL row scan in `sfsq` — classifies with the same
/// boundaries unless explicitly overridden.
pub const DEFAULT_CARDINALITY_THRESHOLD: u32 = 100;

/// Result of indexing a WAL file.
///
/// The earliest log date is derivable from `summary.min_timestamp_s` — it
/// is not returned separately.
pub struct IndexResult {
    /// Cheap summary fields written into the SFST `SUMR` chunk and stored
    /// inline on the registry entry.
    pub summary: Summary,
    /// Heavy index metadata (histogram + id_ranges + field table) written
    /// into the `META` chunk. Used at query time, not by the registry.
    pub metadata: Metadata,
    /// Byte size of the written SFST file.
    pub size: u64,
}

/// Build a split-FST index from a WAL file using default settings.
///
/// Reads the WAL file at `wal_path` and writes the index to `sfst_path`.
pub fn index(wal_path: &Path, sfst_path: &Path) -> Result<IndexResult, IndexError> {
    index_with_options(wal_path, sfst_path, DEFAULT_CARDINALITY_THRESHOLD)
}

/// Build a split-FST index from a WAL file with an explicit cardinality
/// threshold.
///
/// Fields with fewer unique values than `cardinality_threshold` go into the
/// primary FST; fields above split into per-field secondary chunks.
pub fn index_with_options(
    wal_path: &Path,
    sfst_path: &Path,
    cardinality_threshold: u32,
) -> Result<IndexResult, IndexError> {
    let mut reader = wal::Reader::open(wal_path)?;
    let arena = Bump::with_capacity(32 * 1024 * 1024);
    let mut wal_index = WalIndex::new(&arena, cardinality_threshold);

    let mut num_frames = 0;
    while let Some(wal_frame) = reader.next_frame()? {
        num_frames += 1;
        decode_frame(&wal_frame, &mut wal_index)?;
    }

    tracing::info!(
        "WAL file read complete path={} frames={num_frames} logs={}",
        wal_path.display(),
        wal_index.num_logs(),
    );

    let (summary, metadata) = build_and_write(&wal_index, sfst_path)?;
    let size = std::fs::metadata(sfst_path)?.len();

    Ok(IndexResult {
        summary,
        metadata,
        size,
    })
}

/// Index a frame-aligned byte `range` of a WAL file into an **in-memory**
/// SFST, returning its [`Summary`] and the serialized bytes.
///
/// The same two-phase build as [`index`], but reading only the frames in
/// `range` (via [`wal::Reader::open_range`]) and serializing the result to
/// a `Vec<u8>` instead of a file. This is how a query builds an index over
/// a chunk of an active WAL; see [`wal::FrameRange`] / `open_range` for the
/// frame-boundary and durable-prefix soundness checks.
///
/// The returned bytes parse with [`IndexReader::open`]. The caller cross-
/// checks `summary.total_logs` against the expected record count for the
/// range (the registry's `entry_count`) to confirm the prefix wasn't
/// truncated — the count check that [`wal::Reader::open_range`] defers.
pub fn index_range(
    wal_path: &Path,
    range: wal::FrameRange,
) -> Result<(Summary, Vec<u8>), IndexError> {
    // Scope Phase 1 so the 32 MiB arena and the WalIndex (bitmaps,
    // interner, per-log entries) drop before serialization: `build`'s
    // Writer owns its packed chunks outright and borrows neither, so
    // holding them through `write_to` would only inflate peak memory.
    let (writer, summary) = {
        let mut reader = wal::Reader::open_range(wal_path, range)?;
        let arena = Bump::with_capacity(32 * 1024 * 1024);
        let mut wal_index = WalIndex::new(&arena, DEFAULT_CARDINALITY_THRESHOLD);

        let mut num_frames = 0;
        while let Some(wal_frame) = reader.next_frame()? {
            num_frames += 1;
            decode_frame(&wal_frame, &mut wal_index)?;
        }
        tracing::debug!(
            "WAL range read complete path={} start={} end={} frames={num_frames} logs={}",
            wal_path.display(),
            range.start(),
            range.end(),
            wal_index.num_logs(),
        );

        let (writer, summary, _metadata) = build(&wal_index)?;
        (writer, summary)
    };

    let mut bytes = Vec::new();
    writer.write_to(&mut bytes)?;

    Ok((summary, bytes))
}
