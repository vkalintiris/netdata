//! What the status file is refreshed from (`daemon_status_file_migrate_once()` and `daemon_status_file_refresh()`):
//! the process-wide values C reads from its globals, which the startup sets here as it computes them, and the probes of
//! the system (machine and boot ids, the peak memory, the disk space, the disk footprint). Map:
//! `knowledge/map-status-file-refresh-inputs.md` in the status repository.

use std::collections::HashSet;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};

use super::{DaemonStatus, DiskSpace, HostId, OsType, StatusFile, cloud_status, db_mode, exit_reason, rfc3339};
use crate::{guid, system};

/// `stacktrace_backend()`: none in the Rust agent (D87 F3).
pub const STACK_TRACE_BACKEND: &str = "";

/// The profile's bits (`nd_profile_detect_and_configure()`'s cached result), 0 until its first detection.
static PROFILE: AtomicU32 = AtomicU32::new(0);
/// `default_rrd_memory_mode`: the compiled default until `[db] db` is read.
static DB_MODE: AtomicU8 = AtomicU8::new(db_mode::DBENGINE);
/// `nd_profile.storage_tiers`.
static DB_TIERS: AtomicU8 = AtomicU8::new(0);
/// `dbengine_out_of_memory_protection`.
static OOM_PROTECTION: AtomicU64 = AtomicU64::new(0);
/// The hosts (`localhost` among them) once they exist; weak, so they do not outlive the exit's steps.
static HOSTS: Mutex<Option<Weak<Hosts>>> = Mutex::new(None);
/// The directories the disk footprint walks: `[directories] lib` and `cache`.
static DIRS: OnceLock<(String, String)> = OnceLock::new();
/// `netdata_configured_host_prefix`, once `[global]` is read.
static HOST_PREFIX: OnceLock<String> = OnceLock::new();

pub fn set_host_prefix(prefix: &str) {
    let _ = HOST_PREFIX.set(prefix.to_string());
}

pub fn set_profile(bits: u32) {
    PROFILE.store(bits, Ordering::Relaxed);
}

pub fn set_db_mode(mode: u8) {
    DB_MODE.store(mode, Ordering::Relaxed);
}

pub fn set_db_tiers(tiers: u8) {
    DB_TIERS.store(tiers, Ordering::Relaxed);
}

pub fn set_oom_protection(bytes: u64) {
    OOM_PROTECTION.store(bytes, Ordering::Relaxed);
}

pub fn set_hosts(hosts: &Arc<Hosts>) {
    *HOSTS.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::downgrade(hosts));
}

pub(super) fn set_dirs(varlib: &str, cache: &str) {
    let _ = DIRS.set((varlib.to_string(), cache.to_string()));
}

fn hosts() -> Option<Arc<Hosts>> {
    HOSTS.lock().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(Weak::upgrade)
}

fn now_realtime_ut() -> u64 {
    crate::rrdcontext::now_realtime_ut()
}


/// `trim()`: whitespace at both ends.
fn trim(text: &[u8]) -> &[u8] {
    let text = netdata_agent_text::c::c_str(text);
    let start = text.iter().position(|c| !c.is_ascii_whitespace() && *c != 0x0b).unwrap_or(text.len());
    let end = text.iter().rposition(|c| !c.is_ascii_whitespace() && *c != 0x0b).map_or(start, |e| e + 1);
    &text[start..end]
}

/// `ND_UUID`'s halves: `hig64` the first 8 bytes, `low64` the last 8, in the machine's order.
fn halves(uuid: &[u8; 16]) -> (u64, u64) {
    let (hi, lo) = uuid.split_at(8);
    (u64::from_ne_bytes(hi.try_into().unwrap_or_default()), u64::from_ne_bytes(lo.try_into().unwrap_or_default()))
}

fn from_halves(hi: u64, lo: u64) -> [u8; 16] {
    let mut uuid = [0; 16];
    uuid[..8].copy_from_slice(&hi.to_ne_bytes());
    uuid[8..].copy_from_slice(&lo.to_ne_bytes());
    uuid
}


/// `os_machine_id()`: systemd's machine id, D-Bus's, or the DMI product UUID; `NO_MACHINE_ID` without one.
pub fn machine_id() -> [u8; 16] {
    let found = ["/etc/machine-id", "/var/lib/dbus/machine-id", "/sys/class/dmi/id/product_uuid"]
        .iter()
        .find_map(|path| system::read_txt_file(path, 128).and_then(|text| uuid_parse_flexi(trim(&text))));
    match found {
        Some(id) => {
            nd_log!(Source::Daemon, Priority::Notice, "OS_MACHINE_ID: machine ID found '{}'", guid::canonical(&id));
            id
        }
        None => {
            nd_log!(Source::Daemon, Priority::Warning, "OS_MACHINE_ID: Could not detect a reliable machine ID");
            from_halves(1, 1)
        }
    }
}

