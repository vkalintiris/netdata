//! The engine's file access as `rrdenginelib.c` does it: `open_file_for_io()` with direct I/O first and C's fallback
//! record, `check_file_properties()`, unlinks with C's failure record, positioned reads and writes that go through an
//! aligned buffer when a file is open for direct I/O, and C's write retries.

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
            unlink_failed(path, &err);
            false
        }
    }
}

/// A journal's unlink (`journalfile_destroy_unsafe()`): a file already gone is fine, any other failure recorded;
/// whether nothing was recorded.
pub fn unlink_if_exists(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            unlink_failed(path, &err);
            false
        }
        _ => true,
    }
}

fn unlink_failed(path: &Path, err: &io::Error) {
    let errno = err.raw_os_error().unwrap_or(0);
    nd_log!(Source::Daemon, Priority::Err, errno = errno;
        "DBENGINE: uv_fs_unlink(\"{}\"): {}", path.display(), uv_strerror(errno));
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

/// Writes whole blocks at `offset` of a file, through an aligned buffer when it is open for direct I/O.
pub fn write_at(file: &IoFile, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(test)]
    if let Some(errno) = fault::next() {
        return Err(io::Error::from_raw_os_error(errno));
    }
    if !file.direct {
        return file.file.write_all_at(buf, offset);
    }
    let mut raw = vec![0u8; buf.len() + BLOCK_SIZE];
    let skip = raw.as_ptr().align_offset(BLOCK_SIZE);
    raw[skip..skip + buf.len()].copy_from_slice(buf);
    file.file.write_all_at(&raw[skip..skip + buf.len()], offset)
}

/// C's write loop (`retries = 10; while(ret < 0 && --retries)`): up to 9 attempts, 300 ms apart after each failure a
/// retry can pass (the last one too); an error no retry passes stops it at once. The last result.
pub fn write_retrying(file: &IoFile, buf: &[u8], offset: u64) -> io::Result<()> {
    let mut written = Err(io::ErrorKind::Other.into());
    for _ in 0..9 {
        written = write_at(file, buf, offset);
        match &written {
            Ok(()) => break,
            Err(err)
                if matches!(
                    err.raw_os_error(),
                    Some(libc::ENOSPC | libc::EBADF | libc::EACCES | libc::EROFS | libc::EINVAL)
                ) =>
            {
                break;
            }
            Err(_) => pause(),
        }
    }
    written
}

/// The pause between write attempts; tests do not wait.
fn pause() {
    #[cfg(not(test))]
    std::thread::sleep(std::time::Duration::from_millis(300));
}

/// Scripted write failures for tests, per thread.
#[cfg(test)]
pub(crate) mod fault {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static SCRIPT: RefCell<VecDeque<Option<i32>>> = const { RefCell::new(VecDeque::new()) };
    }

    /// The next writes of this thread, in order: `Some(errno)` fails one, `None` lets one through; writes past the
    /// script pass.
    pub fn script(steps: impl IntoIterator<Item = Option<i32>>) {
        SCRIPT.with(|s| *s.borrow_mut() = steps.into_iter().collect());
    }

    pub(super) fn next() -> Option<i32> {
        SCRIPT.with(|s| s.borrow_mut().pop_front().flatten())
    }
}
