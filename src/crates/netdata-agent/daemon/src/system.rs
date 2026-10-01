//! Host resources the daemon sizes itself by: the CPU count (`os_get_system_cpus_uncached()` and
//! `os_read_cpuset_cpus()`, `src/libnetdata/os/get_system_cpus.c`), memory (`os_system_memory()`,
//! `src/libnetdata/os/system_memory.c`) and the page size.

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_text::c::find;
use netdata_agent_text::parse::str2uint64;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use nix::sys::resource::{Resource, getrlimit, setrlimit};
use nix::unistd::{SysconfVar, sysconf};

#[derive(Debug, Clone, Copy)]
pub struct Resources {
    /// `os_get_system_cpus_uncached()`.
    pub system_cpus: i64,
    /// `os_system_memory(true)`; a zero total is `!OS_SYSTEM_MEMORY_OK`.
    pub memory: SystemMemory,
    pub page_size: i64,
}

impl Resources {
    pub fn probe() -> Self {
        let page_size = sysconf(SysconfVar::PAGE_SIZE)
            .ok()
            .flatten()
            .unwrap_or(4096);
        Resources {
            system_cpus: system_cpus(Path::new("/")),
            memory: system_memory(Path::new("/")),
            page_size,
        }
    }
}

/// `os_get_system_cpus_uncached()`: the online processors, else the configured ones, when more than one; otherwise
/// the `cpuN` lines of `<root>/proc/stat`.
pub fn system_cpus(root: &Path) -> i64 {
    for var in [SysconfVar::_NPROCESSORS_ONLN, SysconfVar::_NPROCESSORS_CONF] {
        if let Ok(Some(p)) = sysconf(var)
            && p > 1
        {
            return p;
        }
    }
    let counted = std::fs::read_to_string(root.join("proc/stat")).map(|stat| {
        stat.lines()
            .filter(|l| {
                l.split_whitespace().next().is_some_and(|w| {
                    w.strip_prefix("cpu")
                        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                })
            })
            .count() as i64
    });
    match counted {
        Ok(p) if p >= 1 => p,
        _ => {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "Cannot detect number of CPU cores. Assuming the system has 1 processors."
            );
            1
        }
    }
}

/// `read_txt_file()` into a buffer of `size` bytes: one read of at most `size - 1`, trailing newline kept.
pub fn read_txt_file(path: impl AsRef<Path>, size: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0; size.saturating_sub(1)];
    let n = std::fs::File::open(path).ok()?.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

/// `read_txt_file()` into a 4 KiB buffer.
fn read_txt(path: &Path) -> Option<Vec<u8>> {
    read_txt_file(path, 4096)
}

/// `os_read_cpuset_cpus()`: the CPUs a cpuset list (`0-3,8,10-11`) allows, 0 when unreadable or malformed. As in C,
/// the byte after each number or range is skipped, whatever it is.
pub fn cpuset_cpus(text: &[u8]) -> i64 {
    let mut s = 0;
    let mut ncpus: u64 = 0;
    let number = |s: &mut usize| {
        let start = *s;
        while *s < text.len() && text[*s].is_ascii_digit() {
            *s += 1;
        }
        str2uint64(&text[start..*s]).0
    };
    while s < text.len() {
        if text[s].is_ascii_whitespace() {
            s += 1;
            continue;
        }
        let n = number(&mut s);
        ncpus += 1;
        if text.get(s) == Some(&b',') {
            s += 1;
            continue;
        }
        if text.get(s) == Some(&b'-') {
            s += 1;
            let m = number(&mut s);
            if m < n {
                return 0;
            }
            ncpus += m - n;
        }
        s += 1;
    }
    ncpus as i64
}

/// What `os_system_memory()` reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemMemory {
    pub total: u64,
    pub available: u64,
}

/// Where `os_system_memory()` found the memory (`OS_MEM_SRC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemorySource {
    Meminfo,
    CgroupV1,
    CgroupV2,
}

/// `os_system_memory(true)`: cgroup v2, else v1, when it reports less than `/proc/meminfo`; else meminfo.
pub fn system_memory(root: &Path) -> SystemMemory {
    detect_memory(root).0
}

