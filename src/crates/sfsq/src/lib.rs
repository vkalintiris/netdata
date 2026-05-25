//! Single-file SFST query library.
//!
//! SOW-Q1 exposed the file summary; SOW-Q2 adds selection matchers
//! (field=value with within-field OR and across-field AND), backed by
//! the SFST's per-key bitmap inverted index.
//!
//! Subsequent SOWs add: position decode (Q3), time-range filtering
//! (Q4), regex/pagination (Q5), facet/histogram aggregation (Q6).

use std::path::Path;

use memmap2::Mmap;

pub mod matcher;

pub use matcher::{BitmapSet, ParseError as SelectionParseError, Resolution, Selection,
                  SelectionStats, Tier, parse_selections};

/// Errors that can occur opening or reading an SFST file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O failure opening or mapping the file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The underlying SFST container reported an error.
    #[error("sfst error: {0}")]
    Sfst(#[from] sfst::Error),
}

/// An opened single-file SFST query handle.
///
/// Owns the memory map and the eagerly-decoded summary. The mmap is
/// retained so each query reopens `sfst::Reader` / `log_index::IndexReader`
/// on the in-memory bytes without re-reading the file from disk.
pub struct Reader {
    mmap: Mmap,
    summary: sfst::FileSummary,
    chunk_count: u16,
}

impl Reader {
    /// Open an SFST file via mmap and decode its summary.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let file = std::fs::File::open(path)?;
        // SAFETY: required by memmap2. The mmap is owned by `Reader`
        // and lives as long as the struct; the kernel keeps the
        // mapping valid after `File` is dropped.
        let mmap = unsafe { Mmap::map(&file)? };
        let sfst_reader = sfst::Reader::open(&mmap)?;
        let summary = sfst_reader.summary()?;
        let chunk_count = sfst_reader.chunk_count();
        drop(sfst_reader);
        Ok(Self {
            mmap,
            summary,
            chunk_count,
        })
    }

    /// File-level summary (stream identity, time range, total logs).
    pub fn summary(&self) -> &sfst::FileSummary {
        &self.summary
    }

    /// Shortcut to `self.summary().stream`.
    pub fn stream(&self) -> &sfst::StreamEntry {
        &self.summary.stream
    }

    /// Number of secondary chunks (excludes SUMR/META/FLDS/PRIM).
    pub fn chunk_count(&self) -> u16 {
        self.chunk_count
    }

    /// Resolve a slice of [`Selection`]s against this file.
    ///
    /// Within a selection, values are OR'd; across selections, fields
    /// are AND'd. An empty `selections` list returns a full bitmap
    /// over the file's `total_logs` universe.
    pub fn select(&self, selections: &[Selection]) -> Result<Resolution, Error> {
        let index = log_index::reader::IndexReader::open(&self.mmap)?;
        let field_table = index.field_table()?;
        let resolution = matcher::resolve(
            &index,
            &field_table,
            selections,
            self.summary.total_logs,
        )?;
        Ok(resolution)
    }
}
