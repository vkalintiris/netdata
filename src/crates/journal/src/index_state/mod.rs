pub mod error;

pub use crate::index_state::error::Result;

use crate::index::{FileIndex, FileIndexer, FilterExpr};
use crate::registry::Registry;
use crate::repository::File;
use crate::{JournalFile, file::Mmap};
use allocative::Allocative;
use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{
    Arc,
    mpsc::{Receiver, Sender, channel},
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Allocative)]
pub struct HistogramRequest {
    pub after: u64,
    pub before: u64,
    pub filter_expr: Arc<FilterExpr<String>>,
}

impl HistogramRequest {
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

    pub fn into_bucket_requests(&self) -> Vec<BucketRequest> {
        let bucket_duration = self.calculate_bucket_duration();

        // Buckets are aligned to their duration
        let aligned_start = (self.after / bucket_duration) * bucket_duration;
        let aligned_end = (self.before / bucket_duration + 1) * bucket_duration;

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
}

thread_local! {
    static FILE_INDEXER: RefCell<FileIndexer> = RefCell::new(FileIndexer::default());
}

pub struct FileIndexCache {
    pub cache: Arc<RwLock<FxHashMap<File, FileIndex>>>,
    indexing_tx: Sender<IndexingTask>,
}

impl Default for FileIndexCache {
    fn default() -> Self {
        let cache = Arc::new(RwLock::new(FxHashMap::default()));
        let (tx, rx) = channel();

        // Spawn background indexing thread
        let cache_clone = Arc::clone(&cache);
        std::thread::spawn(move || {
            Self::indexing_worker(rx, cache_clone);
        });

        Self {
            cache,
            indexing_tx: tx,
        }
    }
}

#[derive(Allocative)]
struct IndexingTask {
    files: VecDeque<File>,
    fields: FxHashSet<String>,
}

impl FileIndexCache {
    fn indexing_worker(rx: Receiver<IndexingTask>, cache: Arc<RwLock<FxHashMap<File, FileIndex>>>) {
        while let Ok(task) = rx.recv() {
            let timeout = Duration::from_secs(10);
            let start_time = Instant::now();

            let field_names: Vec<&[u8]> = task.fields.iter().map(|x| x.as_bytes()).collect();

            use rayon::prelude::*;
            let file_indexes: FxHashMap<File, FileIndex> = task
                .files
                .par_iter()
                .filter_map(|file| {
                    // Check timeout before starting work on this file
                    if start_time.elapsed() > timeout {
                        return None;
                    }

                    // Skip indexing if cache already contains this file
                    if cache.read().contains_key(file) {
                        return None;
                    }

                    // Create the file index
                    FILE_INDEXER.with(|indexer| {
                        let mut file_indexer = indexer.borrow_mut();
                        let window_size = 32 * 1024 * 1024;
                        let journal_file =
                            JournalFile::<Mmap>::open(file.path(), window_size).ok()?;

                        file_indexer
                            .index(&journal_file, None, &field_names, 3600)
                            .ok()
                            .map(|file_index| (file.clone(), file_index))
                    })
                })
                .collect();

            let completed = file_indexes.len();
            let total = task.files.len();

            // Always update cache with whatever we managed to complete
            cache.write().extend(file_indexes);

            if start_time.elapsed() > timeout {
                eprintln!(
                    "Indexing timed out after {:?}. Completed {}/{} files",
                    start_time.elapsed(),
                    completed,
                    total
                );
            }
        }
    }

    /// Request files to be indexed (non-blocking)
    pub fn request_indexing(&self, files: VecDeque<File>, fields: FxHashSet<String>) {
        let _ = self.indexing_tx.send(IndexingTask { files, fields });
    }

