use std::time::Duration;

/// Controls when journal files should be rotated (sealed and replaced with a new file).
///
/// A file rotates when *any* configured limit is exceeded. If all fields are `None`,
/// files never rotate automatically.
#[derive(Debug, Copy, Clone, Default)]
pub struct RotationPolicy {
    /// Maximum file size
    pub size_of_journal_file: Option<u64>,
    /// Maximum duration of head/tail entries
    pub duration_of_journal_file: Option<Duration>,
    /// Maximum number of log entries
    pub number_of_entries: Option<usize>,
}

impl RotationPolicy {
    pub fn with_size_of_journal_file(mut self, size_of_journal_file: u64) -> Self {
        self.size_of_journal_file = Some(size_of_journal_file);
        self
    }

    pub fn with_duration_of_journal_file(mut self, duration_of_journal_file: Duration) -> Self {
        self.duration_of_journal_file = Some(duration_of_journal_file);
        self
    }

    pub fn with_number_of_entries(mut self, number_of_entries: usize) -> Self {
        self.number_of_entries = Some(number_of_entries);
        self
    }
}

/// Controls when old journal files should be deleted.
///
/// Old files are removed to satisfy *all* configured limits. Removal starts with
/// the oldest files first. If all fields are `None`, files are never deleted.
#[derive(Debug, Copy, Clone, Default)]
pub struct RetentionPolicy {
    /// Maximum number of journal files to keep
    pub number_of_journal_files: Option<usize>,
    /// Maximum total size of all journal files (in bytes)
    pub size_of_journal_files: Option<u64>,
    /// Maximum age of files to keep
    pub duration_of_journal_files: Option<Duration>,
}

impl RetentionPolicy {
    pub fn with_number_of_journal_files(mut self, number_of_journal_files: usize) -> Self {
        self.number_of_journal_files = Some(number_of_journal_files);
        self
    }

    pub fn with_size_of_journal_files(mut self, size_of_journal_files: u64) -> Self {
        self.size_of_journal_files = Some(size_of_journal_files);
        self
    }

    pub fn with_duration_of_journal_files(mut self, duration_of_journal_files: Duration) -> Self {
        self.duration_of_journal_files = Some(duration_of_journal_files);
        self
    }
}
