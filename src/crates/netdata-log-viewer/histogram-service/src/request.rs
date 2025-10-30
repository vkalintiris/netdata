use journal::collections::HashSet;
use journal::index::FilterExpr;
use journal::repository::File;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use std::sync::Arc;
use std::time::Duration;

/// A bucket request contains a [start, end) time range along with the
/// filter that should be applied.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct BucketRequest {
    // Start time of the bucket request
    pub start: u64,
    // End time of the bucket request
    pub end: u64,
    // Applied filter expression
    pub filter_expr: Arc<FilterExpr<String>>,
}

impl BucketRequest {
    /// The duration of the bucket request in seconds
    pub fn duration(&self) -> u64 {
        self.end - self.start
    }
}

/// A histogram request for a given [start, end) time range with a specific
/// filter expression that should be matched.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct HistogramRequest {
    /// Start time
    pub after: u64,
    /// End time
    pub before: u64,
    /// Filter expression to apply
    pub filter_expr: Arc<FilterExpr<String>>,
}

impl HistogramRequest {
    /// Returns the bucket requests that should be used in order to
    /// generate data for this histogram. The bucket duration is automatically
    /// determined by time range of the histogram request, and it's large
    /// enough to return at least 100 bucket requests.
    pub fn bucket_requests(&self) -> Vec<BucketRequest> {
        let bucket_duration = self.calculate_bucket_duration();

        // Buckets are aligned to their duration
        let aligned_start = (self.after / bucket_duration) * bucket_duration;
        let aligned_end = self.before.div_ceil(bucket_duration) * bucket_duration;

        // Allocate our buckets
        let num_buckets = ((aligned_end - aligned_start) / bucket_duration) as usize;
        let mut buckets = Vec::with_capacity(num_buckets);

        // Create our buckets
        for bucket_index in 0..num_buckets {
            let start = aligned_start + (bucket_index as u64 * bucket_duration);

            buckets.push(BucketRequest {
                start,
                end: start + bucket_duration,
                filter_expr: self.filter_expr.clone(),
            });
        }

        buckets
    }

    fn calculate_bucket_duration(&self) -> u64 {
        const MINUTE: Duration = Duration::from_secs(60);
        const HOUR: Duration = Duration::from_secs(60 * MINUTE.as_secs());
        const DAY: Duration = Duration::from_secs(24 * HOUR.as_secs());

        const VALID_DURATIONS: &[Duration] = &[
            // Seconds
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(15),
            Duration::from_secs(30),
            // Minutes
            MINUTE,
            Duration::from_secs(2 * MINUTE.as_secs()),
            Duration::from_secs(3 * MINUTE.as_secs()),
            Duration::from_secs(5 * MINUTE.as_secs()),
            Duration::from_secs(10 * MINUTE.as_secs()),
            Duration::from_secs(15 * MINUTE.as_secs()),
            Duration::from_secs(30 * MINUTE.as_secs()),
            // Hours
            HOUR,
            Duration::from_secs(2 * HOUR.as_secs()),
            Duration::from_secs(6 * HOUR.as_secs()),
            Duration::from_secs(8 * HOUR.as_secs()),
            Duration::from_secs(12 * HOUR.as_secs()),
            // Days
            DAY,
            Duration::from_secs(2 * DAY.as_secs()),
            Duration::from_secs(3 * DAY.as_secs()),
            Duration::from_secs(5 * DAY.as_secs()),
            Duration::from_secs(7 * DAY.as_secs()),
            Duration::from_secs(14 * DAY.as_secs()),
            Duration::from_secs(30 * DAY.as_secs()),
        ];

        let duration = self.before - self.after;

        VALID_DURATIONS
            .iter()
            .rev()
            .find(|&&bucket_width| duration / bucket_width.as_secs() >= 100)
            .map(|d| d.as_secs())
            .unwrap_or(1)
    }
}

/// Contains the original bucket request along with the set of files
/// that will be used for providing the bucket response.
///
/// We can identify partial vs. complete responses for a bucket request
/// by checking if there are are any files that have not been processed
/// yet.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct RequestMetadata {
    // The original request
    pub request: BucketRequest,

    // Files we need to use to generate a full response
    pub files: HashSet<File>,
}
