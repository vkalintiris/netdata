//! Journal catalog functionality with file monitoring and metadata tracking

#![allow(unused_imports)]

use async_trait::async_trait;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_schema::HttpAccess;
use rt::FunctionHandler;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::RwLock;
use tracing::{error, info, instrument, trace, warn};

/*
 * Cache Backend
*/

/// Generic cache abstraction supporting both in-memory and disk-backed storage
mod cache_backend {
    use foyer::{HybridCache, StorageKey, StorageValue};
    use std::collections::HashMap;
    use std::hash::Hash;
    use std::sync::{Arc, RwLock};
    use thiserror::Error;

    /// Errors that can occur with cache operations
    #[derive(Debug, Error)]
    pub enum CacheError {
        /// Error from the foyer cache
        #[error("Cache error: {0}")]
        Foyer(#[from] foyer::Error),

        /// Lock poisoning error
        #[error("Lock poisoned: {0}")]
        LockPoisoned(String),
    }

    /// A specialized Result type for cache operations
    pub type Result<T> = std::result::Result<T, CacheError>;

    /// An enum that can hold either a foyer HybridCache or a standard HashMap.
    /// This allows runtime selection between an evicting cache and a non-evicting store.
    pub enum Cache<K, V>
    where
        K: Hash + Eq + StorageKey,
        V: StorageValue,
    {
        /// Evicting cache backed by memory and optionally disk
        Foyer(HybridCache<K, V>),
        /// Non-evicting in-memory store
        HashMap(Arc<RwLock<HashMap<K, V>>>),
    }

    impl<K, V> Cache<K, V>
    where
        K: Hash + Eq + StorageKey + Clone,
        V: StorageValue + Clone,
    {
        /// Create a cache backed by a foyer HybridCache instance
        pub fn with_foyer(cache: HybridCache<K, V>) -> Self {
            Cache::Foyer(cache)
        }

        /// Create a cache backed by a HashMap instance
        pub fn with_hashmap(map: HashMap<K, V>) -> Self {
            Cache::HashMap(Arc::new(RwLock::new(map)))
        }

        /// Get a value from the cache
        pub async fn get(&self, key: &K) -> Result<Option<V>> {
            match self {
                Cache::Foyer(cache) => Ok(cache.get(key).await?.map(|entry| entry.value().clone())),
                Cache::HashMap(map) => {
                    let guard = map
                        .read()
                        .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                    Ok(guard.get(key).cloned())
                }
            }
        }

        /// Insert a key-value pair into the cache
        pub fn insert(&self, key: K, value: V) -> Result<()> {
            match self {
                Cache::Foyer(cache) => {
                    cache.insert(key, value);
                    Ok(())
                }
                Cache::HashMap(map) => {
                    let mut guard = map
                        .write()
                        .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                    guard.insert(key, value);
                    Ok(())
                }
            }
        }

        /// Remove a key from the cache
        pub fn remove(&self, key: &K) -> Result<()> {
            match self {
                Cache::Foyer(cache) => {
                    cache.remove(key);
                    Ok(())
                }
                Cache::HashMap(map) => {
                    let mut guard = map
                        .write()
                        .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                    guard.remove(key);
                    Ok(())
                }
            }
        }

        /// Check if the cache contains a key
        pub fn contains(&self, key: &K) -> Result<bool> {
            match self {
                Cache::Foyer(cache) => Ok(cache.contains(key)),
                Cache::HashMap(map) => {
                    let guard = map
                        .read()
                        .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                    Ok(guard.contains_key(key))
                }
            }
        }
    }

    impl<K, V> Clone for Cache<K, V>
    where
        K: Hash + Eq + StorageKey,
        V: StorageValue,
    {
        fn clone(&self) -> Self {
            match self {
                Cache::Foyer(cache) => Cache::Foyer(cache.clone()),
                Cache::HashMap(map) => Cache::HashMap(map.clone()),
            }
        }
    }
}

/*
 * Monitor
 */

/// File system monitoring with async event delivery
pub mod monitor {
    /// Error types for monitor operations
    pub mod error {
        use std::path::PathBuf;
        use thiserror::Error;

