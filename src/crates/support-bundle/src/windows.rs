//! Windows collection: native API implementations of the collectors the
//! PowerShell script gathered with cmdlets — WMI (COM) for CIM classes, the
//! Evt* API for the Event Log, the registry for MSI/proxy state, and
//! GetExtendedTcpTable for listeners. External commands are used only for
//! real CLI tools (w32tm, netsh), the same way the POSIX side runs
//! journalctl or ss.

#![cfg(windows)]

use crate::consts::{CONF_FILE_CAP, LOG_CAP};
use crate::item::{
    Item, PlanOpts, announce_at, push_api, push_file, push_file_capped, push_native,
    push_native_capped,
};
use crate::run::{CaptureMode, run_capped};
use crate::sanitize::Identity;
use crate::summary::SummaryInputs;
use crate::util::{self, info};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use wmi::{COMLibrary, Variant, WMIConnection};

const NETDATA_PREFIX: &str = "C:\\Program Files\\Netdata";

// --- self-demotion -----------------------------------------------------------

pub fn demote_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, IDLE_PRIORITY_CLASS, SetPriorityClass,
    };
    unsafe {
        SetPriorityClass(GetCurrentProcess(), IDLE_PRIORITY_CLASS);
    }
}

// --- interrupt handling ------------------------------------------------------

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> windows_sys::core::BOOL {
    crate::util::INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
    1
}

pub fn install_signal_handlers() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    unsafe {
        SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    }
}

// --- identity ----------------------------------------------------------------

pub fn detect_identity() -> Identity {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNameDnsFullyQualified, GetComputerNameExW,
    };
    let host_short = std::env::var("COMPUTERNAME").unwrap_or_default();
    let mut fqdn_buf = [0u16; 512];
    let mut len = fqdn_buf.len() as u32;
    let host_fqdn = unsafe {
        if GetComputerNameExW(
            ComputerNameDnsFullyQualified,
            fqdn_buf.as_mut_ptr(),
            &mut len,
        ) != 0
        {
            String::from_utf16_lossy(&fqdn_buf[..len as usize])
        } else {
            String::new()
        }
    };
    let run_user = std::env::var("USERNAME").unwrap_or_default();
    Identity::gated(&host_short, &host_fqdn, &run_user)
}

pub fn ran_privileged() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: u32 = 0;
        let mut ret_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
            &mut ret_len,
        );
        CloseHandle(token);
        ok != 0 && elevation != 0
    }
}

// --- WMI helpers -------------------------------------------------------------

pub struct Wmi {
    conn: Option<WMIConnection>,
}

impl Wmi {
    pub fn connect() -> Wmi {
        let conn = COMLibrary::new()
            .ok()
            .and_then(|com| WMIConnection::new(com).ok());
        Wmi { conn }
    }

