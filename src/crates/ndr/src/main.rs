use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use parking_lot::RwLock;
use thiserror::Error;
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

    fn parse_filename(path: &Path) -> (Option<String>, Option<u64>, Option<u64>) {
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => return (None, None, None),
        };

        // Pattern: name@machine_id-seqnum-timestamp.journal
        if let Some(at_pos) = filename.find('@') {
            let after_at = &filename[at_pos + 1..];

            // Look for the .journal extension
            if let Some(dot_pos) = after_at.rfind(".journal") {
                let parts = &after_at[..dot_pos];
                let segments: Vec<&str> = parts.split('-').collect();

                if segments.len() >= 3 {
                    let machine_id = Some(segments[0].to_string());
                    let sequence_number = u64::from_str_radix(segments[1], 16).ok();
                    let first_timestamp = u64::from_str_radix(segments[2], 16).ok();

                    return (machine_id, sequence_number, first_timestamp);
                }
            }
        }

        (None, None, None)
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

/// Registry of journal files with notify-based monitoring
pub struct JournalRegistry {
    /// Currently tracked journal files
    files: Arc<RwLock<HashMap<PathBuf, JournalFile>>>,

    /// Directories being monitored
    watch_dirs: Arc<RwLock<HashSet<PathBuf>>>,

    /// Event listeners
    event_tx: Arc<RwLock<Vec<tokio::sync::mpsc::UnboundedSender<JournalEvent>>>>,
}

/// Separate watcher struct to handle file system events
pub struct JournalWatcher {
    _watcher: RecommendedWatcher,
    event_rx: crossbeam_channel::Receiver<notify::Result<Event>>,
    registry: Arc<JournalRegistry>,
}

impl JournalRegistry {
    /// Create a new journal registry
    pub fn new() -> Result<Self> {
        Ok(Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            watch_dirs: Arc::new(RwLock::new(HashSet::new())),
            event_tx: Arc::new(RwLock::new(Vec::new())),
        })
    }
    
    /// Create a watcher for this registry
    pub fn create_watcher(self: Arc<Self>) -> Result<JournalWatcher> {
        let (tx, rx) = crossbeam_channel::unbounded();
        
        let watcher = RecommendedWatcher::new(
            move |res| {
                if tx.send(res).is_err() {
                    // Channel closed, stop watching
                }
            },
            Config::default(),
        ).map_err(JournalRegistryError::WatcherInit)?;
        
        Ok(JournalWatcher {
            _watcher: watcher,
            event_rx: rx,
            registry: self,
        })
    }

    /// Subscribe to registry events
    pub fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<JournalEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.event_tx.write().push(tx);
        rx
    }

    /// Emit an event to all subscribers
    fn emit_event(&self, event: JournalEvent) {
        let mut txs = self.event_tx.write();
        txs.retain(|tx| tx.send(event.clone()).is_ok());
    }


    /// Scan directory for existing files (used during initial setup)
    pub fn scan_directory(&self, dir: &Path) -> Result<()> {
        // Resolve symlinks
        let dir = match dir.canonicalize() {
            Ok(path) => path,
            Err(e) => {
                warn!("Cannot canonicalize path {:?}: {}", dir, e);
                return Ok(());
            }
        };

        // Check if already watching
        if self.watch_dirs.read().contains(&dir) {
            return Ok(());
        }

        // Walk directory tree to find existing files
        for entry in WalkDir::new(&dir)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if entry.file_type().is_dir() {
                self.add_directory(path)?;
            } else if JournalFile::is_journal_file(path) {
                self.add_journal_file(path)?;
            }
        }

        Ok(())
    }

    fn watch_directory(&self, _dir: &Path) -> Result<()> {
        // Directory watching is now handled at the registry level with notify
        // Individual directory tracking is maintained for bookkeeping
        Ok(())
    }

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

    fn handle_event(&self, event: Event) -> Result<()> {
        for path in event.paths {
            match event.kind {
                EventKind::Create(_) => {
                    if path.is_dir() {
                        info!("New directory created: {:?}", path);
                        self.add_directory(&path)?;
                    } else if JournalFile::is_journal_file(&path) {
                        self.add_journal_file(&path)?;
                    }
                }
                EventKind::Remove(_) => {
                    if path.is_dir() {
                        self.remove_directory(&path)?;
                    } else {
                        if self.files.write().remove(&path).is_some() {
                            debug!("Removed journal file: {:?}", path);
                            self.emit_event(JournalEvent::FileRemoved(path));
                        }
                    }
                }
                EventKind::Modify(_) => {
                    if !path.is_dir() && JournalFile::is_journal_file(&path) {
                        self.add_journal_file(&path)?;
                    }
                }
                _ => {} // Ignore other event types
            }
        }
        Ok(())
    }
    
    fn add_directory(&self, dir: &Path) -> Result<()> {
        let mut watch_dirs = self.watch_dirs.write();
        if !watch_dirs.contains(dir) {
            watch_dirs.insert(dir.to_path_buf());
            debug!("Added directory: {:?}", dir);
            self.emit_event(JournalEvent::DirectoryAdded(dir.to_path_buf()));
        }
        Ok(())
    }

    fn remove_directory(&self, dir: &Path) -> Result<()> {
        self.watch_dirs.write().remove(dir);

        // Remove all files under this directory
        let mut files = self.files.write();
        let removed_files: Vec<_> = files
            .keys()
            .filter(|path| path.starts_with(dir))
            .cloned()
            .collect();

        for path in removed_files {
            files.remove(&path);
            self.emit_event(JournalEvent::FileRemoved(path));
        }

        info!("Removed directory watch: {:?}", dir);
        self.emit_event(JournalEvent::DirectoryRemoved(dir.to_path_buf()));

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
}