        /// Errors that can occur with `monitor`
        #[derive(Debug, Error)]
        pub enum MonitorError {
            /// Error from the file system watcher
            #[error("File system watcher error: {0}")]
            Notify(#[from] notify::Error),

            /// I/O error when reading or scanning directories
            #[error("I/O error: {0}")]
            Io(#[from] std::io::Error),

            /// Error from repository operations
            #[error("Repository error: {0}")]
            Repository(#[from] journal::repository::RepositoryError),

            /// Error when parsing a journal file path
            #[error("Failed to parse journal file path: {path}")]
            InvalidPath { path: String },

            /// Error when a path contains invalid UTF-8
            #[error("Path contains invalid UTF-8: {}", .path.display())]
            InvalidUtf8 { path: PathBuf },

            /// Channel closed error
            #[error("Registry channel closed")]
            ChannelClosed,
        }

        /// A specialized Result type for journal registry operations
        pub type Result<T> = std::result::Result<T, MonitorError>;
    }

    use error::Result;
    use journal::collections::HashSet;
    use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
    use std::path::Path;
    use tokio::sync::mpsc;

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
}

/*
 * Registry
 */

pub mod file_metadata {
    use journal::repository::File;

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
}

/// Journal file registry with automatic metadata tracking
pub mod registry {
    use super::monitor;
    use journal::collections::{HashMap, HashSet};
    use journal::repository::{File, Repository as BaseRepository, scan_journal_files};
    use notify::{
        Event,
        event::{EventKind, ModifyKind, RenameMode},
    };
    use tracing::{debug, error, info, trace, warn};

    use super::file_metadata::{FileInfo, TimeRange};
    pub use super::monitor::error::{MonitorError, Result};

    /// Repository that tracks journal files with metadata
    ///
    /// This wraps the base repository and automatically maintains time range metadata
    /// for each file. Metadata starts as Unknown and can be updated when computed.
    pub struct Repository {
        base: BaseRepository,
        file_metadata: HashMap<File, FileInfo>,
    }

    impl Repository {
        /// Create a new empty repository
        pub fn new() -> Self {
            Self {
                base: BaseRepository::default(),
                file_metadata: HashMap::default(),
            }
        }

        /// Insert a file to the repository
        pub fn insert(&mut self, file: File) -> Result<()> {
            let file_info = FileInfo {
                file: file.clone(),
                time_range: TimeRange::Unknown,
            };

            self.base.insert(file.clone())?;
            self.file_metadata.insert(file, file_info);

            Ok(())
        }

        /// Remove a file from the repository
        pub fn remove(&mut self, file: &File) -> Result<()> {
            self.base.remove(file)?;
            self.file_metadata.remove(file);
            Ok(())
        }

        /// Remove all files from a directory
        pub fn remove_directory(&mut self, path: &str) {
            self.base.remove_directory(path);
            self.file_metadata
                .retain(|file, _| file.dir().ok().map(|dir| dir != path).unwrap_or(true));
        }

        /// Find files in a time range
        pub fn find_files_in_range(&self, start: u32, end: u32) -> Vec<FileInfo> {
            let files: Vec<File> = self.base.find_files_in_range(start, end);

            files
                .into_iter()
                .map(|file| {
                    self.file_metadata.get(&file).cloned().unwrap_or(FileInfo {
                        file,
                        time_range: TimeRange::Unknown,
                    })
                })
                .collect()
        }

        /// Update time range metadata for a file
        pub fn update_file_info(&mut self, file_info: FileInfo) {
            let file = file_info.file.clone();
            self.file_metadata.insert(file, file_info);
        }

        /// Get information for a specific file
        pub fn get_file_info(&self, file: &File) -> Option<&FileInfo> {
            self.file_metadata.get(file)
        }
    }

    impl Default for Repository {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Coordinates file monitoring and repository management
    pub struct Registry {
        repository: Repository,
        watched_directories: HashSet<String>,
        monitor: monitor::Monitor,
    }

    impl Registry {
        /// Create a new registry with the given monitor
        pub fn new(monitor: monitor::Monitor) -> Self {
            Self {
                repository: Repository::new(),
                watched_directories: HashSet::default(),
                monitor,
            }
        }

        /// Watch a directory and perform initial scan
        pub fn watch_directory(&mut self, path: &str) -> Result<()> {
            if self.watched_directories.contains(path) {
                warn!("Directory {} is already being watched", path);
                return Ok(());
            }

            info!("Scanning directory: {}", path);
            let files = scan_journal_files(path)?;
            info!("Found {} journal files in {}", files.len(), path);

            // Start watching with notify
            self.monitor.watch_directory(path)?;
            self.watched_directories.insert(String::from(path));

            // Insert all discovered files into repository (automatically initializes metadata)
            for file in files {
                debug!("Adding file to repository: {:?}", file.path());

                if let Err(e) = self.repository.insert(file) {
                    error!("Failed to insert file into repository: {}", e);
                }
            }

            info!(
                "Now watching directory: {} (total directories: {})",
                path,
                self.watched_directories.len()
            );
            Ok(())
        }

