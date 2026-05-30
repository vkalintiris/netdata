//! Multi-file log-query subsystem over SFST indexes.
//!
//! The pipeline that turns a set of overlapping SFST files plus a
//! [`LogsQuery`] into a single [`LogsData`]: filter → facets / histogram
//! → pagination → row materialization. [`run`] is the entry point.
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

mod cursor;
mod engine;
mod merge;
mod query;
mod result;

pub use cursor::Cursor;
pub use engine::{PreparedQuery, SfstCandidate, run};
pub use query::{Anchor, Direction, LogsQuery};
pub use result::LogsData;