    fn query(&self, wql: &str) -> Vec<std::collections::HashMap<String, Variant>> {
        match &self.conn {
            Some(c) => c.raw_query(wql).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Format-List style output for the selected properties, in order.
    fn list(&self, wql: &str, props: &[&str]) -> String {
        let rows = self.query(wql);
        if rows.is_empty() {
            return "(no results - WMI unavailable or empty result set)\n".to_string();
        }
        let mut out = String::new();
        for row in rows {
            for p in props {
                let v = row
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(p))
                    .map(|(_, v)| variant_to_string(v))
                    .unwrap_or_default();
                out.push_str(&format!("{p:<26}: {v}\n"));
            }
            out.push('\n');
        }
        out
    }
}

fn variant_to_string(v: &Variant) -> String {
    match v {
        Variant::Empty | Variant::Null => String::new(),
        Variant::String(s) => s.clone(),
        Variant::Bool(b) => b.to_string(),
        Variant::I1(n) => n.to_string(),
        Variant::I2(n) => n.to_string(),
        Variant::I4(n) => n.to_string(),
        Variant::I8(n) => n.to_string(),
        Variant::UI1(n) => n.to_string(),
        Variant::UI2(n) => n.to_string(),
        Variant::UI4(n) => n.to_string(),
        Variant::UI8(n) => n.to_string(),
        Variant::R4(n) => n.to_string(),
        Variant::R8(n) => n.to_string(),
        Variant::Array(a) => a
            .iter()
            .map(variant_to_string)
            .collect::<Vec<_>>()
            .join(", "),
        other => format!("{other:?}"),
    }
}

// --- environment discovery ---------------------------------------------------

pub struct Env {
    /// Shared COM/WMI connection; Native producers hold their own handle so
    /// declared items never borrow from the discovery pass.
    pub wmi: Rc<Wmi>,
    pub prefix: PathBuf,
    pub netdata_pid: Option<u32>,
    pub confdir: Option<PathBuf>,
    pub logdir: Option<PathBuf>,
    pub libdir: Option<PathBuf>,
    pub cachedir: Option<PathBuf>,
    pub netdata_bin: Option<PathBuf>,
    pub netdatacli: Option<PathBuf>,
    pub api_ok: bool,
    pub is_container: bool,
    pub docker_logs_needed: bool,
    pub netdata_pids: Vec<u32>,
}

/// Parse a Win32_Service PathName — possibly quoted, possibly carrying
/// arguments — into the install prefix. The exe lives at
/// <prefix>\usr\{bin,sbin}\netdata.exe, so the prefix is three levels up.
/// Pure string logic; the caller validates that the prefix really exists.
fn prefix_from_service_pathname(raw: &str) -> Option<PathBuf> {
    let exe_str = if let Some(rest) = raw.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else {
        raw.split(' ').next().unwrap_or("")
    };
    if exe_str.is_empty() {
        return None;
    }
    PathBuf::from(exe_str)
        .ancestors()
        .nth(3)
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
}

/// Resolve the install prefix at runtime: the Netdata service's PathName
/// points at <prefix>\usr\{bin,sbin}\netdata.exe, so non-default installs
/// are found without configuration; falls back to the MSI default.
fn discover_prefix(wmi: &Wmi) -> PathBuf {
    for row in wmi.query("SELECT PathName FROM Win32_Service WHERE Name='Netdata'") {
        let raw = row
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PathName"))
            .map(|(_, v)| variant_to_string(v))
            .unwrap_or_default();
        if let Some(prefix) = prefix_from_service_pathname(&raw) {
            if prefix.join("etc\\netdata").is_dir() {
                return prefix;
            }
        }
    }
    PathBuf::from(NETDATA_PREFIX)
}

pub fn detect_env() -> Env {
    let wmi = Rc::new(Wmi::connect());
    let prefix = discover_prefix(&wmi);
    let dir = |rel: &str| {
        let p = prefix.join(rel);
        p.is_dir().then_some(p)
    };
    let confdir = dir("etc\\netdata");
    let logdir = dir("var\\log\\netdata");
    let libdir = dir("var\\lib\\netdata");
    let cachedir = dir("var\\cache\\netdata");
    let find_bin = |name: &str| {
        ["usr\\sbin", "usr\\bin"]
            .iter()
            .map(|d| prefix.join(d).join(name))
            .find(|p| p.is_file())
    };
    let exe = find_bin("netdata.exe");
    let cli = find_bin("netdatacli.exe");

    let mut netdata_pids: Vec<u32> = Vec::new();
    let mut netdata_pid = None;
    for row in wmi.query(
        "SELECT Name, ProcessId FROM Win32_Process WHERE Name LIKE '%netdata%' OR Name LIKE '%go.d%' OR Name LIKE '%ebpf%' OR Name LIKE '%windows.plugin%'",
    ) {
        let name = row
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Name"))
            .map(|(_, v)| variant_to_string(v))
            .unwrap_or_default();
        let pid = row
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("ProcessId"))
            .and_then(|(_, v)| variant_to_string(v).parse::<u32>().ok());
        if let Some(pid) = pid {
            netdata_pids.push(pid);
            if name.eq_ignore_ascii_case("netdata.exe") && netdata_pid.is_none() {
                netdata_pid = Some(pid);
            }
        }
    }
    // probe /api/v3/info first: it stays reachable even under bearer
    // protection, where /api/v1/* is locked
    let api_ok = ["/api/v3/info", "/api/v1/info"].iter().any(|p| {
        crate::http::local_get(
            crate::consts::ND_PORT,
            p,
            std::time::Duration::from_secs(3),
            65536,
        )
        .map(|r| (200..300).contains(&r.status))
        .unwrap_or(false)
    });
    Env {
        wmi,
        prefix,
        netdata_pid,
        confdir,
        logdir,
        libdir,
        cachedir,
        netdata_bin: exe,
        netdatacli: cli,
        api_ok,
        is_container: false,
        docker_logs_needed: false,
        netdata_pids,
    }
}

// --- directory listing helper (Get-ChildItem substitute) ---------------------

fn iso_mtime(p: &Path) -> String {
    p.metadata()
        .and_then(|m| m.modified())
        .map(util::iso_from_system_time)
        .unwrap_or_default()
}

fn dir_listing(
    root: &Path,
    max_entries: usize,
    mut exclude: impl FnMut(&Path) -> Option<String>,
) -> String {
    let mut out = String::new();
    let mut n = 0usize;
    let mut withheld: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
    {
        if n >= max_entries {
            break;
        }
        let Ok(entry) = entry else { continue };
        let p = entry.path();
        if p == root {
            continue;
        }
        if let Some(reason) = exclude(p) {
            if !reason.is_empty() && !withheld.contains(&reason) {
                withheld.push(reason);
            }
            continue;
        }
        n += 1;
        let size = if entry.file_type().is_file() {
            entry.metadata().map(|m| m.len()).unwrap_or(0).to_string()
        } else {
            "<dir>".to_string()
        };
        out.push_str(&format!(
            "{:>12}  {}  {}\n",
            size,
            iso_mtime(p),
            p.display()
        ));
    }
    for w in withheld {
        out.push_str(&format!("{w}\n"));
    }
    out
}

// --- 01-system ---------------------------------------------------------------

fn system_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    let wmi = env.wmi.clone();
    push_native(
        v,
        "01-system/os-version.txt",
        "OS version and build",
        "WMI Win32_OperatingSystem",
        move |_| {
            wmi.list(
                "SELECT * FROM Win32_OperatingSystem",
                &[
                    "Caption",
                    "Version",
                    "BuildNumber",
                    "OSArchitecture",
                    "LastBootUpTime",
                    "TotalVisibleMemorySize",
                    "FreePhysicalMemory",
                ],
            )
        },
    );
    let wmi = env.wmi.clone();
    push_native(
        v,
        "01-system/computer-info.txt",
        "Hardware, domain role, virtualization",
        "WMI Win32_ComputerSystem",
        move |_| {
            wmi.list(
                "SELECT * FROM Win32_ComputerSystem",
                &[
                    "Manufacturer",
                    "Model",
                    "SystemType",
                    "NumberOfProcessors",
                    "NumberOfLogicalProcessors",
                    "TotalPhysicalMemory",
                    "DomainRole",
                    "HypervisorPresent",
                ],
            )
        },
    );
    let wmi = env.wmi.clone();
    push_native(
        v,
        "01-system/disk-usage.txt",
        "Volume usage",
        "WMI Win32_LogicalDisk",
        move |_| {
            wmi.list(
                "SELECT * FROM Win32_LogicalDisk",
                &["DeviceID", "Size", "FreeSpace", "FileSystem"],
            )
        },
    );
    push_native(
        v,
        "01-system/clock-timesync.txt",
        "Clock and time sync (drift breaks streaming/cloud)",
        "system clock (UTC); w32tm /query /status",
        |ctx| {
            let mut o = format!("{}\n", util::utc_now_iso());
            let r = run_capped(
                &["w32tm", "/query", "/status"],
                ctx.cmd_timeout(),
                65536,
                CaptureMode::Head,
            );
            o.push_str(&String::from_utf8_lossy(&r.output));
            o
        },
    );
    let wmi = env.wmi.clone();
    push_native(
        v,
        "01-system/uptime.txt",
        "System uptime",
        "uptime via Win32_OperatingSystem",
        move |_| {
            wmi.list(
                "SELECT * FROM Win32_OperatingSystem",
                &["LastBootUpTime", "LocalDateTime"],
            )
        },
    );
    announce_at(v, start, "collecting: system");
}

