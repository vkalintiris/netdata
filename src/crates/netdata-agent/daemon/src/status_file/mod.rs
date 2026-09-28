//! The daemon status file `status-netdata.json` (`src/daemon/status-file.c`, `status-file.h`; D87, D88): the
//! agent's state, saved at every startup and shutdown step and every 15 minutes, and read at the next start for the
//! restart and crash accounting. This module holds its model, its JSON writer and parser, its file I/O, and this
//! run's records (`session`) with the live values they are refreshed from (`live`).

mod dedup;
mod dmi;
pub mod io;
mod json;
mod live;
mod parse;
mod product;
mod report;
mod session;
pub mod signal_code;

pub(crate) use crate::exit_reason;

use netdata_agent_text::c::c_str;
use netdata_agent_text::datetime::{RFC3339_MAX_LENGTH, rfc3339_datetime_utc};

pub use json::to_json;
pub use parse::from_json;
pub use live::{set_db_mode, set_db_tiers, set_host_prefix, set_hosts, set_oom_protection, set_profile};
pub use session::{
    SHUTDOWN_TIMINGS_HEADER, check_crash, deadly_signal, init, product, register_fatal, shutdown_step, shutdown_timeout,
    startup_step, update_status,
};

/// `STATUS_FILE_VERSION`.
pub const VERSION: u32 = 29;

/// A C `char[N]`: at most N-1 bytes before its NUL. A longer text is cut, a UTF-8 character too, as C cuts it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FixedStr<const N: usize>([u8; N]);

impl<const N: usize> Default for FixedStr<N> {
    fn default() -> Self {
        FixedStr([0; N])
    }
}

impl<const N: usize> std::fmt::Debug for FixedStr<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", String::from_utf8_lossy(self.as_bytes()))
    }
}

impl<const N: usize> FixedStr<N> {
    /// `safecpy()`/`strncpyz()`: the text up to its NUL, cut at N-1 bytes.
    pub fn set(&mut self, text: impl AsRef<[u8]>) {
        let text = c_str(text.as_ref());
        let n = text.len().min(N - 1);
        self.0[..n].copy_from_slice(&text[..n]);
        self.0[n..].fill(0);
    }

    pub fn from(text: impl AsRef<[u8]>) -> Self {
        let mut s = Self::default();
        s.set(text);
        s
    }

    pub fn as_bytes(&self) -> &[u8] {
        c_str(&self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0[0] == 0
    }
}

/// An RFC 3339 text C keeps next to its time (`RFC3339_MAX_LENGTH`).
pub type Rfc3339 = FixedStr<RFC3339_MAX_LENGTH>;

/// The UTC RFC 3339 text of a time, centiseconds truncated, as C pre-computes it; empty for 0 is the caller's choice.
pub fn rfc3339(ut: u64) -> Rfc3339 {
    FixedStr::from(rfc3339_datetime_utc(ut, 2))
}

/// The first of `names` whose text is `name`, else `default` (`ENUM_STR_DEFINE_FUNCTIONS`' `_2id`).
fn id_of<T: Copy>(names: &[(T, &str)], name: &[u8], default: T) -> T {
    names
        .iter()
        .find(|(_, n)| n.as_bytes() == name)
        .map_or(default, |(id, _)| *id)
}

/// The name of `id`, else `default` (`_2str`).
fn name_of<T: Copy + PartialEq>(
    names: &[(T, &'static str)],
    id: T,
    default: &'static str,
) -> &'static str {
    names
        .iter()
        .find(|(i, _)| *i == id)
        .map_or(default, |(_, n)| n)
}

/// The names of a bitmap's bits, each name's bits taken once (`BITMAP_STR_DEFINE_FUNCTIONS`' `_2json`).
pub fn bitmap_names<'a>(names: &'a [(u32, &'static str)], mut bits: u32) -> impl Iterator<Item = &'static str> + 'a {
    let mut rest = names.iter();
    std::iter::from_fn(move || {
        for (bit, name) in rest.by_ref() {
            if bits == 0 {
                return None;
            }
            if bits & bit == *bit {
                bits &= !bit;
                return Some(*name);
            }
        }
        None
    })
}

/// `DAEMON_STATUS`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DaemonStatus {
    #[default]
    None,
    Initializing,
    Running,
    Exiting,
    Exited,
}

const DAEMON_STATUS_NAMES: [(DaemonStatus, &str); 5] = [
    (DaemonStatus::None, "none"),
    (DaemonStatus::Initializing, "initializing"),
    (DaemonStatus::Running, "running"),
    (DaemonStatus::Exiting, "exiting"),
    (DaemonStatus::Exited, "exited"),
];

impl DaemonStatus {
    pub fn name(self) -> &'static str {
        name_of(&DAEMON_STATUS_NAMES, self, "none")
    }

    pub fn from_name(name: &[u8]) -> Self {
        id_of(&DAEMON_STATUS_NAMES, name, DaemonStatus::None)
    }
}

/// `DAEMON_OS_TYPE`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OsType {
    #[default]
    Unknown,
    Linux,
    FreeBsd,
    MacOs,
    Windows,
}

