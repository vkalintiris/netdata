//! Container format for split-FST log indexes.
//!
//! An SFST file holds one **primary** [`FstIndex`](fst_index::FstIndex) of
//! low-cardinality `key=value` pairs, zero or more **secondary** chunks
//! (mid-cardinality per-field FSTs, high-cardinality per-field sorted lists),
//! and one stream-log-entries chunk, all keyed by a [`gix_chunk`] TOC for
//! O(1) random access via mmap. Each SFST covers exactly one log stream
//! (one `(service.namespace, service.name)` pair).
//!
//! The complete on-disk specification — chunk layout, ids, encoding,
//! version compatibility, and reader access patterns — lives in
//! `FORMAT.md` alongside this crate. Treat it as the source of truth;
//! the rustdoc here covers only the public API.
//!
//! # Example
//!
//! ```no_run
//! use fst_index::FstIndex;
//! use sfst::BitmapValue;
//! use treight::Bitmap;
//!
//! // Build a minimal primary FST with one `key=value` entry.
//! let bm = BitmapValue { desc: Bitmap::empty(0), data: Vec::new() };
//! let primary: FstIndex<BitmapValue> =
//!     FstIndex::build([("level=info", bm)]).unwrap();
//!
//! // Write
//! let mut writer = sfst::Writer::new();
//! writer.set_primary(sfst::pack(&primary, 1).unwrap());
//! let mut buf = Vec::new();
//! writer.write_to(&mut buf).unwrap();
//!
//! // Read back
//! let reader = sfst::Reader::open(&buf).unwrap();
//! let primary = reader.primary().unwrap();
//! assert!(primary.get(b"level=info").is_some());
//! ```

mod error;
pub mod indexer;
pub mod query;
mod reader;
mod schema;
mod writer;

pub mod registry;

pub use error::{Error, IndexError};
pub use file_registry::ServiceStream;
pub use indexer::{
    BitmapFilter, IndexReader, IndexResult, build_and_write, index, index_with_options,
};
pub use query::{
    Bucket, FacetResult, Filter, Grid, Matcher, MaterializedRow, Timeline, Timestamps,
    compile_query,
};
pub use reader::{Reader, unpack};
pub use registry::{File, Registry};

/// Highest SFST sequence on disk across every tenant subdir of
/// `base`. Returns `0` when `base` is missing or empty. Paired with
/// [`wal::scan_max_sequence_recursive`]; the ingestor takes the max
/// of both at startup so the seq counter stays monotonic even when
/// WALs have been cleaned up but SFSTs remain.
pub fn scan_max_sequence_recursive(base: &std::path::Path) -> std::io::Result<u64> {
    file_registry::scan_max_sequence_recursive(base, registry::SFST_EXT)
}
pub use schema::{
    BitmapValue, FieldEntry, FieldTable, FieldTier, HighField, Histogram, IdRanges, KvId,
    Metadata, StreamBatch,
    Summary,
};
pub use writer::{Writer, pack};

// ── Format constants ─────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"SFST";
// v3: high-card chunks switched from `Vec<String>` keys to the string-arena
//     layout (keys_blob + key_lens).
// v4: stream-batch chunks switched from `Vec<Vec<KvId>>` to the fixed-width
//     arena (kv_bytes + row_lens). Older files are rejected on open.
const VERSION: u32 = 4;
const HEADER_SIZE: usize = 12; // magic(4) + version(4) + num_chunks(4)

const CHUNK_SUMMARY: gix_chunk::Id = *b"SUMR";
const CHUNK_META: gix_chunk::Id = *b"META";
const CHUNK_PRIMARY: gix_chunk::Id = *b"PRIM";
const CHUNK_TIMS: gix_chunk::Id = *b"TIMS";

/// Minimum number of logs in each stream batch. Files with fewer than
/// `MIN_LOGS_PER_BATCH` total logs use a single batch; otherwise the
/// batch count grows up to [`MAX_STREAM_BATCHES`] so that no batch ever
/// holds fewer than ~`MIN_LOGS_PER_BATCH` entries.
pub const MIN_LOGS_PER_BATCH: u32 = 1024;

/// Hard cap on the number of stream-batch chunks per SFST. Chosen so the
/// per-value batch-membership mask fits in a `u8` (one bit per batch).
pub const MAX_STREAM_BATCHES: u8 = 8;

/// Default zstd compression level used by [`pack`] for most chunk
/// payloads — high-card values, stream batches, timestamps, summary,
/// metadata. These payloads either carry random data (string columns,
/// KvId sequences) or are small enough that higher zstd levels don't
/// recoup their CPU cost.
pub const ZSTD_LEVEL_DEFAULT: i32 = 1;

/// Elevated zstd compression level for FST chunks (primary +
/// mid-card). FSTs share prefix structure across many `key=value`
/// strings; the higher level lets zstd's longer-range match search
/// find that redundancy and pay off the extra CPU with a noticeably
/// smaller payload.
pub const ZSTD_LEVEL_FST: i32 = 3;

/// Number of stream-batch (`SB{i}`) chunks in a file with `total_logs`
/// log entries. Both writer and reader call this; the rule is the
/// format invariant, not stored in the file.
pub fn num_stream_batches(total_logs: u32) -> u8 {
    (total_logs / MIN_LOGS_PER_BATCH).clamp(1, MAX_STREAM_BATCHES as u32) as u8
}

/// Logical batch size for a file with `total_logs` log entries. Used by
/// the writer to partition log positions into batches and by the reader
/// to decide which batch a given position belongs to.
///
/// Returns `1` for an empty file (`total_logs == 0`) — there are no
/// positions to partition, but a non-zero divisor lets callers compose
/// the result with integer division without a separate `total_logs == 0`
/// branch.
pub fn stream_batch_size(total_logs: u32) -> u32 {
    if total_logs == 0 {
        return 1;
    }
    total_logs.div_ceil(num_stream_batches(total_logs) as u32)
}

/// Chunk id for the mid-card field FST at `index`. The id encodes the
/// index in its trailing two bytes, big-endian, so each mid-card chunk
/// has a unique 4-byte id of the form `b"MF{hi}{lo}"`.
fn mid_field_id(index: u16) -> gix_chunk::Id {
    [b'M', b'F', (index >> 8) as u8, (index & 0xff) as u8]
}

/// Chunk id for the high-card field sorted list at `index`. Same shape
/// as [`mid_field_id`] but with prefix `b"HF"`.
fn high_field_id(index: u16) -> gix_chunk::Id {
    [b'H', b'F', (index >> 8) as u8, (index & 0xff) as u8]
}

/// Chunk id for the stream-batch chunk at `index` (0..[`MAX_STREAM_BATCHES`]).
/// Encodes the index as a single ASCII digit in the trailing byte, e.g.
/// `b"SB00"` through `b"SB07"`.
fn stream_batch_id(index: u8) -> gix_chunk::Id {
    [b'S', b'B', b'0', b'0' + index]
}

#[cfg(test)]
mod tests;
