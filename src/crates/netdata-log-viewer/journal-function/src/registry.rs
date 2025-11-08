//! Journal file registry with monitoring and metadata tracking
//!
//! This module provides the complete infrastructure for tracking journal files,
//! including file system monitoring, metadata management, and file collection.

use super::{CatalogError, File, Result};
use journal::collections::{HashMap, HashSet};
use journal::repository::{Repository as BaseRepository, scan_journal_files};
use notify::{
    event::{EventKind, ModifyKind, RenameMode},
    Event, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::path::Path;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

// ============================================================================
// File Metadata Types
// ============================================================================

/// Time range information for a journal file derived from indexing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    /// File has not been indexed yet, time range unknown. These files will
    /// be queued for indexing and reported in subsequent poll cycles.
    Unknown,

    /// Active file currently being written to. The end time represents
    /// the latest entry seen when the file was indexed, but new entries
    /// may have been written since.
    Active {
        start: u32,
        end: u32,
        indexed_at: u64,
    },

    /// Archived file with known start and end times.
    Bounded {
        start: u32,
        end: u32,
        indexed_at: u64,
    },
}

/// Pairs a File with its TimeRange.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// The journal file
    pub file: File,
    /// Time range from its file index
    pub time_range: TimeRange,
}

// ============================================================================
// File System Monitor
// ============================================================================

/// File system watcher that sends events through an async channel
#[derive(Debug)]
pub struct Monitor {
    /// The watcher instance
    watcher: RecommendedWatcher,
}

impl Monitor {
    /// Create a new monitor and return it with its event receiver
    pub fn new() -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();

        let watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(event) = res {
                    let _ = event_sender.send(event);
                }
            },
            notify::Config::default(),
        )?;

        Ok((Self { watcher }, event_receiver))
    }

    /// Start watching a directory for file system events
    pub fn watch_directory(&mut self, path: &str) -> Result<()> {
        self.watcher
            .watch(Path::new(path), RecursiveMode::Recursive)?;

        Ok(())
    }

    /// Stop watching a directory
    pub fn unwatch_directory(&mut self, path: &str) -> Result<()> {
        self.watcher.unwatch(Path::new(path))?;
        Ok(())
    }
}

// ============================================================================
// Repository
// ============================================================================

/// Repository that tracks journal files with metadata
///
/// This wraps the base repository and automatically maintains time range metadata
/// for each file. Metadata starts as Unknown and can be updated when computed.
struct Repository {
    base: BaseRepository,
    file_metadata: HashMap<File, FileInfo>,
}

impl Repository {
    /// Create a new empty repository
    fn new() -> Self {
        Self {
            base: BaseRepository::default(),
            file_metadata: HashMap::default(),
        }
    }

    /// Insert a file to the repository
    fn insert(&mut self, file: File) -> Result<()> {
        let file_info = FileInfo {
            file: file.clone(),
            time_range: TimeRange::Unknown,
        };

        self.base.insert(file.clone())?;
        self.file_metadata.insert(file, file_info);

        Ok(())
    }

    /// Remove a file from the repository
    fn remove(&mut self, file: &File) -> Result<()> {
        self.base.remove(file)?;
        self.file_metadata.remove(file);
        Ok(())
    }

    /// Remove all files from a directory
    fn remove_directory(&mut self, path: &str) {
        self.base.remove_directory(path);
        self.file_metadata
            .retain(|file, _| file.dir().ok().map(|dir| dir != path).unwrap_or(true));
    }

    /// Find files in a time range
    fn find_files_in_range(&self, start: u32, end: u32) -> Vec<FileInfo> {
        let files: Vec<File> = self.base.find_files_in_range(start, end);

        files
            .into_iter()
            .map(|file| {
                self.file_metadata
                    .get(&file)
                    .cloned()
                    .unwrap_or_else(|| FileInfo {
                        file: file.clone(),
                        time_range: TimeRange::Unknown,
                    })
            })
            .collect()
    }

    /// Update time range metadata for a file
    fn update_file_info(&mut self, file_info: FileInfo) {
        let file = file_info.file.clone();
        self.file_metadata.insert(file, file_info);
    }
}

