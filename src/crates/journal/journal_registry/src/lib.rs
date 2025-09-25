use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

pub mod cache;
mod paths;
use uuid::Uuid;

use crate::paths::{JournalFileInfo, JournalFileStatus};

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
    path: String,

    /// Parsed journal file information
    info: JournalFileInfo,

    /// Last modification time
    modified: SystemTime,

    /// File size in bytes
    size: u64,
}

impl RegistryFile {
    /// Parse a journal file path and extract metadata
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let metadata = path.metadata().map_err(|e| RegistryError::MetadataError {
            path: path.to_path_buf(),
            source: e,
        })?;

        // Parse the path using JournalFileInfo
        let path_str = path.to_str().ok_or_else(|| {
            RegistryError::InvalidFilename("Path contains invalid UTF-8".to_string())
        })?;

        let info = JournalFileInfo::parse(path_str).ok_or_else(|| {
            RegistryError::InvalidFilename(format!("Cannot parse journal file path: {}", path_str))
        })?;

        Ok(Self {
            path: String::from(path_str),
            info,
            size: metadata.len(),
            modified: metadata.modified()?,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn size(&self) -> u64 {
        self.size
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
}

impl JournalRegistry {
    /// Create a new journal registry that automatically starts monitoring
    pub fn new() -> Result<Self> {
        let registry = Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            watch_dirs: Arc::new(RwLock::new(HashSet::new())),
            watcher_state: Arc::new(RwLock::new(None)),
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
        let watch_dirs = Arc::clone(&self.watch_dirs);

        // Spawn background task to process events
        let task_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(250));
            loop {
                interval.tick().await;

                while let Ok(event_result) = rx.try_recv() {
                    match event_result {
                        Ok(event) => {
                            if let Err(e) = Self::handle_event_internal(&files, &watch_dirs, event)
                            {
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
    ) -> Result<()> {
        for path in event.paths {
            match event.kind {
                EventKind::Create(_) => {
                    if path.is_dir() {
                        info!("New directory created: {:?}", path);
                        watch_dirs.write().insert(path.clone());
                    } else if RegistryFile::is_journal_file(&path) {
                        if let Ok(journal_file) = RegistryFile::from_path(&path) {
                            let is_new = !files.read().contains_key(&path);
                            files.write().insert(path.clone(), journal_file.clone());

                            if is_new {
                                debug!("Added journal file: {:?}", path);
                            }
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
                EventKind::Modify(_) => {
                    if !path.is_dir() && RegistryFile::is_journal_file(&path) {
                        if let Ok(journal_file) = RegistryFile::from_path(&path) {
                            files.write().insert(path.clone(), journal_file.clone());
                            debug!("Modified journal file: {:?}", path);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Create a new query builder for this registry
    pub fn query(&self) -> RegistryQuery<'_> {
        RegistryQuery::new(self)
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

#[derive(Debug, Clone, Copy)]
pub enum SortBy {
    Modified(SortOrder),
    Size(SortOrder),
    Path(SortOrder),
}

#[derive(Debug, Clone, Copy)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// A builder for querying journal files with various filters
pub struct RegistryQuery<'a> {
    registry: &'a JournalRegistry,
    source_filter: Option<SourceFilter>, // Changed this
    statuses: Option<Vec<JournalFileStatus>>,
    machine_ids: Option<Vec<Uuid>>,
    namespaces: Option<Vec<String>>,
    time_range: Option<(Option<SystemTime>, Option<SystemTime>)>,
    size_range: Option<(Option<u64>, Option<u64>)>,
    limit: Option<usize>,
    sort_by: Option<SortBy>,
    include_disposed: bool,
}

/// Filter for journal sources
#[derive(Debug, Clone)]
enum SourceFilter {
    /// Match any system journal
    AnySystem,
    /// Match any user journal
    AnyUser,
    /// Match a specific user's journals
    SpecificUser(u32),
    /// Match any remote journal
    AnyRemote,
    /// Match a specific remote host's journals
    SpecificRemote(String),
}

impl<'a> RegistryQuery<'a> {
    fn new(registry: &'a JournalRegistry) -> Self {
        Self {
            registry,
            source_filter: None,
            statuses: None,
            machine_ids: None,
            namespaces: None,
            time_range: None,
            size_range: None,
            limit: None,
            sort_by: None,
            include_disposed: false,
        }
    }

    /// Filter for system journals only
    pub fn system(mut self) -> Self {
        self.source_filter = Some(SourceFilter::AnySystem);
        self
    }

    /// Filter for user journals (optionally for a specific user)
    pub fn user(mut self, uid: Option<u32>) -> Self {
        self.source_filter = Some(match uid {
            Some(uid) => SourceFilter::SpecificUser(uid),
            None => SourceFilter::AnyUser,
        });
        self
    }

    /// Filter for remote journals (optionally for a specific host)
    pub fn remote(mut self, host: Option<String>) -> Self {
        self.source_filter = Some(match host {
            Some(host) => SourceFilter::SpecificRemote(host),
            None => SourceFilter::AnyRemote,
        });
        self
    }

    /// Filter by journal status
    fn status(mut self, status: JournalFileStatus) -> Self {
        self.statuses.get_or_insert_with(Vec::new).push(status);
        self
    }

    /// Only include active journals
    pub fn active_only(self) -> Self {
        self.status(JournalFileStatus::Active)
    }

    /// Include disposed/corrupted journals
    pub fn include_disposed(mut self) -> Self {
        self.include_disposed = true;
        self
    }

    /// Filter by machine ID
    pub fn machine(mut self, machine_id: Uuid) -> Self {
        self.machine_ids
            .get_or_insert_with(Vec::new)
            .push(machine_id);
        self
    }

    /// Filter by namespace
    pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespaces
            .get_or_insert_with(Vec::new)
            .push(namespace.into());
        self
    }

    /// Filter by modification time range
    pub fn modified_between(mut self, start: SystemTime, end: SystemTime) -> Self {
        self.time_range = Some((Some(start), Some(end)));
        self
    }

    /// Filter by files modified after a specific time
    pub fn modified_after(mut self, time: SystemTime) -> Self {
        self.time_range = Some((Some(time), None));
        self
    }

    /// Filter by files modified before a specific time
    pub fn modified_before(mut self, time: SystemTime) -> Self {
        self.time_range = Some((None, Some(time)));
        self
    }

    /// Filter by file size range
    pub fn size_between(mut self, min: u64, max: u64) -> Self {
        self.size_range = Some((Some(min), Some(max)));
        self
    }

    /// Filter by minimum file size
    pub fn min_size(mut self, size: u64) -> Self {
        self.size_range = Some((Some(size), None));
        self
    }

    /// Filter by maximum file size
    pub fn max_size(mut self, size: u64) -> Self {
        self.size_range = Some((None, Some(size)));
        self
    }

    /// Sort results
    pub fn sort_by(mut self, sort: SortBy) -> Self {
        self.sort_by = Some(sort);
        self
    }

    /// Limit the number of results
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Execute the query and return matching files
    pub fn execute(&self) -> Vec<RegistryFile> {
        let mut results: Vec<RegistryFile> = self
            .registry
            .files
            .read()
            .values()
            .filter(|file| self.matches(file))
            .cloned()
            .collect();

        // Apply sorting
        if let Some(sort_by) = &self.sort_by {
            match sort_by {
                SortBy::Modified(order) => match order {
                    SortOrder::Ascending => results.sort_by_key(|f| f.modified),
                    SortOrder::Descending => results.sort_by_key(|f| std::cmp::Reverse(f.modified)),
                },
                SortBy::Size(order) => match order {
                    SortOrder::Ascending => results.sort_by_key(|f| f.size),
                    SortOrder::Descending => results.sort_by_key(|f| std::cmp::Reverse(f.size)),
                },
                SortBy::Path(order) => match order {
                    SortOrder::Ascending => results.sort_by(|a, b| a.path.cmp(&b.path)),
                    SortOrder::Descending => results.sort_by(|a, b| b.path.cmp(&a.path)),
                },
            }
        }

        // Apply limit
        if let Some(limit) = self.limit {
            results.truncate(limit);
        }

        results
    }

    /// Count matching files without returning them
    pub fn count(&self) -> usize {
        self.registry
            .files
            .read()
            .values()
            .filter(|file| self.matches(file))
            .count()
    }

    /// Get total size of matching files
    pub fn total_size(&self) -> u64 {
        self.registry
            .files
            .read()
            .values()
            .filter(|file| self.matches(file))
            .map(|f| f.size)
            .sum()
    }

    /// Check if any files match the query
    pub fn exists(&self) -> bool {
        self.registry
            .files
            .read()
            .values()
            .any(|file| self.matches(file))
    }

    /// Internal: Check if a file matches all filters
    fn matches(&self, file: &RegistryFile) -> bool {
        // Check if disposed files should be excluded
        if !self.include_disposed && file.info.is_disposed() {
            return false;
        }

        // Check source filter
        if let Some(ref filter) = self.source_filter {
            if !self.matches_source_filter(filter, file) {
                return false;
            }
        }

        // Check status
        if let Some(ref statuses) = self.statuses {
            if !statuses.contains(&file.info.status) {
                return false;
            }
        }

        // Check machine ID
        if let Some(ref ids) = self.machine_ids {
            match &file.info.machine_id {
                Some(id) if ids.contains(id) => {}
                _ => return false,
            }
        }

        // Check namespace
        if let Some(ref namespaces) = self.namespaces {
            match &file.info.namespace {
                Some(ns) if namespaces.contains(ns) => {}
                _ => return false,
            }
        }

        // Check time range
        if let Some((start, end)) = self.time_range {
            if let Some(start) = start {
                if file.modified < start {
                    return false;
                }
            }
            if let Some(end) = end {
                if file.modified > end {
                    return false;
                }
            }
        }

        // Check size range
        if let Some((min, max)) = self.size_range {
            if let Some(min) = min {
                if file.size < min {
                    return false;
                }
            }
            if let Some(max) = max {
                if file.size > max {
                    return false;
                }
            }
        }

        true
    }

    /// Check if a file matches the source filter
    fn matches_source_filter(&self, filter: &SourceFilter, file: &RegistryFile) -> bool {
        match filter {
            SourceFilter::AnySystem => file.info.is_system(),
            SourceFilter::AnyUser => file.info.is_user(),
            SourceFilter::SpecificUser(uid) => file.info.user_id() == Some(*uid),
            SourceFilter::AnyRemote => file.info.is_remote(),
            SourceFilter::SpecificRemote(host) => file.info.remote_host() == Some(host.as_str()),
        }
    }
}
