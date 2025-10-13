mod chain;
use chain::Chain;

mod config;
pub use config::{Config, RetentionPolicy, RotationPolicy};

use crate::error::{JournalError, Result};
use crate::file::mmap::MmapMut;
use crate::file::{
    BucketUtilization, JournalFile, JournalFileOptions, JournalWriter, load_boot_id,
};
use crate::registry::File as RegistryFile;
use std::path::{Path, PathBuf};
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

fn create_chain(path: &Path) -> Result<Chain> {
    let machine_id = crate::file::file::load_machine_id()?;

    if path.exists() && !path.is_dir() {
        return Err(JournalError::NotADirectory);
    }

    if path.to_str().is_none() {
        return Err(JournalError::InvalidFilename);
    }

    let path = PathBuf::from(path).join(machine_id.as_simple().to_string());
    if path.to_str().is_none() {
        return Err(JournalError::InvalidFilename);
    }

    std::fs::create_dir_all(&path)?;

    path.canonicalize()
        .map_err(|_| JournalError::NotADirectory)?;
    if path.to_str().is_none() {
        return Err(JournalError::InvalidFilename);
    }

    Chain::new(path, machine_id)
}

pub struct ChainWriter {
    pub registry_file: Option<RegistryFile>,
    pub journal_file: Option<JournalFile<MmapMut>>,
    pub journal_writer: Option<JournalWriter>,
}

pub struct Log {
    chain: Chain,
    config: Config,
    chain_writer: ChainWriter,
    boot_id: uuid::Uuid,
    seqnum_id: uuid::Uuid,
    previous_bucket_utilization: Option<BucketUtilization>,
    entries_since_rotation: usize,
    current_seqnum: u64,
}

impl Log {
    /// Creates a new journal log.
    pub fn new(path: &Path, config: Config) -> Result<Self> {
        let mut chain = create_chain(path)?;

        // Enforce retention policy on startup to clean up any old files
        chain.retain(&config.retention_policy)?;

        let boot_id = load_boot_id()?;
        let seqnum_id = uuid::Uuid::new_v4();
        let current_seqnum = chain.tail_seqnum()?;

        let chain_writer = ChainWriter {
            registry_file: None,
            journal_file: None,
            journal_writer: None,
        };

        Ok(Log {
            chain,
            config,
            chain_writer,
            boot_id,
            seqnum_id,
            previous_bucket_utilization: None,
            entries_since_rotation: 0,
            current_seqnum,
        })
    }

    /// Writes a journal entry.
    pub fn write_entry(&mut self, items: &[&[u8]]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        let realtime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        let monotonic = realtime;

        self.ensure_active_journal()?;

        let journal_file = self.chain_writer.journal_file.as_mut().unwrap();
        let writer = self.chain_writer.journal_writer.as_mut().unwrap();

        writer.add_entry(journal_file, items, realtime, monotonic)?;

        self.entries_since_rotation += 1;
        self.current_seqnum += 1;

        Ok(())
    }

    fn ensure_active_journal(&mut self) -> Result<()> {
        // Check if rotation is needed before writing
        if let Some(writer) = &self.chain_writer.journal_writer {
            if self.should_rotate(writer) {
                self.rotate_current_file()?;
            }
        }

        if self.chain_writer.journal_file.is_none() {
            let head_seqnum = self.current_seqnum + 1;
            let head_realtime = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64;

            let registry_file =
                self.chain
                    .create_file(self.seqnum_id, head_seqnum, head_realtime)?;

            // Calculate optimal bucket sizes based on previous file utilization
            let (data_buckets, field_buckets) = calculate_bucket_sizes(
                self.previous_bucket_utilization.as_ref(),
                &self.config.rotation_policy,
            );

            let options =
                JournalFileOptions::new(self.chain.machine_id, self.boot_id, self.seqnum_id)
                    .with_window_size(8 * 1024 * 1024)
                    .with_data_hash_table_buckets(data_buckets)
                    .with_field_hash_table_buckets(field_buckets)
                    .with_keyed_hash(true);
            let mut journal_file = JournalFile::create(&registry_file.path, options)?;

            let writer = JournalWriter::new(&mut journal_file, head_seqnum, self.boot_id)?;

            self.chain_writer.journal_file = Some(journal_file);
            self.chain_writer.journal_writer = Some(writer);
            self.chain_writer.registry_file = Some(registry_file);

            // Enforce retention policy after creating new file to account for the new file count
            self.chain.retain(&self.config.retention_policy)?;
        }

        Ok(())
    }

    /// Checks if we have to rotate. Prioritizes file size over file creation
    /// time, then entry count, then duration.
    fn should_rotate(&self, writer: &JournalWriter) -> bool {
        let policy = self.config.rotation_policy;

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
        let Some(file) = &self.chain_writer.journal_file else {
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
        if let Some(file) = &self.chain_writer.journal_file {
            self.previous_bucket_utilization = file.bucket_utilization();
        }

        // Update the current file's size in our tracking before closing
        if let (Some(file), Some(writer)) = (
            &self.chain_writer.registry_file,
            &self.chain_writer.journal_writer,
        ) {
            let current_size = writer.current_file_size();

            // Update the size in the directory's file_sizes HashMap
            let old_size = self.chain.file_sizes.get(&file.path).copied().unwrap_or(0);

            self.chain
                .file_sizes
                .insert(file.path.clone(), current_size);

            // Update total size tracking
            self.chain.total_size = self
                .chain
                .total_size
                .saturating_sub(old_size)
                .saturating_add(current_size);
        }

        // Close current file
        self.chain_writer.registry_file = None;
        self.chain_writer.journal_file = None;
        self.chain_writer.journal_writer = None;

        // Reset entry counter for the new file
        self.entries_since_rotation = 0;

        // Next call to ensure_active_journal() will create new file
        Ok(())
    }
}
