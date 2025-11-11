//! Log querying from indexed journal files.
//!
//! This module provides the `LogQuery` builder for efficiently querying and
//! merging log entries from multiple indexed journal files.

use journal::index::{Direction, FieldName, FileIndex, Filter, LogEntry};

/// Builder for configuring and executing log queries from indexed journal files.
///
/// This builder allows you to specify:
/// - Direction (forward/backward in time)
/// - Anchor timestamp (starting point)
/// - Limit (maximum entries to retrieve)
/// - Source timestamp field (which field to use for timestamps)
/// - Filter (to match specific entries)
///
/// # Example
///
/// ```ignore
/// use journal::index::Direction;
/// use journal_function::logs::LogQuery;
///
/// let entries = LogQuery::new(&file_indexes)
///     .with_direction(Direction::Forward)
///     .with_anchor_usec(since_usec)
///     .with_limit(100)
///     .execute();
/// ```
pub struct LogQuery<'a> {
    file_indexes: &'a [FileIndex],
    direction: Direction,
    anchor_usec: Option<u64>,
    limit: Option<usize>,
    source_timestamp_field: Option<FieldName>,
    filter: Option<Filter>,
}

impl<'a> LogQuery<'a> {
    /// Create a new log query builder with default settings.
    ///
    /// Defaults:
    /// - Direction: Forward
    /// - Anchor: Computed from file indexes (min start time for forward, max end time for backward)
    /// - Limit: None (unlimited)
    /// - Source timestamp field: _SOURCE_REALTIME_TIMESTAMP
    /// - Filter: None
    pub fn new(file_indexes: &'a [FileIndex]) -> Self {
        Self {
            file_indexes,
            direction: Direction::Forward,
            anchor_usec: None,
            limit: None,
            source_timestamp_field: Some(FieldName::new_unchecked(
                "_SOURCE_REALTIME_TIMESTAMP",
            )),
            filter: None,
        }
    }

    /// Set the direction to iterate through log entries.
    ///
    /// - `Direction::Forward`: Retrieve entries from anchor forward in time (oldest to newest)
    /// - `Direction::Backward`: Retrieve entries from anchor backward in time (newest to oldest)
    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Set the anchor timestamp in microseconds.
    ///
    /// For forward direction, retrieves entries with timestamp >= anchor.
    /// For backward direction, retrieves entries with timestamp <= anchor.
    pub fn with_anchor_usec(mut self, anchor: u64) -> Self {
        self.anchor_usec = Some(anchor);
        self
    }

    /// Set the maximum number of log entries to retrieve.
    ///
    /// If not set (None), all matching entries will be retrieved.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the source timestamp field to use for entry timestamps.
    ///
    /// Pass `None` to use the entry's realtime timestamp from the journal header.
    /// Pass `Some(field_name)` to use a custom timestamp field from the entry data.
    pub fn with_source_timestamp_field(mut self, field: Option<FieldName>) -> Self {
        self.source_timestamp_field = field;
        self
    }

    /// Set a filter to apply to log entries.
    ///
    /// Only entries matching the filter will be included in the results.
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Compute the default anchor timestamp based on file indexes and direction.
    fn compute_default_anchor(&self) -> u64 {
        if self.file_indexes.is_empty() {
            return 0;
        }

        match self.direction {
            Direction::Forward => {
                // For forward: use minimum start time
                self.file_indexes
                    .iter()
                    .map(|fi| fi.histogram().start_time() as u64 * 1_000_000)
                    .min()
                    .unwrap_or(0)
            }
            Direction::Backward => {
                // For backward: use maximum end time
                self.file_indexes
                    .iter()
                    .map(|fi| fi.histogram().end_time() as u64 * 1_000_000)
                    .max()
                    .unwrap_or(0)
            }
        }
    }

    /// Execute the query and return log entries.
    ///
    /// This consumes the builder and returns a vector of log entries sorted by timestamp
    /// according to the configured direction.
    pub fn execute(self) -> Vec<LogEntry> {
        // Compute anchor if not explicitly set
        let anchor_usec = self.anchor_usec.unwrap_or_else(|| self.compute_default_anchor());

        // Convert limit to internal representation (usize::MAX for unlimited)
        let limit = self.limit.unwrap_or(usize::MAX);

        retrieve_log_entries(
            self.file_indexes.to_vec(),
            anchor_usec,
            self.direction,
            limit,
            self.source_timestamp_field,
            self.filter,
        )
    }
}