fn detect_memory(root: &Path) -> (SystemMemory, MemorySource) {
    let mi = meminfo(root);
    let v1 = cgroup_v1(root);
    let v2 = cgroup_v2(root);
    let fits = |m: &SystemMemory| {
        m.total != 0 && m.available != 0 && m.total <= mi.total && m.available < mi.available
    };
    if fits(&v2) {
        (v2, MemorySource::CgroupV2)
    } else if fits(&v1) {
        (v1, MemorySource::CgroupV1)
    } else {
        (mi, MemorySource::Meminfo)
    }
}

fn read_memory(root: &Path, source: MemorySource) -> SystemMemory {
    match source {
        MemorySource::Meminfo => meminfo(root),
        MemorySource::CgroupV1 => cgroup_v1(root),
        MemorySource::CgroupV2 => cgroup_v2(root),
    }
}

/// `os_system_memory()`'s cache: the last reading, when it was taken, and where it came from.
#[derive(Debug)]
struct MemoryCache {
    at: Option<Instant>,
    memory: SystemMemory,
    source: Option<MemorySource>,
}

impl MemoryCache {
    const fn new() -> Self {
        MemoryCache {
            at: None,
            memory: SystemMemory {
                total: 0,
                available: 0,
            },
            source: None,
        }
    }
}

/// `os_system_memory()` over `cache` at `now`: the reading of the last millisecond while it holds both numbers, else
/// read again from the source found last, detected anew when `detect` or when none was found yet; read outside the
/// cache's lock, its time taken before the read (C's `last_ut`).
fn cached_memory(
    cache: &Mutex<MemoryCache>,
    root: &Path,
    detect: bool,
    now: Instant,
) -> SystemMemory {
    let source = {
        let c = cache.lock().unwrap_or_else(PoisonError::into_inner);
        let fresh =
            c.at.is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(1));
        if fresh && c.memory.total != 0 && c.memory.available != 0 {
            return c.memory;
        }
        if detect { None } else { c.source }
    };
    let (memory, source) = match source {
        Some(source) => (read_memory(root, source), source),
        None => detect_memory(root),
    };
    *cache.lock().unwrap_or_else(PoisonError::into_inner) = MemoryCache {
        at: Some(now),
        memory,
        source: Some(source),
    };
    memory
}

/// `os_system_memory()`'s process-wide reading (`cached_memory()` of `/`).
pub fn system_memory_cached(detect: bool) -> SystemMemory {
    static CACHE: Mutex<MemoryCache> = Mutex::new(MemoryCache::new());
    cached_memory(&CACHE, Path::new("/"), detect, Instant::now())
}

/// The main cache's reading (`os_system_memory(false)`): the available bytes, while the total is known.
pub fn dbengine_memory_available() -> Option<u64> {
    let memory = system_memory_cached(false);
    (memory.total > 0).then_some(memory.available)
}

fn cgroup_v1(root: &Path) -> SystemMemory {
    cgroup_memory(
        root,
        "sys/fs/cgroup/memory/memory.limit_in_bytes",
        "sys/fs/cgroup/memory/memory.usage_in_bytes",
        "sys/fs/cgroup/memory/memory.stat",
        b"total_inactive_file ",
    )
}

fn cgroup_v2(root: &Path) -> SystemMemory {
    cgroup_memory(
        root,
        "sys/fs/cgroup/memory.max",
        "sys/fs/cgroup/memory.current",
        "sys/fs/cgroup/memory.stat",
        b"inactive_file ",
    )
}

/// `os_system_memory_meminfo()`.
fn meminfo(root: &Path) -> SystemMemory {
    let Some(buf) = read_txt(&root.join("proc/meminfo")) else {
        return SystemMemory::default();
    };
    let field = |key: &[u8]| {
        let at = find(&buf, key)? + key.len();
        let rest = &buf[at..];
        let digits = rest
            .iter()
            .position(|c| !c.is_ascii_whitespace())
            .unwrap_or(rest.len());
        Some(str2uint64(&rest[digits..]).0.wrapping_mul(1024))
    };
    match (field(b"MemTotal:"), field(b"MemAvailable:")) {
        (Some(total), Some(available)) => SystemMemory { total, available },
        _ => SystemMemory::default(),
    }
}

