use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::Result;
use crate::clock::MonotonicClock;
use crate::config::Config;
use crate::format::{
    COMPRESSION_NONE, FLAG_CRC_ENABLED, FORMAT_VERSION, FRAME_ALIGNMENT, FRAME_HEADER_SIZE,
    FileHeader, HEADER_SIZE, WalEvent, parse_sequence,
};

struct ActiveFile {
    path: PathBuf,
    writer: BufWriter<File>,
    frame_count: u64,
    log_entry_count: u64,
    bytes_written: u64,
    min_timestamp_ns: u64,
    max_timestamp_ns: u64,
    first_frame_at_ns: Option<u64>,
}

pub struct WalWriter {
    dir: PathBuf,
    config: Config,
    clock: MonotonicClock,
    active: Option<ActiveFile>,
    file_seq: u64,
    pending_events: Vec<WalEvent>,
}

impl WalWriter {
    pub fn new(dir: &Path, config: Config) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let file_seq = scan_max_sequence(dir);

        Ok(Self {
            dir: dir.to_path_buf(),
            config,
            clock: MonotonicClock::new(),
            active: None,
            file_seq,
            pending_events: Vec::new(),
        })
    }

    pub fn write_frame(&mut self, data: &[u8], log_entry_count: usize) -> Result<u64> {
        if self.should_rotate_with(log_entry_count as u64) {
            self.sync()?;
            self.complete_active_file();
        }

        self.ensure_file()?;

        let ts = self.clock.now_ns();

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
            hasher.update(&ts.to_le_bytes());
            hasher.update(&compressed);
            hasher.finalize()
        } else {
            0
        };

        let active = self.active.as_mut().unwrap();
        let frame_offset = active.bytes_written;
        active.writer.write_all(&payload_len.to_le_bytes())?;
        active.writer.write_all(&uncompressed_len.to_le_bytes())?;
        active.writer.write_all(&entry_count.to_le_bytes())?;
        active.writer.write_all(&ts.to_le_bytes())?;
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
        active.bytes_written += (frame_bytes + padding) as u64;
        if active.first_frame_at_ns.is_none() {
            active.first_frame_at_ns = Some(ts);
        }
        if active.min_timestamp_ns == 0 || ts < active.min_timestamp_ns {
            active.min_timestamp_ns = ts;
        }
        if ts > active.max_timestamp_ns {
            active.max_timestamp_ns = ts;
        }

        Ok(frame_offset)
    }

    pub fn sync(&mut self) -> Result<()> {
        if let Some(active) = &mut self.active {
            active.writer.flush()?;
            active.writer.get_ref().sync_all()?;

            self.pending_events.push(WalEvent::FileSynced {
                path: active.path.clone(),
                valid_up_to: active.bytes_written,
                frame_count: active.frame_count,
                entry_count: active.log_entry_count,
            });
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<Vec<WalEvent>> {
        self.sync()?;
        self.complete_active_file();
        Ok(self.take_events())
    }

    pub fn take_events(&mut self) -> Vec<WalEvent> {
        std::mem::take(&mut self.pending_events)
    }

    fn ensure_file(&mut self) -> Result<()> {
        if self.active.is_some() {
            return Ok(());
        }

        self.file_seq += 1;
        let filename = format!("wal-{:010}.bin", self.file_seq);
        let path = self.dir.join(&filename);

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

        let header = FileHeader {
            version: FORMAT_VERSION,
            flags,
            created_at: self.clock.now_ns(),
        };
        writer.write_all(&header.to_bytes())?;
        writer.flush()?;

        fsync_dir(&self.dir)?;

        self.pending_events.push(WalEvent::FileCreated {
            path: path.clone(),
            created_at_ns: header.created_at,
        });

        self.active = Some(ActiveFile {
            path,
            writer,
            frame_count: 0,
            log_entry_count: 0,
            bytes_written: HEADER_SIZE as u64,
            min_timestamp_ns: 0,
            max_timestamp_ns: 0,
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
            let elapsed_ns = now.saturating_sub(first_frame_at);
            if elapsed_ns >= max_dur.as_nanos() as u64 {
                return true;
            }
        }
        false
    }

    fn complete_active_file(&mut self) {
        if let Some(active) = self.active.take() {
            self.pending_events.push(WalEvent::FileCompleted {
                path: active.path,
                frame_count: active.frame_count,
                min_timestamp_ns: active.min_timestamp_ns,
                max_timestamp_ns: active.max_timestamp_ns,
                size: active.bytes_written,
            });
        }
    }
}

const FRAME_ALIGNMENT_HEADER: usize = FRAME_HEADER_SIZE;

impl Drop for WalWriter {
    fn drop(&mut self) {
        self.complete_active_file();
    }
}

impl Config {
    pub(crate) fn compression_lz4(&self) -> bool {
        self.compression_enabled
    }
}

fn scan_max_sequence(dir: &Path) -> u64 {
    let mut max_seq: u64 = 0;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        if let Some(seq) = parse_sequence(&entry.path()) {
            max_seq = max_seq.max(seq);
        }
    }
    max_seq
}

fn fsync_dir(dir: &Path) -> Result<()> {
    let dir_file = File::open(dir)?;
    dir_file.sync_all()?;
    Ok(())
}
