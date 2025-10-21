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

    /// Try to get a file index if it exists
    pub fn get(&self, file: &File) -> Option<FileIndex> {
        self.cache.read().get(file).cloned()
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

    fn index_buckets(&mut self, bucket_requests: &[BucketRequest]) {
        // FIXME: make this a debug assert one verified
        assert!(bucket_requests.is_sorted_by(|a, b| a.start < b.start));

        let Some(start) = bucket_requests.first().map(|br| br.start) else {
            return;
        };
        let Some(end) = bucket_requests.last().map(|br| br.end) else {
            return;
        };

        // Collect all files that we need to lookup
        let mut files = Vec::new();
        // Lookup the in-range files from the registry
        self.registry.find_files_in_range(start, end, &mut files);

        // Make sure our cache contains the file indexes we need
        if !files.is_empty() {
            files.sort();
            files.dedup();
            self.cache
                .request_indexing(files, self.indexed_fields.clone());
        }
    }

    pub fn resolve_buckets(
        &mut self,
        bucket_requests: &[BucketRequest],
    ) -> Vec<BucketPartialResponse> {
        // Request indexing of files covered by the bucket requests
        self.index_buckets(bucket_requests);

        // Now iterate each bucket and try to resolve it.
        let bucket_responses: Vec<BucketPartialResponse> = Vec::new();
        let mut bucket_files = Vec::new();
        for bucket_request in bucket_requests {
            let BucketRequest { start, end } = *bucket_request;

            // Look up the files need to process this bucket request
            bucket_files.clear();
            self.registry
                .find_files_in_range(start, end, &mut bucket_files);

            todo!()
        }

        bucket_responses
    }
}

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug)]
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
            .find(|&&bucket_width| duration / bucket_width.as_secs() >= 10)
            .map(|d| d.as_secs())
            .unwrap_or(1)
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

    pub fn histogram(&self, request: HistogramRequest) {
        let bucket_requests = request.into_bucket_requests();

        for (idx, bucket_request) in bucket_requests.iter().enumerate() {
            println!(
                "[{}] Bucket[{}, +{})",
                idx,
                bucket_request.start,
                bucket_request.end - bucket_request.start
            );
        }
    }
}
