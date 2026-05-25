//! Single-file SFST query library.
//!
//! SOW-Q1 exposes the file summary (stream identity, time range,
//! total logs, secondary chunk count). Selection matchers, decode,
//! time-range filtering, regex/pagination, and facet/histogram
//! aggregation land in SOW-Q2 through SOW-Q6.

use std::path::Path;

use memmap2::Mmap;

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
/// retained so later SOWs can re-open `sfst::Reader` for chunk access
/// without re-reading the file from disk.
pub struct Reader {
    // Kept alive so future SOWs can re-borrow it for chunk lookups.
    #[allow(dead_code)]
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
}
