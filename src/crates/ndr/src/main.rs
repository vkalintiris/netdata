use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use regex::Regex;
use thiserror::Error;
use tokio::sync::mpsc;
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

impl TryFrom<&Path> for JournalSourceType {
    type Error = SourceTypeError;

    fn try_from(path: &Path) -> std::result::Result<Self, Self::Error> {
        // Check if path is empty
        if path.as_os_str().is_empty() {
            return Err(SourceTypeError::EmptyPath);
        }

        let path_str = path.to_string_lossy();

        // Determine the source type
        let source_type = if path_str.contains("/remote/") {
            JournalSourceType::Remote
        } else if path_str.contains("/system") || path_str.contains("/system.journal") {
            JournalSourceType::System
        } else if path_str.contains("/user") || path_str.contains("/user-") {
            JournalSourceType::User
        } else if path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().contains('.'))
            .unwrap_or(false)
        {
            JournalSourceType::Namespace
        } else {
            // Instead of defaulting to "Other", you could make this explicit
            // by checking if it's actually a valid journal path
            if !path_str.contains("/journal") && !path_str.ends_with(".journal") {
                return Err(SourceTypeError::Indeterminate(path.to_path_buf()));
            }
            JournalSourceType::Other
        };

        Ok(source_type)
    }
}

