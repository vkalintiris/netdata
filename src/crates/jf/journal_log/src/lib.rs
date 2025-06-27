use error::{JournalError, Result};
use std::cmp::Ordering;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFileInfo {
    pub path: PathBuf,
    pub timestamp: SystemTime,
    pub counter: u64,
    pub size: Option<u64>,
}

impl JournalFileInfo {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let filename = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or(JournalError::InvalidFilename)?;

        let (timestamp, counter) = Self::parse_filename(filename)?;

        Ok(Self {
            path: path.to_path_buf(),
            timestamp,
            counter,
            size: None,
        })
    }

    pub fn from_parts(
        timestamp: SystemTime,
        counter: u64,
        size: Option<u64>,
    ) -> Result<JournalFileInfo> {
        let duration = timestamp
            .duration_since(UNIX_EPOCH)
            .map_err(|_| JournalError::SystemTimeError)?;
        let micros = duration.as_secs() * 1_000_000 + duration.subsec_micros() as u64;
        let path = PathBuf::from(format!("journal-{}-{}.journal", micros, counter));

        Ok(Self {
            path,
            timestamp,
            counter,
            size,
        })
    }

    /// Parse timestamp and counter from filename
    /// Expected format: "journal-{timestamp_micros}-{counter}.journal"
    fn parse_filename(filename: &str) -> Result<(SystemTime, u64)> {
        let name = filename.strip_suffix(".journal").unwrap_or(filename);

        if let Some(stripped) = name.strip_prefix("journal-") {
            let parts: Vec<&str> = stripped.split('-').collect();
            if parts.len() == 2 {
                let timestamp_micros: u64 = parts[0]
                    .parse()
                    .map_err(|_| JournalError::InvalidFilename)?;
                let counter: u64 = parts[1]
                    .parse()
                    .map_err(|_| JournalError::InvalidFilename)?;

                let timestamp = UNIX_EPOCH + Duration::from_micros(timestamp_micros);
                return Ok((timestamp, counter));
            }
        }

        Err(JournalError::InvalidFilename)
    }

    /// Get file size, loading from filesystem if not cached
    pub fn get_size(&mut self) -> Result<u64> {
        if let Some(size) = self.size {
            Ok(size)
        } else {
            let metadata = std::fs::metadata(&self.path)?;
            let size = metadata.len();
            self.size = Some(size);
            Ok(size)
        }
    }
}

// Implement ordering based on counter (for detecting duplicates and ordering)
impl Ord for JournalFileInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        // Order by counter only - this enables duplicate detection
        self.counter.cmp(&other.counter)
    }
}

impl PartialOrd for JournalFileInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Sealing policy - determines when an active journal file should be sealed
#[derive(Debug, Clone)]
pub struct SealingPolicy {
    /// Maximum file size before sealing (in bytes)
    pub max_file_size: Option<u64>,
    /// Maximum duration that entries in a single file can span
    pub max_entry_span: Option<Duration>,
}

impl SealingPolicy {
    pub fn new() -> Self {
        Self {
            max_file_size: None,
            max_entry_span: None,
        }
    }

    pub fn with_max_file_size(mut self, max_size: u64) -> Self {
        self.max_file_size = Some(max_size);
        self
    }

    pub fn with_max_entry_span(mut self, max_span: Duration) -> Self {
        self.max_entry_span = Some(max_span);
        self
    }
}

impl Default for SealingPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Retention policy - determines when files should be removed
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Maximum number of journal files to keep
    pub max_files: Option<usize>,
    /// Maximum total size of all journal files (in bytes)
    pub max_total_size: Option<u64>,
    /// Maximum age of entries to keep
    pub max_entry_age: Option<Duration>,
}

impl RetentionPolicy {
    pub fn new() -> Self {
        Self {
            max_files: None,
            max_total_size: None,
            max_entry_age: None,
        }
    }

    pub fn with_max_files(mut self, max_files: usize) -> Self {
        self.max_files = Some(max_files);
        self
    }

    pub fn with_max_total_size(mut self, max_size: u64) -> Self {
        self.max_total_size = Some(max_size);
        self
    }

    pub fn with_max_entry_age(mut self, max_age: Duration) -> Self {
        self.max_entry_age = Some(max_age);
        self
    }
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for journal directory management
#[derive(Debug, Clone)]
pub struct JournalDirectoryConfig {
    /// Directory path where journal files are stored
    pub directory: PathBuf,
    /// Policy for when to seal active files
    pub sealing_policy: SealingPolicy,
    /// Policy for when to remove old files
    pub retention_policy: RetentionPolicy,
}

impl JournalDirectoryConfig {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            sealing_policy: SealingPolicy::default(),
            retention_policy: RetentionPolicy::default(),
        }
    }

    pub fn with_sealing_policy(mut self, policy: SealingPolicy) -> Self {
        self.sealing_policy = policy;
        self
    }

    pub fn with_retention_policy(mut self, policy: RetentionPolicy) -> Self {
        self.retention_policy = policy;
        self
    }
}

/// Manages a directory of journal files with automatic cleanup and sealing
#[derive(Debug)]
pub struct JournalDirectory {
    config: JournalDirectoryConfig,
    /// Files ordered by counter (oldest counter first)
    files: Vec<JournalFileInfo>,
    /// Next counter value for new files
    next_counter: u64,
    /// Cached total size of all files
    total_size: u64,
}

impl JournalDirectory {
    /// Scan the directory and load existing journal files
    pub fn with_config(config: JournalDirectoryConfig) -> Result<Self> {
        // Create directory if it does not already exist.
        if !config.directory.exists() {
            std::fs::create_dir_all(&config.directory)?;
        } else if !config.directory.is_dir() {
            return Err(JournalError::NotADirectory);
        }

        let mut journal_directory = Self {
            config,
            files: Vec::new(),
            next_counter: 0,
            total_size: 0,
        };

        // Read all .journal files from directory
        for entry in std::fs::read_dir(&journal_directory.config.directory)? {
            let entry = entry?;
            let file_path = entry.path();

            if file_path.extension() != Some(OsStr::new("journal")) {
                continue;
            }

            match JournalFileInfo::from_path(&file_path) {
                Ok(file_info) => {
                    journal_directory.total_size += file_info.size.unwrap_or(0);
                    journal_directory.next_counter =
                        journal_directory.next_counter.max(file_info.counter + 1);
                    journal_directory.files.push(file_info);
                }
                Err(_) => {
                    // Skip files with invalid names
                    continue;
                }
            }
        }

        // Sort files by counter to maintain order
        journal_directory.files.sort();

        Ok(journal_directory)
    }

    // Get information about all the files in the journal directory
    pub fn files(&self) -> Vec<JournalFileInfo> {
        self.files.clone()
    }

    /// Add a new journal file to the directory representation
    pub fn new_file(&mut self, existing_file: Option<JournalFileInfo>) -> Result<JournalFileInfo> {
        let timestamp = SystemTime::now();
        let new_file = JournalFileInfo::from_parts(timestamp, self.next_counter, None)?;

        self.files.push(new_file.clone());
        self.next_counter += 1;

        if let Some(existing_file) = existing_file {
            self.total_size += existing_file.size.unwrap_or(0);
        }

        Ok(new_file)
    }
}