// --- 02-install --------------------------------------------------------------

fn install_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    // NOTE: never use Win32_Product here - querying it triggers MSI
    // reconfiguration of every installed package. The uninstall registry keys
    // are the safe source.
    push_native(
        v,
        "02-install/msi-info.txt",
        "Installed Netdata MSI package info (from uninstall registry)",
        "registry uninstall keys (Netdata)",
        |_| {
            use winreg::RegKey;
            use winreg::enums::HKEY_LOCAL_MACHINE;
            let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
            let mut out = String::new();
            for root in [
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ] {
                let Ok(key) = hklm.open_subkey(root) else {
                    continue;
                };
                for sub in key.enum_keys().filter_map(|k| k.ok()) {
                    let Ok(subkey) = key.open_subkey(&sub) else {
                        continue;
                    };
                    let name: String = subkey.get_value("DisplayName").unwrap_or_default();
                    if !name.to_ascii_lowercase().contains("netdata") {
                        continue;
                    }
                    for prop in [
                        "DisplayName",
                        "DisplayVersion",
                        "InstallDate",
                        "InstallLocation",
                        "Publisher",
                    ] {
                        let v: String = subkey.get_value(prop).unwrap_or_default();
                        out.push_str(&format!("{prop:<16}: {v}\n"));
                    }
                    out.push('\n');
                }
            }
            if out.is_empty() {
                out.push_str("(no Netdata entries in the uninstall registry)\n");
            }
            out
        },
    );
    let root = env.prefix.clone();
    push_native(
        v,
        "02-install/install-tree.txt",
        "Install dir layout (top levels)",
        &format!("dir {} (2 levels)", root.display()),
        move |_| {
            let mut out = String::new();
            for entry in walkdir::WalkDir::new(&root)
                .max_depth(2)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let p = entry.path();
                if p == root {
                    continue;
                }
                let size = if entry.file_type().is_file() {
                    entry.metadata().map(|m| m.len()).unwrap_or(0).to_string()
                } else {
                    "<dir>".to_string()
                };
                out.push_str(&format!(
                    "{:>12}  {}  {}\n",
                    size,
                    iso_mtime(p),
                    p.display()
                ));
            }
            if out.is_empty() {
                out.push_str("(install dir not found)\n");
            }
            out
        },
    );
    if let Some(conf) = &env.confdir {
        let marker = conf.join(".install-type");
        if marker.is_file() {
            push_file(
                v,
                "02-install/install-type.file.txt",
                "Install type marker",
                &marker,
            );
        }
    }
    announce_at(v, start, "collecting: install");
}

// --- 03-process --------------------------------------------------------------

fn process_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    let wmi = env.wmi.clone();
    push_native(
        v,
        "03-process/netdata-processes.txt",
        "Netdata process tree with CPU/memory",
        "WMI Win32_Process (netdata family)",
        move |_| {
            wmi.list(
                "SELECT * FROM Win32_Process WHERE Name LIKE '%netdata%' OR Name LIKE '%go.d%' OR Name LIKE '%ebpf%' OR Name LIKE '%windows.plugin%'",
                &[
                    "ProcessId",
                    "Name",
                    "WorkingSetSize",
                    "HandleCount",
                    "ThreadCount",
                    "UserModeTime",
                    "KernelModeTime",
                    "CommandLine",
                ],
            )
        },
    );
    let wmi = env.wmi.clone();
    push_native(
        v,
        "03-process/service-status.txt",
        "Netdata service state and config",
        "WMI Win32_Service Name='Netdata'",
        move |_| {
            wmi.list(
                "SELECT * FROM Win32_Service WHERE Name='Netdata'",
                &[
                    "Name",
                    "State",
                    "StartMode",
                    "StartName",
                    "PathName",
                    "ExitCode",
                    "ProcessId",
                ],
            )
        },
    );
    announce_at(v, start, "collecting: process");
}

// --- 04-config ---------------------------------------------------------------

