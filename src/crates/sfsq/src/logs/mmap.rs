//! The logs engine's view of the shared source plumbing
//! ([`crate::source`]): same `Mapped`/`release_cold_region`, plus a
//! mapping that logs its failure and returns `None` — the caller leaves
//! that source out and counts it, so one bad source never sinks a query.
//! (The traces engine consumes the structured error instead.)

pub(super) use crate::source::{Mapped, release_cold_region};

use crate::source::Source;

/// Obtain a candidate's bytes from its [`Source`], logging and returning
/// `None` on failure.
pub(super) fn map_source(source: &Source) -> Option<Mapped> {
    match crate::source::map_source(source) {
        Ok(mapped) => Some(mapped),
        Err(e) => {
            tracing::warn!("sfsq: failed to {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The logs contract this wrapper preserves: a mapping failure
    /// degrades to `None` (logged), it does not error.
    #[test]
    fn failure_degrades_to_none() {
        let missing = Source::File(std::path::PathBuf::from("/nonexistent/sfsq-logs-mmap-test"));
        assert!(map_source(&missing).is_none());
    }
}
