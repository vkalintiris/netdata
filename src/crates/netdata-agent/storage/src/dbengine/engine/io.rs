//! The engine's file access as `rrdenginelib.c` does it: `open_file_for_io()` with direct I/O first and C's fallback
//! record, `check_file_properties()`, unlinks with C's failure record, and positioned reads that go through an aligned
//! buffer when a file is open for direct I/O.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error, uv_strerror};

use crate::dbengine::format::{BLOCK_SIZE, ReadAt};

/// A file of the engine, and whether it is open for direct I/O.
#[derive(Debug)]
pub struct IoFile {
    pub file: File,
    pub direct: bool,
}

/// `open_file_for_io()`: direct I/O first when asked, falling back to buffered I/O with C's record when the file
/// system refuses it (`EINVAL`); any other failure is recorded with its errno. Files are created 0600.
pub fn open_for_io(path: &Path, create: bool, direct: bool) -> io::Result<IoFile> {
    let open = |direct: bool| {
        let mut o = OpenOptions::new();
        o.read(true).write(true).mode(0o600);
        if create {
            o.create(true).truncate(true);
        }
        if direct {
            o.custom_flags(libc::O_DIRECT);
        }
        o.open(path)
    };
    if direct {
        match open(true) {
            Ok(file) => return Ok(IoFile { file, direct: true }),
            Err(err) if err.raw_os_error() == Some(libc::EINVAL) => {
                nd_log!(Source::Daemon, Priority::Err, errno = libc::EINVAL;
                    "File \"{}\" does not support direct I/O, falling back to buffered I/O.", path.display());
            }
            Err(err) => {
                nd_log!(Source::Daemon, Priority::Err, errno = err.raw_os_error().unwrap_or(0);
                    "Failed to open file \"{}\".", path.display());
                return Err(err);
            }
        }
    }
    match open(false) {
        Ok(file) => Ok(IoFile {
            file,
            direct: false,
        }),
        Err(err) => {
            nd_log!(Source::Daemon, Priority::Err, errno = err.raw_os_error().unwrap_or(0);
                "Failed to open file \"{}\".", path.display());
            Err(err)
        }
    }
}

/// `check_file_properties()`: a regular file of at least `min_size` bytes; its size.
pub fn check_file_properties(file: &File, min_size: u64) -> Option<u64> {
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        netdata_log_error!("Not a regular file.\n");
        return None;
    }
    if meta.len() < min_size {
        netdata_log_error!("File length is too short.\n");
        return None;
    }
    Some(meta.len())
}

/// `UNLINK_FILE()`: true when removed; a failure is recorded with libuv's text.
pub fn unlink(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(err) => {
            let errno = err.raw_os_error().unwrap_or(0);
            netdata_log_error!(
                "DBENGINE: uv_fs_unlink(\"{}\"): {}",
                path.display(),
                uv_strerror(errno)
            );
            false
        }
    }
}

/// `ALIGN_BYTES_CEILING()` and `ALIGN_BYTES_FLOOR()` to the 4096-byte block.
pub fn align_ceiling(n: u64) -> u64 {
    n.div_ceil(BLOCK_SIZE as u64) * BLOCK_SIZE as u64
}

pub fn align_floor(n: u64) -> u64 {
    n / BLOCK_SIZE as u64 * BLOCK_SIZE as u64
}

impl ReadAt for IoFile {
    /// Direct I/O needs block-aligned offsets, lengths and memory: a read through a direct file goes through an
    /// aligned buffer covering whole blocks.
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        if !self.direct {
            return FileExt::read_exact_at(&self.file, buf, offset);
        }
        let block = BLOCK_SIZE as u64;
        let start = offset / block * block;
        let end = (offset + buf.len() as u64).div_ceil(block) * block;
        let len = (end - start) as usize;
        let mut raw = vec![0u8; len + BLOCK_SIZE];
        let skip = raw.as_ptr().align_offset(BLOCK_SIZE);
        let aligned = &mut raw[skip..skip + len];
        FileExt::read_exact_at(&self.file, aligned, start)?;
        let from = (offset - start) as usize;
        buf.copy_from_slice(&aligned[from..from + buf.len()]);
        Ok(())
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
}

/// Writes a whole block at `offset` of a file, through an aligned buffer when it is open for direct I/O.
pub fn write_block(file: &IoFile, block: &[u8; BLOCK_SIZE], offset: u64) -> io::Result<()> {
    if !file.direct {
        return file.file.write_all_at(block, offset);
    }
    let mut raw = vec![0u8; 2 * BLOCK_SIZE];
    let skip = raw.as_ptr().align_offset(BLOCK_SIZE);
    raw[skip..skip + BLOCK_SIZE].copy_from_slice(block);
    file.file
        .write_all_at(&raw[skip..skip + BLOCK_SIZE], offset)
}