fn config_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    if env.api_ok {
        push_api(
            v,
            "04-config/effective-netdata.conf",
            "EFFECTIVE running config (merged, annotated) - authoritative over on-disk file",
            "/netdata.conf",
        );
    }
    // no early return: cloud.conf below must be reachable without a
    // confdir, matching the POSIX side
    if let Some(confdir) = env.confdir.clone() {
        let confdir_s = confdir.display().to_string();
        push_native(
            v,
            "04-config/config-tree.txt",
            "User config dir tree (files here = user-customized; ssl and key material excluded)",
            &format!("dir {confdir_s} recursive (ssl/key material excluded)"),
            {
                let confdir = confdir.clone();
                move |_| {
                    dir_listing(&confdir, 2000, |p| {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        let in_ssl = p
                            .components()
                            .any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("ssl"));
                        if in_ssl {
                            Some("  [ssl directory contents withheld]".to_string())
                        } else if name.ends_with(".pem") || name.ends_with(".key") {
                            Some(String::new())
                        } else {
                            None
                        }
                    })
                }
            },
        );
        push_file(
            v,
            "04-config/netdata.conf",
            "On-disk main config",
            confdir.join("netdata.conf"),
        );
        push_file(
            v,
            "04-config/stream.conf",
            "Streaming config (parent/child; api key redacted)",
            confdir.join("stream.conf"),
        );
        push_file(
            v,
            "04-config/claim.conf",
            "Cloud claim config (token redacted)",
            confdir.join("claim.conf"),
        );
        push_file(
            v,
            "04-config/go.d.conf",
            "go.d orchestrator config",
            confdir.join("go.d.conf"),
        );
        push_file(
            v,
            "04-config/exporting.conf",
            "Exporting engine config (credentials redacted)",
            confdir.join("exporting.conf"),
        );
        // every user-customized config, nested dirs included (go.d\sd\, otel.d\,
        // vnodes\, ...), relative paths preserved; ssl and key material excluded;
        // capped at 200 files - the same rules as the POSIX side
        let mut collected = 0usize;
        for entry in walkdir::WalkDir::new(&confdir)
            .follow_links(false)
            .sort_by_file_name()
        {
            if collected >= 200 {
                break;
            }
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if ![".conf", ".yml", ".yaml"].iter().any(|e| name.ends_with(e)) {
                continue;
            }
            let rel_s = p
                .strip_prefix(&confdir)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            if rel_s.split('/').any(|c| c.eq_ignore_ascii_case("ssl"))
                || name.ends_with(".pem")
                || name.ends_with(".key")
            {
                continue;
            }
            if [
                "netdata.conf",
                "stream.conf",
                "claim.conf",
                "go.d.conf",
                "exporting.conf",
            ]
            .contains(&rel_s.as_str())
            {
                continue;
            }
            collected += 1;
            push_file_capped(
                v,
                &format!("04-config/{rel_s}"),
                "User config (secrets redacted)",
                p,
                CONF_FILE_CAP,
            );
        }
    }
    if let Some(libdir) = &env.libdir {
        let cloud_conf = libdir.join("cloud.d\\cloud.conf");
        if cloud_conf.is_file() {
            push_file(
                v,
                "04-config/cloud.conf",
                "Cloud connection config (token redacted)",
                &cloud_conf,
            );
        }
    }
    announce_at(v, start, "collecting: config");
}

// --- 05-logs (Windows: Event Log is the primary destination) ------------------

fn logs_items(v: &mut Vec<Item>, env: &Env, since_hours: u64) {
    let start = v.len();
    push_native_capped(
        v,
        "05-logs/eventlog-netdata.txt",
        "Netdata events from Windows Event Log (NetdataWEL + Application)",
        &format!("EvtQuery NetdataWEL/Application (Netdata providers, last {since_hours}h)"),
        LOG_CAP,
        move |ctx| {
            let mut out = String::new();
            for channel in ["NetdataWEL", "Application"] {
                out.push_str(&format!("== {channel} ==\n"));
                out.push_str(&eventlog::query_channel(
                    channel,
                    since_hours,
                    2000,
                    ctx.cmd_timeout(),
                ));
            }
            out
        },
    );
    if let Some(logdir) = env.logdir.clone() {
        if let Ok(dir) = std::fs::read_dir(&logdir) {
            let mut logs: Vec<PathBuf> = dir
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e == "log")
                })
                .collect();
            logs.sort();
            for lf in logs {
                let name = lf
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("x")
                    .to_string();
                push_file_capped(
                    v,
                    &format!("05-logs/{name}"),
                    &format!("Agent log file: {name}"),
                    &lf,
                    LOG_CAP,
                );
            }
        }
    }
    announce_at(
        v,
        start,
        &format!("collecting: logs (last {since_hours}h, Event Log + files)"),
    );
}

// --- 06-state ----------------------------------------------------------------

