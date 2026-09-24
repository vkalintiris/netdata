//! Host resources the daemon sizes itself by: `os_get_system_cpus_uncached()` (`src/libnetdata/os/get_system_cpus.c`),
//! the total RAM of `os_system_memory()`, and the page size.

use netdata_agent_inicfg::LogLevel;
use nix::sys::resource::{Resource, getrlimit, setrlimit};
use nix::unistd::{SysconfVar, sysconf};

#[derive(Debug, Clone, Copy)]
pub struct Resources {
    /// The `cpuN` lines of `/proc/stat`, at least 1.
    pub cpus: i64,
    /// `MemTotal` of `/proc/meminfo`, when readable.
    pub ram_total_bytes: Option<u64>,
    pub page_size: i64,
}

impl Resources {
    pub fn probe() -> Self {
        let cpus = std::fs::read_to_string("/proc/stat")
            .map(|stat| {
                stat.lines()
                    .filter(|l| {
                        l.strip_prefix("cpu")
                            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                    })
                    .count() as i64
            })
            .unwrap_or(0)
            .max(1);
        let ram_total_bytes = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| {
            m.lines()
                .find_map(|l| l.strip_prefix("MemTotal:"))
                .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                .map(|kib| kib * 1024)
        });
        let page_size = sysconf(SysconfVar::PAGE_SIZE)
            .ok()
            .flatten()
            .unwrap_or(4096);
        Resources {
            cpus,
            ram_total_bytes,
            page_size,
        }
    }

    /// `libuv_worker_threads` of `libuv_initialize()` without its memory cap and `[global] libuv worker threads`
    /// (both come with the port of `libuv_initialize()`): six per CPU, 16..=128.
    pub fn libuv_worker_threads(&self) -> i64 {
        (self.cpus * 6).clamp(16, 128)
    }
}

/// `set_nofile_limit()`: the soft limit of open files raised to the hard one.
pub fn set_nofile_limit(log: &mut impl FnMut(LogLevel, &str)) {
    let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) else {
        log(LogLevel::Error, "getrlimit(RLIMIT_NOFILE) failed");
        return;
    };
    log(
        LogLevel::Info,
        &format!("resources control: allowed file descriptors: soft = {soft}, max = {hard}"),
    );
    if setrlimit(Resource::RLIMIT_NOFILE, hard, hard).is_err() {
        log(
            LogLevel::Error,
            &format!("setrlimit(RLIMIT_NOFILE, {{ {hard}, {hard} }}) failed"),
        );
    }
    match getrlimit(Resource::RLIMIT_NOFILE) {
        Ok((soft, _)) if soft < 1024 => log(
            LogLevel::Error,
            &format!(
                "Number of open file descriptors allowed for this process is too low (RLIMIT_NOFILE={soft})"
            ),
        ),
        Ok(_) => {}
        Err(_) => log(LogLevel::Error, "getrlimit(RLIMIT_NOFILE) failed"),
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
        let _ = netdata_agent_sys::setenv("NETDATA_RUN_DIR", &path);
    }
    Some(path)
}