impl JournalWatcher {
    /// Start watching directories
    pub fn start_watching(&mut self, dirs: &[PathBuf]) -> Result<()> {
        // Add watches for all directories
        for dir in dirs {
            self._watcher.watch(dir, RecursiveMode::Recursive)
                .map_err(|e| JournalRegistryError::WatchError {
                    path: dir.to_path_buf(),
                    source: e,
                })?;
            
            // Scan existing files
            self.registry.scan_directory(dir)?;
        }
        Ok(())
    }
    
    /// Process notify events (non-blocking)
    pub fn process_events(&self) -> Result<()> {
        // Try to receive events without blocking
        while let Ok(event_result) = self.event_rx.try_recv() {
            match event_result {
                Ok(event) => self.registry.handle_event(event)?,
                Err(e) => error!("File watch error: {}", e),
            }
        }
        Ok(())
    }
}

// Example main function
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create the registry
    let registry = Arc::new(JournalRegistry::new()?);

    // Subscribe to events
    let mut event_rx = registry.subscribe();

    // Create watcher
    let mut watcher = registry.clone().create_watcher()?;

    // Add directories to monitor
    let dirs = vec![
        PathBuf::from("/var/log/journal"),
        PathBuf::from("/run/log/journal"),
    ];

    info!("Starting journal monitor for directories: {:?}", dirs);
    watcher.start_watching(&dirs)?;

    // Print initial files
    let files = registry.get_files();
    println!("\n=== Initial journal files found: {} ===", files.len());
    for file in files {
        println!("  [{:8}] {:?}", file.source_type, file.path);
        if let Some(machine_id) = &file.machine_id {
            println!("             Machine ID: {}", machine_id);
        }
        if let Some(seq) = file.sequence_number {
            println!("             Sequence: 0x{:x}", seq);
        }
        println!(
            "             Size: {} bytes, Modified: {:?}",
            file.size, file.modified
        );
    }
    println!();

    // Spawn task to process notify events
    let watcher = Arc::new(parking_lot::RwLock::new(watcher));
    let watcher_clone = Arc::clone(&watcher);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(250));
        loop {
            interval.tick().await;
            if let Err(e) = watcher_clone.write().process_events() {
                error!("Error processing events: {}", e);
            }
        }
    });

    // Listen for events
    println!("=== Monitoring for changes (press Ctrl+C to exit) ===\n");
    while let Some(event) = event_rx.recv().await {
        match event {
            JournalEvent::FileAdded(file) => {
                println!("📄 NEW: [{:8}] {:?}", file.source_type, file.path);
                if let Some(machine_id) = &file.machine_id {
                    println!("        Machine ID: {}", machine_id);
                }
            }
            JournalEvent::FileModified(file) => {
                println!(
                    "📝 MOD: [{:8}] {:?} (size: {} bytes)",
                    file.source_type, file.path, file.size
                );
            }
            JournalEvent::FileRemoved(path) => {
                println!("🗑️  DEL: {:?}", path);
            }
            JournalEvent::DirectoryAdded(path) => {
                println!("📁 NEW DIR: {:?}", path);
            }
            JournalEvent::DirectoryRemoved(path) => {
                println!("📁 DEL DIR: {:?}", path);
            }
        }
    }

    Ok(())
}