const OS_TYPE_NAMES: [(OsType, &str); 5] = [
    (OsType::Unknown, "unknown"),
    (OsType::Linux, "linux"),
    (OsType::FreeBsd, "freebsd"),
    (OsType::MacOs, "macos"),
    (OsType::Windows, "windows"),
];

impl OsType {
    pub fn name(self) -> &'static str {
        name_of(&OS_TYPE_NAMES, self, "unknown")
    }

    pub fn from_name(name: &[u8]) -> Self {
        id_of(&OS_TYPE_NAMES, name, OsType::Unknown)
    }
}

/// `ND_PROFILE`'s names (`netdata-conf-profile.c`).
pub mod profile {
    pub const PARENT: u32 = 1 << 30;
    pub const STANDALONE: u32 = 1 << 29;
    pub const CHILD: u32 = 1 << 28;
    pub const IOT: u32 = 1 << 27;

    pub const NAMES: [(u32, &str); 4] = [
        (STANDALONE, "standalone"),
        (PARENT, "parent"),
        (CHILD, "child"),
        (IOT, "iot"),
    ];

    pub fn from_name(name: &[u8]) -> u32 {
        super::id_of(&NAMES, name, 0)
    }
}

/// `CLOUD_STATUS` (`claim/cloud-status.h`): 0 is not a status; it prints and parses as `available`.
pub mod cloud_status {
    pub const AVAILABLE: u8 = 1;
    pub const BANNED: u8 = 2;
    pub const OFFLINE: u8 = 3;
    pub const INDIRECT: u8 = 4;
    pub const ONLINE: u8 = 5;

    const NAMES: [(u8, &str); 5] = [
        (ONLINE, "online"),
        (INDIRECT, "indirect"),
        (AVAILABLE, "available"),
        (BANNED, "banned"),
        (OFFLINE, "offline"),
    ];

    pub fn name(status: u8) -> &'static str {
        super::name_of(&NAMES, status, "available")
    }

    pub fn from_name(name: &[u8]) -> u8 {
        super::id_of(&NAMES, name, AVAILABLE)
    }
}

/// `RRD_DB_MODE`'s values and names (`rrd_memory_mode_name()`, `rrd_memory_mode_id()`).
pub mod db_mode {
    pub const NONE: u8 = 0;
    pub const RAM: u8 = 1;
    pub const ALLOC: u8 = 4;
    pub const DBENGINE: u8 = 5;

    const NAMES: [(u8, &str); 4] = [
        (NONE, "none"),
        (RAM, "ram"),
        (ALLOC, "alloc"),
        (DBENGINE, "dbengine"),
    ];

    pub fn name(mode: u8) -> &'static str {
        super::name_of(&NAMES, mode, "ram")
    }

    pub fn from_name(name: &[u8]) -> u8 {
        super::id_of(&NAMES, name, RAM)
    }
}

/// `ND_MACHINE_GUID`: the machine GUID and when its file was last modified.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostId {
    /// The GUID's text when it comes from the GUID file, empty in a loaded record (C's parser leaves it); only the
    /// crash report's hash reads it.
    pub txt: FixedStr<37>,
    pub uuid: [u8; 16],
    pub last_modified_ut: u64,
    pub last_modified_rfc3339: Rfc3339,
}

/// The status file's timings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timings {
    pub init_started_ut: u64,
    pub init: i64,
    pub exit_started_ut: u64,
    pub exit: i64,
}

/// `OS_SYSTEM_MEMORY`.
pub use crate::system::SystemMemory as Memory;

/// `OS_SYSTEM_DISK_SPACE`.
pub use netdata_agent_sys::DiskSpace;

