use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use file_registry::TimestampNs;

use crate::format::{COMPRESSION_LZ4, FRAME_ALIGNMENT, FRAME_HEADER_SIZE, FileHeader, HEADER_SIZE};
use crate::{Error, Result};

/// Reject frames claiming to be larger than 64 MiB.
const MAX_FRAME_PAYLOAD: usize = 64 * 1024 * 1024;

/// A single frame read from the WAL file.
pub struct Frame<'a> {
    /// Ingestion timestamp in nanoseconds since the Unix epoch.
    pub timestamp_ns: TimestampNs,
    /// Number of log entries in this frame.
    pub entry_count: u32,
    /// Decompressed payload data.
    pub data: &'a [u8],
}

/// Reads WAL files produced by [`WalWriter`](crate::WalWriter).
pub struct Reader {
    reader: BufReader<File>,
    header: FileHeader,
    compressed_buf: Vec<u8>,
    data_buf: Vec<u8>,
    /// Absolute byte offset of the next frame to read.
    position: u64,
    /// Frames are read while they fit fully below this offset. For
    /// [`open`](Self::open) it is the file length (read to EOF); for
    /// [`open_range`](Self::open_range) it is the caller's `end` bound.
    end_bound: u64,
}

impl Reader {
    /// Open a WAL file and read every frame, from the first frame to
    /// end-of-file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path, HEADER_SIZE as u64, None)
    }

    /// Open a WAL file and read only the frames within the byte range
    /// `[start, end)`.
    ///
    /// This is how a query reads the durable, fully-written prefix of a
    /// file another process is still appending to (`end =
    /// File::valid_up_to`, the last fsync boundary), or a sub-range of a
    /// sealed file (chunk building). Both `start` and `end` must be
    /// **frame boundaries** — `HEADER_SIZE` or a frame end offset
    /// recorded from a prior read / a `Synced` event's `valid_up_to`;
    /// the reader cannot detect a mid-frame offset and would decode
    /// garbage. `start` defaults to the first frame when it equals
    /// `HEADER_SIZE`.
    ///
    /// Validations (the durable-prefix soundness checks): the file must
    /// physically contain `end` (`file_len >= end`), and `start` must
    /// lie in `[HEADER_SIZE, end]`. A frame is yielded only if it fits
    /// **fully** below `end`; the bytes beyond `end` may be a torn frame
    /// (the writer's buffer can flush mid-frame) and are never read.
    pub fn open_range(path: &Path, start: u64, end: u64) -> Result<Self> {
        Self::open_inner(path, start, Some(end))
    }

    fn open_inner(path: &Path, start: u64, end: Option<u64>) -> Result<Self> {
        let file = File::open(path)?;
        let file_len = file.metadata()?.len();

        let end_bound = match end {
            Some(end) => {
                if end > file_len {
                    return Err(Error::Deserialization(format!(
                        "durable bound ({end} bytes) exceeds file length ({file_len} bytes)"
                    )));
                }
                end
            }
            None => file_len,
        };

        if start < HEADER_SIZE as u64 || start > end_bound {
            return Err(Error::Deserialization(format!(
                "start offset ({start}) outside [{}, {end_bound}]",
                HEADER_SIZE
            )));
        }

        let mut reader = BufReader::new(file);

        let mut header_buf = [0u8; HEADER_SIZE];
        reader.read_exact(&mut header_buf)?;
        let header = FileHeader::from_bytes(&header_buf)?;

        // The header read leaves the cursor at `HEADER_SIZE`; seek only
        // when starting at a later frame boundary.
        if start > HEADER_SIZE as u64 {
            reader.seek(SeekFrom::Start(start))?;
        }

        Ok(Self {
            reader,
            header,
            compressed_buf: Vec::with_capacity(1024 * 1024),
            data_buf: Vec::with_capacity(1024 * 1024),
            position: start,
            end_bound,
        })
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Advise the kernel to drop the file's pages from the page cache.
    /// Call this after you're done reading the file.
    pub fn drop_cache(&self) {
        #[cfg(target_os = "linux")]
        {
            use nix::fcntl::{PosixFadviseAdvice, posix_fadvise};
            let _ = posix_fadvise(
                self.reader.get_ref(),
                0,
                0,
                PosixFadviseAdvice::POSIX_FADV_DONTNEED,
            );
        }
    }

    pub fn next_frame(&mut self) -> Result<Option<Frame<'_>>> {
        // Stop cleanly at the bound: only read a header if the whole
        // header fits below it. `valid_up_to` (and a padded file length)
        // is frame-aligned, so in the normal case this fires exactly at
        // the last frame's end.
        if self.position + FRAME_HEADER_SIZE as u64 > self.end_bound {
            return Ok(None);
        }

        let mut frame_header = [0u8; FRAME_HEADER_SIZE];
        match self.reader.read_exact(&mut frame_header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let payload_len = u32::from_le_bytes(frame_header[0..4].try_into().unwrap()) as usize;
        let uncompressed_len = u32::from_le_bytes(frame_header[4..8].try_into().unwrap()) as usize;
        let entry_count = u32::from_le_bytes(frame_header[8..12].try_into().unwrap());
        let timestamp_ns = u64::from_le_bytes(frame_header[12..20].try_into().unwrap());
        let stored_crc = u32::from_le_bytes(frame_header[20..24].try_into().unwrap());

        if payload_len > MAX_FRAME_PAYLOAD {
            return Err(Error::Deserialization(format!(
                "frame payload ({payload_len} bytes) exceeds maximum ({MAX_FRAME_PAYLOAD} bytes)"
            )));
        }
        if uncompressed_len > MAX_FRAME_PAYLOAD {
            return Err(Error::Deserialization(format!(
                "uncompressed size ({uncompressed_len} bytes) exceeds maximum ({MAX_FRAME_PAYLOAD} bytes)"
            )));
        }

        let frame_bytes = FRAME_HEADER_SIZE + payload_len;
        let padding = (FRAME_ALIGNMENT - (frame_bytes % FRAME_ALIGNMENT)) % FRAME_ALIGNMENT;
        let total = (frame_bytes + padding) as u64;

        // The header fit, but the payload would cross the bound: the
        // prefix isn't frame-aligned at `end_bound` (a torn or misaligned
        // tail). Don't read the partial payload; latch done so a re-call
        // also stops, and stop cleanly — the caller's entry-count
        // cross-check catches the resulting short read.
        if self.position + total > self.end_bound {
            self.position = self.end_bound;
            return Ok(None);
        }

        self.compressed_buf.clear();
        self.compressed_buf.resize(payload_len, 0);
        self.reader.read_exact(&mut self.compressed_buf)?;

        if padding > 0 {
            let mut pad_buf = [0u8; FRAME_ALIGNMENT];
            self.reader.read_exact(&mut pad_buf[..padding])?;
        }
        self.position += total;

        if self.header.crc_enabled() {
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&(payload_len as u32).to_le_bytes());
            hasher.update(&(uncompressed_len as u32).to_le_bytes());
            hasher.update(&entry_count.to_le_bytes());
            hasher.update(&timestamp_ns.to_le_bytes());
            hasher.update(&self.compressed_buf);
            let actual_crc = hasher.finalize();
            if actual_crc != stored_crc {
                return Err(Error::CrcMismatch {
                    expected: stored_crc,
                    actual: actual_crc,
                });
            }
        }

        let lz4 = self.header.compression() == COMPRESSION_LZ4;
        if lz4 {
            self.data_buf.clear();
            self.data_buf.reserve(uncompressed_len);
            // SAFETY: decompress_into writes all output bytes before they are read.
            // We set the length so the slice is large enough, then truncate to actual output.
            unsafe {
                self.data_buf.set_len(uncompressed_len);
            }
            let n = lz4_flex::block::decompress_into(&self.compressed_buf, &mut self.data_buf)
                .map_err(|e| Error::Decompression(e.to_string()))?;
            self.data_buf.truncate(n);
        } else {
            self.data_buf.clear();
            self.data_buf.extend_from_slice(&self.compressed_buf);
        }

        Ok(Some(Frame {
            timestamp_ns: TimestampNs(timestamp_ns),
            entry_count,
            data: &self.data_buf,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Writer};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    /// Write one frame per payload, syncing after each so every frame
    /// boundary surfaces as a `Synced { valid_up_to }`. Returns the WAL
    /// path and the cumulative `valid_up_to` after each frame (i.e. the
    /// byte offset of the end of frame `i`).
    fn write_frames(dir: &Path, payloads: &[&[u8]]) -> (std::path::PathBuf, Vec<u64>) {
        let seq = Arc::new(AtomicU64::new(0));
        let mut writer = Writer::new(dir, Config::default(), seq).unwrap();
        let mut bounds = Vec::new();
        for (i, payload) in payloads.iter().enumerate() {
            writer
                .write_frame(
                    0,
                    payload,
                    1,
                    TimestampNs(i as u64 + 1),
                    TimestampNs::ZERO,
                    TimestampNs::ZERO,
                )
                .unwrap();
            writer.sync_all().unwrap();
            let valid_up_to = writer
                .take_all_events()
                .iter()
                .rev()
                .find_map(|e| match e {
                    crate::FileEvent::Synced { valid_up_to, .. } => Some(valid_up_to.0),
                    _ => None,
                })
                .expect("a Synced event after sync_all");
            bounds.push(valid_up_to);
        }
        writer.shutdown_all().unwrap();

        let path = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "wal"))
            .expect("a .wal file");
        (path, bounds)
    }

    fn collect(reader: &mut Reader) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        while let Some(frame) = reader.next_frame().unwrap() {
            out.push((frame.entry_count, frame.data.to_vec()));
        }
        out
    }

    #[test]
    fn open_reads_every_frame_to_eof() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _) = write_frames(dir.path(), &[b"alpha", b"bravo", b"charlie"]);

        let mut reader = Reader::open(&path).unwrap();
        let frames = collect(&mut reader);
        assert_eq!(
            frames,
            vec![
                (1, b"alpha".to_vec()),
                (1, b"bravo".to_vec()),
                (1, b"charlie".to_vec()),
            ]
        );
    }

    #[test]
    fn open_range_stops_at_the_bound() {
        let dir = tempfile::tempdir().unwrap();
        let (path, bounds) = write_frames(dir.path(), &[b"alpha", b"bravo", b"charlie"]);

        // Bound at the end of frame 1 → exactly the first two frames.
        let mut reader = Reader::open_range(&path, HEADER_SIZE as u64, bounds[1]).unwrap();
        let frames = collect(&mut reader);
        assert_eq!(frames, vec![(1, b"alpha".to_vec()), (1, b"bravo".to_vec())]);
    }

    #[test]
    fn open_range_starts_at_a_frame_offset() {
        let dir = tempfile::tempdir().unwrap();
        let (path, bounds) = write_frames(dir.path(), &[b"alpha", b"bravo", b"charlie"]);

        // Start at the end of frame 0 (= start of frame 1), read to EOF
        // of the durable prefix (end of frame 2).
        let mut reader = Reader::open_range(&path, bounds[0], bounds[2]).unwrap();
        let frames = collect(&mut reader);
        assert_eq!(frames, vec![(1, b"bravo".to_vec()), (1, b"charlie".to_vec())]);
    }

    #[test]
    fn open_range_mid_frame_bound_stops_at_prior_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let (path, bounds) = write_frames(dir.path(), &[b"alpha", b"bravo", b"charlie"]);

        // A bound one byte into frame 2: frame 2 doesn't fit fully, so
        // only the first two frames are yielded (the partial tail is
        // never read).
        let mut reader = Reader::open_range(&path, HEADER_SIZE as u64, bounds[1] + 1).unwrap();
        let frames = collect(&mut reader);
        assert_eq!(frames, vec![(1, b"alpha".to_vec()), (1, b"bravo".to_vec())]);
    }

    #[test]
    fn open_range_rejects_bound_past_eof() {
        let dir = tempfile::tempdir().unwrap();
        let (path, bounds) = write_frames(dir.path(), &[b"alpha"]);
        match Reader::open_range(&path, HEADER_SIZE as u64, bounds[0] + 4096) {
            Err(Error::Deserialization(_)) => {}
            Err(e) => panic!("wrong error: {e:?}"),
            Ok(_) => panic!("expected a bound-past-EOF error"),
        }
    }

    #[test]
    fn open_range_rejects_start_outside_window() {
        let dir = tempfile::tempdir().unwrap();
        let (path, bounds) = write_frames(dir.path(), &[b"alpha"]);
        // start below the header
        assert!(Reader::open_range(&path, 0, bounds[0]).is_err());
        // start past the end bound
        assert!(Reader::open_range(&path, bounds[0], HEADER_SIZE as u64).is_err());
    }

    #[test]
    fn open_range_empty_window_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _) = write_frames(dir.path(), &[b"alpha", b"bravo"]);
        // start == end (at the header): a zero-length durable prefix.
        let mut reader =
            Reader::open_range(&path, HEADER_SIZE as u64, HEADER_SIZE as u64).unwrap();
        assert!(reader.next_frame().unwrap().is_none());
    }
}
