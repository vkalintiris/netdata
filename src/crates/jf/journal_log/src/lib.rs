use error::{JournalError, Result};
use journal_file::{load_boot_id, JournalFile, JournalFileOptions, JournalWriter};
use memmap2::MmapMut;
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

/// Determines when an active journal file should be sealed
#[derive(Debug, Clone, Default)]
pub struct RotationPolicy {
    /// Maximum file size before rotating (in bytes)
    pub max_file_size: Option<u64>,
    /// Maximum duration that entries in a single file can span
    pub max_entry_span: Option<Duration>,
}

impl RotationPolicy {
    pub fn with_max_file_size(mut self, max_size: u64) -> Self {
        self.max_file_size = Some(max_size);
        self
    }

    pub fn with_max_entry_span(mut self, max_span: Duration) -> Self {
        self.max_entry_span = Some(max_span);
        self
    }
}

/// Retention policy - determines when files should be removed
#[derive(Debug, Clone, Default)]
pub struct RetentionPolicy {
    /// Maximum number of journal files to keep
    pub max_files: Option<usize>,
    /// Maximum total size of all journal files (in bytes)
    pub max_total_size: Option<u64>,
    /// Maximum age of entries to keep
    pub max_entry_age: Option<Duration>,
}

impl RetentionPolicy {
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

/// Configuration for journal directory management
#[derive(Debug, Clone)]
pub struct JournalDirectoryConfig {
    /// Directory path where journal files are stored
    pub directory: PathBuf,
    /// Policy for when to seal active files
    pub sealing_policy: RotationPolicy,
    /// Policy for when to remove old files
    pub retention_policy: RetentionPolicy,
}

impl JournalDirectoryConfig {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            sealing_policy: RotationPolicy::default(),
            retention_policy: RetentionPolicy::default(),
        }
    }

    pub fn with_sealing_policy(mut self, policy: RotationPolicy) -> Self {
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

    pub fn directory_path(&self) -> &Path {
        &self.config.directory
    }

    pub fn get_full_path(&self, file_info: &JournalFileInfo) -> PathBuf {
        if file_info.path.is_absolute() {
            file_info.path.clone()
        } else {
            self.config.directory.join(&file_info.path)
        }
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

fn generate_uuid() -> [u8; 16] {
    uuid::Uuid::new_v4().into_bytes()
}

/// Configuration for JournalLog
#[derive(Debug, Clone)]
pub struct JournalLogConfig {
    /// Directory where journal files are stored
    pub journal_dir: PathBuf,
    /// Maximum file size in bytes before sealing
    pub max_file_size: u64,
    /// Maximum number of files to keep
    pub max_files: usize,
    /// Maximum total size of all files in bytes
    pub max_total_size: u64,
    /// Maximum age of entries in seconds
    pub max_entry_age_secs: u64,
}

impl JournalLogConfig {
    pub fn new(journal_dir: impl Into<PathBuf>) -> Self {
        Self {
            journal_dir: journal_dir.into(),
            max_file_size: 100 * 1024 * 1024, // 100MB
            max_files: 10,
            max_total_size: 1024 * 1024 * 1024, // 1GB
            max_entry_age_secs: 7 * 24 * 3600,  // 7 days
        }
    }

    pub fn with_max_file_size(mut self, max_file_size: u64) -> Self {
        self.max_file_size = max_file_size;
        self
    }

    pub fn with_max_files(mut self, max_files: usize) -> Self {
        self.max_files = max_files;
        self
    }

    pub fn with_max_total_size(mut self, max_total_size: u64) -> Self {
        self.max_total_size = max_total_size;
        self
    }

    pub fn with_max_entry_age_secs(mut self, max_entry_age_secs: u64) -> Self {
        self.max_entry_age_secs = max_entry_age_secs;
        self
    }
}

pub struct JournalLog {
    directory: JournalDirectory,
    current_file: Option<JournalFile<MmapMut>>,
    current_writer: Option<JournalWriter>,
    current_file_info: Option<JournalFileInfo>,
    machine_id: [u8; 16],
    boot_id: [u8; 16],
    seqnum_id: [u8; 16],
}

impl JournalLog {
    pub fn new(config: JournalLogConfig) -> Result<Self> {
        let sealing_policy = RotationPolicy::default()
            .with_max_file_size(config.max_file_size)
            .with_max_entry_span(Duration::from_secs(60));

        let retention_policy = RetentionPolicy::default()
            .with_max_files(config.max_files)
            .with_max_total_size(config.max_total_size)
            .with_max_entry_age(Duration::from_secs(config.max_entry_age_secs));

        let journal_config = JournalDirectoryConfig::new(&config.journal_dir)
            .with_sealing_policy(sealing_policy)
            .with_retention_policy(retention_policy);

        let directory = JournalDirectory::with_config(journal_config)?;

        let machine_id = journal_file::file::load_machine_id()?;
        let boot_id = load_boot_id().unwrap_or_else(|_| generate_uuid());
        let seqnum_id = generate_uuid();

        Ok(JournalLog {
            directory,
            current_file: None,
            current_writer: None,
            current_file_info: None,
            machine_id,
            boot_id,
            seqnum_id,
        })
    }

    fn ensure_active_journal(&mut self) -> Result<()> {
        // Check if rotation is needed before writing
        if let Some(writer) = &self.current_writer {
            if self.should_rotate(writer) {
                self.rotate_current_file()?;
            }
        }

        if self.current_file.is_none() {
            // Create a new journal file
            let file_info = self.directory.new_file(None)?;

            // Get the full path for the journal file
            let file_path = self.directory.get_full_path(&file_info);

            let options = JournalFileOptions::new(
                self.machine_id,
                self.boot_id,
                self.seqnum_id,
                generate_uuid(),
            )
            .with_window_size(8 * 1024 * 1024)
            .with_data_hash_table_buckets(4096)
            .with_field_hash_table_buckets(512)
            .with_keyed_hash(true);

            let mut journal_file = JournalFile::create(&file_path, options)?;
            let writer = JournalWriter::new(&mut journal_file)?;

            self.current_file = Some(journal_file);
            self.current_writer = Some(writer);
            self.current_file_info = Some(file_info);
        }

        Ok(())
    }

    /// Checks if we have to rotate. Prioritizes file size over file creation
    /// time.
    fn should_rotate(&self, writer: &JournalWriter) -> bool {
        let policy = &self.directory.config.sealing_policy;

        if let Some(max_size) = policy.max_file_size {
            if writer.current_file_size() >= max_size {
                return true;
            }
        }

        // FIXME: The proper implementation would check first/last entries'
        // timestamps.
        if let Some(max_span) = policy.max_entry_span {
            if let Some(file_info) = &self.current_file_info {
                let file_age = SystemTime::now()
                    .duration_since(file_info.timestamp)
                    .unwrap_or_default();
                if file_age >= max_span {
                    return true;
                }
            }
        }

        false
    }

    fn rotate_current_file(&mut self) -> Result<()> {
        // Close current file
        self.current_file = None;
        self.current_writer = None;
        self.current_file_info = None;

        // Next call to ensure_active_journal() will create new file
        Ok(())
    }

    pub fn write_entry(&mut self, items: &[&[u8]]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        self.ensure_active_journal()?;

        let journal_file = self.current_file.as_mut().unwrap();
        let writer = self.current_writer.as_mut().unwrap();

        let now = SystemTime::now();
        let realtime = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        // Use realtime for monotonic as well for simplicity
        let monotonic = realtime;

        writer.add_entry(journal_file, items, realtime, monotonic, self.boot_id)?;

        Ok(())
    }
}