/// The disk footprint of the agent's files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskFootprint {
    pub dbengine: u64,
    pub sqlite: u64,
    pub other: u64,
    pub last_updated_ut: u64,
    pub last_updated_rfc3339: Rfc3339,
}

/// A collected and available count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub collected: u64,
    pub available: u64,
}

/// `RRDSTATS_METADATA` as the file keeps it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Metrics {
    pub nodes_total: u64,
    pub nodes_receiving: u64,
    pub nodes_sending: u64,
    pub nodes_archived: u64,
    pub metrics: Counts,
    pub instances: Counts,
    pub contexts: Counts,
}

/// `DMI_INFO` (`status-file-dmi.h`), the members the file carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dmi {
    pub sys_vendor: FixedStr<64>,
    pub sys_uuid: FixedStr<64>,
    pub product_id: FixedStr<64>,
    pub product_name: FixedStr<96>,
    pub product_version: FixedStr<64>,
    pub product_sku: FixedStr<64>,
    pub product_family: FixedStr<64>,
    pub board_name: FixedStr<64>,
    pub board_version: FixedStr<64>,
    pub board_vendor: FixedStr<64>,
    pub chassis_type: FixedStr<16>,
    pub chassis_vendor: FixedStr<64>,
    pub chassis_version: FixedStr<64>,
    pub bios_date: FixedStr<16>,
    pub bios_release: FixedStr<64>,
    pub bios_version: FixedStr<64>,
    pub bios_vendor: FixedStr<64>,
}

/// The normalized product, from the cloud provider and the hardware.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Product {
    pub vendor: FixedStr<64>,
    pub id: FixedStr<64>,
    pub name: FixedStr<96>,
    pub kind: FixedStr<16>,
}

/// What a fatal error, a deadly signal or a shutdown step leaves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fatal {
    pub line: i64,
    pub filename: FixedStr<256>,
    pub function: FixedStr<128>,
    pub errno: FixedStr<64>,
    pub message: FixedStr<512>,
    pub stack_trace: FixedStr<4096>,
    /// `ND_THREAD_TAG_MAX + 1`.
    pub thread: FixedStr<16>,
    pub thread_id: i32,
    pub signal_code: u64,
    pub fault_address: u64,
    pub worker_job_id: u32,
    pub sentry: bool,
}

/// `DAEMON_STATUS_FILE`: plain data, copied whole as C copies its snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusFile {
    /// The record's version: the gates of the writer.
    pub v: u32,
    pub version: FixedStr<32>,
    pub status: DaemonStatus,
    pub exit_reason: u32,
    pub profile: u32,
    pub os_type: OsType,
    pub db_mode: u8,
    pub cloud_status: u8,
    pub db_tiers: u8,
    pub kubernetes: bool,
    pub sentry_available: bool,

    pub boottime: i64,
    pub uptime: i64,
    pub timestamp_ut: u64,
    pub timestamp_rfc3339: Rfc3339,
    pub restarts: u64,
    pub crashes: u64,
    pub posts: u64,
    pub reliability: i64,
    pub pid: i32,

    pub host_id: HostId,
    pub boot_id: [u8; 16],
    pub invocation: [u8; 16],
    pub node_id: [u8; 16],
    pub claim_id: [u8; 16],
    pub machine_id: [u8; 16],

    pub timings: Timings,
    pub oom_protection: u64,
    pub netdata_max_rss: u64,
    pub memory: Memory,
    pub var_cache: DiskSpace,
    pub disk_footprint: DiskFootprint,
    pub metrics: Metrics,

    pub install_type: FixedStr<32>,
    pub architecture: FixedStr<32>,
    pub virtualization: FixedStr<32>,
    pub container: FixedStr<32>,
    pub kernel_version: FixedStr<32>,
    pub os_name: FixedStr<32>,
    pub os_version: FixedStr<32>,
    pub os_id: FixedStr<64>,
    pub os_id_like: FixedStr<64>,
    pub timezone: FixedStr<32>,
    pub cloud_provider_type: FixedStr<32>,
    pub cloud_instance_type: FixedStr<32>,
    pub cloud_instance_region: FixedStr<32>,
    pub system_cpus: u64,

    pub hw: Dmi,
    pub product: Product,
    pub stack_traces: FixedStr<63>,
    pub fatal: Fatal,
    /// Localhost's system info was copied in (it is, once); not in the file.
    pub read_system_info: bool,
}

#[cfg(test)]
mod tests;
