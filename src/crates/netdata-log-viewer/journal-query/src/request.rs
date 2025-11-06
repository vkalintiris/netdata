#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::HashSet;
use journal::index::Filter;
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
    pub start: u32,
    // End time of the bucket request
    pub end: u32,
    // Facets to use for file index
    pub facets: HistogramFacets,
    // Applied filter expression
    pub filter_expr: Filter,
}

impl BucketRequest {
    /// The duration of the bucket request in seconds
    pub fn duration(&self) -> u32 {
        self.end - self.start
    }

    /// Returns the next bucket request with the same duration, facets, and filter.
    /// The next bucket starts where this bucket ends.
    pub fn next(&self) -> Self {
        let duration = self.duration();
        Self {
            start: self.end,
            end: self.end + duration,
            facets: self.facets.clone(),
            filter_expr: self.filter_expr.clone(),
        }
    }

    /// Returns the previous bucket request with the same duration, facets, and filter.
    /// The previous bucket ends where this bucket starts.
    pub fn prev(&self) -> Self {
        let duration = self.duration();
        Self {
            start: self.start.saturating_sub(duration),
            end: self.start,
            facets: self.facets.clone(),
            filter_expr: self.filter_expr.clone(),
        }
    }
}

/// A histogram request for a given [start, end) time range with a specific
/// filter expression that should be matched.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct HistogramRequest {
    /// Start time
    pub after: u32,
    /// End time
    pub before: u32,
    /// Facets to use for file indexes
    pub facets: HistogramFacets,
    /// Filter expression to apply
    pub filter_expr: Filter,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct HistogramFacets {
    fields: Arc<Vec<journal::FieldName>>,
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
    fn default_facets() -> Vec<journal::FieldName> {
        let v: Vec<&str> = vec![
            "_HOSTNAME",
            "PRIORITY",
            "SYSLOG_FACILITY",
            "ERRNO",
            "SYSLOG_IDENTIFIER",
            // "UNIT",
            "USER_UNIT",
            "MESSAGE_ID",
            "_BOOT_ID",
            "_SYSTEMD_OWNER_UID",
            "_UID",
            "OBJECT_SYSTEMD_OWNER_UID",
            "OBJECT_UID",
            "_GID",
            "OBJECT_GID",
            "_CAP_EFFECTIVE",
            "_AUDIT_LOGINUID",
            "OBJECT_AUDIT_LOGINUID",
            "CODE_FUNC",
            "ND_LOG_SOURCE",
            "CODE_FILE",
            "ND_ALERT_NAME",
            "ND_ALERT_CLASS",
            "_SELINUX_CONTEXT",
            "_MACHINE_ID",
            "ND_ALERT_TYPE",
            "_SYSTEMD_SLICE",
            "_EXE",
            // "_SYSTEMD_UNIT",
            "_NAMESPACE",
            "_TRANSPORT",
            "_RUNTIME_SCOPE",
            "_STREAM_ID",
            "ND_NIDL_CONTEXT",
            "ND_ALERT_STATUS",
            // "_SYSTEMD_CGROUP",
            "ND_NIDL_NODE",
            "ND_ALERT_COMPONENT",
            "_COMM",
            "_SYSTEMD_USER_UNIT",
            "_SYSTEMD_USER_SLICE",
            // "_SYSTEMD_SESSION",
            "__logs_sources",
        ];

        // Convert to FieldName - use new_unchecked since these are trusted constants
        v.into_iter()
            .map(journal::FieldName::new_unchecked)
            .collect()
    }

    pub fn new(facets: &[String]) -> Self {
        let mut facets = if facets.is_empty() {
            Self::default_facets()
        } else {
            // Parse and validate each facet string into FieldName
            facets
                .iter()
                .filter_map(|s| journal::FieldName::new(s.clone()))
                .collect()
        };

        facets.sort();

        use std::hash::Hasher;
        let mut hasher = std::hash::DefaultHasher::new();
        // Hash the string representation for consistency
        for field in &facets {
            field.as_str().hash(&mut hasher);
        }
        let precomputed_hash = hasher.finish();

        Self {
            fields: Arc::new(facets),
            precomputed_hash,
        }
    }

    /// Returns an iterator over the facet field names
    pub fn iter(&self) -> impl Iterator<Item = &journal::FieldName> {
        self.fields.iter()
    }

    /// Returns the facet fields as a slice
    pub fn as_slice(&self) -> &[journal::FieldName] {
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
    pub fn new(after: u32, before: u32, facets: &[String], filter_expr: &Filter) -> Self {
        Self {
            after,
            before,
            facets: HistogramFacets::new(facets),
            filter_expr: filter_expr.clone(),
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
        assert!(
            num_buckets > 0,
            "histogram requests should always have at least one bucket"
        );

        // Create our buckets
        for bucket_index in 0..num_buckets {
            let start = aligned_start + (bucket_index as u32 * bucket_duration);

            buckets.push(BucketRequest {
                start,
                end: start + bucket_duration,
                facets: self.facets.clone(),
                filter_expr: self.filter_expr.clone(),
            });
        }

        buckets
    }

    fn calculate_bucket_duration(&self) -> u32 {
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
            .find(|&&bucket_width| duration as u64 / bucket_width.as_secs() >= 100)
            .map(|d| d.as_secs())
            .unwrap_or(1) as u32
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
