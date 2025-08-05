use journal::repository::File;

/// Time range information for a journal file, derived from its FileIndex.
///
/// This is an internal type used to cache time range metadata extracted from FileIndexes.
/// It is not exposed in the public API - use FileInfo accessor methods instead.
///
/// # Lifecycle
///
/// FileTimeRange progresses through these states:
/// 1. **Unknown**: File discovered but not yet indexed
/// 2. **Active/Bounded**: File indexed, time range known
///
/// The distinction between Active and Bounded comes from the File's status:
/// - Active files are still being written to (may grow)
/// - Bounded files are archived (immutable)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeRange {
    /// File has not been indexed yet, time range unknown. These files will
    /// be queued for indexing and reported in subsequent poll cycles.
    Unknown,

    /// Active file currently being written to. The end time represents the latest
    /// entry seen when the file was indexed, but new entries may have been
    /// written since. The indexed_at timestamp allows consumers to decide
    /// whether to re-index for fresher data.
    Active {
        start: u32,
        end: u32,
        indexed_at: u64,
    },

    /// Archived file with known start and end times.
    Bounded {
        start: u32,
        end: u32,
        indexed_at: u64,
    },
}

#[allow(dead_code)]
impl TimeRange {
    /// Creates a TimeRange from a FileIndex.
    ///
    /// Automatically determines whether to create Active or Bounded based on
    /// whether the file is currently active (being written to).
    pub(crate) fn from_file_index(file_index: &journal::index::FileIndex) -> Self {
        let (start, end) = file_index.histogram().time_range();
        let indexed_at = file_index.indexed_at();

        if file_index.file().is_active() {
            Self::Active {
                start,
                end,
                indexed_at,
            }
        } else {
            Self::Bounded {
                start,
                end,
                indexed_at,
            }
        }
    }

    /// Returns the time bounds (start, end) if known.
    pub(crate) fn time_bounds(&self) -> Option<(u32, u32)> {
        match self {
            Self::Unknown => None,
            Self::Active { start, end, .. } => Some((*start, *end)),
            Self::Bounded { start, end, .. } => Some((*start, *end)),
        }
    }

    /// Returns the indexed_at timestamp if the file has been indexed.
    pub(crate) fn indexed_at(&self) -> Option<u64> {
        match self {
            Self::Unknown => None,
            Self::Active { indexed_at, .. } => Some(*indexed_at),
            Self::Bounded { indexed_at, .. } => Some(*indexed_at),
        }
    }

    /// Returns true if this is an Active file.
    pub(crate) fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// Pairs a File with its cached time range metadata from the IndexingService.
///
/// This type is used internally to pass both a file's identity and its time range
/// together through the query pipeline. The time range represents the actual temporal
/// span of log entries in the file (in seconds since epoch).
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used internally for query pipeline
pub(crate) struct FileInfo {
    /// The journal file
    pub(crate) file: File,
    /// Cached time range from the FileIndex
    pub(crate) time_range: TimeRange,
}

#[allow(dead_code)]
impl FileInfo {
    /// Creates a new FileInfo from a file and time range.
    pub(crate) fn new(file: File, time_range: TimeRange) -> Self {
        Self { file, time_range }
    }

    /// Gets the time range as (start, end) in seconds since epoch.
    ///
    /// Returns None if the file has not been indexed yet.
    pub(crate) fn time_range(&self) -> Option<(u32, u32)> {
        match self.time_range {
            TimeRange::Unknown => None,
            TimeRange::Active { start, end, .. } => Some((start, end)),
            TimeRange::Bounded { start, end, .. } => Some((start, end)),
        }
    }

    /// Returns true if the file has been indexed (time range is known).
    pub(crate) fn is_indexed(&self) -> bool {
        !matches!(self.time_range, TimeRange::Unknown)
    }

    /// Returns true if the file is active (still being written to).
    ///
    /// Active files may need periodic re-indexing to capture new entries.
    /// Returns false for archived files or files that haven't been indexed yet.
    pub(crate) fn is_active(&self) -> bool {
        matches!(self.time_range, TimeRange::Active { .. })
    }

    /// Gets the timestamp (Unix seconds) when this file was indexed.
    ///
    /// Returns None if the file has not been indexed yet.
    pub(crate) fn indexed_at(&self) -> Option<u64> {
        match self.time_range {
            TimeRange::Unknown => None,
            TimeRange::Active { indexed_at, .. } => Some(indexed_at),
            TimeRange::Bounded { indexed_at, .. } => Some(indexed_at),
        }
    }

    /// Returns true if this active file's index is older than the given threshold.
    ///
    /// This is useful for determining when to re-index active files to get fresh data.
    /// Returns false for:
    /// - Non-active (archived/bounded) files (they're immutable)
    /// - Unknown files (not indexed yet)
    /// - Active files indexed more recently than the threshold
    ///
    /// # Arguments
    /// * `threshold_secs` - Age threshold in seconds
    pub(crate) fn is_stale(&self, threshold_secs: u64) -> bool {
        match self.time_range {
            TimeRange::Active { indexed_at, .. } => {
                use std::time::{SystemTime, UNIX_EPOCH};
                if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                    let now_secs = now.as_secs();
                    now_secs.saturating_sub(indexed_at) > threshold_secs
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}
