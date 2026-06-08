//! Multi-file log-query subsystem over SFST indexes.
//!
//! The pipeline that turns a set of overlapping SFST files plus a
//! [`LogsQuery`] into a single [`LogsData`]: filter → facets / histogram
//! → pagination → row materialization. [`run`] is the all-in-one entry
//! point for the local case.
//!
//! The work splits into two steps. Step 1 (statistics — matched, facets,
//! histogram, fields) is an aggregatable monoid: [`LogsShard::evaluate`]
//! produces a [`LogsShard`] per file and [`LogsShard::merge`] folds them, so the
//! query can fan out across nodes and aggregate. Step 2 (row
//! materialization) needs a global order and lives in the pagination
//! path. [`run`] composes both.
//!
//! The API is neutral — plain Rust data in ([`LogsQuery`]), plain Rust
//! data out ([`LogsData`], built from `sfst` types). It carries no
//! transport or wire concerns; a consumer maps its own request format
//! onto [`LogsQuery`] and shapes [`LogsData`] into whatever its frontend
//! expects.
//!
//! The query itself is pure and synchronous; opening and decompressing
//! the SFST files is its only I/O. Resolving which files overlap a
//! request window, and scheduling the work off an async runtime thread,
//! is left to the caller.

mod aggregate;
mod cursor;
mod engine;
mod merge;
mod mmap;
mod page;
mod query;
mod result;
mod wal_scan;

pub use aggregate::LogsShard;
pub use cursor::Cursor;
pub use engine::{SfstCandidate, Source, WalTail, run};
pub use page::PageShard;
pub use query::{Anchor, Direction, LogsQuery, LogsQueryBuilder};
pub use result::LogsData;
pub use wal_scan::{WalScan, WalScanError};