/// `get_install_type()`'s install type, when `<user config>/.install-type` names one.
pub fn install_type(user_config_dir: &str) -> Option<String> {
    crate::system_info::install_type_of(user_config_dir)
}

/// `os_boot_id()`: the kernel's boot id; without it, the boot time in seconds as the lower half.
pub fn boot_id() -> [u8; 16] {
    if let Some(id) =
        system::read_txt_file("/proc/sys/kernel/random/boot_id", 37).and_then(|text| uuid_parse_flexi(trim(&text)))
    {
        return id;
    }
    from_halves(0, boottime_epoch_s())
}

/// `os_boottime()`: `btime` of `/proc/stat`, else now less the uptime.
fn boottime_epoch_s() -> u64 {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    if let Some(btime) = stat.lines().find_map(|l| l.strip_prefix("btime ")).and_then(|v| v.trim().parse().ok()) {
        return btime;
    }
    let uptime = std::fs::read_to_string("/proc/uptime").unwrap_or_default();
    let up = uptime.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (now_realtime_ut() / 1_000_000).saturating_sub(up as u64)
}

/// `os_boot_ids_match()`: the same id, or two made from boot times at most 3 seconds apart.
pub fn boot_ids_match(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let ((ahi, alo), (bhi, blo)) = (halves(a), halves(b));
    a == b || (ahi == 0 && bhi == 0 && alo.abs_diff(blo) <= 3)
}

/// `machine_guid_get()` as the record keeps it.
pub fn machine_guid(varlib: &str, previous: &HostId) -> HostId {
    let guid = guid::machine_guid_get(varlib, &previous.uuid);
    HostId {
        uuid: guid.uuid,
        last_modified_ut: guid.last_modified_ut,
        last_modified_rfc3339: rfc3339(guid.last_modified_ut),
    }
}

/// `os_get_system_cpus()`, cached.
pub fn system_cpus() -> u64 {
    static CPUS: OnceLock<u64> = OnceLock::new();
    *CPUS.get_or_init(|| system::system_cpus(Path::new("/")).max(0) as u64)
}

/// `os_process_memory(0)`: the peak resident size (`VmHWM`) in bytes, when the resident size is known.
fn max_rss() -> Option<u64> {
    let statm = system::read_txt_file("/proc/self/statm", 4097)?;
    let resident: u64 = std::str::from_utf8(&statm).ok()?.split_whitespace().nth(1)?.parse().ok()?;
    if resident == 0 {
        return None;
    }
    let status = system::read_txt_file("/proc/self/status", 4097).unwrap_or_default();
    let status = String::from_utf8_lossy(&status);
    let hwm = status.find("VmHWM:").map_or(0, |at| {
        let rest = status[at + 6..].trim_start();
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        rest[..digits].parse::<u64>().unwrap_or(0)
    });
    Some(hwm.wrapping_mul(1024))
}

/// `calc_dir_size_recursive()`: every regular file under `base` (its apparent size, and its path relative to `base`
/// for `count`), each directory walked once, symlinks and special files not counted.
fn walk(base: &[u8], rel: &[u8], visited: &mut HashSet<(u64, u64)>, count: &mut impl FnMut(&[u8], u64)) {
    let join = |dir: &[u8], name: &[u8]| {
        let mut path = dir.to_vec();
        if !dir.is_empty() && dir.last() != Some(&b'/') {
            path.push(b'/');
        }
        path.extend_from_slice(name);
        path
    };
    let path = if rel.is_empty() { base.to_vec() } else { join(base, rel) };
    let path = Path::new(std::ffi::OsStr::from_bytes(&path));
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if !visited.insert((meta.ino(), meta.dev())) || !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let next = if rel.is_empty() { name.as_bytes().to_vec() } else { join(rel, name.as_bytes()) };
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
        if meta.is_dir() {
            walk(base, &next, visited, count);
        } else if meta.is_file() {
            count(&next, meta.size());
        }
    }
}

/// The disk footprint of `[directories] lib` and `cache`.
fn disk_footprint() -> (u64, u64, u64) {
    DIRS.get().map_or((0, 0, 0), |(varlib, cache)| footprint(&[varlib, cache]))
}

