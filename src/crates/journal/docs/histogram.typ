#set document(title: "My Rust Library Documentation")
#set page(numbering: "1")
#set align(left)
#set par(justify: true)

= Journal file format

We have implemented `systemd`'s journal file format in Rust. The journal
file format is used to store log entries. Each entry is a collection of
`key=value` pairs. `key`s are called fields and they are deduplicated at
the file-level. `key=value` pairs are also deduplicated at the file level.
In this context, _deduplicated_ means that the objects that represent fields
and `key=value` pairs, are written exactly once in the journal file. The
journal file format provides two global hash tables, one for fields and
another for `key=value` pairs. Also, it provides a global array that contains
the file offsets of each log entry.

`key`s, ie. fields, are linked to the `key=value` pairs. That is, given a
field, we can iterate all the values it takes. `key=value` pairs are linked
to the entries they appear. That is, given a `key=value` pair, we can iterate
all the entries where it appears.

Our implementation allows us to create new file object types and use
collections - hash tables, arrays and linked lists - easily.

== Systemd (in)compatibility

`systemd` enforces several limits on the values that fields can take.

= Journal files indexing

We have written a library in Rust that is able to read and write systemd's
journal file format. On top of this library, we've developed types that allow
someone to index log directories that contain multiple journal files. This
will allow users to query the logs contained in journal files.

For the time being, we are primarily interested in allowing users to specify
a time range and create histograms for log entry fields.

Our code provides the `FileIndex` type that represents an index of a single
journal file:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileIndex {
    // The journal file's histogram
    pub histogram: Histogram,

    // Set of fields in the file
    pub fields: HashSet<String>,

    // Bitmap for each indexed field
    pub bitmaps: HashMap<String, Bitmap>,
}
```

The `FileIndex` contains all the fields that appear in a journal file. Also,
it keeps a sparse histogram which simply contains the duration of each bucket
and a vector of buckets:

```rust
/// An index structure for efficiently generating time-based histograms from journal entries.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Histogram {
    /// The duration of each bucket
    bucket_duration: u64,
    /// Sparse vector containing only bucket boundaries where changes occur.
    buckets: Vec<Bucket>,
}
```

Each bucket contains it's start time and the _running_ count of entries:

```rust
/// A time-aligned bucket in the file histogram.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Bucket {
    /// Start time of this bucket
    pub start_time: u32,
    /// Count of items in this bucket
    pub count: u32,
}
```

The choice to keep the _running_ count in buckets allows us to use a compact
representation of the entries in which a specific `field=value` appears in
the file. We use the `roaring` crate and provide the `Bitmap` _newtype_ on
top of the `RoaringBitmap` type.

Given the `Histogram` of a specific journal file, we can quickly generate the
histogram of a specific `field=value` by getting the `Bitmap` from the
`bitmaps` field. We do this by iterating each bucket of the `Histogram` and
iterating each set bit of the `Bitmap` in a nested loop. Whenever the set bit
in the `Bitmap` is larger than the _running_ count of the histogram bucket,
we create a new _bucket_ for the histogram of the `field=value` pair.

The chosen representation allows us to generate not only histograms of a
single `key=value`, but also a combination of them. For example, to generate
the histogram of `key1=value1 AND key2=value2`, we can simply perform the
intersection of the `key1=value1` bitmap with the bitmap of `key2=value2`. We
can then use the new `bitmap` to generate a histogram of log entries where
both `key1=value1` and `key2=value2` appears over time.

It should be noted that:

- A `Histogram`'s buckets are aligned to its bucket duration.
- Given a `Histogram` with a smaller bucket duration, we can generate a new
  one with a bucket duration that is a multiple of the original one.

The last point means that we can generate, for example, minute-aligned
histograms from a histogram that contains per-second aligned buckets.

= Histogram Request Workflow

When a user requests a histogram over a time range, the system follows a
multi-stage workflow to efficiently compute and cache the results.

== Request Types

The workflow involves several key types that work together:

=== HistogramRequest

A `HistogramRequest` represents the user's query:

```rust
pub struct HistogramRequest {
    /// Start time
    pub after: u64,
    /// End time
    pub before: u64,
    /// Filter expression to apply
    pub filter_expr: Arc<FilterExpr<String>>,
}
```

The `HistogramRequest` is automatically broken down into a vector of
`BucketRequest`s. The bucket duration is calculated dynamically to produce at
least 100 buckets based on the time range. The system selects from predefined
durations (1s, 2s, 5s, 10s, 15s, 30s, 1m, 2m, 3m, 5m, 10m, 15m, 30m, 1h, 2h,
6h, 8h, 12h, 1d, 2d, 3d, 5d, 7d, 14d, 30d).

=== BucketRequest

Each `BucketRequest` represents a single time bucket:

```rust
pub struct BucketRequest {
    pub start: u64,        // Bucket start time (aligned to duration)
    pub end: u64,          // Bucket end time
    pub filter_expr: Arc<FilterExpr<String>>,
}
```

Buckets are aligned to their duration. For example, with a 1-hour bucket
duration, buckets start at times like 00:00:00, 01:00:00, 02:00:00, etc.

=== RequestMetadata

For each `BucketRequest`, we create `RequestMetadata` that tracks which journal
files need to be processed:

```rust
pub struct RequestMetadata {
    pub request: BucketRequest,
    pub files: VecDeque<File>,  // Files overlapping this bucket's time range
}
```

The `files` queue is populated by querying the registry for files that overlap
the bucket's time range.

=== Response Types

The system maintains two types of responses:

```rust
pub struct BucketPartialResponse {
    pub request_metadata: RequestMetadata,
    pub indexed_fields: FxHashMap<String, (usize, usize)>,  // (unfiltered, filtered) counts
    pub unindexed_fields: FxHashSet<String>,
}

