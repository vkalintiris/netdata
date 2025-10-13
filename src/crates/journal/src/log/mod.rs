mod chain;
use chain::Chain;

mod config;
pub use config::{Config, RetentionPolicy, RotationPolicy};

use crate::error::{JournalError, Result};
use crate::file::mmap::MmapMut;
use crate::file::{JournalFile, JournalFileOptions, JournalWriter, load_boot_id};
use crate::registry::File as RegistryFile;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Tracks rotation state for size and count limits
struct RotationState {
    size: Option<(u64, u64)>,      // (max, current)
    count: Option<(usize, usize)>, // (max, current)
}

impl RotationState {
    fn new(rotation_policy: &RotationPolicy) -> Self {
        Self {
            size: rotation_policy.size_of_journal_file.map(|max| (max, 0)),
            count: rotation_policy.number_of_entries.map(|max| (max, 0)),
        }
    }

    fn should_rotate(&self) -> bool {
        self.size.is_some_and(|(max, current)| current >= max)
            || self.count.is_some_and(|(max, current)| current >= max)
    }

    fn update(&mut self, journal_writer: &JournalWriter) {
        if let Some((_, ref mut current)) = self.size {
            *current = journal_writer.current_file_size();
        }
        if let Some((_, ref mut current)) = self.count {
            *current += 1;
        }
    }

    fn reset(&mut self) {
        if let Some((_, ref mut current)) = self.size {
            *current = 0;
        }
        if let Some((_, ref mut current)) = self.count {
            *current = 0;
        }
    }
}

/// Groups a journal file and its writer together
pub struct ActiveFile {
    registry_file: RegistryFile,
    journal_file: JournalFile<MmapMut>,
    writer: JournalWriter,
}

impl ActiveFile {
    /// Creates a new journal file with the given parameters
    fn create(
        chain: &mut Chain,
        seqnum_id: uuid::Uuid,
        boot_id: uuid::Uuid,
        next_seqnum: u64,
        max_file_size: Option<u64>,
    ) -> Result<Self> {
        let head_seqnum = next_seqnum;
        let head_realtime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        let registry_file = chain.create_file(seqnum_id, head_seqnum, head_realtime)?;

        let options = JournalFileOptions::new(chain.machine_id, boot_id, seqnum_id)
            .with_window_size(8 * 1024 * 1024)
            .with_optimized_buckets(None, max_file_size)
            .with_keyed_hash(true);

        let mut journal_file = JournalFile::create(&registry_file.path, options)?;
        let writer = JournalWriter::new(&mut journal_file, head_seqnum, boot_id)?;

        Ok(Self {
            registry_file,
            journal_file,
            writer,
        })
    }

    /// Creates a successor file, inheriting settings from this file
    fn rotate(self, chain: &mut Chain, max_file_size: Option<u64>) -> Result<Self> {
        let next_seqnum = self.writer.next_seqnum();
        let boot_id = self.writer.boot_id();

        let head_seqnum = next_seqnum;
        let head_realtime = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        let seqnum_id = uuid::Uuid::from_bytes(self.journal_file.journal_header_ref().seqnum_id);
        let registry_file = chain.create_file(seqnum_id, head_seqnum, head_realtime)?;

        let mut journal_file = self
            .journal_file
            .create_successor(&registry_file.path, max_file_size)?;
        let writer = JournalWriter::new(&mut journal_file, head_seqnum, boot_id)?;

        Ok(Self {
            registry_file,
            journal_file,
            writer,
        })
    }

    /// Writes a journal entry
    fn write_entry(&mut self, items: &[&[u8]], realtime: u64, monotonic: u64) -> Result<()> {
        self.writer
            .add_entry(&mut self.journal_file, items, realtime, monotonic)
    }

    /// Gets the current file size
    fn current_file_size(&self) -> u64 {
        self.writer.current_file_size()
    }
}

pub struct Log {
    chain: Chain,
    config: Config,
    active_file: Option<ActiveFile>,
    rotation_state: RotationState,
    boot_id: uuid::Uuid,
    seqnum_id: uuid::Uuid,
    current_seqnum: u64,
}

impl Log {
    /// Creates a new journal log.
    pub fn new(path: &Path, config: Config) -> Result<Self> {
        let chain = create_chain(path)?;

        let current_seqnum = chain.tail_seqnum()?;
        let boot_id = load_boot_id()?;
        let seqnum_id = uuid::Uuid::new_v4();
        let rotation_state = RotationState::new(&config.rotation_policy);

        Ok(Log {
            chain,
            config,
            active_file: None,
            rotation_state,
            boot_id,
            seqnum_id,
            current_seqnum,
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

        let active_file = self.active_file.as_mut().unwrap();
        active_file.write_entry(items, realtime, monotonic)?;

        self.rotation_state.update(&active_file.writer);
        self.current_seqnum += 1;

        Ok(())
    }

    fn should_rotate(&self) -> bool {
        self.active_file.is_none() || self.rotation_state.should_rotate()
    }

    fn rotate(&mut self) -> Result<()> {
        // Update chain with current file size before rotating
        if let Some(active_file) = &self.active_file {
            self.chain.update_file_size(
                &active_file.registry_file.path,
                active_file.current_file_size(),
            );
        }

        // Respect retention policy
        self.chain.retain(&self.config.retention_policy)?;

        // Create new file (either initial or rotated)
        let max_file_size = self.config.rotation_policy.size_of_journal_file;
        let new_file = if let Some(old_file) = self.active_file.take() {
            old_file.rotate(&mut self.chain, max_file_size)?
        } else {
            ActiveFile::create(
                &mut self.chain,
                self.seqnum_id,
                self.boot_id,
                self.current_seqnum + 1,
                max_file_size,
            )?
        };

        self.active_file = Some(new_file);
        self.rotation_state.reset();

        Ok(())
    }
}
