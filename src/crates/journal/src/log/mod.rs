mod directory;
use directory::{JournalDirectory, JournalDirectoryConfig};

mod config;
pub use config::{Config, RetentionPolicy, RotationPolicy};

use crate::error::Result;
use crate::file::mmap::MmapMut;
use crate::file::{
    BucketUtilization, JournalFile, JournalFileOptions, JournalWriter, load_boot_id,
};
use crate::registry::File as JournalFile_;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Calculate optimal bucket sizes based on previous file utilization or rotation policy
fn calculate_bucket_sizes(
    previous_utilization: Option<&BucketUtilization>,
    rotation_policy: &RotationPolicy,
) -> (usize, usize) {
    if let Some(utilization) = previous_utilization {
        let data_utilization = utilization.data_utilization();
        let field_utilization = utilization.field_utilization();

        let data_buckets = if data_utilization > 0.75 {
            (utilization.data_total * 2).next_power_of_two()
        } else if data_utilization < 0.25 && utilization.data_total > 4096 {
            (utilization.data_total / 2).next_power_of_two()
        } else {
            utilization.data_total
        };

        let field_buckets = if field_utilization > 0.75 {
            (utilization.field_total * 2).next_power_of_two()
        } else if field_utilization < 0.25 && utilization.field_total > 512 {
            (utilization.field_total / 2).next_power_of_two()
        } else {
            utilization.field_total
        };

        (data_buckets, field_buckets)
    } else {
        // Initial sizing based on rotation policy max file size
        let max_file_size = rotation_policy
            .size_of_journal_file
            .unwrap_or(8 * 1024 * 1024);

        // 16 MiB -> 4096 data buckets
        let data_buckets = (max_file_size / 4096).max(1024).next_power_of_two() as usize;
        let field_buckets = 128; // Assume ~8:1 data:field ratio

        (data_buckets, field_buckets)
    }
}

/// High-level journal writer with automatic rotation and retention.
///
/// Creates journal files in systemd format under `{journal_dir}/{machine_id}/`.
/// Files are automatically rotated based on configured policies, and old files
/// are deleted to satisfy retention limits.
pub struct Log {
    directory: JournalDirectory,
    current_file: Option<JournalFile<MmapMut>>,
    current_writer: Option<JournalWriter>,
    current_file_info: Option<JournalFile_>,
    machine_id: uuid::Uuid,
    boot_id: uuid::Uuid,
    seqnum_id: uuid::Uuid,
    previous_bucket_utilization: Option<BucketUtilization>,
    entries_since_rotation: usize,
    /// The sequence number that the next file will start with
    next_file_head_seqnum: u64,
}

impl Log {
    /// Creates a new journal log.
    ///
    /// Scans for existing journal files and enforces retention policies on startup.
    pub fn new(path: String, config: Config) -> Result<Self> {
        let machine_id = crate::file::file::load_machine_id()?;
        let boot_id = load_boot_id()?;
        let seqnum_id = uuid::Uuid::new_v4();

        // Create the machine_id subdirectory: {journal_dir}/{machine_id}/
        let mut origin_dir = path.clone();
        origin_dir.push_str(&machine_id.to_string());

        let journal_config = JournalDirectoryConfig::new(&origin_dir)
            .with_sealing_policy(config.rotation_policy)
            .with_retention_policy(config.retention_policy);

        let mut directory = JournalDirectory::with_config(journal_config)?;

        // Enforce retention policy on startup to clean up any old files
        directory.enforce_retention_policy()?;

        Ok(Log {
            directory,
            current_file: None,
            current_writer: None,
            current_file_info: None,
            machine_id,
            boot_id,
            seqnum_id,
            previous_bucket_utilization: None,
            entries_since_rotation: 0,
            next_file_head_seqnum: 1, // First file starts with seqnum 1
        })
    }

    /// Writes a journal entry.
    ///
    /// Each item should be a field in the format `FIELD_NAME=value`. Automatically
    /// handles file rotation and retention policy enforcement.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use journal_log::{JournalLog, JournalLogConfig};
    /// # fn example(journal: &mut JournalLog) -> Result<(), Box<dyn std::error::Error>> {
    /// journal.write_entry(&[
    ///     b"MESSAGE=System started",
    ///     b"PRIORITY=6",
    ///     b"SYSLOG_FACILITY=3",
    /// ])?;
    /// # Ok(())
    /// # }
    /// ```
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

        writer.add_entry(journal_file, items, realtime, monotonic, &self.boot_id)?;

