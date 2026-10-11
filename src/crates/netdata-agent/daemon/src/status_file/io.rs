//! `status-file-io.c`: where the status file lives and how it is written. The primary directory is `[directories]
//! lib`, the fallbacks the cache directory, `/tmp`, `/run`, `/var/run` and `.`. A save writes a temporary file and
//! renames it, calling only what a signal handler may (no allocation on its path, D88.4); the first save that lands
//! in the primary directory removes the file from every fallback, once per process.

use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use netdata_agent_log::{Priority, Source, nd_log};
use nix::errno::Errno;
use nix::fcntl::{AT_FDCWD, OFlag, open, renameat};
use nix::sys::stat::{FileStat, Mode, SFlag, fchmod, fstat, lstat};
use nix::unistd::{close, fsync, ftruncate, unlink, write};

/// `STATUS_FILENAME`.
pub const STATUS_FILENAME: &CStr = c"status-netdata.json";

/// `FILENAME_MAX`.
const PATH_MAX: usize = 4096;

/// `status_file_io_tmp_attempt_counter`: every attempt's temporary name, process-wide.
static TMP_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
/// The directories, prepared before any save so a signal handler's save allocates nothing. The process holds one,
/// shared by every file saved there, as C's statics are.
#[derive(Debug)]
pub struct Locations {
    primary: CString,
    /// The cache directory first (`status_file_io_fallback_dirs_update()`).
    fallbacks: [CString; 5],
    /// `status_file_io_obsolete_removed`: set by the first save of any file into the primary directory.
    obsolete_removed: AtomicBool,
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Locations {
    pub fn new(varlib: &str, cache: &str) -> Locations {
        Locations {
            primary: cstring(varlib),
            fallbacks: [
                cstring(cache),
                cstring("/tmp"),
                cstring("/run"),
                cstring("/var/run"),
                cstring("."),
            ],
            obsolete_removed: AtomicBool::new(false),
        }
    }
}

/// A path built on the stack, `strcatz()` style: what does not fit before the final NUL is cut.
struct StackPath {
    buf: [u8; PATH_MAX],
    len: usize,
}

impl StackPath {
    fn new() -> Self {
        StackPath {
            buf: [0; PATH_MAX],
            len: 0,
        }
    }

    fn push(&mut self, part: &[u8]) -> &mut Self {
        let n = part.len().min(PATH_MAX - 1 - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&part[..n]);
        self.len += n;
        self
    }

    /// `dir/filename`, the slash added only when `dir` does not end with one (the load's and the cleanup's paths).
    fn joined(dir: &CStr, filename: &CStr) -> Self {
        let mut p = StackPath::new();
        p.push(dir.to_bytes());
        if p.len == 0 || p.buf[p.len - 1] != b'/' {
            p.push(b"/");
        }
        p.push(filename.to_bytes());
        p
    }

    /// The buffer starts zeroed and only grows, so the byte after the text is always the NUL.
    fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.buf[..=self.len]).unwrap_or(c"")
    }

    fn as_path(&self) -> &Path {
        Path::new(OsStr::from_bytes(&self.buf[..self.len]))
    }
}

/// `status_file_io_check()` with `OS_FILE_METADATA_OK()`: the file's modification time, when `stat()` gives it both
/// a time and a size.
fn check(path: &StackPath) -> Option<i64> {
    let meta = std::fs::metadata(path.as_path()).ok()?;
    (meta.mtime() > 0 && meta.size() > 0).then_some(meta.mtime())
}

/// `status_file_io_load()`: the newest of the primary's and the fallbacks' files (a fallback replaces the choice
/// only when strictly newer) goes to `parse`; C's error record when there is none or `parse` refuses it.
pub fn load(
    loc: &Locations,
    filename: &CStr,
    log: bool,
    parse: impl FnOnce(&Path) -> bool,
) -> bool {
    let mut newest: Option<(StackPath, i64)> = None;
    let dirs = std::iter::once(&loc.primary).chain(&loc.fallbacks);
    for dir in dirs.filter(|d| !d.is_empty() && !filename.is_empty()) {
        let path = StackPath::joined(dir, filename);
        if let Some(mtime) = check(&path)
            && newest
                .as_ref()
                .is_none_or(|(_, newest_mtime)| mtime > *newest_mtime)
        {
            newest = Some((path, mtime));
        }
    }
    if let Some((path, _)) = newest
        && parse(path.as_path())
    {
        return true;
    }
    if log {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Cannot find a status file in any location"
        );
    }
    false
}

/// `read_txt_file_to_buffer()`: a regular file of at most `max` bytes, read whole (`O_NONBLOCK`, so a FIFO put in
/// its place cannot block the open).
pub fn read_text(path: &Path, max: u64) -> Option<Vec<u8>> {
    read_text_errno(path, max).ok()
}

