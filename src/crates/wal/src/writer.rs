use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use file_registry::FileDir;
use uuid::Uuid;

use crate::Result;
use crate::clock::MonotonicClock;
use crate::config::Config;
use crate::format::{
    COMPRESSION_NONE, FLAG_CRC_ENABLED, FORMAT_VERSION, FRAME_ALIGNMENT, FRAME_HEADER_SIZE,
    FileHeader, HEADER_SIZE, WalEvent,
};
use crate::{ByteSize, FileId, TimestampNs};

const WAL_EXT: &str = "wal";

struct ActiveFile {
    file_id: FileId,
    #[allow(dead_code)]
    path: PathBuf,
    writer: BufWriter<File>,
    frame_count: u64,
    log_entry_count: u64,
    bytes_written: ByteSize,
    min_timestamp_ns: TimestampNs,
    max_timestamp_ns: TimestampNs,
    first_frame_at_ns: Option<TimestampNs>,
}

/// Shared sequence counter for globally unique file numbering.
struct SeqCounter(Arc<AtomicU64>);

impl SeqCounter {
    fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// A single WAL output stream for one service (identified by `ns_hash`).
///
/// Handles frame writing, compression, rotation, and lifecycle event emission.
struct Stream {
    dir: Arc<FileDir>,
    machine_id: Uuid,
    boot_id: Uuid,
    config: Config,
    clock: MonotonicClock,
    active: Option<ActiveFile>,
    seq: SeqCounter,
    ns_hash: u64,
    pending_events: Vec<WalEvent>,
}

impl Stream {
    fn new(
        dir: Arc<FileDir>,
        machine_id: Uuid,
        boot_id: Uuid,
        config: Config,
        seq: SeqCounter,
        ns_hash: u64,
    ) -> Self {
        Self {
            dir,
            machine_id,
            boot_id,
            config,
            clock: MonotonicClock::new(),
            active: None,
            seq,
            ns_hash,
            pending_events: Vec::new(),
        }
    }

    /// Create a [`FileId`] stamped with this stream's identity.
    fn file_id(&self, seq: u64) -> FileId {
        FileId::new(self.machine_id, self.boot_id, seq, self.ns_hash)
    }

    fn write_frame(&mut self, data: &[u8], log_entry_count: usize) -> Result<u64> {
        if self.should_rotate_with(log_entry_count as u64) {
            self.sync()?;
            self.complete_active_file();
        }

        self.ensure_file()?;

        let ts = TimestampNs(self.clock.now_ns());

        let compressed = if self.config.compression_lz4() {
            lz4_flex::block::compress(data)
        } else {
            data.to_vec()
        };

        let payload_len = compressed.len() as u32;
        let uncompressed_len = data.len() as u32;
        let entry_count = log_entry_count as u32;

        let crc = if self.config.crc_enabled {
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&payload_len.to_le_bytes());
            hasher.update(&uncompressed_len.to_le_bytes());
            hasher.update(&entry_count.to_le_bytes());
            hasher.update(&ts.0.to_le_bytes());
            hasher.update(&compressed);
            hasher.finalize()
        } else {
            0
        };

        let active = self.active.as_mut().unwrap();
        let frame_offset = active.bytes_written.0;
        active.writer.write_all(&payload_len.to_le_bytes())?;
        active.writer.write_all(&uncompressed_len.to_le_bytes())?;
        active.writer.write_all(&entry_count.to_le_bytes())?;
        active.writer.write_all(&ts.0.to_le_bytes())?;
        active.writer.write_all(&crc.to_le_bytes())?;
        active.writer.write_all(&compressed)?;

        let frame_bytes = FRAME_ALIGNMENT_HEADER + compressed.len();
        let padding = (FRAME_ALIGNMENT - (frame_bytes % FRAME_ALIGNMENT)) % FRAME_ALIGNMENT;
        if padding > 0 {
            active
                .writer
                .write_all(&[0u8; FRAME_ALIGNMENT][..padding])?;
        }

        active.frame_count += 1;
        active.log_entry_count += log_entry_count as u64;
        active.bytes_written = ByteSize(active.bytes_written.0 + (frame_bytes + padding) as u64);
        if active.first_frame_at_ns.is_none() {
            active.first_frame_at_ns = Some(ts);
        }
        if active.min_timestamp_ns == TimestampNs::ZERO || ts < active.min_timestamp_ns {
            active.min_timestamp_ns = ts;
        }
        if ts > active.max_timestamp_ns {
            active.max_timestamp_ns = ts;
        }

