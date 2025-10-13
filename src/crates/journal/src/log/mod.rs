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
use std::time::{SystemTime, UNIX_EPOCH};

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

trait Layer: Send + Sync {
    fn new_entry(&mut self, journal_writer: &JournalWriter) -> Result<()>;
    fn should_rotate(&self) -> bool;
    fn rotate(&mut self) -> Result<()>;
}

pub struct SizeLayer {
    pub max_size: u64,
    pub current_size: u64,
}

impl SizeLayer {
    fn new(max_size: u64) -> Self {
        Self {
            max_size,
            current_size: 0,
        }
    }
}

impl Layer for SizeLayer {
    fn new_entry(&mut self, journal_writer: &JournalWriter) -> Result<()> {
        self.current_size = journal_writer.current_file_size();
        Ok(())
    }

    fn should_rotate(&self) -> bool {
        self.current_size >= self.max_size
    }

    fn rotate(&mut self) -> Result<()> {
        self.current_size = 0;
        Ok(())
    }
}

pub struct CountLayer {
    pub max_count: usize,
    pub current_count: usize,
}

impl CountLayer {
    fn new(max_count: usize) -> Self {
        Self {
            max_count,
            current_count: 0,
        }
    }
}

impl Layer for CountLayer {
    fn new_entry(&mut self, _journal_writer: &JournalWriter) -> Result<()> {
        self.current_count += 1;
        Ok(())
    }

    fn should_rotate(&self) -> bool {
        self.current_count >= self.max_count
    }

    fn rotate(&mut self) -> Result<()> {
        self.current_count = 0;
        Ok(())
    }
}

pub struct ChainWriter {
    pub registry_file: Option<RegistryFile>,
    pub journal_writer: Option<JournalWriter>,
    pub journal_file: Option<JournalFile<MmapMut>>,
    pub boot_id: uuid::Uuid,
    pub seqnum_id: uuid::Uuid,
    pub current_seqnum: u64,
    pub previous_bucket_utilization: Option<BucketUtilization>,
}

impl ChainWriter {
    pub fn new(boot_id: uuid::Uuid, current_seqnum: u64) -> Self {
        Self {
            registry_file: None,
            journal_writer: None,
            journal_file: None,
            boot_id,
            seqnum_id: uuid::Uuid::new_v4(),
            current_seqnum,
            previous_bucket_utilization: None,
        }
    }
}

pub struct Log {
    chain: Chain,
    config: Config,
    chain_writer: ChainWriter,
    layers: Vec<Box<dyn Layer>>,
    current_seqnum: u64,
}

impl Log {
    /// Creates a new journal log.
    pub fn new(path: &Path, config: Config) -> Result<Self> {
        let chain = create_chain(path)?;

        let current_seqnum = chain.tail_seqnum()?;
        let boot_id = load_boot_id()?;
        let chain_writer = ChainWriter::new(boot_id, current_seqnum);

        let mut layers: Vec<Box<dyn Layer>> = Vec::new();
        if let Some(max_size) = config.rotation_policy.size_of_journal_file {
            layers.push(Box::new(SizeLayer::new(max_size)));
        }
        if let Some(max_count) = config.rotation_policy.number_of_entries {
            layers.push(Box::new(CountLayer::new(max_count)));
        }

        Ok(Log {
            chain,
            config,
            chain_writer,
            current_seqnum,
            layers,
        })
    }

    /// Writes a journal entry.
    pub fn write_entry(&mut self, items: &[&[u8]]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }

        if self.should_rotate() {
            self.rotate()?;
        }

        let realtime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        let monotonic = realtime;

        let journal_file = self.chain_writer.journal_file.as_mut().unwrap();
        let journal_writer = self.chain_writer.journal_writer.as_mut().unwrap();

        journal_writer.add_entry(journal_file, items, realtime, monotonic)?;

        for layer in &mut self.layers {
            layer.new_entry(journal_writer)?;
        }

        self.current_seqnum += 1;

        Ok(())
    }

    fn should_rotate(&self) -> bool {
        if self.chain_writer.journal_writer.is_none() {
            return true;
        }

        self.layers.iter().any(|l| l.should_rotate())
    }

    fn rotate(&mut self) -> Result<()> {
        self.update_chain()?;
        self.create_file()?;

        for layer in &mut self.layers {
            let _ = layer.rotate();
        }

        Ok(())
    }

    fn update_chain(&mut self) -> Result<()> {
        // Update chain with the file size we are about to rotate
        if let (Some(registry_file), Some(journal_writer)) = (
            &self.chain_writer.registry_file,
            &self.chain_writer.journal_writer,
        ) {
            let current_size = journal_writer.current_file_size();
            let old_size = self
                .chain
                .file_sizes
                .get(&registry_file.path)
                .copied()
                .unwrap_or(0);

            self.chain
                .file_sizes
                .insert(registry_file.path.clone(), current_size);

            self.chain.total_size = self
                .chain
                .total_size
                .saturating_sub(old_size)
                .saturating_add(current_size);
        }

        // Respect retention policy
        self.chain.retain(&self.config.retention_policy)
    }

    fn create_file(&mut self) -> Result<()> {
        let head_seqnum = self.current_seqnum + 1;
        let head_realtime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        let registry_file =
            self.chain
                .create_file(self.chain_writer.seqnum_id, head_seqnum, head_realtime)?;

        // Calculate optimal bucket sizes based on previous file utilization
        let (data_buckets, field_buckets) = calculate_bucket_sizes(
            self.chain_writer.previous_bucket_utilization.as_ref(),
            &self.config.rotation_policy,
        );

        let options = JournalFileOptions::new(
            self.chain.machine_id,
            self.chain_writer.boot_id,
            self.chain_writer.seqnum_id,
        )
        .with_window_size(8 * 1024 * 1024)
        .with_data_hash_table_buckets(data_buckets)
        .with_field_hash_table_buckets(field_buckets)
        .with_keyed_hash(true);

        let mut journal_file = JournalFile::create(&registry_file.path, options)?;
        let writer = JournalWriter::new(&mut journal_file, head_seqnum, self.chain_writer.boot_id)?;

        self.chain_writer.journal_file = Some(journal_file);
        self.chain_writer.journal_writer = Some(writer);
        self.chain_writer.registry_file = Some(registry_file);

        Ok(())
    }
}