/// [`read_text`], failing with the `errno` C's failed call leaves behind: 0 where C fails on a test of its own (not
/// a regular file, too large, a short read), which sets none.
pub fn read_text_errno(path: &Path, max: u64) -> Result<Vec<u8>, i32> {
    let os = |e: std::io::Error| e.raw_os_error().unwrap_or(0);
    let regular = |m: &std::fs::Metadata| m.file_type().is_file();
    if !regular(&std::fs::metadata(path).map_err(os)?) {
        return Err(0);
    }
    let mut file = File::options()
        .read(true)
        .custom_flags(OFlag::O_NONBLOCK.bits())
        .open(path)
        .map_err(os)?;
    let meta = file.metadata().map_err(os)?;
    if !regular(&meta) || meta.size() > max {
        return Err(0);
    }
    let mut content = vec![0; usize::try_from(meta.size()).map_err(|_| 0)?];
    file.read_exact(&mut content).map_err(os)?;
    Ok(content)
}

/// `status_file_io_remove_obsolete()`: once, the file leaves every fallback but the protected one.
fn remove_obsolete(loc: &Locations, protected: &CStr, filename: &CStr) {
    if loc.obsolete_removed.swap(true, Ordering::Relaxed) {
        return;
    }
    for dir in loc.fallbacks.iter().filter(|d| d.as_c_str() != protected) {
        let _ = unlink(StackPath::joined(dir, filename).as_cstr());
    }
}

/// `print_uint64()` into a stack buffer.
fn decimal(mut value: u64, out: &mut [u8; 20]) -> &[u8] {
    let mut at = out.len();
    loop {
        at -= 1;
        out[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    &out[at..]
}

fn is_regular(st: &FileStat) -> bool {
    SFlag::from_bits_truncate(st.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG
}

/// `status_file_io_save_this()`: a temporary `dir/filename-N` (a regular one left by an interrupted save is
/// reused), written whole, synced, made 0664, closed and renamed over `dir/filename`; a failure once the write
/// starts removes the temporary file.
fn save_this(dir: &CStr, filename: &CStr, data: &[u8]) -> bool {
    if dir.is_empty() {
        return false;
    }
    let attempt = TMP_ATTEMPTS.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    save_attempt(dir, filename, data, attempt)
}

/// [`save_this`] with the temporary file's number given.
pub(super) fn save_attempt(dir: &CStr, filename: &CStr, data: &[u8], attempt: u64) -> bool {
    let mut digits = [0u8; 20];
    let attempt = decimal(attempt, &mut digits);
    let (dir, filename) = (dir.to_bytes(), filename.to_bytes());
    if dir.len() + 1 + filename.len() + 1 + attempt.len() + 1 >= PATH_MAX {
        return false;
    }
    let mut target = StackPath::new();
    target.push(dir).push(b"/").push(filename);
    let mut temp = StackPath::new();
    temp.push(dir)
        .push(b"/")
        .push(filename)
        .push(b"-")
        .push(attempt);
    let (target, temp) = (target.as_cstr(), temp.as_cstr());

    let before = match lstat(temp) {
        Ok(st) if !is_regular(&st) => return false,
        Ok(st) => Some(st),
        Err(Errno::ENOENT) => None,
        Err(_) => return false,
    };
    let mut flags = OFlag::O_WRONLY | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW;
    if before.is_none() {
        flags |= OFlag::O_CREAT | OFlag::O_EXCL;
    }
    let mode = Mode::from_bits_truncate(0o664);
    let Ok(fd) = open(temp, flags, mode) else {
        return false;
    };
    // an early return drops `fd`, which closes it as C's `close(fd)` does
    let same = |after: &FileStat| {
        before.is_none_or(|b| b.st_dev == after.st_dev && b.st_ino == after.st_ino)
    };
    if !fstat(&fd).is_ok_and(|after| is_regular(&after) && same(&after)) {
        return false;
    }
    if before.is_some() && ftruncate(&fd, 0).is_err() {
        return false;
    }
    let mut written = 0;
    while written < data.len() {
        match write(&fd, &data[written..]) {
            Ok(n) if n > 0 => written += n,
            Err(Errno::EINTR) => {}
            _ => {
                drop(fd);
                let _ = unlink(temp);
                return false;
            }
        }
    }
    if fsync(&fd).is_err() || fchmod(&fd, mode).is_err() {
        drop(fd);
        let _ = unlink(temp);
        return false;
    }
    if close(fd).is_err() || renameat(AT_FDCWD, temp, AT_FDCWD, target).is_err() {
        let _ = unlink(temp);
        return false;
    }
    true
}

/// `status_file_io_save()`: the primary directory, else each fallback until one takes it; with `log`, C's records.
pub fn save(loc: &Locations, filename: &CStr, data: &[u8], log: bool) -> bool {
    if save_this(&loc.primary, filename, data) {
        remove_obsolete(loc, &loc.primary, filename);
        return true;
    }
    if log {
        let primary = loc.primary.to_string_lossy();
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Failed to save status file in primary directory {primary}"
        );
    }
    for dir in &loc.fallbacks {
        if save_this(dir, filename, data) {
            if log {
                let dir = dir.to_string_lossy();
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "Saved status file in fallback {dir}"
                );
            }
            return true;
        }
    }
    if log {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Failed to save status file in any location"
        );
    }
    false
}
