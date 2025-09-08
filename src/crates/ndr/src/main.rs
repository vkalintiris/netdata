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

/// Parser for journal filenames using regex
struct JournalFilenameParser {
    // Matches: name@machine_id-seqnum-timestamp.journal
    full_pattern: Regex,
    // Matches: system@machine_id-seqnum-timestamp.journal (without name prefix)
    system_pattern: Regex,
}

impl JournalFilenameParser {
    fn new() -> Self {
        Self {
            full_pattern: Regex::new(
                r"^(?P<name>[^@]+)@(?P<machine>[a-f0-9]+)-(?P<seq>[a-f0-9]+)-(?P<ts>[a-f0-9]+)\.journal(?:~)?$"
            ).unwrap(),
            system_pattern: Regex::new(
                r"^system@(?P<machine>[a-f0-9]+)-(?P<seq>[a-f0-9]+)-(?P<ts>[a-f0-9]+)\.journal(?:~)?$"
            ).unwrap(),
        }
    }

    fn parse(&self, filename: &str) -> (Option<String>, Option<u64>, Option<u64>) {
        if let Some(caps) = self.full_pattern.captures(filename) {
            let machine_id = caps.name("machine").map(|m| m.as_str().to_string());
            let sequence_number = caps
                .name("seq")
                .and_then(|m| u64::from_str_radix(m.as_str(), 16).ok());
            let first_timestamp = caps
                .name("ts")
                .and_then(|m| u64::from_str_radix(m.as_str(), 16).ok());

            return (machine_id, sequence_number, first_timestamp);
        }

        // Try simpler patterns for special cases
        if filename == "system.journal" || filename == "user.journal" {
            return (None, None, None);
        }

        (None, None, None)
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

        let source_type = Self::determine_source_type(path);

        // Use regex parser for filename parsing
        let parser = JournalFilenameParser::new();
        let (machine_id, sequence_number, first_timestamp) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|filename| parser.parse(filename))
            .unwrap_or((None, None, None));

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

    fn determine_source_type(path: &Path) -> JournalSourceType {
        let path_str = path.to_string_lossy();

        if path_str.contains("/remote/") {
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
            JournalSourceType::Other
        }
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

    /// Get journal files filtered by source type
    pub fn get_files_by_source(&self, source_type: JournalSourceType) -> Vec<JournalFile> {
        self.files
            .read()
            .values()
            .filter(|f| f.source_type == source_type)
            .cloned()
            .collect()
    }

    /// Get journal files within a time range
    pub fn get_files_in_range(&self, start: SystemTime, end: SystemTime) -> Vec<JournalFile> {
        self.files
            .read()
            .values()
            .filter(|f| f.modified >= start && f.modified <= end)
            .cloned()
            .collect()
    }

    /// Get journal files by machine ID
    pub fn get_files_by_machine(&self, machine_id: &str) -> Vec<JournalFile> {
        self.files
            .read()
            .values()
            .filter(|f| f.machine_id.as_deref() == Some(machine_id))
            .cloned()
            .collect()
    }

    /// Get total size of all journal files
    pub fn get_total_size(&self) -> u64 {
        self.files.read().values().map(|f| f.size).sum()
    }

    /// Get count of files by source type
    pub fn get_file_counts(&self) -> HashMap<JournalSourceType, usize> {
        let mut counts = HashMap::new();
        for file in self.files.read().values() {
            *counts.entry(file.source_type).or_insert(0) += 1;
        }
        counts
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
    let all_files = registry.get_files();
    println!("Total files: {}", all_files.len());

    Ok(())
}
