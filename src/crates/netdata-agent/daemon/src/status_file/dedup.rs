//! `status-file-dedup.c`: the hashes of the last runs' reports this agent posted, so the same crash is reported once a
//! day. `dedup-netdata.dat` is C's `DAEMON_STATUS_DEDUP` written raw (1224 bytes on 64-bit Linux, native byte
//! order); the table is kept as those bytes, so a file C wrote keeps its padding when this agent rewrites it. The
//! sentry side of C's table (its signal handler's reports) exists only in builds with sentry and is not ported.

use std::ffi::CStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use super::io::{self, Locations};
use super::StatusFile;

/// `DEDUP_FILENAME`.
pub const DEDUP_FILENAME: &CStr = c"dedup-netdata.dat";
/// `DEDUP_MAGIC`.
const MAGIC: u64 = 0x1DED_A9F1_7EDA_7150;
/// `DEDUP_VERSION`, a `size_t`.
const VERSION: u64 = 1;
/// `REPORT_EVENTS_EVERY` in microseconds: a day less an hour, for cron's randomness.
const WINDOW_US: u64 = (86_400 - 3_600) * 1_000_000;

/// The layout of `DAEMON_STATUS_DEDUP` on 64-bit targets (DWARF of the production build): the magic, the version and
/// the slots' hash, then 50 slots of `{bool sentry; 7 padding; u64 hash; u64 timestamp_ut}`. 32-bit targets lay it out
/// otherwise (BACKLOG).
const SLOTS: usize = 50;
const SLOT: usize = 24;
const HEADER: usize = 24;
const SIZE: usize = HEADER + SLOTS * SLOT;

/// The table (`static DAEMON_STATUS_DEDUP dedup`), zero until a file is read.
static TABLE: Mutex<[u8; SIZE]> = Mutex::new([0; SIZE]);

/// `fnv1a_hash_bin64()`.
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(14_695_981_039_346_656_037, |hash, &b| (hash ^ u64::from(b)).wrapping_mul(1_099_511_628_211))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_ne_bytes(b)
}

fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

/// `stack_trace_anonymize()`: every `0x` and the hex digits after it become zeros, so addresses that move between
/// runs hash alike. The text ends at its NUL.
fn anonymize(text: &mut [u8]) {
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    let mut at = 0;
    while at + 1 < end {
        if text[at] != b'0' || text[at + 1] != b'x' {
            at += 1;
            continue;
        }
        text[at + 1] = b'0';
        at += 2;
        while at < end && text[at].is_ascii_hexdigit() {
            text[at] = b'0';
            at += 1;
        }
    }
}

/// The byte size of `daemon_status_file_hash()`'s zeroed `to_hash` struct on 64-bit targets.
const HASHED: usize = 4856;

/// `daemon_status_file_hash()`: FNV-1a over C's zeroed struct of the record's identity, its fatal error (the stack
/// trace anonymized), the message and the cause, at the struct's offsets (DWARF of the production build). A loaded
/// record's host id has no text (C's parser fills only the UUID and the time).
pub fn hash(ds: &StatusFile, msg: &str, cause: &str) -> u64 {
    let mut b = [0u8; HASHED];
    let text = |b: &mut [u8], at: usize, size: usize, value: &[u8]| {
        let n = value.len().min(size - 1);
        b[at..at + n].copy_from_slice(&value[..n]);
    };
    put(&mut b, 0, &ds.v.to_ne_bytes());
    put(&mut b, 4, &(ds.status as u32).to_ne_bytes());
    put(&mut b, 8, &ds.fatal.signal_code.to_ne_bytes());
    put(&mut b, 16, &ds.profile.to_ne_bytes());
    put(&mut b, 20, &ds.exit_reason.to_ne_bytes());
    b[24] = ds.db_mode;
    put(&mut b, 28, &ds.fatal.worker_job_id.to_ne_bytes());
    b[32] = ds.db_tiers;
    b[33] = u8::from(ds.kubernetes);
    b[34] = u8::from(ds.sentry_available);
    b[35] = u8::from(ds.fatal.sentry);
    // ND_MACHINE_GUID: txt[37], 3 padding, uuid, last_modified_ut, rfc3339[36], 4 padding
    text(&mut b, 40, 37, ds.host_id.txt.as_bytes());
    put(&mut b, 80, &ds.host_id.uuid);
    put(&mut b, 96, &ds.host_id.last_modified_ut.to_ne_bytes());
    text(&mut b, 104, 36, ds.host_id.last_modified_rfc3339.as_bytes());
    put(&mut b, 144, &ds.machine_id);
    put(&mut b, 160, &ds.fatal.line.to_ne_bytes());
    text(&mut b, 168, 32, ds.version.as_bytes());
    text(&mut b, 200, 256, ds.fatal.filename.as_bytes());
    text(&mut b, 456, 128, ds.fatal.function.as_bytes());
    text(&mut b, 584, 4096, ds.fatal.stack_trace.as_bytes());
    text(&mut b, 4680, 16, ds.fatal.thread.as_bytes());
    text(&mut b, 4696, 128, msg.as_bytes());
    text(&mut b, 4824, 32, cause.as_bytes());
    anonymize(&mut b[584..4680]);
    fnv1a64(&b)
}