/// `os_system_memory_cgroup_v1()` and `_v2()`: the limit, the usage and the inactive page cache. The v2 limit `max`
/// is compared with the file's text, newline included, so it never matches and the limit reads as 0 (as in C).
fn cgroup_memory(
    root: &Path,
    limit: &str,
    usage: &str,
    stat: &str,
    inactive_key: &[u8],
) -> SystemMemory {
    let Some(buf) = read_txt(&root.join(limit)) else {
        return SystemMemory::default();
    };
    let total = if buf == b"max" {
        u64::MAX
    } else {
        str2uint64(&buf).0
    };
    if total == 0 {
        return SystemMemory::default();
    }
    let Some(buf) = read_txt(&root.join(usage)) else {
        return SystemMemory::default();
    };
    let used = str2uint64(&buf).0;
    if used == 0 || used > total {
        return SystemMemory::default();
    }
    let inactive = read_txt(&root.join(stat))
        .and_then(|buf| {
            let at = find(&buf, inactive_key)? + inactive_key.len();
            Some(str2uint64(&buf[at..]).0)
        })
        .filter(|&inactive| inactive != 0 && inactive <= used)
        .unwrap_or(0);
    SystemMemory {
        total,
        available: total - (used - inactive),
    }
}

/// `set_nofile_limit()`: the soft limit of open files raised to the hard one.
pub fn set_nofile_limit() {
    let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "getrlimit(RLIMIT_NOFILE) failed"
        );
        return;
    };
    nd_log!(
        Source::Daemon,
        Priority::Info,
        "resources control: allowed file descriptors: soft = {soft}, max = {hard}"
    );
    if let Err(e) = setrlimit(Resource::RLIMIT_NOFILE, hard, hard) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            errno = e as i32;
            "setrlimit(RLIMIT_NOFILE, {{ {hard}, {hard} }}) failed"
        );
    }
    match getrlimit(Resource::RLIMIT_NOFILE) {
        Ok((soft, _)) if soft < 1024 => nd_log!(
            Source::Daemon,
            Priority::Err,
            "Number of open file descriptors allowed for this process is too low (RLIMIT_NOFILE={soft})"
        ),
        Ok(_) => {}
        Err(_) => nd_log!(
            Source::Daemon,
            Priority::Err,
            "getrlimit(RLIMIT_NOFILE) failed"
        ),
    }
}

/// `os_run_dir()` (`src/libnetdata/os/run_dir.c`): `$NETDATA_RUN_DIR` when usable, else `/run/netdata`,
/// `/var/run/netdata` or `/tmp/netdata`, created when missing; cached, and exported as `NETDATA_RUN_DIR` once found
/// for writing.
pub fn run_dir(rw: bool) -> Option<String> {
    static CACHED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHED.get_or_init(|| detect_run_dir(rw)).clone()
}

fn is_dir_accessible(dir: &str, rw: bool) -> bool {
    use nix::unistd::{AccessFlags, access};
    std::fs::metadata(dir).is_ok_and(|m| m.is_dir())
        && access(
            dir,
            if rw {
                AccessFlags::W_OK
            } else {
                AccessFlags::R_OK
            },
        )
        .is_ok()
}

/// `netdata_dir_in_parent()`.
fn netdata_dir_in_parent(parent: &str, rw: bool) -> Option<String> {
    use std::os::unix::fs::DirBuilderExt;
    let path = format!("{parent}/netdata");
    if is_dir_accessible(&path, rw) {
        return Some(path);
    }
    if !is_dir_accessible(parent, rw) {
        return None;
    }
    match std::fs::DirBuilder::new().mode(0o755).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    is_dir_accessible(&path, rw).then_some(path)
}