impl Default for Repository {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Registry
// ============================================================================

/// Internal state for Registry
struct RegistryInner {
    repository: Repository,
    watched_directories: HashSet<String>,
    monitor: Monitor,
}

/// Coordinates file monitoring and repository management (thread-safe)
#[derive(Clone)]
pub struct Registry {
    inner: Arc<RwLock<RegistryInner>>,
}

impl Registry {
    /// Create a new registry with the given monitor
    pub fn new(monitor: Monitor) -> Self {
        let inner = RegistryInner {
            repository: Repository::new(),
            watched_directories: HashSet::default(),
            monitor,
        };

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    /// Watch a directory and perform initial scan
    pub fn watch_directory(&self, path: &str) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| CatalogError::LockPoisoned(e.to_string()))?;

        if inner.watched_directories.contains(path) {
            warn!("Directory {} is already being watched", path);
            return Ok(());
        }

        info!("Scanning directory: {}", path);
        let files = scan_journal_files(path)?;
        info!("Found {} journal files in {}", files.len(), path);

        // Start watching with notify
        inner.monitor.watch_directory(path)?;
        inner.watched_directories.insert(String::from(path));

        // Insert all discovered files into repository (automatically initializes metadata)
        for file in files {
            debug!("Adding file to repository: {:?}", file.path());

            if let Err(e) = inner.repository.insert(file) {
                error!("Failed to insert file into repository: {}", e);
            }
        }

        info!(
            "Now watching directory: {} (total directories: {})",
            path,
            inner.watched_directories.len()
        );
        Ok(())
    }

    /// Stop watching a directory and clean up its files
    pub fn unwatch_directory(&self, path: &str) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| CatalogError::LockPoisoned(e.to_string()))?;

        if !inner.watched_directories.contains(path) {
            warn!("Directory {} is not being watched", path);
            return Ok(());
        }

        inner.monitor.unwatch_directory(path)?;
        inner.repository.remove_directory(path); // Handles both repository and metadata cleanup
        inner.watched_directories.remove(path);

        info!("Stopped watching directory: {}", path);
        Ok(())
    }

    /// Process a file system event and update the repository
    pub fn process_event(&self, event: Event) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| CatalogError::LockPoisoned(e.to_string()))?;

        match event.kind {
            EventKind::Create(_) => {
                for path in &event.paths {
                    debug!("Adding file to repository: {:?}", path);

                    if let Some(file) = File::from_path(path) {
                        if let Err(e) = inner.repository.insert(file) {
                            error!("Failed to insert file: {}", e);
                        }
                    } else {
                        warn!("Path is not a valid journal file: {:?}", path);
                    }
                }
            }
            EventKind::Remove(_) => {
                for path in &event.paths {
                    debug!("Removing file from repository: {:?}", path);

                    if let Some(file) = File::from_path(path) {
                        if let Err(e) = inner.repository.remove(&file) {
                            error!("Failed to remove file: {}", e);
                        }
                    } else {
                        warn!("Path is not a valid journal file: {:?}", path);
                    }
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                // Handle renames: remove old, add new
                if event.paths.len() >= 2 {
                    let old_path = &event.paths[0];
                    let new_path = &event.paths[1];
                    info!("Rename event: {:?} -> {:?}", old_path, new_path);

                    if let Some(old_file) = File::from_path(old_path) {
                        info!("Removing old file: {:?}", old_file.path());
                        if let Err(e) = inner.repository.remove(&old_file) {
                            error!("Failed to remove old file: {}", e);
                        }
                    }

                    if let Some(new_file) = File::from_path(new_path) {
                        info!("Inserting new file: {:?}", new_file.path());
                        if let Err(e) = inner.repository.insert(new_file) {
                            error!("Failed to insert new file: {}", e);
                        }
                    }
                } else {
                    error!(
                        "Rename event with unexpected path count: {:#?}",
                        event.paths
                    );
                }
            }
            EventKind::Modify(ModifyKind::Name(rename_mode)) => {
                error!("Unhandled rename mode: {:?}", rename_mode);
            }
            event_kind => {
                // Ignore other events (content modifications, access, etc.)
                trace!("Ignoring notify event kind: {:?}", event_kind);
            }
        }
        Ok(())
    }

    /// Find files in a time range
    pub fn find_files_in_range(&self, start: u32, end: u32) -> Result<Vec<FileInfo>> {
        let inner = self
            .inner
            .read()
            .map_err(|e| CatalogError::LockPoisoned(e.to_string()))?;

        Ok(inner.repository.find_files_in_range(start, end))
    }

    /// Update time range of a file
    pub fn update_time_range(&self, file: &File, time_range: TimeRange) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| CatalogError::LockPoisoned(e.to_string()))?;

        let file_info = FileInfo {
            file: file.clone(),
            time_range,
        };
        inner.repository.update_file_info(file_info);
        Ok(())
    }
}