        /// Stop watching a directory and clean up its files
        pub fn unwatch_directory(&mut self, path: &str) -> Result<()> {
            if !self.watched_directories.contains(path) {
                warn!("Directory {} is not being watched", path);
                return Ok(());
            }

            self.monitor.unwatch_directory(path)?;
            self.repository.remove_directory(path); // Handles both repository and metadata cleanup
            self.watched_directories.remove(path);

            info!("Stopped watching directory: {}", path);
            Ok(())
        }

        /// Process a file system event and update the repository
        pub fn process_event(&mut self, event: Event) -> Result<()> {
            match event.kind {
                EventKind::Create(_) => {
                    for path in &event.paths {
                        debug!("Adding file to repository: {:?}", path);

                        if let Some(file) = File::from_path(path) {
                            if let Err(e) = self.repository.insert(file) {
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
                            if let Err(e) = self.repository.remove(&file) {
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
                            if let Err(e) = self.repository.remove(&old_file) {
                                error!("Failed to remove old file: {}", e);
                            }
                        }

                        if let Some(new_file) = File::from_path(new_path) {
                            info!("Inserting new file: {:?}", new_file.path());
                            if let Err(e) = self.repository.insert(new_file) {
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
        pub fn find_files_in_range(&self, start: u32, end: u32) -> Vec<FileInfo> {
            self.repository.find_files_in_range(start, end)
        }

        /// Update time range of a file
        pub fn update_time_range(&mut self, file: &File, time_range: TimeRange) {
            let file_info = FileInfo {
                file: file.clone(),
                time_range,
            };
            self.repository.update_file_info(file_info);
        }

        /// Get file information
        pub fn get_file_info(&self, file: &File) -> Option<&FileInfo> {
            self.repository.get_file_info(file)
        }
    }
}

/*
 * CatalogFunction
*/

use tokio::sync::mpsc::UnboundedReceiver;

/// Request parameters for the catalog function
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CatalogRequest {}

/// Response from the catalog function
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogResponse {}

/// Inner state for CatalogFunction (enables cloning)
struct CatalogFunctionInner {
    registry: Arc<RwLock<registry::Registry>>,
}

/// Function handler that provides catalog information about journal files
#[derive(Clone)]
pub struct CatalogFunction {
    inner: Arc<CatalogFunctionInner>,
}

impl CatalogFunction {
    /// Create a new catalog function with the given monitor
    pub fn new(monitor: monitor::Monitor) -> Self {
        let registry = registry::Registry::new(monitor);

        let inner = CatalogFunctionInner {
            registry: Arc::new(RwLock::new(registry)),
        };

        Self {
            inner: Arc::new(inner),
        }
    }

    /// Watch a directory for journal files
    pub fn watch_directory(&self, path: &str) -> Result<()> {
        let mut registry = self.inner.registry.write().unwrap();
        registry.watch_directory(path).map_err(|e| {
            netdata_plugin_error::NetdataPluginError::Other {
                message: format!("Failed to watch directory: {}", e),
            }
        })
    }

    /// Stop watching a directory for journal files
    pub fn unwatch_directory(&self, path: &str) -> Result<()> {
        let mut registry = self.inner.registry.write().unwrap();
        registry.unwatch_directory(path).map_err(|e| {
            netdata_plugin_error::NetdataPluginError::Other {
                message: format!("Failed to unwatch directory: {}", e),
            }
        })
    }

    /// Find files in a time range
    pub fn find_files_in_range(&self, start: u32, end: u32) -> Vec<file_metadata::FileInfo> {
        let registry = self.inner.registry.read().unwrap();
        registry.find_files_in_range(start, end)
    }

    /// Process a notify event
    pub fn process_notify_event(&self, event: notify::Event) {
        let mut registry = self.inner.registry.write().unwrap();

        if let Err(e) = registry.process_event(event) {
            error!("Failed to process notify event: {}", e);
        }
    }
}

#[async_trait]
impl FunctionHandler for CatalogFunction {
    type Request = CatalogRequest;
    type Response = CatalogResponse;

    #[instrument(name = "catalog_function_call", skip_all)]
    async fn on_call(&self, _request: Self::Request) -> Result<Self::Response> {
        info!("Processing catalog function call");
        Ok(CatalogResponse {})
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        warn!("Catalog function call cancelled by Netdata");

        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Catalog function cancelled by user".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress report requested for catalog function call");
    }

    fn declaration(&self) -> FunctionDeclaration {
        info!("Generating function declaration for catalog");
        let mut func_decl =
            FunctionDeclaration::new("catalog", "Get information about journal catalog");
        func_decl.global = true;
        func_decl.tags = Some(String::from("catalog"));
        func_decl.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        func_decl
    }
}
