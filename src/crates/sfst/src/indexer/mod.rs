//! WAL → SFST indexing pipeline.
//!
//! Two-phase build:
//!
//! - **Phase 1** (read) — [`process_frame`] iterates each WAL frame, interns
//!   `key=value` attributes, and accumulates a [`WalIndex`](wal_index::WalIndex)
//!   (string interner + per-attribute bitmaps + per-log entries + per-log
//!   timestamps).
//! - **Phase 2** (write) — [`build_and_write`] consumes the `WalIndex` and
//!   emits the on-disk SFST file via [`crate::Writer`].
//!
//! The public entry points are [`index`] (defaults) and
//! [`index_with_options`] (cardinality threshold override).

mod arrow_columns;
mod bitset;
mod fst_builder;
pub mod kv_interner;
mod otap_frame;
mod process_frame;
pub mod reader;
pub mod wal_index;

pub use fst_builder::build_and_write;
pub use kv_interner::KvSlot;
pub use reader::IndexReader;

use std::path::Path;

use bumpalo::Bump;

use crate::{IndexError, Metadata, Summary};
use process_frame::process_frame;
use wal_index::WalIndex;

/// Default cardinality threshold for tier classification (see [`crate::FieldTier`]).
const DEFAULT_CARDINALITY_THRESHOLD: u32 = 100;

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
        process_frame(&mut wal_index, &wal_frame)?;
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
