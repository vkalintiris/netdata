//! Metadata types for journal files
//!
//! This module provides metadata tracking for journal files, including time ranges
//! derived from indexing operations.

use super::File;

/// Time range information for a journal file derived from indexing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    /// File has not been indexed yet, time range unknown. These files will
    /// be queued for indexing and reported in subsequent poll cycles.
    Unknown,

    /// Active file currently being written to. The end time represents
    /// the latest entry seen when the file was indexed, but new entries
    /// may have been written since.
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

/// Pairs a File with its TimeRange.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// The journal file
    pub file: File,
    /// Time range from its file index
    pub time_range: TimeRange,
}
