use super::Facets;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::HashSet;
use journal::index::Filter;
use journal::repository::File;

/// A request to index files within a time range for specific fields.
///
/// This is a hermetic indexing request that knows nothing about histograms or buckets.
/// It simply requests: "index these files for this time range and these fields".
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub(crate) struct IndexRequest {
    /// Start time (inclusive) in seconds since epoch
    pub start: u32,

    /// End time (exclusive) in seconds since epoch
    pub end: u32,

    /// Fields to index (facets)
    pub facets: Facets,

    /// Filter expression to apply when counting
    pub filter: Filter,

    /// Files that need to be indexed
    pub files: HashSet<File>,
}

impl IndexRequest {
    /// Create a new index request
    pub(crate) fn new(start: u32, end: u32, facets: Facets, filter: Filter, files: HashSet<File>) -> Self {
        Self {
            start,
            end,
            facets,
            filter,
            files,
        }
    }
}
