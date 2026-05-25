//! End-to-end WAL file indexing — reads a WAL file and produces a `.sfst` index.
//!
//! This is the entry point for the two-phase indexing pipeline:
//! - **Phase 1**: sequential scan of WAL frames via [`crate::process_frame`].
//! - **Phase 2**: [`crate::build_and_write`] transforms the in-memory data
//!   into the on-disk split-FST format.

use std::path::Path;

use bumpalo::Bump;

use sfst::IndexMetadata;

use crate::process_frame::process_frame;
use crate::wal_index::WalIndex;

const DEFAULT_CARDINALITY_THRESHOLD: u32 = 100;

/// Result of indexing a WAL file.
///
/// The earliest log date is derivable from `summary.min_timestamp_s` — we
/// don't return it separately.
pub struct IndexResult {
    /// Cheap summary fields written into the SFST SUMR chunk and stored
    /// inline on the registry entry.
    pub summary: sfst::FileSummary,
    /// Heavy index metadata (histogram + id_ranges) written into the SFST
    /// META chunk. Used at query time, not by the registry.
    pub metadata: IndexMetadata,
    /// Byte size of the written SFST file.
    pub size: u64,
}

/// Build a split-FST index from a WAL file using default settings.
///
/// Reads the WAL file at `wal_path` and writes the index to `out_path`.
pub fn index_wal_file(
    wal_path: &Path,
    out_path: &Path,
) -> Result<IndexResult, Box<dyn std::error::Error>> {
    index_wal_file_with_options(wal_path, out_path, DEFAULT_CARDINALITY_THRESHOLD)
}

/// Build a split-FST index from a WAL file with explicit cardinality threshold.
///
/// Fields with fewer unique values than `cardinality_threshold` go into the
/// primary FST; fields above go into per-field secondary chunks.
pub fn index_wal_file_with_options(
    wal_path: &Path,
    out_path: &Path,
    cardinality_threshold: u32,
) -> Result<IndexResult, Box<dyn std::error::Error>> {
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

    let (summary, metadata) = crate::build_and_write(&wal_index, out_path)?;
    let size = std::fs::metadata(out_path)?.len();

    Ok(IndexResult {
        summary,
        metadata,
        size,
    })
}