/// `dir_size_multiple()` of `roots` for each pattern: the dbengine's files, SQLite's, and the rest, which wraps as
/// C's unsigned subtraction.
fn footprint(roots: &[&str]) -> (u64, u64, u64) {
    let pattern = |list: &[u8]| SimplePattern::new(list, Separators::Bytes(b" "), SimplePatternMode::Exact, false);
    let (dbengine, sqlite) = (pattern(b"*dbengine*/*.ndf *dbengine*/*.njf*"), pattern(b"*.db *.wal *.shm"));
    let (mut total, mut engine, mut db) = (0u64, 0u64, 0u64);
    for root in roots {
        // dir_size(): the root must stat, following a link, as a directory; the walk then lstat()s it
        if root.is_empty() || !std::fs::metadata(root).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        walk(root.as_bytes(), b"", &mut HashSet::new(), &mut |rel, size| {
            total = total.wrapping_add(size);
            if dbengine.matches(rel) {
                engine = engine.wrapping_add(size);
            }
            if sqlite.matches(rel) {
                db = db.wrapping_add(size);
            }
        });
    }
    (engine, db, total.wrapping_sub(engine).wrapping_sub(db))
}

/// C's `strncpyz()` of a detected text, when there is one.
fn copy<const N: usize>(dst: &mut super::FixedStr<N>, src: &Option<String>) {
    if let Some(src) = src {
        dst.set(src);
    }
}

/// `get_daemon_status_fields_from_system_info()`: localhost's system info, once, then the product named anew.
fn system_info_once(s: &mut StatusFile, host: &Host) {
    if s.read_system_info {
        return;
    }
    let si = host.info().system_info;
    s.read_system_info = true;
    copy(&mut s.architecture, &si.architecture);
    copy(&mut s.virtualization, &si.virtualization);
    copy(&mut s.container, &si.container);
    copy(&mut s.kernel_version, &si.kernel_version);
    copy(&mut s.os_name, &si.host_os_name);
    copy(&mut s.os_version, &si.host_os_version);
    copy(&mut s.os_id, &si.host_os_id);
    copy(&mut s.os_id_like, &si.host_os_id_like);
    s.kubernetes = si.is_k8s_node.as_deref() == Some("true");
    let known = |v: &Option<String>| v.clone().filter(|v| !v.eq_ignore_ascii_case("unknown"));
    copy(&mut s.cloud_provider_type, &known(&si.cloud_provider_type));
    copy(&mut s.cloud_instance_type, &known(&si.cloud_instance_type));
    copy(&mut s.cloud_instance_region, &known(&si.cloud_instance_region));
    super::product::normalize(s, HOST_PREFIX.get().map_or("", String::as_str));
}

/// `rrdstats_metadata_collect()` for a refresh (D92.2), taken before the status file's lock: none once the hosts are
/// gone, or while a thread is in a fatal error, which exits without unwinding, so a tree lock it holds would stop the
/// walk for good (the last counts stay).
pub(super) fn metrics() -> Option<super::Metrics> {
    if netdata_agent_log::fatal_in_progress() {
        return None;
    }
    let m = hosts()?.metadata_stats();
    let counts = |c: netdata_agent_rrd::metadata_stats::Counts| super::Counts {
        collected: c.collected,
        available: c.available,
    };
    Some(super::Metrics {
        nodes_total: m.nodes_total,
        nodes_receiving: m.nodes_receiving,
        nodes_sending: m.nodes_sending,
        nodes_archived: m.nodes_archived,
        metrics: counts(m.metrics),
        instances: counts(m.instances),
        contexts: counts(m.contexts),
    })
}