        // Increment entry counter
        self.entries_since_rotation += 1;

        Ok(())
    }

    fn ensure_active_journal(&mut self) -> Result<()> {
        // Check if rotation is needed before writing
        if let Some(writer) = &self.current_writer {
            if self.should_rotate(writer) {
                self.rotate_current_file()?;
            }
        }

        if self.current_file.is_none() {
            // Compute head values for the new file
            // head_realtime: current time in microseconds since epoch
            let now = SystemTime::now();
            let head_realtime = now
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64;

            // head_seqnum: use the tracked value (which was set during rotation or is 1 for first file)
            let head_seqnum = self.next_file_head_seqnum;

            // Create a new journal file entry
            let file_info = self.directory.new_file(
                self.machine_id,
                self.seqnum_id,
                head_seqnum,
                head_realtime,
            )?;

            // Get the full path for the journal file
            let file_path = self.directory.get_full_path(&file_info);

            // Calculate optimal bucket sizes based on previous file utilization
            let (data_buckets, field_buckets) = calculate_bucket_sizes(
                self.previous_bucket_utilization.as_ref(),
                &self.directory.config.rotation_policy,
            );

            let file_id = uuid::Uuid::new_v4();

            let options =
                JournalFileOptions::new(self.machine_id, self.boot_id, self.seqnum_id, file_id)
                    .with_window_size(8 * 1024 * 1024)
                    .with_data_hash_table_buckets(data_buckets)
                    .with_field_hash_table_buckets(field_buckets)
                    .with_keyed_hash(true);

            let mut journal_file = JournalFile::create(&file_path, options)?;
            let mut writer = JournalWriter::new(&mut journal_file)?;

            // Set the correct initial sequence number for this file
            writer.set_next_seqnum(head_seqnum);

            self.current_file = Some(journal_file);
            self.current_writer = Some(writer);
            self.current_file_info = Some(file_info);

            // Enforce retention policy after creating new file to account for the new file count
            self.directory.enforce_retention_policy()?;
        }

        Ok(())
    }

    /// Checks if we have to rotate. Prioritizes file size over file creation
    /// time, then entry count, then duration.
    fn should_rotate(&self, writer: &JournalWriter) -> bool {
        let policy = self.directory.config.rotation_policy;

        // Check if the file size went over the limit
        if let Some(max_size) = policy.size_of_journal_file {
            if writer.current_file_size() >= max_size {
                return true;
            }
        }

        // Check if the entry count went over the limit
        if let Some(max_entries) = policy.number_of_entries {
            if self.entries_since_rotation >= max_entries {
                return true;
            }
        }

        // Check if the time span between first and last entries exceeds the limit
        let Some(file) = &self.current_file else {
            return false;
        };
        let Some(max_entry_span) = policy.duration_of_journal_file else {
            return false;
        };
        let Some(first_monotonic) = writer.first_entry_monotonic() else {
            return false;
        };

        let header = file.journal_header_ref();
        let last_monotonic = header.tail_entry_monotonic;

        // Convert monotonic timestamps (microseconds) to duration
        let entry_span = if last_monotonic >= first_monotonic {
            Duration::from_micros(last_monotonic - first_monotonic)
        } else {
            return false;
        };

        if entry_span >= max_entry_span {
            return true;
        }

        false
    }

    fn rotate_current_file(&mut self) -> Result<()> {
        // Capture bucket utilization and next seqnum before closing the file
        if let Some(file) = &self.current_file {
            self.previous_bucket_utilization = file.bucket_utilization();
        }

        // Capture the next sequence number for the new file
        if let Some(writer) = &self.current_writer {
            self.next_file_head_seqnum = writer.next_seqnum();
        }

        // Update the current file's size in our tracking before closing
        if let (Some(file_info), Some(writer)) = (&self.current_file_info, &self.current_writer) {
            let current_size = writer.current_file_size();

            // Update the size in the directory's file_sizes HashMap
            let old_size = self
                .directory
                .file_sizes
                .get(&file_info.path)
                .copied()
                .unwrap_or(0);

            self.directory
                .file_sizes
                .insert(file_info.path.clone(), current_size);

            // Update total size tracking
            self.directory.total_size = self
                .directory
                .total_size
                .saturating_sub(old_size)
                .saturating_add(current_size);
        }

        // Close current file
        self.current_file = None;
        self.current_writer = None;
        self.current_file_info = None;

        // Reset entry counter for the new file
        self.entries_since_rotation = 0;

        // Next call to ensure_active_journal() will create new file
        Ok(())
    }
}