/// Retrieve and merge log entries from multiple indexed journal files.
///
/// This function efficiently retrieves log entries from multiple journal files,
/// merging them in timestamp order while respecting the limit constraint.
///
/// Note: Most users should use `LogQuery` builder instead of calling this directly.
///
/// # Arguments
///
/// * `file_indexes` - Vector of indexed journal files to retrieve from
/// * `anchor_usec` - Starting timestamp in microseconds
/// * `direction` - Direction to iterate (Forward or Backward)
/// * `limit` - Maximum number of log entries to retrieve
/// * `source_timestamp_field` - Optional field to use for timestamps (None uses realtime)
/// * `filter` - Optional filter to apply to entries
///
/// # Returns
///
/// A vector of log entries sorted by timestamp, limited to `limit` entries.
pub fn retrieve_log_entries(
    file_indexes: Vec<FileIndex>,
    anchor_usec: u64,
    direction: Direction,
    limit: usize,
    source_timestamp_field: Option<FieldName>,
    filter: Option<Filter>,
) -> Vec<LogEntry> {
    // Handle edge cases
    if limit == 0 || file_indexes.is_empty() {
        return Vec::new();
    }

    // Filter to FileIndex instances that could contain relevant entries
    let mut relevant_indexes: Vec<&FileIndex> = match direction {
        Direction::Forward => {
            // For forward: end timestamp must be at or after the anchor
            file_indexes
                .iter()
                .filter(|fi| fi.histogram().end_time() as u64 * 1_000_000 >= anchor_usec)
                .collect()
        }
        Direction::Backward => {
            // For backward: start timestamp must be at or before the anchor
            file_indexes
                .iter()
                .filter(|fi| fi.histogram().start_time() as u64 * 1_000_000 <= anchor_usec)
                .collect()
        }
    };

    if relevant_indexes.is_empty() {
        return Vec::new();
    }

    // Sort files to process them in temporal order
    match direction {
        Direction::Forward => {
            // Sort by start timestamp ascending to process files in temporal order
            relevant_indexes.sort_by_key(|fi| fi.histogram().start_time());
        }
        Direction::Backward => {
            // Sort by end timestamp descending to process files in reverse temporal order
            relevant_indexes.sort_by_key(|fi| std::cmp::Reverse(fi.histogram().end_time()));
        }
    }

    // Initialize result vector with capacity for efficiency
    let mut collected_entries: Vec<LogEntry> = Vec::with_capacity(limit);

    for file_index in relevant_indexes {
        // Pruning optimization: if we have a full result set, check if we can skip
        // remaining files based on their time ranges
        if collected_entries.len() >= limit {
            if let Some(should_break) = can_prune_file(file_index, &collected_entries, direction) {
                if should_break {
                    break;
                }
            }
        }

        // Perform I/O to retrieve entries from this FileIndex
        let file = file_index.file();
        let new_entries = match file_index.retrieve_sorted_entries(
            file,
            source_timestamp_field.as_ref(),
            filter.as_ref(),
            anchor_usec,
            direction,
            limit,
        ) {
            Ok(entries) => entries,
            Err(_) => continue, // Skip files that fail to read
        };

        if new_entries.is_empty() {
            continue;
        }

        // Merge the new entries with our existing results, maintaining
        // sorted order and respecting the limit constraint
        collected_entries = merge_log_entries(collected_entries, new_entries, limit, direction);
    }

    collected_entries
}

/// Check if we can prune (skip) a file based on its time range and current results.
///
/// Returns Some(true) if we should break early, Some(false) if we should continue,
/// or None if we can't determine (shouldn't happen with a full result set).
fn can_prune_file(
    file_index: &FileIndex,
    result: &[LogEntry],
    direction: Direction,
) -> Option<bool> {
    match direction {
        Direction::Forward => {
            // For forward: if file starts after our latest entry, skip all remaining files
            let max_timestamp = result.last()?.timestamp;
            Some(file_index.histogram().start_time() as u64 * 1_000_000 > max_timestamp)
        }
        Direction::Backward => {
            // For backward: if file ends before our earliest entry, skip all remaining files
            let min_timestamp = result.first()?.timestamp;
            Some(file_index.histogram().end_time() as u64 * 1_000_000 < min_timestamp)
        }
    }
}

/// Merges two sorted vectors into a single sorted vector with at most `limit` elements.
///
/// This function performs a two-pointer merge, which is efficient for combining
/// sorted sequences. It only retains the smallest/largest `limit` entries by timestamp
/// depending on the direction.
///
/// # Arguments
///
/// * `a` - First sorted vector
/// * `b` - Second sorted vector
/// * `limit` - Maximum number of elements in the result
/// * `direction` - Direction determines ascending (Forward) or descending (Backward) order
///
/// # Returns
///
/// A new vector containing the merged and limited results
fn merge_log_entries(
    a: Vec<LogEntry>,
    b: Vec<LogEntry>,
    limit: usize,
    direction: Direction,
) -> Vec<LogEntry> {
    // Handle simple cases
    if a.is_empty() {
        return b.into_iter().take(limit).collect();
    }
    if b.is_empty() {
        return a.into_iter().take(limit).collect();
    }

    // Allocate result vector with appropriate capacity
    let mut result = Vec::with_capacity(limit);
    let mut i = 0;
    let mut j = 0;

    // Two-pointer merge: always take the appropriate element based on direction
    while result.len() < limit {
        let take_from_a = match (i < a.len(), j < b.len()) {
            (true, false) => true,
            (false, true) => false,
            (false, false) => break,
            (true, true) => match direction {
                Direction::Forward => a[i].timestamp <= b[j].timestamp,
                Direction::Backward => a[i].timestamp >= b[j].timestamp,
            },
        };

        if take_from_a {
            result.push(a[i].clone());
            i += 1;
        } else {
            result.push(b[j].clone());
            j += 1;
        }
    }

    result
}