        Ok(frame_offset)
    }

    fn sync(&mut self) -> Result<()> {
        if let Some(active) = &mut self.active {
            active.writer.flush()?;
            active.writer.get_ref().sync_all()?;

            self.pending_events.push(WalEvent::FileSynced {
                file_id: active.file_id,
                valid_up_to: active.bytes_written,
                frame_count: active.frame_count,
                entry_count: active.log_entry_count,
            });
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<Vec<WalEvent>> {
        self.sync()?;
        self.complete_active_file();
        Ok(self.take_events())
    }

    fn take_events(&mut self) -> Vec<WalEvent> {
        std::mem::take(&mut self.pending_events)
    }

    fn ensure_file(&mut self) -> Result<()> {
        if self.active.is_some() {
            return Ok(());
        }

        let file_seq = self.seq.next();
        let file_id = self.file_id(file_seq);
        let path = self.dir.file_path(file_id);

        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut writer = BufWriter::new(file);

        let mut flags: u16 = 0;
        if self.config.crc_enabled {
            flags |= FLAG_CRC_ENABLED;
        }
        if !self.config.compression_lz4() {
            flags |= COMPRESSION_NONE;
        }

        let created_at_ns = TimestampNs(self.clock.now_ns());
        let header = FileHeader {
            version: FORMAT_VERSION,
            flags,
            created_at: created_at_ns.0,
        };
        writer.write_all(&header.to_bytes())?;
        writer.flush()?;

        fsync_dir(self.dir.path())?;

        self.pending_events.push(WalEvent::FileCreated {
            file_id,
            created_at_ns,
        });

        self.active = Some(ActiveFile {
            file_id,
            path,
            writer,
            frame_count: 0,
            log_entry_count: 0,
            bytes_written: ByteSize(HEADER_SIZE as u64),
            min_timestamp_ns: TimestampNs::ZERO,
            max_timestamp_ns: TimestampNs::ZERO,
            first_frame_at_ns: None,
        });

        Ok(())
    }

    fn should_rotate_with(&self, incoming_entries: u64) -> bool {
        let Some(active) = &self.active else {
            return false;
        };
        if active.log_entry_count + incoming_entries > self.config.rotation.max_log_entries as u64 {
            return true;
        }
        if active.bytes_written >= self.config.rotation.max_file_size {
            return true;
        }
        if let (Some(max_dur), Some(first_frame_at)) =
            (self.config.rotation.max_duration, active.first_frame_at_ns)
        {
            let now = self.clock.last_ns;
            let elapsed_ns = now.saturating_sub(first_frame_at.0);
            if elapsed_ns >= max_dur.as_nanos() as u64 {
                return true;
            }
        }
        false
    }

    fn complete_active_file(&mut self) {
        if let Some(active) = self.active.take() {
            self.pending_events.push(WalEvent::FileCompleted {
                file_id: active.file_id,
                frame_count: active.frame_count,
                min_timestamp_ns: active.min_timestamp_ns,
                max_timestamp_ns: active.max_timestamp_ns,
                size: active.bytes_written,
            });
        }
    }
}

const FRAME_ALIGNMENT_HEADER: usize = FRAME_HEADER_SIZE;

impl Drop for Stream {
    fn drop(&mut self) {
        self.complete_active_file();
    }
}

impl Config {
    pub(crate) fn compression_lz4(&self) -> bool {
        self.compression_enabled
    }
}

// ---------------------------------------------------------------------------
// Ingester
// ---------------------------------------------------------------------------

/// Manages multiple WAL output [`Stream`]s keyed by `ns_hash`, with a shared
/// monotonic sequence counter so that file sequence numbers are globally
/// unique within the WAL directory.
pub struct Ingester {
    dir: Arc<FileDir>,
    machine_id: Uuid,
    boot_id: Uuid,
    config: Config,
    seq: Arc<AtomicU64>,
    streams: HashMap<u64, Stream>,
}

impl Ingester {
    /// Create a new ingester.
    ///
    /// Machine and boot IDs are loaded from the system. The caller provides
    /// a shared sequence counter (e.g., shared across per-tenant ingesters).
    /// The directory is created if it doesn't exist.
    pub fn new(path: &Path, config: Config, seq: Arc<AtomicU64>) -> Result<Self> {
        let machine_id = journal_common::load_machine_id().map_err(|e| crate::Error::Io(e))?;
        let boot_id = journal_common::load_boot_id().map_err(|e| crate::Error::Io(e))?;
        let dir = Arc::new(FileDir::new(path, WAL_EXT));
        std::fs::create_dir_all(dir.path())?;
        Ok(Self {
            dir,
            machine_id,
            boot_id,
            config,
            seq,
            streams: HashMap::new(),
        })
    }

    /// Write a frame to the stream for the given `ns_hash`.
    ///
    /// Lazily creates a new stream if one doesn't exist for this `ns_hash`.
    pub fn write_frame(
        &mut self,
        ns_hash: u64,
        data: &[u8],
        log_entry_count: usize,
    ) -> Result<u64> {
        self.get_or_create(ns_hash)
            .write_frame(data, log_entry_count)
    }

    /// Get or lazily create a stream for the given `ns_hash`.
    fn get_or_create(&mut self, ns_hash: u64) -> &mut Stream {
        self.streams.entry(ns_hash).or_insert_with(|| {
            Stream::new(
                Arc::clone(&self.dir),
                self.machine_id,
                self.boot_id,
                self.config.clone(),
                SeqCounter(Arc::clone(&self.seq)),
                ns_hash,
            )
        })
    }

    /// Drain pending events from all streams.
    pub fn take_all_events(&mut self) -> Vec<WalEvent> {
        let mut events = Vec::new();
        for stream in self.streams.values_mut() {
            events.append(&mut stream.take_events());
        }
        events
    }

    /// Sync all active streams to disk.
    pub fn sync_all(&mut self) -> Result<()> {
        for stream in self.streams.values_mut() {
            stream.sync()?;
        }
        Ok(())
    }

    /// Shut down all streams, returning any remaining events.
    pub fn shutdown_all(&mut self) -> Result<Vec<WalEvent>> {
        let mut events = Vec::new();
        for stream in self.streams.values_mut() {
            events.append(&mut stream.shutdown()?);
        }
        Ok(events)
    }
}

/// Scan all immediate subdirectories of `base` for WAL files and return
/// the highest sequence number found across all of them.
///
/// Returns 0 if the directory doesn't exist or contains no WAL files.
pub fn scan_max_sequence_recursive(base: &Path) -> Result<u64> {
    let mut max_seq: u64 = 0;
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    for entry in entries.flatten() {
        if entry.file_type().map_or(false, |ft| ft.is_dir()) {
            let dir = FileDir::new(&entry.path(), WAL_EXT);
            let seq = dir.scan_max_sequence()?;
            max_seq = max_seq.max(seq);
        }
    }
    Ok(max_seq)
}

fn fsync_dir(dir: &std::path::Path) -> Result<()> {
    let dir_file = File::open(dir)?;
    dir_file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn test_ingester(tmp: &std::path::Path) -> Ingester {
        let seq = Arc::new(AtomicU64::new(0));
        Ingester::new(tmp, Config::default(), seq).unwrap()
    }

    #[test]
    fn creates_separate_files_per_ns_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ingester = test_ingester(tmp.path());

        let data = b"test payload";

        ingester.write_frame(1, data, 1).unwrap();
        ingester.write_frame(2, data, 1).unwrap();
        ingester.write_frame(1, data, 1).unwrap();

        ingester.sync_all().unwrap();

        // Two distinct ns_hash values → two WAL files.
        let wal_files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "wal"))
            .collect();
        assert_eq!(wal_files.len(), 2);

        // Verify filenames carry distinct ns_hash suffixes.
        let mut hashes: Vec<u64> = wal_files
            .iter()
            .map(|e| crate::FileId::parse(&e.path()).unwrap().ns_hash)
            .collect();
        hashes.sort();
        assert_eq!(hashes, vec![1, 2]);
    }

    #[test]
    fn shared_seq_is_globally_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ingester = test_ingester(tmp.path());

        let data = b"test payload";
        ingester.write_frame(10, data, 1).unwrap();
        ingester.write_frame(20, data, 1).unwrap();

        ingester.sync_all().unwrap();

        let mut seqs: Vec<u64> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| crate::FileId::parse(&e.path()).unwrap().seq)
            .collect();
        seqs.sort();
        // Sequences must be distinct (1, 2) — not both 1.
        assert_eq!(seqs, vec![1, 2]);
    }
}
