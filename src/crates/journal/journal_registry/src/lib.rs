use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use journal_file::index::HistogramIndex;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use regex::Regex;
use thiserror::Error;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

use std::convert::TryFrom;

#[derive(Debug, Error)]
pub enum SourceTypeError {
    #[error("Path contains invalid UTF-8")]
    InvalidUtf8,

    #[error("Path is empty")]
    EmptyPath,

    #[error("Cannot determine source type from path: {0}")]
    Indeterminate(PathBuf),
}

impl TryFrom<&Path> for SourceType {
    type Error = SourceTypeError;

    fn try_from(path: &Path) -> std::result::Result<Self, Self::Error> {
        // Check if path is empty
        if path.as_os_str().is_empty() {
            return Err(SourceTypeError::EmptyPath);
        }

        let path_str = path.to_string_lossy();

        // Determine the source type
        let source_type = if path_str.contains("/remote/") {
            SourceType::Remote
        } else if path_str.contains("/system") || path_str.contains("/system.journal") {
            SourceType::System
        } else if path_str.contains("/user") || path_str.contains("/user-") {
            SourceType::User
        } else if path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().contains('.'))
            .unwrap_or(false)
        {
            SourceType::Namespace
        } else {
            if !path_str.contains("/journal") && !path_str.ends_with(".journal") {
                return Err(SourceTypeError::Indeterminate(path.to_path_buf()));
            }
            SourceType::Other
        };

        Ok(source_type)
    }
}

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
    /// Full path to the journal file
    pub path: PathBuf,

    /// File size in bytes
    pub size: u64,

    /// Last modification time
    pub modified: SystemTime,

    /// Source type based on directory location
    pub source_type: SourceType,

    /// Machine ID or writer ID if extractable from filename
    pub machine_id: Option<String>,

    /// Sequence number if extractable from filename
    pub sequence_number: Option<u64>,

    /// First message timestamp if extractable from filename
    pub first_timestamp: Option<u64>,
}

/// Represents the histogram information for a systemd journal file
#[derive(Debug, Clone)]
pub struct RegistryFileHistogram {
    histogram_index: HistogramIndex,
    facet_entries: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceType {
    System,
    User,
    Remote,
    Namespace,
    Other,
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::User => write!(f, "user"),
            Self::Remote => write!(f, "remote"),
            Self::Namespace => write!(f, "namespace"),
            Self::Other => write!(f, "other"),
        }
    }
}

impl RegistryFile {
    /// Parse a journal file path and extract metadata
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let metadata = path.metadata().map_err(|e| RegistryError::MetadataError {
            path: path.to_path_buf(),
            source: e,
        })?;

        let source_type = SourceType::try_from(path)
            .map_err(|e| RegistryError::InvalidFilename(e.to_string()))?;

        let (machine_id, sequence_number, first_timestamp) = Self::parse_filename(path);

        Ok(Self {
            path: path.to_path_buf(),
            size: metadata.len(),
            modified: metadata.modified()?,
            source_type,
            machine_id,
            sequence_number,
            first_timestamp,
        })
    }

    fn parse_filename(path: &Path) -> (Option<String>, Option<u64>, Option<u64>) {
        let jr = Regex::new(
                r"(?:^|/)(?:[^@/]+@)?(?P<machine>[a-f0-9]+)-(?P<seq>[a-f0-9]+)-(?P<ts>[a-f0-9]+)\.journal(?:~)?$"
            ).unwrap();

        path.file_name()
            .and_then(|n| n.to_str())
            .and_then(|filename| {
                jr.captures(filename).map(|caps| {
                    let machine_id = caps.name("machine").map(|m| m.as_str().to_string());
                    let sequence_number = caps
                        .name("seq")
                        .and_then(|m| u64::from_str_radix(m.as_str(), 16).ok());
                    let first_timestamp = caps
                        .name("ts")
                        .and_then(|m| u64::from_str_radix(m.as_str(), 16).ok());

                    (machine_id, sequence_number, first_timestamp)
                })
            })
            .unwrap_or((None, None, None))
    }

    /// Check if a path looks like a journal file
    pub fn is_journal_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext == "journal" || ext == "journal~")
            .unwrap_or(false)
    }

    // pub fn histogram(&self) -> RegistryFileHistogram {}
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

/// A builder for querying journal files with various filters
pub struct RegistryQuery<'a> {
    registry: &'a JournalRegistry,
    source_types: Option<Vec<SourceType>>,
    machine_ids: Option<Vec<String>>,
    time_range: Option<(Option<SystemTime>, Option<SystemTime>)>,
    size_range: Option<(Option<u64>, Option<u64>)>,
    path_pattern: Option<Regex>,
    limit: Option<usize>,
    sort_by: Option<SortBy>,
}

#[derive(Debug, Clone, Copy)]
pub enum SortBy {
    Modified(SortOrder),
    Size(SortOrder),
    Path(SortOrder),
    Sequence(SortOrder),
}

#[derive(Debug, Clone, Copy)]
pub enum SortOrder {
    Ascending,
    Descending,
}

impl<'a> RegistryQuery<'a> {
    fn new(registry: &'a JournalRegistry) -> Self {
        Self {
            registry,
            source_types: None,
            machine_ids: None,
            time_range: None,
            size_range: None,
            path_pattern: None,
            limit: None,
            sort_by: None,
        }
    }

    /// Filter by source type(s)
    pub fn source(mut self, source_type: SourceType) -> Self {
        self.source_types
            .get_or_insert_with(Vec::new)
            .push(source_type);
        self
    }

    /// Filter by multiple source types
    pub fn sources(mut self, types: impl IntoIterator<Item = SourceType>) -> Self {
        self.source_types = Some(types.into_iter().collect());
        self
    }

    /// Filter by machine ID
    pub fn machine(mut self, machine_id: impl Into<String>) -> Self {
        self.machine_ids
            .get_or_insert_with(Vec::new)
            .push(machine_id.into());
        self
    }

    /// Filter by multiple machine IDs
    pub fn machines(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.machine_ids = Some(ids.into_iter().map(Into::into).collect());
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
                SortBy::Sequence(order) => match order {
                    SortOrder::Ascending => results.sort_by_key(|f| f.sequence_number.unwrap_or(0)),
                    SortOrder::Descending => {
                        results.sort_by_key(|f| std::cmp::Reverse(f.sequence_number.unwrap_or(0)))
                    }
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
        // Check source type
        if let Some(ref types) = self.source_types {
            if !types.contains(&file.source_type) {
                return false;
            }
        }

        // Check machine ID
        if let Some(ref ids) = self.machine_ids {
            match &file.machine_id {
                Some(id) if ids.contains(id) => {}
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

        // Check path pattern
        if let Some(ref pattern) = self.path_pattern {
            if !pattern.is_match(&file.path.to_string_lossy()) {
                return false;
            }
        }

        true
    }
}