/// `status_file_dedup_load_and_parse()`: one read of the table; a short one, another magic or version, or slots that
/// do not match their hash leave the table empty. A longer file is read up to the table's size.
fn parse(path: &Path, table: &mut [u8; SIZE]) -> bool {
    table.fill(0);
    let read = File::open(path).and_then(|mut f| f.read(table));
    if matches!(read, Ok(SIZE))
        && u64_at(table, 0) == MAGIC
        && u64_at(table, 8) == VERSION
        && u64_at(table, 16) == fnv1a64(&table[HEADER..])
    {
        return true;
    }
    table.fill(0);
    false
}

/// `daemon_status_dedup_load(true)`: the newest file of the status file's locations replaces the table; the table
/// stays as it was when there is none (C's record says so).
fn reload(loc: &Locations, name: &CStr, table: &mut [u8; SIZE]) {
    io::load(loc, name, true, |path| parse(path, table));
}

/// `dedup_already_posted(..., false, DEDUP_RELOAD_FROM_DISK)`: whether a report with `hash` went out within the
/// window. A slot from the future does not count (C's unsigned difference wraps); sentry slots never match.
pub fn already_posted(loc: &Locations, hash: u64, now_ut: u64) -> bool {
    already_posted_in(loc, DEDUP_FILENAME, hash, now_ut)
}

fn already_posted_in(loc: &Locations, name: &CStr, hash: u64, now_ut: u64) -> bool {
    let mut table = TABLE.lock().unwrap_or_else(PoisonError::into_inner);
    reload(loc, name, &mut table);
    (0..SLOTS).map(|i| HEADER + i * SLOT).any(|at| {
        let ts = u64_at(&table[..], at + 16);
        ts != 0
            && u64_at(&table[..], at + 8) == hash
            && table[at] == 0
            && now_ut.wrapping_sub(ts) < WINDOW_US
    })
}

/// `dedup_keep_hash(..., false, DEDUP_RELOAD_FROM_DISK)`: `hash` stamped `now_ut` in its own (non-sentry) slot, else
/// the first empty one, else the oldest, then the table saved (unlogged, as C).
pub fn keep(loc: &Locations, hash: u64, now_ut: u64) {
    keep_in(loc, DEDUP_FILENAME, hash, now_ut);
}