fn state_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    if let Some(libdir) = env.libdir.clone() {
        let status = libdir.join("status-netdata.json");
        if status.is_file() {
            push_file(
                v,
                crate::consts::PATH_STATUS_FILE,
                "Daemon status file: LAST EXIT/CRASH RECORD incl. fatal stack trace (read this first for crashes)",
                &status,
            );
        }
        let libdir_s = libdir.display().to_string();
        push_native(
            v,
            "06-state/state-tree.txt",
            "State dir listing (bearer token filenames withheld - they are live tokens)",
            &format!("dir {libdir_s} recursive (bearer_tokens contents withheld)"),
            {
                let libdir = libdir.clone();
                move |_| {
                    let mut token_count = 0usize;
                    let mut out = dir_listing(&libdir, 2000, |p| {
                        let in_tokens = p
                            .components()
                            .any(|c| c.as_os_str().to_string_lossy() == "bearer_tokens");
                        let is_dir_itself = p.file_name().is_some_and(|n| n == "bearer_tokens");
                        if in_tokens && !is_dir_itself {
                            token_count += 1;
                            Some(String::new())
                        } else {
                            None
                        }
                    });
                    out.push_str(&format!(
                        "[{token_count} token file(s) - names withheld, they ARE the tokens]\n"
                    ));
                    out
                }
            },
        );
        let claimed = libdir.join("cloud.d\\claimed_id");
        if claimed.is_file() {
            push_file(
                v,
                "06-state/claimed-id.txt",
                "Cloud claim id (safe identifier; token/private.pem never collected)",
                &claimed,
            );
        }
        push_file(
            v,
            "06-state/health-silencers.json",
            "Persisted alert silencers",
            libdir.join("health.silencers.json"),
        );
        let mut job_status_files: Vec<PathBuf> = vec![libdir.join("god-jobs-statuses.json")];
        if let Ok(dir) = std::fs::read_dir(&libdir) {
            let mut globbed: Vec<PathBuf> = dir
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.contains("jobs-statuses") && n.ends_with(".json"))
                })
                .collect();
            globbed.sort();
            job_status_files.extend(globbed);
        }
        if let Some(f) = job_status_files.iter().find(|p| p.is_file()) {
            push_file(
                v,
                "06-state/go.d-job-statuses.json",
                "go.d collector job states (which jobs run/fail)",
                f,
            );
        }
        let dyncfg_dir = libdir.join("config");
        if dyncfg_dir.is_dir() {
            if let Ok(dir) = std::fs::read_dir(&dyncfg_dir) {
                let mut files: Vec<PathBuf> = dir
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.is_file()
                            && p.file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.ends_with(".dyncfg"))
                    })
                    .collect();
                files.sort();
                for dc in files {
                    let name = dc
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("x")
                        .to_string();
                    push_file_capped(
                        v,
                        &format!("06-state/dyncfg/{name}"),
                        "Dynamic config created via UI/API (secrets redacted)",
                        &dc,
                        CONF_FILE_CAP,
                    );
                }
            }
        }
    }
    if let Some(cachedir) = env.cachedir.clone() {
        push_native(
            v,
            "06-state/db-disk-usage.txt",
            "Database disk usage per tier + sqlite sizes",
            &format!("du of {}", cachedir.display()),
            move |ctx| {
                let mut out = String::new();
                if let Ok(dir) = std::fs::read_dir(&cachedir) {
                    let mut subdirs: Vec<PathBuf> = dir
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.is_dir())
                        .collect();
                    subdirs.sort();
                    for sd in subdirs {
                        if ctx.deadline_exceeded() {
                            out.push_str("(stopped: global deadline reached)\n");
                            break;
                        }
                        // bounded traversal: dbengine tiers hold few large
                        // files, so a generous entry cap never distorts sizes
                        let sum: u64 = walkdir::WalkDir::new(&sd)
                            .follow_links(false)
                            .into_iter()
                            .filter_map(|e| e.ok())
                            .take(100_000)
                            .filter(|e| e.file_type().is_file())
                            .filter_map(|e| e.metadata().ok())
                            .map(|m| m.len())
                            .sum();
                        let name = sd.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                        out.push_str(&format!("{name}  {:.1} MB\n", sum as f64 / 1048576.0));
                    }
                }
                if let Ok(dir) = std::fs::read_dir(&cachedir) {
                    let mut dbs: Vec<PathBuf> = dir
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| {
                            p.is_file()
                                && p.file_name()
                                    .and_then(|n| n.to_str())
                                    .is_some_and(|n| n.contains(".db"))
                        })
                        .collect();
                    dbs.sort();
                    for db in dbs {
                        let len = db.metadata().map(|m| m.len()).unwrap_or(0);
                        out.push_str(&format!(
                            "{:>12}  {}\n",
                            len,
                            db.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        ));
                    }
                }
                out.push_str(
                    "== sqlite corruption/recovery sentinels (presence = past corruption) ==\n",
                );
                let mut sentinels: Vec<String> = Vec::new();
                if let Ok(dir) = std::fs::read_dir(&cachedir) {
                    for p in dir.filter_map(|e| e.ok()).map(|e| e.path()) {
                        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if n.contains(".bad") || n.ends_with(".recover") {
                            let len = p.metadata().map(|m| m.len()).unwrap_or(0);
                            sentinels.push(format!("{len:>12}  {n}"));
                        }
                    }
                }
                if sentinels.is_empty() {
                    out.push_str("(none found)\n");
                } else {
                    sentinels.sort();
                    for s in sentinels {
                        out.push_str(&s);
                        out.push('\n');
                    }
                }
                out
            },
        );
    }
    announce_at(v, start, "collecting: state");
}

// --- 07-runtime --------------------------------------------------------------

// --- 08-network --------------------------------------------------------------

fn network_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    let pids = env.netdata_pids.clone();
    push_native(
        v,
        "08-network/listening-sockets.txt",
        "Listening sockets (netdata-related)",
        "GetExtendedTcpTable (listeners, filtered to 19999/netdata)",
        move |_| tcp::listeners_report(crate::consts::ND_PORT, &pids),
    );
    let wmi = env.wmi.clone();
    push_native(
        v,
        "08-network/dns-config.txt",
        "DNS resolver config",
        "WMI Win32_NetworkAdapterConfiguration (IPEnabled)",
        move |_| {
            wmi.list(
                "SELECT * FROM Win32_NetworkAdapterConfiguration WHERE IPEnabled = TRUE",
                &[
                    "Description",
                    "DNSServerSearchOrder",
                    "DNSDomainSuffixSearchOrder",
                ],
            )
        },
    );
    push_native(
        v,
        "08-network/proxy-config.txt",
        "System proxy configuration",
        "netsh winhttp show proxy + HKCU Internet Settings",
        |ctx| {
            let mut out = String::new();
            let r = run_capped(
                &["netsh", "winhttp", "show", "proxy"],
                ctx.cmd_timeout(),
                65536,
                CaptureMode::Head,
            );
            out.push_str(&String::from_utf8_lossy(&r.output));
            use winreg::RegKey;
            use winreg::enums::HKEY_CURRENT_USER;
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            if let Ok(key) =
                hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
            {
                let enable: u32 = key.get_value("ProxyEnable").unwrap_or(0);
                let server: String = key.get_value("ProxyServer").unwrap_or_default();
                let autoconfig: String = key.get_value("AutoConfigURL").unwrap_or_default();
                out.push_str(&format!(
                    "ProxyEnable  : {enable}\nProxyServer  : {server}\nAutoConfigURL: {autoconfig}\n"
                ));
            }
            out
        },
    );
    push_native(
        v,
        "08-network/cloud-connectivity.txt",
        "Reachability of Netdata Cloud (DNS + TLS + response code, no data sent)",
        "in-process probe: dns, tcp 443, certificate-validating tls, http status",
        |_| crate::netprobe::cloud_connectivity_report(crate::consts::CLOUD_HOST),
    );
    announce_at(v, start, "collecting: network");
}

