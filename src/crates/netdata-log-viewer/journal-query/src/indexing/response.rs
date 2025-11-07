#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::{HashMap, HashSet};
use journal::repository::File;
use journal::{FieldName, FieldValuePair};

/// Progress report for an index request.
///
/// Contains the indexed data so far, along with information about what's
/// still pending. This is a hermetic type that knows nothing about buckets.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub(crate) struct IndexProgress {
    /// Files that still need to be indexed
    pub pending_files: HashSet<File>,

    /// Field=value counts: (unfiltered_count, filtered_count)
    ///
    /// The unfiltered count is the total occurrences of this field=value.
    /// The filtered count is how many match the filter expression.
    pub fv_counts: HashMap<FieldValuePair, (usize, usize)>,

    /// Fields that exist in the files but were not indexed
    /// (e.g., because they weren't in the facets list or indexing was skipped)
    pub unindexed_fields: HashSet<FieldName>,
}

impl IndexProgress {
    /// Create a new empty index progress
    pub(crate) fn new() -> Self {
        Self {
            pending_files: HashSet::default(),
            fv_counts: HashMap::default(),
            unindexed_fields: HashSet::default(),
        }
    }
}

impl Default for IndexProgress {
    fn default() -> Self {
        Self::new()
    }
}
