pub mod error;

pub use crate::index_state::error::Result;

use crate::index::{FileIndex, FileIndexer};
use crate::registry::Registry;
use crate::repository::File;
use crate::{JournalFile, file::Mmap};
use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    mpsc::{Receiver, Sender, channel},
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct HistogramRequest {
    pub after: u64,
    pub before: u64,
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
            .find(|&&bucket_width| duration / (bucket_width.as_micros() as u64) >= 100)
            .map(|d| d.as_micros())
            .unwrap_or(1) as u64
    }

    pub fn into_bucket_requests(self) -> Vec<BucketRequest> {
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
            });
        }

        buckets
    }
}

thread_local! {
    static FILE_INDEXER: RefCell<FileIndexer> = RefCell::new(FileIndexer::default());
}

pub struct FileIndexCache {
    pub cache: Arc<RwLock<HashMap<File, FileIndex>>>,
    indexing_tx: Sender<IndexingTask>,
}

impl Default for FileIndexCache {
    fn default() -> Self {
        let cache = Arc::new(RwLock::new(HashMap::new()));
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

struct IndexingTask {
    files: Vec<File>,
    fields: HashSet<String>,
}

impl FileIndexCache {
    fn indexing_worker(rx: Receiver<IndexingTask>, cache: Arc<RwLock<HashMap<File, FileIndex>>>) {
        while let Ok(task) = rx.recv() {
            let timeout = Duration::from_secs(10);
            let start_time = Instant::now();

            let field_names: Vec<&[u8]> = task.fields.iter().map(|x| x.as_bytes()).collect();

            use rayon::prelude::*;
            let file_indexes: HashMap<File, FileIndex> = task
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
                            .index(&journal_file, None, &field_names, 1)
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
    pub fn request_indexing(&self, files: Vec<File>, fields: HashSet<String>) {
        let _ = self.indexing_tx.send(IndexingTask { files, fields });
    }

    // /// Try to get a file index if it exists
    // pub fn resolve_partial_response(&self, partial_response: &mut BucketPartialResponse) {
    //     for file in &partial_response.request_metadata.files {
    //         if let Some(file_index) = self.cache.write().get_mut(file) {
    //             // Add any missing unindexed fields to the bucket
    //             for unindexed_field in file_index.fields() {
    //                 if partial_response.unindexed_fields.contains(unindexed_field) {
    //                     continue;
    //                 }

    //                 partial_response
    //                     .unindexed_fields
    //                     .insert(unindexed_field.clone());
    //             }

    //             let start_time = partial_response.request_metadata.request.start;
    //             let end_time = partial_response.request_metadata.request.end;

    //             for (indexed_field, bitmap) in file_index.bitmaps() {
    //                 let count = file_index
    //                     .count_bitmap_entries_in_range(bitmap, start_time, end_time)
    //                     .unwrap_or(0);

    //                 if let Some(total) = partial_response.indexed_fields.get_mut(indexed_field) {
    //                     *total += count;
    //                 } else {
    //                     partial_response
    //                         .indexed_fields
    //                         .insert(indexed_field.clone(), count);
    //                 }
    //             }
    //         };
    //     }
    // }

    pub fn resolve_partial_response(&self, partial_response: &mut BucketPartialResponse) {
        partial_response.request_metadata.files.retain(|file| {
            if let Some(file_index) = self.cache.write().get_mut(file) {
                // Add any missing unindexed fields to the bucket
                for unindexed_field in file_index.fields() {
                    if partial_response.unindexed_fields.contains(unindexed_field) {
                        continue;
                    }

                    partial_response
                        .unindexed_fields
                        .insert(unindexed_field.clone());
                }

                let start_time = partial_response.request_metadata.request.start;
                let end_time = partial_response.request_metadata.request.end;

                for (indexed_field, bitmap) in file_index.bitmaps() {
                    let count = file_index
                        .count_bitmap_entries_in_range(bitmap, start_time, end_time)
                        .unwrap_or(0);

                    if let Some(total) = partial_response.indexed_fields.get_mut(indexed_field) {
                        *total += count;
                    } else {
                        partial_response
                            .indexed_fields
                            .insert(indexed_field.clone(), count);
                    }
                }

                false // Remove this file (it's been processed)
            } else {
                true // Keep this file (not found in cache)
            }
        });
    }
}

pub struct IndexState {
    pub registry: Registry,
    pub cache: FileIndexCache,
    pub indexed_fields: HashSet<String>,
}

impl IndexState {
    pub fn new(registry: Registry, indexed_fields: HashSet<String>) -> Self {
        Self {
            registry,
            cache: FileIndexCache::default(),
            indexed_fields,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct BucketRequest {
    pub start: u64,
    pub end: u64,
}

impl BucketRequest {
    pub fn duration(&self) -> Option<u64> {
        self.end.checked_sub(self.start)
    }
}

#[derive(Debug, Clone)]
pub struct RequestMetadata {
    // The original request
    pub request: BucketRequest,

    // Files we need to use to generate a full response
    pub files: Vec<File>,
}

#[derive(Debug, Clone)]
pub struct BucketPartialResponse {
    // Used to incrementally progress request
    pub request_metadata: RequestMetadata,

    pub indexed_fields: HashMap<String, usize>,
    pub unindexed_fields: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct BucketCompleteResponse {
    pub indexed_fields: HashMap<String, usize>,
    pub unindexed_fields: HashSet<String>,
}

#[derive(Debug, Clone)]
pub enum BucketResponse {
    Complete(BucketCompleteResponse),
    Partial(BucketPartialResponse),
}

pub struct AppState {
    pub index_state: IndexState,
    pub partial_responses: HashMap<BucketRequest, BucketPartialResponse>,
    pub complete_responses: HashMap<BucketRequest, BucketCompleteResponse>,
}

impl AppState {
    pub fn new(path: &str, indexed_fields: HashSet<String>) -> Result<Self> {
        let mut registry = Registry::new()?;
        registry.watch_directory(path)?;

        let index_state = IndexState::new(registry, indexed_fields);

        Ok(Self {
            index_state,
            partial_responses: HashMap::new(),
            complete_responses: HashMap::new(),
        })
    }

    pub fn histogram(&mut self, request: HistogramRequest) {
        // Create any new partial requests
        for bucket_request in request.into_bucket_requests().iter() {
            if self.complete_responses.contains_key(bucket_request) {
                // eprintln!("Found complete response for request: {:?}", bucket_request);
                continue;
            }

            if self.partial_responses.contains_key(bucket_request) {
                // eprintln!("Found partial response for request: {:?}", bucket_request);
                continue;
            }

            let mut request_metadata = RequestMetadata {
                request: *bucket_request,
                files: Vec::new(),
            };

            self.index_state.registry.find_files_in_range(
                bucket_request.start,
                bucket_request.end,
                &mut request_metadata.files,
            );

            let partial_response = BucketPartialResponse {
                request_metadata,
                indexed_fields: HashMap::default(),
                unindexed_fields: HashSet::default(),
            };

            self.partial_responses
                .insert(*bucket_request, partial_response);
        }

        // Collect all files needed to complete the partial responses, and
        // send indexing request
        let mut files = Vec::new();

        for partial_response in self.partial_responses.values() {
            files.extend_from_slice(&partial_response.request_metadata.files);
        }

        self.index_state
            .cache
            .request_indexing(files, self.index_state.indexed_fields.clone());

        // Try to resolve/progress partial responses
        for partial_response in self.partial_responses.values_mut() {
            self.index_state
                .cache
                .resolve_partial_response(partial_response);
        }

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
                        .insert(*bucket_request, complete_response);
                    false // Remove from partial_responses
                } else {
                    true // Keep in partial_responses
                }
            });
    }
}