pub struct BucketCompleteResponse {
    pub indexed_fields: FxHashMap<String, (usize, usize)>,
    pub unindexed_fields: FxHashSet<String>,
}
```

A `BucketPartialResponse` has pending files (in `request_metadata.files`),
while a `BucketCompleteResponse` has no pending files and is ready to use.

== Response Caching and Deduplication

The system maintains two caches for bucket responses:

```rust
pub struct AppState {
    pub partial_responses: FxHashMap<BucketRequest, BucketPartialResponse>,
    pub complete_responses: FxHashMap<BucketRequest, BucketCompleteResponse>,
}
```

`BucketRequest` serves as the cache key. Since it implements `Eq`, `PartialEq`,
and `Hash`, the system can deduplicate requests based on:
- Bucket start and end times (aligned to duration)
- Filter expression

This means:
- Multiple `HistogramRequest`s with overlapping time ranges share cached bucket
  responses
- Repeated queries for the same time range return immediately from cache
- Different filter expressions create separate cache entries (no sharing)

Before creating a new partial response, the system checks both caches:
```rust
if self.complete_responses.contains_key(bucket_request) {
    continue;  // Already complete
}
if self.partial_responses.contains_key(bucket_request) {
    continue;  // Already in progress
}
```

This deduplication provides significant efficiency gains. When users issue
multiple histogram requests with overlapping time ranges, the system
automatically shares cached bucket responses, avoiding redundant file indexing.
This is particularly valuable in interactive scenarios like panning or zooming
in a UI, where successive queries often request overlapping time ranges. As
background indexing progresses, the work benefits all concurrent requests that
need the same buckets, rather than duplicating effort for each request
separately.

== Workflow

When a `HistogramRequest` arrives, the system first calculates an appropriate
bucket duration to produce at least 100 buckets covering the requested time
range. It then generates time-aligned `BucketRequest`s and immediately consults
the response caches. For each bucket, if a `BucketCompleteResponse` exists in
the cache, that response is used directly. If a `BucketPartialResponse` exists,
the system knows indexing is already in progress for that bucket. Only for
buckets missing from both caches does the system create new
`BucketPartialResponse` entries, querying the file registry to determine which
journal files overlap each bucket's time range.

Once the system identifies which buckets need work, it examines all partial
responses to determine what files need indexing. For each unique file required
across all partial responses, the system computes the _minimum_ bucket duration
needed. This ensures files are indexed with sufficient granularity to satisfy
all pending requests. The system then sends `FileIndexRequest`s to a background
thread pool that uses Rayon for parallel indexing:

```rust
pub struct FileIndexRequest {
    pub file: File,
    pub bucket_duration: u64,  // Minimum granularity needed
}
```

As background indexing completes, the system attempts to resolve partial
responses using the newly cached file indexes. Before using a cached file
index, the system performs a granularity check by comparing the cached
histogram's bucket duration with what the bucket request needs:

```rust
if file_index.histogram().bucket_duration > requested_duration {
    // Cached index is too coarse, skip it
    continue;
}
```

A file indexed with `bucket_duration = 60` seconds can satisfy requests for
`bucket_duration = 3600` seconds (1 hour), but not vice versa. If the cached
index lacks sufficient granularity, the file remains in the pending queue and
will be re-indexed later with a finer bucket duration.

When a cached file index meets the granularity requirement, the system removes
that file from the partial response's pending queue and processes it. The
filter expression is evaluated against the file index's bitmaps to identify
matching entries. The system then counts entries within the bucket's time range
(tracking both filtered and unfiltered counts) and accumulates these in the
partial response's `indexed_fields` map.

As each partial response's pending file queue empties, the system promotes it
to a `BucketCompleteResponse` and moves it from the partial responses cache to
the complete responses cache. On subsequent requests, the `get_histogram`
method can return these complete responses immediately without any indexing
work.

This architecture enables progressive refinement. Initial histogram requests may
return partial responses showing approximate data based on whatever files have
been indexed so far. As background indexing progresses, these responses
automatically become more complete. Users querying the same buckets again see
updated results that incorporate newly indexed files. When users request finer
time granularity (like zooming in from hourly to minute-level buckets), the
system recognizes that the cached file indexes are too coarse and triggers
re-indexing with smaller bucket durations. The deduplication mechanism ensures
that overlapping queries share this indexing work, making interactive
exploration efficient.

= Things to implement

== Index state cache

Change the index state cache to use the `foyer` crate instead of keeping
all file indexes in memory with a `HashMap`. We also need to change the
way we are indexing files in parallel. The idea is to drop rayon, use a
bounded channel, and `try_send` indexing requests annotated with the time
they were generated at. If indexing requests contain the request time, I can
drop them if they timeout, ie. if they take more than `X` seconds to receive.

I still need to think if there's a way to service newer indexing requests,
instead of older ones. One idea is to use `try_recv` in order to batch `Y`
requests at a time from a single thread. This means that I'll have a queue
of requests in the thread and I can decide which ones to process first.
However, I need to check thread fairness. In other words, I want to avoid
the scenario where a single thread receives many requests and leaves the
rest of the threads idle.

== Indexing metrics

/ Throughput & Completion: Tracking how much work is being completed and at
  what rate.
  - _Request throughput_: Completed requests per second, broken down by
    outcome (successful vs timed out). This is the primary indicator of
    system capacity and workload.
  - _Timeout rate_: Percentage or absolute count of requests that exceed
    the timeout threshold. High timeout rates indicate the system is
    overwhelmed or individual files are too large/complex.

/ Queueing & Backpressure: Understanding how work accumulates before processing.
  - _Channel queue depth_: Current number of pending requests waiting in the
    crossbeam channel. Sustained high values indicate backpressure - threads
    can't keep up with incoming work.
  - _Request queue time_: Time each request spends waiting in the channel
    before a thread picks it up. Helps distinguish between queueing delays
    versus actual processing time.

/ Latency & Performance: Measuring how long individual requests take to process.
  - _Processing latency (p50, p99)_: Wall-clock time to complete successful
    indexing requests. Track both median (typical case) and p99 (tail latency)
    to catch performance degradation.

/ Health & Errors: Detecting failures and verifying correct operation.
  - _Fatal errors_: Count of unexpected errors that aren't timeouts
    (panics, I/O errors, parsing failures). These indicate bugs or
    infrastructure issues requiring immediate attention.
  - _Active vs idle threads_: Number of threads currently processing work
    versus waiting for requests. Quick sanity check for thread pool health.

== Histogram visualization and correctness

Manipulate bucket responses to visualize results. I need to decide which option
I should use for visualization. I can use either `ratatui` to visualize the
histograms in my terminal, or `actix` to use Netdata's dashboard.

Using `ratatui` allows fast iteration but the code will be useless. Using
`actix` will end up in Netdata's repository. However, getting things right
with `actix` might be too slow. In any case, I need to do two things:

- make sure I can visualize histograms, and
- verify the correctness of the results I'm generating.

== Miscellaneous

- Dynamic calculation of bucket duration when using `FileIndexer`.
- Enable indexing based on source-time field.
- Check if replacing the `HashMap` on the `FileIndexer` with an implicitly
  indexed vector, works and leads to lower memory consumption with faster
  execution time.
- Check if I need to identify delta requests and extract the proper information
  from the difference.
- Simplify code.
- Add tests throughout.
- Support mapping of `OpenTelemetry` fields to `systemd` journal field names.
  Figure out how to do this efficiently and pay-zero cost by default.
- Provide log entries for Netdata's table in the UI.
- Facets for log sources.
- Dynamic configuration for log directories.
- Invalidate file index cache for different set of indexed fields.
- Decide if we need different Netdata _function_ for systemd journals versus
  otel journals?
- Auto-detect fields to index by sampling historical logs.
