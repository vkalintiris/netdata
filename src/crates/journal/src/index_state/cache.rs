use crate::collections::{HashMap, HashSet};
use crate::index::{FileIndex, FileIndexer};
use crate::index_state::request::BucketRequest;
use crate::index_state::response::BucketPartialResponse;
use crate::repository::File;
use crate::{JournalFile, file::Mmap};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{
    Arc,
    mpsc::{Receiver, Sender, channel},
};
use std::time::{Duration, Instant};

thread_local! {
    static FILE_INDEXER: RefCell<FileIndexer> = RefCell::new(FileIndexer::default());
}

/// Request to index a file with a specific bucket duration
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct IndexRequest {
    pub file: File,
    pub bucket_duration: u64,
}

#[cfg_attr(feature = "allocative", derive(Allocative))]
struct IndexingTask {
    requests: VecDeque<IndexRequest>,
    fields: HashSet<String>,
}

pub struct IndexCache {
    pub file_indexes: Arc<RwLock<HashMap<File, FileIndex>>>,
    indexing_tx: Sender<IndexingTask>,
}

impl Default for IndexCache {
    fn default() -> Self {
        let file_indexes = Arc::new(RwLock::new(HashMap::default()));
        let (tx, rx) = channel();

        // Spawn background indexing thread
        let cache_clone = Arc::clone(&file_indexes);
        std::thread::spawn(move || {
            Self::indexing_worker(rx, cache_clone);
        });

        Self {
            file_indexes,
            indexing_tx: tx,
        }
    }
}

impl IndexCache {
    /// Request files to be indexed (non-blocking)
    pub fn request_indexing(&self, requests: VecDeque<IndexRequest>, fields: HashSet<String>) {
        let _ = self.indexing_tx.send(IndexingTask { requests, fields });
    }

    /// Resolve as many partial responses as we can using cached files
    pub fn resolve_partial_responses(
        &self,
        partial_responses: &mut HashMap<BucketRequest, BucketPartialResponse>,
        pending_files: HashSet<File>,
    ) {
        let cache = self.file_indexes.read();

        for file in pending_files {
            if let Some(file_index) = cache.get(&file) {
                for partial_response in partial_responses.values_mut() {
                    partial_response.update(&file, file_index);
                }
            }
        }
    }

    fn indexing_worker(rx: Receiver<IndexingTask>, cache: Arc<RwLock<HashMap<File, FileIndex>>>) {
        while let Ok(task) = rx.recv() {
            let timeout = Duration::from_secs(10);
            let start_time = Instant::now();

            let field_names: Vec<&[u8]> = task.fields.iter().map(|x| x.as_bytes()).collect();

            use rayon::prelude::*;
            let file_indexes: HashMap<File, FileIndex> = task
                .requests
                .par_iter()
                .filter_map(|request| {
                    // Check timeout before starting work on this file
                    if start_time.elapsed() > timeout {
                        return None;
                    }

                    // Skip indexing if cache already contains this file with sufficient granularity
                    // (cached duration <= requested duration means cached is more granular or equal)
                    if let Some(cached_index) = cache.read().get(&request.file) {
                        if cached_index.bucket_duration() <= request.bucket_duration {
                            return None;
                        }
                        // Otherwise, fall through and re-index with finer granularity
                    }

                    // Create the file index
                    FILE_INDEXER.with(|indexer| {
                        let mut file_indexer = indexer.borrow_mut();
                        let window_size = 32 * 1024 * 1024;
                        let journal_file =
                            JournalFile::<Mmap>::open(request.file.path(), window_size).ok()?;

                        file_indexer
                            .index(&journal_file, None, &field_names, request.bucket_duration)
                            .ok()
                            .map(|file_index| (request.file.clone(), file_index))
                    })
                })
                .collect();

            let completed = file_indexes.len();
            let total = task.requests.len();

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
}