/// The startup banner: what discovery found, before collection begins.
pub fn startup_info(env: &Env) {
    info(&format!(
        "netdata-support-bundle {} (Windows)",
        crate::consts::TOOL_VERSION
    ));
    info(&format!(
        "agent pid: {} | api: {} | config: {}",
        env.netdata_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "not running".to_string()),
        if env.api_ok { "up" } else { "unreachable" },
        env.confdir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".to_string()),
    ));
}

/// Tier 2: the declared collection plan for this environment, in execution
/// (= triage priority) order. Pure: builds items, runs nothing.
pub fn build_items(env: &Env, opts: &PlanOpts) -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    system_items(&mut v, env);
    install_items(&mut v, env);
    process_items(&mut v, env);
    config_items(&mut v, env);
    logs_items(&mut v, env, opts.since_hours);
    state_items(&mut v, env);
    v.extend(crate::runtime::runtime_items(
        env.api_ok,
        env.netdata_pid.is_some(),
        env.netdata_bin.as_deref(),
        env.netdatacli.as_deref(),
    ));
    network_items(&mut v, env);
    v
}

pub fn bundle_facts(env: &Env) -> crate::platform_api::BundleFacts {
    crate::platform_api::BundleFacts {
        summary: summary_inputs(env),
        agent_running: env.netdata_pid.is_some(),
        api_ok: env.api_ok,
        is_container: env.is_container,
        docker_logs_needed: env.docker_logs_needed,
    }
}

pub fn summary_inputs(env: &Env) -> SummaryInputs {
    SummaryInputs {
        agent_pid: env.netdata_pid,
        agent_note: String::new(),
        api_ok: env.api_ok,
        is_container: env.is_container,
        confdir: env.confdir.as_ref().map(|p| p.display().to_string()),
        ran_privileged: ran_privileged(),
        docker_logs_needed: env.docker_logs_needed,
    }
}

// --- Event Log via the Evt* API ----------------------------------------------

mod eventlog {
    use windows_sys::Win32::System::EventLog::{
        EVT_HANDLE, EvtClose, EvtFormatMessage, EvtFormatMessageEvent, EvtNext,
        EvtOpenPublisherMetadata, EvtQuery, EvtQueryChannelPath, EvtQueryReverseDirection,
        EvtRender, EvtRenderEventXml,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn xml_attr(xml: &str, elem: &str, attr: &str) -> Option<String> {
        let elem_pos = xml.find(&format!("<{elem}"))?;
        let rest = &xml[elem_pos..];
        let end = rest.find('>')?;
        let tag = &rest[..end];
        let needle = format!("{attr}=");
        let a = tag.find(&needle)? + needle.len();
        let quote = tag.as_bytes().get(a).copied()?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let vstart = a + 1;
        let vend = tag[vstart..].find(quote as char)? + vstart;
        Some(tag[vstart..vend].to_string())
    }

    pub(super) fn xml_elem_text(xml: &str, elem: &str) -> Option<String> {
        let open = format!("<{elem}>");
        let close = format!("</{elem}>");
        let s = xml.find(&open)? + open.len();
        let e = xml[s..].find(&close)? + s;
        Some(xml[s..e].to_string())
    }

    fn level_name(level: &str) -> &'static str {
        match level {
            "1" => "Critical",
            "2" => "Error",
            "3" => "Warning",
            "4" => "Information",
            "5" => "Verbose",
            _ => "Unknown",
        }
    }