fn keep_in(loc: &Locations, name: &CStr, hash: u64, now_ut: u64) {
    let mut table = TABLE.lock().unwrap_or_else(PoisonError::into_inner);
    reload(loc, name, &mut table);
    let slot = |i: usize| HEADER + i * SLOT;
    let own = (0..SLOTS).find(|&i| u64_at(&table[..], slot(i) + 8) == hash && table[slot(i)] == 0);
    let empty = || (0..SLOTS).find(|&i| u64_at(&table[..], slot(i) + 8) == 0);
    let oldest = || {
        (1..SLOTS).fold(0, |oldest, i| {
            if u64_at(&table[..], slot(i) + 16) < u64_at(&table[..], slot(oldest) + 16) { i } else { oldest }
        })
    };
    let at = slot(own.or_else(empty).unwrap_or_else(oldest));
    table[at] = 0;
    put(&mut table[..], at + 8, &hash.to_ne_bytes());
    put(&mut table[..], at + 16, &now_ut.to_ne_bytes());
    // daemon_status_dedup_save()
    let slots_hash = fnv1a64(&table[HEADER..]);
    put(&mut table[..], 0, &MAGIC.to_ne_bytes());
    put(&mut table[..], 8, &VERSION.to_ne_bytes());
    put(&mut table[..], 16, &slots_hash.to_ne_bytes());
    io::save(loc, name, &table[..], false);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a 64's published vectors.
    #[test]
    fn fnv1a_as_libnetdata() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    /// C's scan: the `0` of `0x` stays, the `x` and the digits after it become zeros, the scan goes on after them.
    #[test]
    fn stack_traces_anonymize_as_c() {
        let mut t = *b"at 0x7fFe12 in f (0x) and 10x0x1g\0 0xff";
        anonymize(&mut t);
        assert_eq!(&t, b"at 00000000 in f (00) and 1000x1g\0 0xff");
    }

    fn locations(dir: &Path) -> Locations {
        Locations::new(dir.to_str().unwrap(), dir.join("cache").to_str().unwrap())
    }

    /// The table is the process's: one test at a time, each from an empty table.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let guard = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        TABLE.lock().unwrap_or_else(PoisonError::into_inner).fill(0);
        guard
    }

    /// A name no other test or process uses: a save unlinks it from `/tmp`, `/run`, `/var/run` and `.` too.
    fn name(tag: &str) -> std::ffi::CString {
        std::ffi::CString::new(format!("dedup-netdata-test-{}-{tag}.dat", std::process::id())).unwrap()
    }

    /// A kept hash is posted within the window, not after it nor from the future; the file C would read back is the
    /// table's 1224 bytes with its header.
    #[test]
    fn a_kept_hash_is_posted_for_a_day_less_an_hour() {
        let _serial = fresh();
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let n = name("window");
        let posted = |hash, now| already_posted_in(&loc, &n, hash, now);
        let now = 1_800_000_000_000_000;
        keep_in(&loc, &n, 42, now);
        let bytes = std::fs::read(dir.path().join(n.to_str().unwrap())).unwrap();
        assert_eq!(bytes.len(), SIZE);
        assert_eq!((u64_at(&bytes, 0), u64_at(&bytes, 8)), (MAGIC, VERSION));
        assert_eq!(u64_at(&bytes, 16), fnv1a64(&bytes[HEADER..]));
        assert!(posted(42, now + WINDOW_US - 1));
        assert!(!posted(42, now + WINDOW_US));
        assert!(!posted(42, now - 1));
        assert!(!posted(43, now));
    }

    /// Slots are reused by hash, then the first empty one, then the oldest; a file that fails its checks empties the
    /// table, one longer than the table is read.
    #[test]
    fn slots_and_files_as_c() {
        let _serial = fresh();
        let dir = tempfile::tempdir().unwrap();
        let loc = locations(dir.path());
        let n = name("slots");
        for i in 0..SLOTS as u64 {
            keep_in(&loc, &n, i + 1, 1_000 + i);
        }
        keep_in(&loc, &n, 1, 5_000);
        keep_in(&loc, &n, 99, 6_000);
        let path = dir.path().join(n.to_str().unwrap());
        let bytes = std::fs::read(&path).unwrap();
        // slot 0 kept hash 1 (restamped), slot 1 (the oldest, 1001) went to 99
        assert_eq!((u64_at(&bytes, HEADER + 8), u64_at(&bytes, HEADER + 16)), (1, 5_000));
        assert_eq!((u64_at(&bytes, HEADER + SLOT + 8), u64_at(&bytes, HEADER + SLOT + 16)), (99, 6_000));
        let mut longer = bytes.clone();
        longer.extend_from_slice(&[7; 100]);
        std::fs::write(&path, &longer).unwrap();
        assert!(already_posted_in(&loc, &n, 99, 7_000));
        let mut corrupt = bytes.clone();
        corrupt[HEADER + 100] ^= 1;
        std::fs::write(&path, &corrupt).unwrap();
        assert!(!already_posted_in(&loc, &n, 99, 7_000));
        std::fs::write(&path, &bytes[..SIZE - 1]).unwrap();
        assert!(!already_posted_in(&loc, &n, 1, 7_000));
    }

    /// The struct's text fields hold at most their size less one byte, as `safecpy()` copies them.
    #[test]
    fn texts_are_cut_as_safecpy() {
        let mut ds = StatusFile::default();
        let a = hash(&ds, &"m".repeat(127), "c");
        let b = hash(&ds, &"m".repeat(200), "c");
        assert_eq!(a, b);
        ds.fatal.stack_trace.set("at 0x1234");
        let c = hash(&ds, "m", "c");
        ds.fatal.stack_trace.set("at 0xabcd");
        assert_eq!(c, hash(&ds, "m", "c"));
    }
}
