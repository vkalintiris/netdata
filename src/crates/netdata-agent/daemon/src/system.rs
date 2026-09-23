//! Host resources the daemon sizes itself by: `os_get_system_cpus_uncached()` (`src/libnetdata/os/get_system_cpus.c`),
//! the total RAM of `os_system_memory()`, and the page size.

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