    fn render_xml(event: EVT_HANDLE) -> Option<String> {
        unsafe {
            let mut used: u32 = 0;
            let mut props: u32 = 0;
            EvtRender(
                0,
                event,
                EvtRenderEventXml,
                0,
                std::ptr::null_mut(),
                &mut used,
                &mut props,
            );
            if used == 0 {
                return None;
            }
            let mut buf: Vec<u16> = vec![0; used as usize / 2 + 1];
            if EvtRender(
                0,
                event,
                EvtRenderEventXml,
                (buf.len() * 2) as u32,
                buf.as_mut_ptr() as *mut _,
                &mut used,
                &mut props,
            ) == 0
            {
                return None;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        }
    }

    fn format_message(publisher: &str, event: EVT_HANDLE) -> Option<String> {
        unsafe {
            let pw = wide(publisher);
            let meta = EvtOpenPublisherMetadata(0, pw.as_ptr(), std::ptr::null(), 0, 0);
            if meta == 0 {
                return None;
            }
            let mut used: u32 = 0;
            EvtFormatMessage(
                meta,
                event,
                0,
                0,
                std::ptr::null(),
                EvtFormatMessageEvent,
                0,
                std::ptr::null_mut(),
                &mut used,
            );
            if used == 0 {
                EvtClose(meta);
                return None;
            }
            let mut buf: Vec<u16> = vec![0; used as usize + 1];
            let ok = EvtFormatMessage(
                meta,
                event,
                0,
                0,
                std::ptr::null(),
                EvtFormatMessageEvent,
                buf.len() as u32,
                buf.as_mut_ptr(),
                &mut used,
            );
            EvtClose(meta);
            if ok == 0 {
                return None;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        }
    }

    /// Newest-first events from `channel` within the window, filtered to
    /// Netdata providers, at most `max_events`.
    /// Newest-first Netdata-provider events from `channel` within the window.
    /// Bounded three ways, matching the ps1's `-MaxEvents` semantics: at most
    /// `max_events` events are SCANNED per channel (the provider filter is
    /// client-side, so a busy Application log must not be walked end to end),
    /// at most `max_events` matches are kept, and `budget` caps wall time.
    pub fn query_channel(
        channel: &str,
        since_hours: u64,
        max_events: usize,
        budget: std::time::Duration,
    ) -> String {
        let started = std::time::Instant::now();
        let ms = since_hours.saturating_mul(3_600_000);
        let query = format!("*[System[TimeCreated[timediff(@SystemTime) <= {ms}]]]");
        let cw = wide(channel);
        let qw = wide(&query);
        let mut out = String::new();
        unsafe {
            let handle = EvtQuery(
                0,
                cw.as_ptr(),
                qw.as_ptr(),
                EvtQueryChannelPath | EvtQueryReverseDirection,
            );
            if handle == 0 {
                return format!("(channel {channel} not readable or missing)\n");
            }
            let mut total = 0usize;
            let mut scanned = 0usize;
            loop {
                if scanned >= max_events || started.elapsed() > budget {
                    break;
                }
                let mut events: [EVT_HANDLE; 16] = [0; 16];
                let mut returned: u32 = 0;
                if EvtNext(handle, 16, events.as_mut_ptr(), 5000, 0, &mut returned) == 0
                    || returned == 0
                {
                    break;
                }
                for &ev in &events[..returned as usize] {
                    scanned += 1;
                    if total < max_events && scanned <= max_events {
                        if let Some(xml) = render_xml(ev) {
                            let provider = xml_attr(&xml, "Provider", "Name").unwrap_or_default();
                            if provider.to_ascii_lowercase().contains("netdata") {
                                total += 1;
                                let time =
                                    xml_attr(&xml, "TimeCreated", "SystemTime").unwrap_or_default();
                                let level = xml_elem_text(&xml, "Level").unwrap_or_default();
                                let message = format_message(&provider, ev)
                                    .or_else(|| xml_elem_text(&xml, "Data"))
                                    .unwrap_or_default();
                                out.push_str(&format!(
                                    "{time} | {provider} | {} | {}\n",
                                    level_name(&level),
                                    message.replace(['\r', '\n'], " "),
                                ));
                            }
                        }
                    }
                    EvtClose(ev);
                }
                if total >= max_events {
                    break;
                }
            }
            EvtClose(handle);
        }
        if out.is_empty() {
            out.push_str("(no matching Netdata events in the window)\n");
        }
        out
    }
}

// --- TCP listeners via GetExtendedTcpTable -----------------------------------

mod tcp {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    fn table(af: u32) -> Vec<u8> {
        unsafe {
            let mut size: u32 = 0;
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                af,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
            if size == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u8; size as usize];
            if GetExtendedTcpTable(
                buf.as_mut_ptr() as *mut _,
                &mut size,
                0,
                af,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            ) != 0
            {
                return Vec::new();
            }
            buf
        }
    }

    pub fn listeners_report(nd_port: u16, netdata_pids: &[u32]) -> String {
        let mut out = String::from("proto  local address          port   pid\n");
        let mut matched = 0usize;

        let buf = table(AF_INET as u32);
        if buf.len() >= 4 {
            let count = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
            for i in 0..count {
                let off = 4 + i * row_size;
                if off + row_size > buf.len() {
                    break;
                }
                let row = unsafe {
                    std::ptr::read_unaligned(buf[off..].as_ptr() as *const MIB_TCPROW_OWNER_PID)
                };
                // dwLocalAddr is in network byte order in memory
                let addr = std::net::Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes());
                let port = u16::from_be((row.dwLocalPort & 0xffff) as u16);
                if port == nd_port || netdata_pids.contains(&row.dwOwningPid) {
                    matched += 1;
                    out.push_str(&format!(
                        "tcp    {:<22} {:<6} {}\n",
                        addr.to_string(),
                        port,
                        row.dwOwningPid
                    ));
                }
            }
        }

        let buf6 = table(AF_INET6 as u32);
        if buf6.len() >= 4 {
            let count = u32::from_ne_bytes([buf6[0], buf6[1], buf6[2], buf6[3]]) as usize;
            let row_size = std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
            for i in 0..count {
                let off = 4 + i * row_size;
                if off + row_size > buf6.len() {
                    break;
                }
                let row = unsafe {
                    std::ptr::read_unaligned(buf6[off..].as_ptr() as *const MIB_TCP6ROW_OWNER_PID)
                };
                let addr = std::net::Ipv6Addr::from(row.ucLocalAddr);
                let port = u16::from_be((row.dwLocalPort & 0xffff) as u16);
                if port == nd_port || netdata_pids.contains(&row.dwOwningPid) {
                    matched += 1;
                    out.push_str(&format!(
                        "tcp6   {:<22} {:<6} {}\n",
                        addr.to_string(),
                        port,
                        row.dwOwningPid
                    ));
                }
            }
        }

        if matched == 0 {
            out.push_str("(no netdata-related listeners found)\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_pathname_parsing() {
        assert_eq!(
            prefix_from_service_pathname(
                "\"C:\\Program Files\\Netdata\\usr\\bin\\netdata.exe\" -W flag"
            ),
            Some(PathBuf::from("C:\\Program Files\\Netdata"))
        );
        assert_eq!(
            prefix_from_service_pathname("D:\\nd\\usr\\sbin\\netdata.exe"),
            Some(PathBuf::from("D:\\nd"))
        );
        assert_eq!(prefix_from_service_pathname(""), None);
    }

    #[test]
    fn eventlog_xml_extraction() {
        let xml = "<Event><System><Provider Name='Netdata Agent'/><TimeCreated SystemTime='2026-07-17T10:00:00Z'/><Level>2</Level></System><EventData><Data>daemon exited</Data></EventData></Event>";
        assert_eq!(
            eventlog::xml_attr(xml, "Provider", "Name").as_deref(),
            Some("Netdata Agent")
        );
        assert_eq!(
            eventlog::xml_attr(xml, "TimeCreated", "SystemTime").as_deref(),
            Some("2026-07-17T10:00:00Z")
        );
        assert_eq!(eventlog::xml_elem_text(xml, "Level").as_deref(), Some("2"));
        assert_eq!(
            eventlog::xml_elem_text(xml, "Data").as_deref(),
            Some("daemon exited")
        );
        assert_eq!(eventlog::xml_attr(xml, "Missing", "Name"), None);
        let xml2 = "<Provider Name=\"X\"/>";
        assert_eq!(
            eventlog::xml_attr(xml2, "Provider", "Name").as_deref(),
            Some("X")
        );
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::item::Source;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sb-winplan-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_env() -> Env {
        Env {
            wmi: Rc::new(Wmi { conn: None }),
            prefix: PathBuf::from("C:\\Program Files\\Netdata"),
            netdata_pid: Some(1234),
            confdir: None,
            logdir: None,
            libdir: None,
            cachedir: None,
            netdata_bin: None,
            netdatacli: None,
            api_ok: true,
            is_container: false,
            docker_logs_needed: false,
            netdata_pids: vec![1234],
        }
    }

    fn opts() -> PlanOpts {
        PlanOpts {
            since_hours: 24,
            obfuscate: true,
        }
    }

    #[test]
    fn plan_starts_with_os_version_ends_with_cloud_probe_and_rels_are_unique() {
        let items = build_items(&fixture_env(), &opts());
        let r: Vec<&str> = items.iter().map(|i| i.rel.as_str()).collect();
        assert_eq!(r.first(), Some(&"01-system/os-version.txt"));
        assert_eq!(r.last(), Some(&"08-network/cloud-connectivity.txt"));
        let mut sorted = r.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), r.len(), "duplicate bundle paths in the plan");
        let section = |p: &str| {
            p.split('-')
                .next()
                .unwrap()
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("bundle path without a numeric section prefix: {p}"))
        };
        for w in r.windows(2) {
            assert!(
                section(w[0]) <= section(w[1]),
                "section order regressed: {} then {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn plan_gates_follow_environment_facts() {
        let mut env = fixture_env();
        env.api_ok = false;
        let r_down: Vec<String> = build_items(&env, &opts())
            .iter()
            .map(|i| i.rel.clone())
            .collect();
        assert!(
            !r_down
                .iter()
                .any(|p| p == "04-config/effective-netdata.conf")
        );
        assert!(r_down.iter().any(|p| p == "07-runtime/AGENT-WAS-DOWN.txt"));

        env.api_ok = true;
        let r_up: Vec<String> = build_items(&env, &opts())
            .iter()
            .map(|i| i.rel.clone())
            .collect();
        assert!(r_up.iter().any(|p| p == "04-config/effective-netdata.conf"));
        assert!(r_up.iter().any(|p| p == "07-runtime/info-v3.json"));
        assert!(!r_up.iter().any(|p| p == "07-runtime/AGENT-WAS-DOWN.txt"));
    }

    #[test]
    fn config_walk_is_eager_bounded_and_excludes_key_material() {
        let dir = scratch("confwalk");
        std::fs::create_dir_all(dir.join("go.d")).unwrap();
        std::fs::create_dir_all(dir.join("ssl")).unwrap();
        std::fs::write(dir.join("go.d\\nginx.conf"), "x\n").unwrap();
        std::fs::write(dir.join("ssl\\cert.conf"), "x\n").unwrap();
        std::fs::write(dir.join("key.pem"), "x\n").unwrap();
        std::fs::write(dir.join("netdata.conf"), "x\n").unwrap();
        for i in 0..210 {
            std::fs::write(dir.join(format!("zz-extra-{i:03}.conf")), "x\n").unwrap();
        }
        let mut env = fixture_env();
        env.api_ok = false;
        env.confdir = Some(dir.clone());
        let items = build_items(&env, &opts());
        let walk: Vec<&str> = items
            .iter()
            .filter(|i| {
                i.rel.starts_with("04-config/")
                    && matches!(&i.source, Source::File { cap, .. } if *cap == CONF_FILE_CAP)
            })
            .map(|i| i.rel.as_str())
            .collect();
        assert_eq!(walk.len(), 200, "walk budget must cap at 200");
        assert!(walk.contains(&"04-config/go.d/nginx.conf"));
        assert!(!walk.iter().any(|p| p.contains("ssl/")));
        assert!(
            !walk
                .iter()
                .any(|p| p.ends_with(".pem") || p.ends_with(".key"))
        );
        assert!(!walk.contains(&"04-config/netdata.conf"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_listing_contains_no_secret_shaped_output() {
        let mut env = fixture_env();
        env.confdir = Some(PathBuf::from("C:\\Program Files\\Netdata\\etc\\netdata"));
        env.libdir = Some(PathBuf::from(
            "C:\\Program Files\\Netdata\\var\\lib\\netdata",
        ));
        let items = build_items(&env, &opts());
        for it in &items {
            let line = format!("{} {}", it.rel, it.describe_source());
            for needle in ["password=", "token=", "secret=", "Bearer "] {
                assert!(
                    !line.contains(needle),
                    "plan line looks secret-bearing: {line:?}"
                );
            }
        }
    }
}