    /// Resolve as many partial responses as we can using cached files
    pub fn resolve_partial_responses(
        &self,
        partial_responses: &mut FxHashMap<BucketRequest, BucketPartialResponse>,
    ) {
        // Collect all unique files that any partial response is waiting for
        let mut pending_files = FxHashSet::default();
        for partial_response in partial_responses.values() {
            for file in &partial_response.request_metadata.files {
                pending_files.insert(file.clone());
            }
        }

        // Get cache lock once
        let mut cache = self.cache.write();

        // For each file that might be in cache, update all responses that need it
        for file in pending_files {
            if let Some(file_index) = cache.get_mut(&file) {
                // Update all partial responses that contain this file
                for partial_response in partial_responses.values_mut() {
                    // Check if this response needs this file
                    let file_idx = partial_response
                        .request_metadata
                        .files
                        .iter()
                        .position(|f| f == &file);

                    let Some(idx) = file_idx else {
                        continue;
                    };

                    // Remove the file from the queue
                    partial_response.request_metadata.files.remove(idx);

                    // Add any missing unindexed fields to the bucket
                    for field in file_index.fields() {
                        if file_index.is_indexed(field) {
                            continue;
                        }

                        partial_response.unindexed_fields.insert(field.clone());
                    }

                    let start_time = partial_response.request_metadata.request.start;
                    let end_time = partial_response.request_metadata.request.end;

                    let filter_bitmap = if *partial_response.request_metadata.request.filter_expr
                        != FilterExpr::<String>::None
                    {
                        let filter_expr = partial_response
                            .request_metadata
                            .request
                            .filter_expr
                            .resolve(file_index);

                        Some(filter_expr.evaluate())
                    } else {
                        None
                    };

                    for (indexed_field, bitmap) in file_index.bitmaps() {
                        // once for unfiltered count
                        {
                            let unfiltered_count = file_index
                                .count_bitmap_entries_in_range(bitmap, start_time, end_time)
                                .unwrap_or(0);

                            if let Some((unfiltered_total, _)) =
                                partial_response.indexed_fields.get_mut(indexed_field)
                            {
                                *unfiltered_total += unfiltered_count;
                            } else {
                                partial_response
                                    .indexed_fields
                                    .insert(indexed_field.clone(), (unfiltered_count, 0));
                            }
                        }

                        // once more for filtered count
                        if let Some(filter_bitmap) = &filter_bitmap {
                            let bitmap = bitmap & filter_bitmap;
                            let filtered_count = file_index
                                .count_bitmap_entries_in_range(&bitmap, start_time, end_time)
                                .unwrap_or(0);

                            if let Some((_, filtered_total)) =
                                partial_response.indexed_fields.get_mut(indexed_field)
                            {
                                *filtered_total += filtered_count;
                            } else {
                                partial_response
                                    .indexed_fields
                                    .insert(indexed_field.clone(), (0, filtered_count));
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Allocative)]
pub struct IndexState {
    pub registry: Registry,
    #[allocative(skip)]
    pub cache: FileIndexCache,
    pub indexed_fields: FxHashSet<String>,
}

impl IndexState {
    pub fn new(registry: Registry, indexed_fields: FxHashSet<String>) -> Self {
        Self {
            registry,
            cache: FileIndexCache::default(),
            indexed_fields,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Allocative)]
pub struct BucketRequest {
    pub start: u64,
    pub end: u64,
    pub filter_expr: Arc<FilterExpr<String>>,
}

impl BucketRequest {
    pub fn duration(&self) -> Option<u64> {
        self.end.checked_sub(self.start)
    }
}

#[derive(Debug, Clone, Allocative)]
pub struct RequestMetadata {
    // The original request
    pub request: BucketRequest,

    // Files we need to use to generate a full response
    pub files: VecDeque<File>,
}

// #[derive(Debug, Clone, Copy, Allocative)]
// pub enum Count {
//     Unfiltered(usize),
//     Filtered(usize),
// }

// impl std::ops::AddAssign<usize> for Count {
//     fn add_assign(&mut self, rhs: usize) {
//         match self {
//             Count::Unfiltered(val) => *val += rhs,
//             Count::Filtered(val) => *val += rhs,
//         }
//     }
// }

#[derive(Debug, Clone, Allocative)]
pub struct BucketPartialResponse {
    // Used to incrementally progress request
    pub request_metadata: RequestMetadata,

    pub indexed_fields: FxHashMap<String, (usize, usize)>,
    pub unindexed_fields: FxHashSet<String>,
}

#[derive(Debug, Clone, Allocative)]
pub struct BucketCompleteResponse {
    pub indexed_fields: FxHashMap<String, (usize, usize)>,
    pub unindexed_fields: FxHashSet<String>,
}

#[derive(Debug, Clone, Allocative)]
pub enum BucketResponse {
    Complete(BucketCompleteResponse),
    Partial(BucketPartialResponse),
}

#[derive(Debug, Clone, Allocative)]
pub struct HistogramResult {
    pub buckets: Vec<(BucketRequest, BucketResponse)>,
}

#[derive(Allocative)]
pub struct AppState {
    pub index_state: IndexState,
    pub partial_responses: FxHashMap<BucketRequest, BucketPartialResponse>,
    pub complete_responses: FxHashMap<BucketRequest, BucketCompleteResponse>,
}

impl AppState {
    pub fn new(path: &str, indexed_fields: FxHashSet<String>) -> Result<Self> {
        let mut registry = Registry::new()?;
        registry.watch_directory(path)?;

        let index_state = IndexState::new(registry, indexed_fields);

        Ok(Self {
            index_state,
            partial_responses: FxHashMap::default(),
            complete_responses: FxHashMap::default(),
        })
    }

    pub fn get_histogram(&mut self, request: HistogramRequest) -> HistogramResult {
        // Process the histogram request to ensure buckets are computed/in-progress
        self.process_histogram_request(&request);

        // Generate the bucket requests for this histogram
        let bucket_requests = request.into_bucket_requests();

        // Collect the responses for each bucket
        let mut buckets = Vec::with_capacity(bucket_requests.len());
        for bucket_request in bucket_requests {
            let response = if let Some(complete) = self.complete_responses.get(&bucket_request) {
                BucketResponse::Complete(complete.clone())
            } else if let Some(partial) = self.partial_responses.get(&bucket_request) {
                BucketResponse::Partial(partial.clone())
            } else {
                // This shouldn't happen after process_histogram_request, but handle it gracefully
                continue;
            };

            buckets.push((bucket_request, response));
        }

        HistogramResult { buckets }
    }

    pub fn process_histogram_request(&mut self, request: &HistogramRequest) {
        // Create any new partial requests
        for bucket_request in request.into_bucket_requests().iter() {
            if self.complete_responses.contains_key(bucket_request) {
                continue;
            }

            if self.partial_responses.contains_key(bucket_request) {
                continue;
            }

            let mut request_metadata = RequestMetadata {
                request: bucket_request.clone(),
                files: VecDeque::new(),
            };

            self.index_state.registry.find_files_in_range(
                bucket_request.start,
                bucket_request.end,
                &mut request_metadata.files,
            );

            let partial_response = BucketPartialResponse {
                request_metadata,
                indexed_fields: FxHashMap::default(),
                unindexed_fields: FxHashSet::default(),
            };

            self.partial_responses
                .insert(bucket_request.clone(), partial_response);
        }

        // Collect all files needed to complete the partial responses, and
        // send indexing request
        let mut files = VecDeque::new();

        for partial_response in self.partial_responses.values() {
            files.extend(partial_response.request_metadata.files.iter().cloned());
        }

        // Sort and deduplicate
        {
            let mut v: Vec<File> = files.drain(..).collect();

            v.sort();
            v.dedup();

            // Put them back
            files.extend(v);
        }

        self.index_state
            .cache
            .request_indexing(files, self.index_state.indexed_fields.clone());

        // Try to resolve/progress partial responses
        // Use the optimized method that updates all responses when a file is found in cache
        self.index_state
            .cache
            .resolve_partial_responses(&mut self.partial_responses);

        // Move completed partial responses to complete responses
        self.partial_responses
            .retain(|bucket_request, partial_response| {
                if partial_response.request_metadata.files.is_empty() {
                    // No more files to process - this response is complete
                    let complete_response = BucketCompleteResponse {
                        indexed_fields: partial_response.indexed_fields.clone(),
                        unindexed_fields: partial_response.unindexed_fields.clone(),
                    };
                    self.complete_responses
                        .insert(bucket_request.clone(), complete_response);
                    false // Remove from partial_responses
                } else {
                    true // Keep in partial_responses
                }
            });
    }

    pub fn build_fg(&self) {
        use allocative::FlameGraphBuilder;
        use std::fmt::Write as _;

        let mut output = String::new();

        for (file, file_index) in self.index_state.cache.cache.read().iter() {
            let mut flamegraph = FlameGraphBuilder::default();
            flamegraph.visit_root(file_index);

            let fg_output = flamegraph.finish();

            let fg_str = fg_output.flamegraph().write();
            for line in fg_str.lines() {
                if !line.is_empty() {
                    // Prepend the file path to each stack trace
                    writeln!(output, "{};{}", file.path(), line).unwrap();
                }
            }
        }

        let mut flamegraph = FlameGraphBuilder::default();
        flamegraph.visit_root(self);

        let fg_output = flamegraph.finish();
        let fg_str = fg_output.flamegraph().write();
        writeln!(output, "{}", fg_str).unwrap();

        std::fs::write("/home/vk/mo/flamegraph.fg", output).unwrap();

        // Generate SVG using inferno
        use std::process::Command;
        let status = Command::new("sh")
            .arg("-c")
            .arg(r#"cat ~/mo/flamegraph.fg | inferno-flamegraph --title "Journal Cache Memory by File" --colors mem --countname "bytes" > ~/mo/flamegraph.svg"#)
            .status()
            .expect("Failed to execute inferno-flamegraph");

        if status.success() {
            println!("Flamegraph SVG generated at ~/mo/flamegraph.svg");
        } else {
            eprintln!("Failed to generate flamegraph SVG");
        }
    }
}