fn detect_run_dir(rw: bool) -> Option<String> {
    use std::os::unix::fs::DirBuilderExt;
    if let Ok(dir) = std::env::var("NETDATA_RUN_DIR")
        && !dir.is_empty()
        && is_dir_accessible(&dir, rw)
    {
        return Some(dir);
    }
    let path =
        match netdata_dir_in_parent("/run", rw).or_else(|| netdata_dir_in_parent("/var/run", rw)) {
            Some(path) => path,
            None => {
                if !is_dir_accessible("/tmp", rw) && rw {
                    match std::fs::DirBuilder::new().mode(0o1777).create("/tmp") {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(_) => return None,
                    }
                }
                let path = "/tmp/netdata".to_string();
                if rw {
                    match std::fs::DirBuilder::new().mode(0o755).create(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(_) => return None,
                    }
                }
                path
            }
        };
    if rw {
        // Still single-threaded at the "run dir" startup step; the plugins inherit it.
        let _ = netdata_agent_spawn::env::set("NETDATA_RUN_DIR", &path);
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpuset_lists_count_like_c() {
        let cases = std::collections::BTreeMap::from([
            ("range", (&b"0-15\n"[..], 16)),
            ("list", (b"0-3,8,10-11\n", 7)),
            ("single", (b"5\n", 1)),
            ("descending range", (b"4-2\n", 0)),
            ("empty", (b"\n", 0)),
        ]);
        for (name, (text, want)) in cases {
            assert_eq!(cpuset_cpus(text), want, "{name}");
        }
    }

    #[test]
    fn the_reading_is_cached_as_c() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let meminfo = |total: u64, available: u64| {
            write(
                "proc/meminfo",
                &format!("MemTotal: {total} kB\nMemAvailable: {available} kB\n"),
            );
        };
        let cache = Mutex::new(MemoryCache::new());
        let t0 = Instant::now();
        let at = |us| t0 + Duration::from_micros(us);
        meminfo(16384, 8192);
        assert_eq!(
            cached_memory(&cache, root, false, at(0)).available,
            8192 * 1024
        );
        meminfo(16384, 4096);
        // a reading holds for a millisecond, detection asked or not
        assert_eq!(
            cached_memory(&cache, root, true, at(999)).available,
            8192 * 1024
        );
        assert_eq!(
            cached_memory(&cache, root, false, at(1000)).available,
            4096 * 1024
        );
        // a limited v2 cgroup appears: without detection meminfo stays the source, with it the cgroup is found
        write("sys/fs/cgroup/memory.max", "4194304\n");
        write("sys/fs/cgroup/memory.current", "1048576\n");
        write("sys/fs/cgroup/memory.stat", "inactive_file 524288\n");
        assert_eq!(
            cached_memory(&cache, root, false, at(2000)).total,
            16384 * 1024
        );
        assert_eq!(cached_memory(&cache, root, true, at(3000)).total, 4194304);
        assert_eq!(cached_memory(&cache, root, false, at(4000)).total, 4194304);
        // a reading without both numbers is not kept: the next call reads again within the millisecond
        write("sys/fs/cgroup/memory.current", "0\n");
        assert_eq!(
            cached_memory(&cache, root, false, at(5000)),
            SystemMemory::default()
        );
        write("sys/fs/cgroup/memory.current", "1048576\n");
        assert_eq!(cached_memory(&cache, root, false, at(5001)).total, 4194304);
    }

    #[test]
    fn memory_prefers_a_smaller_cgroup() {
        let root = std::env::temp_dir().join(format!("nd-sysmem-{}", std::process::id()));
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "proc/meminfo",
            "MemTotal:       16384 kB\nMemFree:  1 kB\nMemAvailable:    8192 kB\n",
        );
        assert_eq!(
            system_memory(&root),
            SystemMemory {
                total: 16384 * 1024,
                available: 8192 * 1024
            }
        );
        // An unlimited v2 cgroup reads "max\n": no limit parsed, meminfo stays.
        write("sys/fs/cgroup/memory.max", "max\n");
        write("sys/fs/cgroup/memory.current", "1000\n");
        assert_eq!(system_memory(&root).total, 16384 * 1024);
        // A limited one wins, minus what it uses, plus its inactive file pages.
        write("sys/fs/cgroup/memory.max", "4194304\n");
        write("sys/fs/cgroup/memory.current", "1048576\n");
        write(
            "sys/fs/cgroup/memory.stat",
            "active_file 5\ninactive_file 524288\n",
        );
        assert_eq!(
            system_memory(&root),
            SystemMemory {
                total: 4194304,
                available: 4194304 - (1048576 - 524288)
            }
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
