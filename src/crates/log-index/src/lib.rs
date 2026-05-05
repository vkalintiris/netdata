pub mod arrow_columns;
pub mod bitmap_convert;
pub mod bitset;
pub mod fst_builder;
mod indexer;
pub mod kv_interner;
mod otap_frame;
mod process_frame;
pub mod reader;
pub mod wal_index;

pub use fst_builder::{IndexMetadata, build_and_write};
pub use indexer::{IndexResult, index_wal_file};
pub use kv_interner::KeyValueId;
// Canonical stream identifier lives in `file_registry`; the file format
// is owned by `sfst`. Re-export both for caller convenience.
pub use file_registry::StreamEntry;
pub use sfst::FileSummary;