#[derive(Debug, Error)]
pub enum JournalRegistryError {
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

type Result<T> = std::result::Result<T, JournalRegistryError>;

/// Represents a systemd journal file with parsed metadata
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFile {
    /// Full path to the journal file
    pub path: PathBuf,

    /// File size in bytes
    pub size: u64,

    /// Last modification time
    pub modified: SystemTime,

    /// Source type based on directory location
    pub source_type: JournalSourceType,

    /// Machine ID or writer ID if extractable from filename
    pub machine_id: Option<String>,

    /// Sequence number if extractable from filename
    pub sequence_number: Option<u64>,

    /// First message timestamp if extractable from filename
    pub first_timestamp: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JournalSourceType {
    System,
    User,
    Remote,
    Namespace,
    Other,
}

impl std::fmt::Display for JournalSourceType {
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

impl JournalFile {
    /// Parse a journal file path and extract metadata
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let metadata = path
            .metadata()
            .map_err(|e| JournalRegistryError::MetadataError {
                path: path.to_path_buf(),
                source: e,
            })?;

        let source_type = JournalSourceType::try_from(path)
            .map_err(|e| JournalRegistryError::InvalidFilename(e.to_string()))?;

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
}

/// Events emitted by the journal registry
#[derive(Debug, Clone)]
pub enum JournalEvent {
    FileAdded(JournalFile),
    FileModified(JournalFile),
    FileRemoved(PathBuf),
    DirectoryAdded(PathBuf),
    DirectoryRemoved(PathBuf),
}

/// Internal watcher state
struct WatcherState {
    watcher: RecommendedWatcher,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Registry of journal files with automatic file system monitoring
pub struct JournalRegistry {
    /// Currently tracked journal files
    files: Arc<RwLock<HashMap<PathBuf, JournalFile>>>,

    /// Directories being monitored
    watch_dirs: Arc<RwLock<HashSet<PathBuf>>>,

    /// Event listeners
    event_tx: Arc<RwLock<Vec<mpsc::UnboundedSender<JournalEvent>>>>,

    /// Internal watcher state
    watcher_state: Arc<RwLock<Option<WatcherState>>>,
}

impl JournalRegistry {
    /// Create a new journal registry that automatically starts monitoring
    pub fn new() -> Result<Self> {
        let registry = Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            watch_dirs: Arc::new(RwLock::new(HashSet::new())),
            event_tx: Arc::new(RwLock::new(Vec::new())),
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
        .map_err(JournalRegistryError::WatcherInit)?;

        // Clone what we need for the background task
        let files = Arc::clone(&self.files);
        let watch_dirs = Arc::clone(&self.watch_dirs);
        let event_tx = Arc::clone(&self.event_tx);

        // Spawn background task to process events
        let task_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(250));
            loop {
                interval.tick().await;

                while let Ok(event_result) = rx.try_recv() {
                    match event_result {
                        Ok(event) => {
                            if let Err(e) =
                                Self::handle_event_internal(&files, &watch_dirs, &event_tx, event)
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
                .map_err(|e| JournalRegistryError::WatchError {
                    path: canonical_dir.clone(),
                    source: e,
                })?;
        }

        // Scan for existing files
        self.scan_directory(&canonical_dir)?;

        // Add to watch list
        self.watch_dirs.write().insert(canonical_dir.clone());
        self.emit_event(JournalEvent::DirectoryAdded(canonical_dir));

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
            self.emit_event(JournalEvent::FileRemoved(path));
        }

        self.emit_event(JournalEvent::DirectoryRemoved(canonical_dir));
        Ok(())
    }

    /// Get a snapshot of all current journal files
    pub fn get_files(&self) -> Vec<JournalFile> {
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

            if !entry.file_type().is_dir() && JournalFile::is_journal_file(path) {
                self.add_journal_file(path)?;
            }
        }
        Ok(())
    }

    /// Internal: Add or update a journal file
    fn add_journal_file(&self, path: &Path) -> Result<()> {
        match JournalFile::from_path(path) {
            Ok(journal_file) => {
                let is_new = !self.files.read().contains_key(path);
                let mut files = self.files.write();
                files.insert(path.to_path_buf(), journal_file.clone());

                if is_new {
                    debug!("Added journal file: {:?}", path);
                    self.emit_event(JournalEvent::FileAdded(journal_file));
                } else {
                    debug!("Modified journal file: {:?}", path);
                    self.emit_event(JournalEvent::FileModified(journal_file));
                }
                Ok(())
            }
            Err(e) => {
                warn!("Failed to add journal file {:?}: {}", path, e);
                Ok(())
            }
        }
    }

    /// Internal: Emit an event to all subscribers
    fn emit_event(&self, event: JournalEvent) {
        let mut txs = self.event_tx.write();
        txs.retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Internal: Handle filesystem events
    fn handle_event_internal(
        files: &Arc<RwLock<HashMap<PathBuf, JournalFile>>>,
        watch_dirs: &Arc<RwLock<HashSet<PathBuf>>>,
        event_tx: &Arc<RwLock<Vec<mpsc::UnboundedSender<JournalEvent>>>>,
        event: Event,
    ) -> Result<()> {
        for path in event.paths {
            match event.kind {
                EventKind::Create(_) => {
                    if path.is_dir() {
                        info!("New directory created: {:?}", path);
                        watch_dirs.write().insert(path.clone());
                        Self::emit_event_internal(event_tx, JournalEvent::DirectoryAdded(path));
                    } else if JournalFile::is_journal_file(&path) {
                        if let Ok(journal_file) = JournalFile::from_path(&path) {
                            let is_new = !files.read().contains_key(&path);
                            files.write().insert(path.clone(), journal_file.clone());

                            if is_new {
                                debug!("Added journal file: {:?}", path);
                                Self::emit_event_internal(
                                    event_tx,
                                    JournalEvent::FileAdded(journal_file),
                                );
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
                            Self::emit_event_internal(
                                event_tx,
                                JournalEvent::FileRemoved(file_path),
                            );
                        }

                        Self::emit_event_internal(event_tx, JournalEvent::DirectoryRemoved(path));
                    } else if files.write().remove(&path).is_some() {
                        debug!("Removed journal file: {:?}", path);
                        Self::emit_event_internal(event_tx, JournalEvent::FileRemoved(path));
                    }
                }
                EventKind::Modify(_) => {
                    if !path.is_dir() && JournalFile::is_journal_file(&path) {
                        if let Ok(journal_file) = JournalFile::from_path(&path) {
                            files.write().insert(path.clone(), journal_file.clone());
                            debug!("Modified journal file: {:?}", path);
                            Self::emit_event_internal(
                                event_tx,
                                JournalEvent::FileModified(journal_file),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Internal: Emit event helper
    fn emit_event_internal(
        event_tx: &Arc<RwLock<Vec<mpsc::UnboundedSender<JournalEvent>>>>,
        event: JournalEvent,
    ) {
        let mut txs = event_tx.write();
        txs.retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Create a new query builder for this registry
    pub fn query(&self) -> JournalQuery {
        JournalQuery::new(self)
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
pub struct JournalQuery<'a> {
    registry: &'a JournalRegistry,
    source_types: Option<Vec<JournalSourceType>>,
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

impl<'a> JournalQuery<'a> {
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
    pub fn source(mut self, source_type: JournalSourceType) -> Self {
        self.source_types
            .get_or_insert_with(Vec::new)
            .push(source_type);
        self
    }

    /// Filter by multiple source types
    pub fn sources(mut self, types: impl IntoIterator<Item = JournalSourceType>) -> Self {
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
    pub fn execute(&self) -> Vec<JournalFile> {
        let mut results: Vec<JournalFile> = self
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
    fn matches(&self, file: &JournalFile) -> bool {
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

use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create the registry - it automatically starts monitoring
    let registry = JournalRegistry::new()?;
    info!("Journal registry initialized");

    // Add directories to monitor
    let dirs = vec!["/var/log/journal", "/run/log/journal"];

    for dir in &dirs {
        match registry.add_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    // Give it a moment to scan existing files
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Display initial statistics
    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    // Query 1: Get system journal files sorted by size
    println!("\n=== Query 1: System Journal Files (sorted by size) ===");
    let system_files = registry
        .query()
        .source(JournalSourceType::System)
        .sort_by(SortBy::Size(SortOrder::Descending))
        .execute();

    println!("Found {} system journal files:", system_files.len());
    for (idx, file) in system_files.iter().take(5).enumerate() {
        println!(
            "  [{}] {} ({:.2} MB) - modified: {:?}",
            idx,
            file.path.display(),
            file.size as f64 / (1024.0 * 1024.0),
            file.modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| {
                    let secs = d.as_secs();
                    format!(
                        "{} hours ago",
                        (SystemTime::now()
                            .duration_since(file.modified)
                            .unwrap_or_default()
                            .as_secs()
                            / 3600)
                    )
                })
                .unwrap_or_else(|_| "unknown".to_string())
        );
    }

    // Recent large files (modified in last 24 hours, > 1MB)
    println!("\n=== Recent Large Files (last 24h, >1MB) ===");
    let recent_large = registry
        .query()
        .modified_after(SystemTime::now() - Duration::from_hours(24))
        .min_size(1024 * 1024) // 1MB
        .sort_by(SortBy::Modified(SortOrder::Descending))
        .limit(10)
        .execute();

    if recent_large.is_empty() {
        println!("No large files modified in the last 24 hours");
    } else {
        println!(
            "Found {} large files modified recently:",
            recent_large.len()
        );
        for file in &recent_large {
            println!(
                "  {} ({:.2} MB) - {}",
                file.path.file_name().unwrap_or_default().to_string_lossy(),
                file.size as f64 / (1024.0 * 1024.0),
                file.source_type
            );
        }
    }

    // Group files by source type
    println!("\n=== Files by Source Type ===");
    for source_type in &[
        JournalSourceType::System,
        JournalSourceType::User,
        JournalSourceType::Remote,
        JournalSourceType::Namespace,
        JournalSourceType::Other,
    ] {
        let files = registry.query().source(*source_type).execute();
        let total_size = registry.query().source(*source_type).total_size();

        if !files.is_empty() {
            println!(
                "  {:10} - {} files, {:.2} MB total",
                source_type.to_string(),
                files.len(),
                total_size as f64 / (1024.0 * 1024.0)
            );
        }
    }

    // Find files by machine ID (if any exist)
    println!("\n=== Files by Machine ID ===");
    let all_files = registry.query().execute();
    let machine_ids: std::collections::HashSet<_> = all_files
        .iter()
        .filter_map(|f| f.machine_id.as_ref())
        .cloned()
        .collect();

    if machine_ids.is_empty() {
        println!("No files with machine IDs found");
    } else {
        for (idx, machine_id) in machine_ids.iter().take(3).enumerate() {
            let machine_files = registry
                .query()
                .machine(machine_id)
                .sort_by(SortBy::Sequence(SortOrder::Ascending))
                .execute();

            println!(
                "  Machine {} ({}...): {} files",
                idx + 1,
                &machine_id[..8.min(machine_id.len())],
                machine_files.len()
            );
        }
    }

    Ok(())
}