/// `daemon_status_file_refresh()`: the record from the live agent; `status` replaces the record's own unless it is
/// `None`. The timings of a phase are measured while the record is in it.
pub fn refresh(s: &mut StatusFile, status: DaemonStatus, metrics: Option<super::Metrics>) {
    let now_ut = now_realtime_ut();
    s.os_type = OsType::Linux;
    if s.timings.init_started_ut == 0 {
        s.timings.init_started_ut = now_ut;
    }
    if status == DaemonStatus::Exiting && s.timings.exit_started_ut == 0 {
        s.timings.exit_started_ut = now_ut;
    }
    let seconds = |since: u64| (now_ut.wrapping_sub(since).wrapping_add(500_000) / 1_000_000) as i64;
    if s.status == DaemonStatus::Initializing {
        s.timings.init = seconds(s.timings.init_started_ut);
    }
    if s.status == DaemonStatus::Exiting {
        s.timings.exit = seconds(s.timings.exit_started_ut);
    }

    if let Some(g) = guid::machine_guid() {
        s.host_id = HostId {
            uuid: g.uuid,
            last_modified_ut: g.last_modified_ut,
            last_modified_rfc3339: rfc3339(g.last_modified_ut),
        };
    }
    s.boottime = nix::time::clock_gettime(nix::time::ClockId::CLOCK_BOOTTIME).map_or(0, |t| t.tv_sec());
    s.uptime = (now_ut / 1_000_000) as i64 - netdata_agent_rrd::host::netdata_start_time();
    s.timestamp_ut = now_ut;
    s.timestamp_rfc3339 = rfc3339(now_ut);
    s.invocation = netdata_agent_log::invocation_id();
    s.db_mode = DB_MODE.load(Ordering::Relaxed);
    s.db_tiers = DB_TIERS.load(Ordering::Relaxed);
    s.pid = std::process::id() as i32;

    // the highest cloud status is kept
    let cs = crate::cloud::status();
    if matches!(s.cloud_status, 0 | cloud_status::AVAILABLE | cloud_status::OFFLINE)
        || matches!(cs, cloud_status::BANNED | cloud_status::ONLINE | cloud_status::INDIRECT)
    {
        s.cloud_status = cs;
    }
    s.oom_protection = OOM_PROTECTION.load(Ordering::Relaxed);
    if let Some(rss) = max_rss() {
        s.netdata_max_rss = rss;
    }
    // claim_id_get_uuid(): the Rust agent is never claimed (D61.3)
    s.claim_id = [0; 16];
    let hosts = hosts();
    if let Some(host) = hosts.as_ref().map(|h| Arc::clone(h.localhost())) {
        if let Some(id) = crate::meta_store::host_id(&host).filter(|id| *id != [0; 16]) {
            s.host_id.uuid = id;
        }
        let node_id = host.node_id();
        if node_id != [0; 16] {
            s.node_id = node_id;
        }
        system_info_once(s, &host);
    }
    s.timezone.set(crate::timezone::system_tz_name());
    s.exit_reason = exit_reason::get();
    s.profile = PROFILE.load(Ordering::Relaxed);
    if status != DaemonStatus::None {
        s.status = status;
    }
    s.memory = system::system_memory_cached(true);
    s.var_cache =
        DIRS.get().map_or_else(DiskSpace::default, |(_, cache)| netdata_agent_sys::disk_space(Path::new(cache)));
    s.system_cpus = system_cpus();
    if let Some(metrics) = metrics {
        s.metrics = metrics;
    }

    // at most once every 10 minutes
    let updated_ut = s.disk_footprint.last_updated_ut;
    if now_ut.wrapping_sub(updated_ut) >= 600 * 1_000_000 || updated_ut == 0 {
        let (dbengine, sqlite, other) = disk_footprint();
        let f = &mut s.disk_footprint;
        (f.dbengine, f.sqlite, f.other) = (dbengine, sqlite, other);
        f.last_updated_ut = now_ut;
        f.last_updated_rfc3339 = rfc3339(now_ut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_ids_match_as_c() {
        let (a, b) = (from_halves(0, 1_000), from_halves(0, 1_003));
        assert!(boot_ids_match(&a, &b) && boot_ids_match(&b, &a));
        assert!(!boot_ids_match(&a, &from_halves(0, 1_004)));
        assert!(!boot_ids_match(&from_halves(7, 1_000), &from_halves(7, 1_001)));
        assert!(boot_ids_match(&[9; 16], &[9; 16]));
        assert_eq!(guid::canonical(&from_halves(1, 1)).replace('-', "").len(), 32);
    }

    /// `dir_size()`'s rules: apparent sizes, paths relative to the root, no symlinks, `-wal` files are "other".
    #[test]
    fn measures_the_footprint_as_c() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let put = |rel: &str, size: usize| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![0; size]).unwrap();
        };
        put("dbengine/datafile-1-0000000001.ndf", 1000);
        put("dbengine-tier1/journalfile-1-0000000001.njf", 200);
        put("dbengine-tier1/journalfile-1-0000000001.njfv2", 30);
        put("dbengine/sub/deeper.NDF", 4);
        put("datafile.ndf", 5);
        put("netdata-meta.db", 600);
        put("netdata-meta.db-wal", 70);
        put("cache/context-meta.DB", 8);
        std::os::unix::fs::symlink(root.join("netdata-meta.db"), root.join("link.db")).unwrap();
        std::os::unix::fs::symlink(root, root.join("loop")).unwrap();
        let root = root.to_str().unwrap();
        // "*dbengine*/*.ndf" matches below any directory named so, at any depth, in any case
        assert_eq!(footprint(&[root]), (1234, 608, 75));
        // a missing root counts nothing; each root has its own walk
        assert_eq!(footprint(&[root, "/nonexistent/netdata"]), (1234, 608, 75));
        assert_eq!(footprint(&[root, root]), (2468, 1216, 150));
    }
}
