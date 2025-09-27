use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

pub mod cache;
mod paths;

use crate::paths::JournalFile;

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

/// Represents a systemd journal file with parsed metadata
#[derive(Debug, Clone)]
pub struct RegistryFile {
    /// Parsed journal file information
    info: JournalFile,
}

impl RegistryFile {
    /// Parse a journal file path and extract metadata
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        // Parse the path using JournalFileInfo
        let path_str = path.to_str().ok_or_else(|| {
            RegistryError::InvalidFilename("Path contains invalid UTF-8".to_string())
        })?;

        let info = JournalFile::parse(path_str).ok_or_else(|| {
            RegistryError::InvalidFilename(format!("Cannot parse journal file path: {}", path_str))
        })?;

        Ok(Self { info })
    }

    pub fn path(&self) -> &str {
        &self.info.path
    }

    /// Check if a path looks like a journal file
    pub fn is_journal_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext == "journal" || ext == "journal~")
            .unwrap_or(false)
    }

    /// Check if this is an active journal file
    pub fn is_active(&self) -> bool {
        self.info.is_active()
    }

    /// Check if this is a disposed/corrupted journal file
    pub fn is_disposed(&self) -> bool {
        self.info.is_disposed()
    }

    /// Get the user ID if this is a user journal
    pub fn user_id(&self) -> Option<u32> {
        self.info.user_id()
    }

    /// Get the remote host if this is a remote journal
    pub fn remote_host(&self) -> Option<&str> {
        self.info.remote_host()
    }

    /// Get the namespace if this journal belongs to a namespace
    pub fn namespace(&self) -> Option<&str> {
        self.info.namespace()
    }
}

/// Internal watcher state
struct WatcherState {
    watcher: RecommendedWatcher,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Registry of journal files with automatic file system monitoring
pub struct JournalRegistry {
    /// Currently tracked journal files
    files: Arc<RwLock<HashMap<PathBuf, RegistryFile>>>,

    /// Directories being monitored
    watch_dirs: Arc<RwLock<HashSet<PathBuf>>>,

    /// Internal watcher state
    watcher_state: Arc<RwLock<Option<WatcherState>>>,

    /// Newly discovered files
    new_files: Arc<RwLock<Vec<RegistryFile>>>,

    /// Newly discovered files
    deleted_files: Arc<RwLock<Vec<RegistryFile>>>,
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
        let mut files = self.files.write();
        let removed_files: Vec<_> = files
            .keys()
            .filter(|path| path.starts_with(&canonical_dir))
            .cloned()
            .collect();

        for path in removed_files {
            files.remove(&path);
        }

        Ok(())
    }

    /// Get a snapshot of all current journal files
    pub fn get_files(&self) -> Vec<RegistryFile> {
        self.files.read().values().cloned().collect()
    }

    /// Internal: Scan directory for existing files
    fn scan_directory(&self, dir: &Path) -> Result<()> {
        for entry in WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if !entry.file_type().is_dir() && RegistryFile::is_journal_file(path) {
                self.add_journal_file(path)?;
            }
        }
        Ok(())
    }

    /// Internal: Add or update a journal file
    fn add_journal_file(&self, path: &Path) -> Result<()> {
        match RegistryFile::from_path(path) {
            Ok(journal_file) => {
                let mut files = self.files.write();
                files.insert(path.to_path_buf(), journal_file.clone());

                Ok(())
            }
            Err(e) => {
                warn!("Failed to add journal file {:?}: {}", path, e);
                Ok(())
            }
        }
    }

    /// Internal: Handle filesystem events
    fn handle_event_internal(
        files: &Arc<RwLock<HashMap<PathBuf, RegistryFile>>>,
        watch_dirs: &Arc<RwLock<HashSet<PathBuf>>>,
        event: Event,
        new_files: &Arc<RwLock<Vec<RegistryFile>>>,
        _deleted_files: &Arc<RwLock<Vec<RegistryFile>>>,
    ) -> Result<()> {
        for path in event.paths {
            match event.kind {
                EventKind::Create(_) => {
                    if path.is_dir() {
                        info!("New directory created: {:?}", path);
                        watch_dirs.write().insert(path.clone());

                        /* TODO */
                    } else if RegistryFile::is_journal_file(&path) {
                        if let Ok(journal_file) = RegistryFile::from_path(&path) {
                            new_files.write().push(journal_file);
                        }
                    }
                }
                EventKind::Remove(_) => {
                    if path.is_dir() {
                        watch_dirs.write().remove(&path);

                        // Remove all files under this directory
                        let mut files_lock = files.write();
                        let removed: Vec<_> = files_lock
                            .keys()
                            .filter(|p| p.starts_with(&path))
                            .cloned()
                            .collect();

                        for file_path in removed {
                            files_lock.remove(&file_path);
                        }
                    } else if files.write().remove(&path).is_some() {
                        debug!("Removed journal file: {:?}", path);
                    }
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
