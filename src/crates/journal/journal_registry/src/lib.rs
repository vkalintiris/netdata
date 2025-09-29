use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, error, warn};
use walkdir::WalkDir;

pub mod cache;
pub(crate) mod monitor;
pub mod paths;

pub use crate::paths as log;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Notify error: {0}")]
    Notify(#[from] notify::Error),

    #[error("Invalid journal filename: {0}")]
    InvalidFilename(String),

    #[error("Failed to read metadata for {path:?}: {source}")]
    MetadataError {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to initialize watcher: {0}")]
    WatcherInit(notify::Error),

    #[error("Failed to add watch for {path:?}: {source}")]
    WatchError {
        path: PathBuf,
        source: notify::Error,
    },
}

pub type Result<T> = std::result::Result<T, RegistryError>;

/// Internal watcher state
struct WatcherState {
    watcher: RecommendedWatcher,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Registry of journal files with automatic file system monitoring
pub struct JournalRegistry {
    /// Currently tracked journal files
    files: Arc<RwLock<HashMap<String, log::File>>>,

    /// Directories being monitored
    watch_dirs: Arc<RwLock<HashSet<PathBuf>>>,

    /// Internal watcher state
    watcher_state: Arc<RwLock<Option<WatcherState>>>,

    /// Newly discovered files
    new_files: Arc<RwLock<Vec<log::File>>>,

    /// Newly discovered files
    deleted_files: Arc<RwLock<Vec<log::File>>>,
}

impl JournalRegistry {
    /// Create a new journal registry that automatically starts monitoring
    pub fn new() -> Result<Self> {
        let registry = Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            watch_dirs: Arc::new(RwLock::new(HashSet::new())),
            watcher_state: Arc::new(RwLock::new(None)),
            new_files: Arc::new(RwLock::new(Vec::new())),
            deleted_files: Arc::new(RwLock::new(Vec::new())),
        };

        registry.start_internal_watcher()?;
        Ok(registry)
    }

    /// Start the internal watcher
    fn start_internal_watcher(&self) -> Result<()> {
        let mut state_lock = self.watcher_state.write();

        let (tx, rx) = crossbeam_channel::unbounded();

        let watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )
        .map_err(RegistryError::WatcherInit)?;

        // Clone what we need for the background task
        let files = Arc::clone(&self.files);
        let new_files = Arc::clone(&self.new_files);
        let deleted_files = Arc::clone(&self.deleted_files);
        let watch_dirs = Arc::clone(&self.watch_dirs);

        // Spawn background task to process events
        let task_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(250));

            loop {
                interval.tick().await;

                while let Ok(event_result) = rx.try_recv() {
                    match event_result {
                        Ok(event) => {
                            if let Err(e) = Self::handle_event_internal(
                                &files,
                                &watch_dirs,
                                event,
                                &new_files,
                                &deleted_files,
                            ) {
                                error!("Error handling event: {}", e);
                            }
                        }
                        Err(e) => error!("File watch error: {}", e),
                    }
                }
            }
        });

        *state_lock = Some(WatcherState {
            watcher,
            task_handle: Some(task_handle),
        });

        Ok(())
    }

    /// Add a directory to monitor for journal files
    pub fn add_directory(&self, dir: impl AsRef<Path>) -> Result<()> {
        let dir = dir.as_ref();

        // Resolve symlinks
        let canonical_dir = match dir.canonicalize() {
            Ok(path) => path,
            Err(e) => {
                warn!("Cannot canonicalize path {:?}: {}", dir, e);
                return Ok(());
            }
        };

        // Check if already watching
        if self.watch_dirs.read().contains(&canonical_dir) {
            debug!("Already watching directory: {:?}", canonical_dir);
            return Ok(());
        }

        // Add to watcher
        {
            let mut state_lock = self.watcher_state.write();
            let state = state_lock.as_mut().unwrap();

            state
                .watcher
                .watch(&canonical_dir, RecursiveMode::Recursive)
                .map_err(|e| RegistryError::WatchError {
                    path: canonical_dir.clone(),
                    source: e,
                })?;
        }

        // Scan for existing files
        self.scan_directory(&canonical_dir)?;

        // Add to watch list
        self.watch_dirs.write().insert(canonical_dir.clone());

        Ok(())
    }

    /// Remove a directory from monitoring
    pub fn remove_directory(&self, dir: impl AsRef<Path>) -> Result<()> {
        let dir = dir.as_ref();
        let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

        // Remove from watcher
        {
            let mut state_lock = self.watcher_state.write();
            let state = state_lock.as_mut().unwrap();

            let _ = state.watcher.unwatch(&canonical_dir);
        }

        // Remove from watch list
        self.watch_dirs.write().remove(&canonical_dir);

        // Remove all files under this directory
        todo!();
    }

    /// Get a snapshot of all current journal files
    pub fn get_files(&self) -> Vec<log::File> {
        self.files.read().values().cloned().collect()
    }

    /// Internal: Scan directory for existing files
    fn scan_directory(&self, dir: &Path) -> Result<()> {
        let entries = WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file());

        for entry in entries {
            let Some(path) = entry.path().to_str() else {
                continue;
            };

            let Some(file) = log::File::from_str(path) else {
                continue;
            };

            let mut files = self.files.write();
            files.insert(file.path.clone(), file.clone());
        }

        Ok(())
    }

    /// Internal: Handle filesystem events
    fn handle_event_internal(
        _files: &Arc<RwLock<HashMap<String, log::File>>>,
        _watch_dirs: &Arc<RwLock<HashSet<PathBuf>>>,
        event: Event,
        _new_files: &Arc<RwLock<Vec<log::File>>>,
        _deleted_files: &Arc<RwLock<Vec<log::File>>>,
    ) -> Result<()> {
        for _path in event.paths {
            match event.kind {
                EventKind::Create(_) => {
                    todo!()
                }
                EventKind::Remove(_) => {
                    todo!()
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl Drop for JournalRegistry {
    fn drop(&mut self) {
        // Clean shutdown of the background task
        if let Some(mut state) = self.watcher_state.write().take() {
            if let Some(handle) = state.task_handle.take() {
                handle.abort();
            }
        }
    }
}
