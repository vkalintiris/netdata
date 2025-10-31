#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::HashSet;
use journal::index::FilterExpr;
use journal::repository::File;
use std::hash::Hash;
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
    // Facets to use for file index
    pub facets: HistogramFacets,
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
    /// Facets to use for file indexes
    pub facets: HistogramFacets,
    /// Filter expression to apply
    pub filter_expr: Arc<FilterExpr<String>>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct HistogramFacets {
    fields: Arc<Vec<String>>,
    precomputed_hash: u64,
}

impl Hash for HistogramFacets {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.precomputed_hash);
    }
}

impl PartialEq for HistogramFacets {
    fn eq(&self, other: &Self) -> bool {
        if self.precomputed_hash != other.precomputed_hash {
            return false;
        }

        // NOTE: Maybe a panic is warranted here.
        Arc::ptr_eq(&self.fields, &other.fields) || self.fields == other.fields
    }
}

impl Eq for HistogramFacets {}

impl HistogramFacets {
    fn default_facets() -> Vec<String> {
        let v: Vec<String> = vec![
            String::from("_HOSTNAME"),
            String::from("PRIORITY"),
            String::from("SYSLOG_FACILITY"),
            String::from("ERRNO"),
            String::from("SYSLOG_IDENTIFIER"),
            // b"UNIT",
            String::from("USER_UNIT"),
            String::from("MESSAGE_ID"),
            String::from("_BOOT_ID"),
            String::from("_SYSTEMD_OWNER_UID"),
            String::from("_UID"),
            String::from("OBJECT_SYSTEMD_OWNER_UID"),
            String::from("OBJECT_UID"),
            String::from("_GID"),
            String::from("OBJECT_GID"),
            String::from("_CAP_EFFECTIVE"),
            String::from("_AUDIT_LOGINUID"),
            String::from("OBJECT_AUDIT_LOGINUID"),
            String::from("CODE_FUNC"),
            String::from("ND_LOG_SOURCE"),
            String::from("CODE_FILE"),
            String::from("ND_ALERT_NAME"),
            String::from("ND_ALERT_CLASS"),
            String::from("_SELINUX_CONTEXT"),
            String::from("_MACHINE_ID"),
            String::from("ND_ALERT_TYPE"),
            String::from("_SYSTEMD_SLICE"),
            String::from("_EXE"),
            // b"_SYSTEMD_UNIT",
            String::from("_NAMESPACE"),
            String::from("_TRANSPORT"),
            String::from("_RUNTIME_SCOPE"),
            String::from("_STREAM_ID"),
            String::from("ND_NIDL_CONTEXT"),
            String::from("ND_ALERT_STATUS"),
            // b"_SYSTEMD_CGROUP",
            String::from("ND_NIDL_NODE"),
            String::from("ND_ALERT_COMPONENT"),
            String::from("_COMM"),
            String::from("_SYSTEMD_USER_UNIT"),
            String::from("_SYSTEMD_USER_SLICE"),
            // b"_SYSTEMD_SESSION",
            String::from("__logs_sources"),
        ];

        v
    }

    pub fn new(facets: &[String]) -> Self {
        let mut facets = if facets.is_empty() {
            Self::default_facets()
        } else {
            facets.to_vec()
        };

        facets.sort();

        use std::hash::Hasher;
        let mut hasher = std::hash::DefaultHasher::new();
        facets.hash(&mut hasher);
        let precomputed_hash = hasher.finish();

        Self {
            fields: Arc::new(facets),
            precomputed_hash,
        }
    }

    /// Returns an iterator over the facet field names
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.fields.iter()
    }

    /// Returns the facet fields as a slice
    pub fn as_slice(&self) -> &[String] {
        &self.fields
    }

    /// Returns the number of facet fields
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Returns true if there are no facet fields
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

impl HistogramRequest {
    pub fn new(
        after: u64,
        before: u64,
        facets: &[String],
        filter_expr: &FilterExpr<String>,
    ) -> Self {
        Self {
            after,
            before,
            facets: HistogramFacets::new(facets),
            filter_expr: Arc::new(filter_expr.clone()),
        }
    }

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
                facets: self.facets.clone(),
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
