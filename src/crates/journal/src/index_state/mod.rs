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
            let field_names: Vec<&[u8]> = task.fields.iter().map(|x| x.as_bytes()).collect();
            let timeout = Duration::from_secs(30);
            let start_time = Instant::now();

            use rayon::prelude::*;
            let file_indexes: HashMap<File, FileIndex> = task
                .files
                .par_iter()
                .filter_map(|file| {
                    // Check timeout before starting work on this file
                    if start_time.elapsed() > timeout {
                        return None;
                    }

                    if cache.read().contains_key(file) {
                        return None;
                    }

                    FILE_INDEXER.with(|indexer| {
                        let mut file_indexer = indexer.borrow_mut();
                        let window_size = 32 * 1024 * 1024;
                        let journal_file =
                            JournalFile::<Mmap>::open(file.path.clone(), window_size).ok()?;

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
    pub indexed_fields: HashSet<String>,

    pub cache: FileIndexCache,
}

impl IndexState {
    pub fn new(registry: Registry, indexed_fields: HashSet<String>) -> Self {
        Self {
            registry,
            indexed_fields,
            cache: FileIndexCache::default(),
        }
    }

    pub fn resolve_buckets(&mut self, bucket_requests: &[BucketRequest]) -> Vec<BucketResponse> {
        // Collect all files that we need to lookup
        let mut files = Vec::new();
        for bucket_request in bucket_requests {
            let BucketRequest { start, end } = *bucket_request;

            // Lookup the in-range files from the registry
            self.registry.find_files_in_range(start, end, &mut files);
        }

        // Make sure our cache contains the file indexes we need
        if !files.is_empty() {
            files.sort();
            files.dedup();
            self.cache
                .request_indexing(files, self.indexed_fields.clone());
        }

        // Now iterate each bucket and try to reolve it.
        let bucket_responses: Vec<BucketResponse> = Vec::new();
        for _bucket_request in bucket_requests {
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
pub struct BucketResponse {
    pub request: BucketRequest,
    pub indexed_fields: HashMap<String, usize>,
    pub unindexed_fields: HashSet<String>,
}
