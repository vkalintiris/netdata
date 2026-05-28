mod config;
mod error;
mod format;
mod reader;
pub mod registry;
mod writer;

pub use config::{Config, RotationConfig};
pub use error::{Error, Result};
pub use format::{FileEvent, Message};
pub use reader::{Frame, Reader};
pub use registry::{File, Registry};
pub use writer::Writer;

/// Highest WAL sequence on disk across every tenant subdir of `base`.
/// Returns `0` when `base` is missing or empty. Used at process
/// startup to seed the seq counter so it stays monotonic across
/// restarts.
pub fn scan_max_sequence_recursive(base: &std::path::Path) -> std::io::Result<u64> {
    file_registry::scan_max_sequence_recursive(base, registry::WAL_EXT)
}
