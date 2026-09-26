//! The shared query time window — one definition of the file-granular
//! pruning the explorer and its value suggestions both apply, so the
//! half-open-nanosecond comparison can never fork between them.

/// A half-open `[start_ns, end_ns)` nanosecond window.
/// Construction validates `start_ns < end_ns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    start_ns: i64,
    end_ns: i64,
}

/// An invalid window is a request error on whichever operation received
/// it; the per-operation error types wrap this via `From`.
#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[error("invalid time window [{start_ns}, {end_ns}): start must be before end")]
    Invalid { start_ns: i64, end_ns: i64 },
}

impl TimeWindow {
    pub fn new(start_ns: i64, end_ns: i64) -> Result<Self, WindowError> {
        if start_ns >= end_ns {
            return Err(WindowError::Invalid { start_ns, end_ns });
        }
        Ok(Self { start_ns, end_ns })
    }

    /// Whether a summary's inclusive-seconds `[min_s, max_s]` range
    /// overlaps this window: the file range expands to nanoseconds as
    /// `[min_s·10⁹, (max_s+1)·10⁹)` (saturating) and the two half-open
    /// ranges intersect iff each starts before the other ends. THE
    /// file-pruning comparison.
    pub(crate) fn overlaps_summary(&self, min_s: u32, max_s: u32) -> bool {
        const NS: i64 = 1_000_000_000;
        let file_start = i64::from(min_s).saturating_mul(NS);
        let file_end = (i64::from(max_s) + 1).saturating_mul(NS);
        self.start_ns < file_end && file_start < self.end_ns
    }
}
