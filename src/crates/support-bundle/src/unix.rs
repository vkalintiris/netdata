//! POSIX collection: environment discovery, the 01–08 collector sections,
//! priority self-demotion, and signal handling. Each collector degrades on
//! its own — a missing tool or file loses that one capture, never the run.

#![cfg(unix)]

use crate::collect::Ctx;
use crate::consts::{API_CAP, CONF_FILE_CAP, LOG_CAP, ND_PORT};
use crate::item::{
    Item, PlanOpts, announce_at, push_api, push_cmd, push_file, push_file_capped, push_generated,
    push_native, push_native_capped,
};
use crate::run::{CaptureMode, run_capped};
use crate::sanitize::Identity;
use crate::summary::SummaryInputs;
use crate::util::{self, have, info};
use std::path::{Path, PathBuf};
use std::time::Duration;

// --- self-demotion: never compete with real workloads -----------------------

pub fn demote_priority() {
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        const IOPRIO_WHO_PROCESS: libc::c_int = 1;
        const IOPRIO_CLASS_IDLE: libc::c_int = 3;
        const IOPRIO_CLASS_SHIFT: libc::c_int = 13;
        // best effort: may be denied without CAP_SYS_NICE, which is fine
        libc::syscall(
            libc::SYS_ioprio_set,
            IOPRIO_WHO_PROCESS,
            0,
            IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT,
        );
    }
}

// --- interrupt handling ------------------------------------------------------

extern "C" fn on_signal(_sig: libc::c_int) {
    crate::util::INTERRUPTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn install_signal_handlers() {
    // sigaction, not signal(): SysV-style handler reset on delivery would
    // default-terminate on a second signal mid-collection
    let handler = on_signal as extern "C" fn(libc::c_int);
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as libc::sighandler_t;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0;
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }
}

// --- identity ----------------------------------------------------------------

pub fn detect_identity() -> Identity {
    let mut short = [0u8; 256];
    let host_short = unsafe {
        if libc::gethostname(short.as_mut_ptr() as *mut libc::c_char, short.len()) == 0 {
            let end = short.iter().position(|&b| b == 0).unwrap_or(short.len());
            String::from_utf8_lossy(&short[..end]).into_owned()
        } else {
            String::new()
        }
    };
    // hostname -f resolves the FQDN the same way the script did
    let host_fqdn = {
        let r = run_capped(
            &["hostname", "-f"],
            Duration::from_secs(3),
            1024,
            CaptureMode::Head,
        );
        String::from_utf8_lossy(&r.output).trim().to_string()
    };
    let run_user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default();
    let run_user = if run_user.is_empty() {
        let r = run_capped(
            &["id", "-un"],
            Duration::from_secs(3),
            256,
            CaptureMode::Head,
        );
        String::from_utf8_lossy(&r.output).trim().to_string()
    } else {
        run_user
    };
    Identity::gated(&host_short, &host_fqdn, &run_user)
}

pub fn ran_privileged() -> bool {
    unsafe { libc::geteuid() == 0 }
}

// --- environment discovery ---------------------------------------------------

pub struct Env {
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
}

fn first_dir(candidates: &[&str]) -> Option<PathBuf> {
    candidates.iter().map(PathBuf::from).find(|p| p.is_dir())
}

fn find_netdata_pid() -> Option<u32> {
    let r = run_capped(
        &["ps", "-eo", "pid=,comm="],
        Duration::from_secs(5),
        4 * 1024 * 1024,
        CaptureMode::Head,
    );
    for line in String::from_utf8_lossy(&r.output).lines() {
        let mut it = line.split_whitespace();
        if let (Some(pid), Some(comm)) = (it.next(), it.next()) {
            if comm == "netdata" {
                return pid.parse().ok();
            }
        }
    }
    None
}

pub fn detect_env() -> Env {
    let netdata_pid = find_netdata_pid();
    // path candidates per install type: FHS packages, static (/opt/netdata),
    // FreeBSD ports (/usr/local + /var/db), Homebrew (incl. Apple Silicon)
    let confdir = first_dir(&[
        "/etc/netdata",
        "/opt/netdata/etc/netdata",
        "/usr/local/etc/netdata",
        "/opt/homebrew/etc/netdata",
    ]);
    let logdir = first_dir(&[
        "/var/log/netdata",
        "/opt/netdata/var/log/netdata",
        "/usr/local/var/log/netdata",
        "/opt/homebrew/var/log/netdata",
    ]);
    let libdir = first_dir(&[
        "/var/lib/netdata",
        "/opt/netdata/var/lib/netdata",
        "/var/db/netdata",
        "/usr/local/var/lib/netdata",
        "/opt/homebrew/var/lib/netdata",
    ]);
    let cachedir = first_dir(&[
        "/var/cache/netdata",
        "/opt/netdata/var/cache/netdata",
        "/var/db/netdata/cache",
        "/usr/local/var/cache/netdata",
        "/opt/homebrew/var/cache/netdata",
    ]);
    let netdata_bin = util::which("netdata")
        .or_else(|| {
            let pid = netdata_pid?;
            std::fs::read_link(format!("/proc/{pid}/exe")).ok()
        })
        .or_else(|| {
            let p = PathBuf::from("/opt/netdata/usr/sbin/netdata");
            p.is_file().then_some(p)
        });
    let netdatacli = util::which("netdatacli");
    // probe /api/v3/info first: it stays reachable even under bearer
    // protection, where /api/v1/* is locked (so a protected-but-running
    // agent isn't mis-flagged down)
    let api_ok = ["/api/v3/info", "/api/v1/info"].iter().any(|p| {
        crate::http::local_get(ND_PORT, p, Duration::from_secs(3), 65536)
            .map(|r| (200..300).contains(&r.status))
            .unwrap_or(false)
    });
    // docker images symlink logs to /dev/stdout|stderr - history only
    // exists in `docker logs` on the host (a tier-1 fact: the symlink does
    // not change mid-run)
    let docker_logs_needed = logdir.as_ref().is_some_and(|ld| {
        std::fs::read_link(ld.join("daemon.log"))
            .or_else(|_| std::fs::read_link(ld.join("error.log")))
            .map(|t| t.to_string_lossy().starts_with("/dev/std"))
            .unwrap_or(false)
    });
    let is_container = Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|t| {
                ["docker", "containerd", "kubepods", "lxc"]
                    .iter()
                    .any(|k| t.contains(k))
            })
            .unwrap_or(false);
    Env {
        netdata_pid,
        confdir,
        logdir,
        libdir,
        cachedir,
        netdata_bin,
        netdatacli,
        api_ok,
        is_container,
        docker_logs_needed,
    }
}

// --- helpers for composite collectors ---------------------------------------

fn run_s(ctx: &Ctx, argv: &[&str]) -> String {
    // composite collectors run several commands; stop starting new ones once
    // the global deadline has passed
    if ctx.deadline_exceeded() {
        return String::new();
    }
    let r = run_capped(
        argv,
        ctx.cmd_timeout(),
        (API_CAP * 4) as usize,
        CaptureMode::Head,
    );
    String::from_utf8_lossy(&r.output).into_owned()
}

fn run_stdout_ok(ctx: &Ctx, argv: &[&str]) -> Option<String> {
    if ctx.deadline_exceeded() {
        return None;
    }
    let r = run_capped(
        argv,
        ctx.cmd_timeout(),
        (API_CAP * 4) as usize,
        CaptureMode::Head,
    );
    if r.exit_desc == "0" {
        Some(String::from_utf8_lossy(&r.output).into_owned())
    } else {
        None
    }
}

fn grep_ci<'a>(text: &'a str, needles: &[&str]) -> impl Iterator<Item = &'a str> {
    let needles: Vec<String> = needles.iter().map(|n| n.to_ascii_lowercase()).collect();
    text.lines().filter(move |l| {
        let ll = l.to_ascii_lowercase();
        needles.iter().any(|n| ll.contains(n))
    })
}

/// Keep the newest whole lines that fit in `cap` bytes (the generic
/// head-aligned truncation would keep the OLDEST part of a journal window).
fn fit_tail_to_cap(text: &str, cap: u64) -> String {
    if text.len() as u64 <= cap {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut budget = 0usize;
    let mut start = lines.len();
    for (i, line) in lines.iter().enumerate().rev() {
        budget += line.len() + 1;
        if budget as u64 > cap {
            break;
        }
        start = i;
    }
    if start >= lines.len() {
        return "[content withheld: journal window exceeds the cap without a usable line break]\n"
            .to_string();
    }
    let mut out = format!(
        "### TRUNCATED: window was {} bytes, newest {} kept (line-aligned) ###\n",
        text.len(),
        cap
    );
    out.push_str(&lines[start..].join("\n"));
    out.push('\n');
    out
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    let mut out = lines[start..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn head_lines(text: &str, n: usize) -> String {
    let mut out: String = text.lines().take(n).collect::<Vec<_>>().join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

// --- 01-system ---------------------------------------------------------------

fn system_items(v: &mut Vec<Item>, since_hours: u64) {
    let start = v.len();
    push_cmd(
        v,
        "01-system/uname.txt",
        "Kernel and architecture",
        &["uname", "-a"],
    );
    push_native(
        v,
        "01-system/os-release.txt",
        "OS distribution (first of os-release/lsb-release)",
        "first readable of /etc/os-release /usr/lib/os-release /etc/lsb-release",
        |_| {
            // symlink-following is deliberate here (upstream parity: on most
            // distros /etc/os-release links to /usr/lib/os-release); the
            // read is bounded because the closure bypasses collect_file
            use std::io::Read as _;
            for f in ["/etc/os-release", "/usr/lib/os-release", "/etc/lsb-release"] {
                let Ok(file) = std::fs::File::open(f) else {
                    continue;
                };
                let mut content = String::new();
                if file.take(65536).read_to_string(&mut content).is_ok() && !content.is_empty() {
                    return format!("# source: {f}\n{content}");
                }
            }
            String::new()
        },
    );
    push_cmd(
        v,
        "01-system/uptime-load.txt",
        "Uptime and load",
        &["uptime"],
    );
    push_native(
        v,
        "01-system/cpu-count.txt",
        "CPU count",
        "nproc || sysctl -n hw.ncpu || getconf _NPROCESSORS_ONLN",
        |ctx| {
            if have("nproc") {
                run_s(ctx, &["nproc"])
            } else if let Some(o) = run_stdout_ok(ctx, &["sysctl", "-n", "hw.ncpu"]) {
                o
            } else {
                run_s(ctx, &["getconf", "_NPROCESSORS_ONLN"])
            }
        },
    );
    push_native(
        v,
        "01-system/memory.txt",
        "Memory overview",
        "free -m || head -6 /proc/meminfo || sysctl hw.memsize hw.physmem + vm_stat",
        |ctx| {
            if have("free") {
                run_s(ctx, &["free", "-m"])
            } else if let Ok(mi) = std::fs::read_to_string("/proc/meminfo") {
                head_lines(&mi, 6)
            } else {
                let mut o = run_s(ctx, &["sysctl", "-n", "hw.memsize", "hw.physmem"]);
                if have("vm_stat") {
                    o.push_str(&run_s(ctx, &["vm_stat"]));
                }
                o
            }
        },
    );
    push_cmd(
        v,
        "01-system/disk-usage.txt",
        "Filesystem usage",
        &["df", "-h"],
    );
    push_native(
        v,
        "01-system/virtualization.txt",
        "Virtualization/container detection",
        "systemd-detect-virt",
        |ctx| {
            if have("systemd-detect-virt") {
                run_s(ctx, &["systemd-detect-virt"])
            } else {
                "systemd-detect-virt not available\n".to_string()
            }
        },
    );
    if Path::new("/sys/fs/cgroup").is_dir() {
        push_cmd(
            v,
            "01-system/cgroups.txt",
            "cgroup version",
            &["stat", "-fc", "%T", "/sys/fs/cgroup"],
        );
    }
    push_native(
        v,
        "01-system/clock-timesync.txt",
        "Clock and time sync (drift breaks streaming/cloud)",
        "date -u; timedatectl status",
        |ctx| {
            let mut o = run_s(ctx, &["date", "-u"]);
            if have("timedatectl") {
                o.push_str(&run_s(ctx, &["timedatectl", "status"]));
            }
            o
        },
    );
    push_native(
        v,
        "01-system/mountinfo.txt",
        "Mount table (namespace visibility issues)",
        "cat /proc/self/mountinfo || mount",
        |ctx| {
            std::fs::read_to_string("/proc/self/mountinfo")
                .unwrap_or_else(|_| run_s(ctx, &["mount"]))
        },
    );
    push_native(
        v,
        "01-system/selinux-apparmor.txt",
        "MAC status",
        "getenforce; test -d /sys/kernel/security/apparmor",
        |ctx| {
            let mut o = String::new();
            if have("getenforce") {
                let e = run_s(ctx, &["getenforce"]);
                o.push_str(&format!("selinux: {}", e.trim()));
            }
            if Path::new("/sys/kernel/security/apparmor").is_dir() {
                if !o.is_empty() {
                    o.push(' ');
                }
                o.push_str("apparmor: present");
            }
            if o.is_empty() {
                o.push_str("(no SELinux/AppArmor detected)");
            }
            o.push('\n');
            o
        },
    );
    push_native(
        v,
        "01-system/kernel-messages.txt",
        "Kernel messages: OOM/segfault/netdata (evidence of kills and crashes)",
        "journalctl -k | grep -iE 'oom|out of memory|segfault|netdata' | tail -300 (dmesg fallback)",
        {
            let since = format!("-{since_hours} hours");
            move |ctx| {
                let needles = ["oom", "out of memory", "segfault", "netdata"];
                let mut matched = String::new();
                if have("journalctl") {
                    let t = run_s(ctx, &["journalctl", "-k", "--no-pager", "--since", &since]);
                    matched = grep_ci(&t, &needles).collect::<Vec<_>>().join("\n");
                }
                if matched.is_empty() {
                    let t = run_s(ctx, &["dmesg"]);
                    matched = grep_ci(&t, &needles).collect::<Vec<_>>().join("\n");
                }
                if matched.is_empty() {
                    "(no matching kernel messages, or kernel log not readable in this environment)\n"
                        .to_string()
                } else {
                    tail_lines(&matched, 300)
                }
            }
        },
    );
    announce_at(v, start, "collecting: system");
}

// --- 02-install --------------------------------------------------------------

fn install_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    let conf = env.confdir.clone();
    let mut env_candidates: Vec<PathBuf> = Vec::new();
    if let Some(c) = &conf {
        env_candidates.push(c.join(".environment"));
    }
    env_candidates.push(PathBuf::from("/etc/netdata/.environment"));
    env_candidates.push(PathBuf::from("/opt/netdata/etc/netdata/.environment"));
    if let Some(f) = env_candidates.iter().find(|p| p.is_file()) {
        push_file(
            v,
            "02-install/environment-file.txt",
            "Install-time environment (method, flags, channel; contains no secrets)",
            f,
        );
    }
    let mut it_candidates: Vec<PathBuf> = Vec::new();
    if let Some(c) = &conf {
        it_candidates.push(c.join(".install-type"));
    }
    it_candidates.push(PathBuf::from("/etc/netdata/.install-type"));
    it_candidates.push(PathBuf::from("/opt/netdata/etc/netdata/.install-type"));
    if let Some(f) = it_candidates.iter().find(|p| p.is_file()) {
        push_file(
            v,
            "02-install/install-type.file.txt",
            "Install type marker (kickstart-build|kickstart-static|oci|custom|binpkg-*)",
            f,
        );
    }
    push_native(
        v,
        "02-install/package-info.txt",
        "Netdata packages installed (name/version/status)",
        "dpkg-query -W '*netdata*'; rpm -qa '*netdata*'; apk list --installed | grep netdata",
        |ctx| {
            let mut out = String::new();
            let mut found = false;
            if have("dpkg-query") {
                let o = run_s(
                    ctx,
                    &[
                        "dpkg-query",
                        "-W",
                        "-f",
                        "${Package} ${Version} [${Status}]\n",
                        "*netdata*",
                    ],
                );
                out.push_str(&o);
                if o.contains("install ok installed") {
                    found = true;
                }
            }
            if have("rpm") {
                if let Some(o) = run_stdout_ok(ctx, &["rpm", "-qa", "*netdata*"]) {
                    if !o.trim().is_empty() {
                        out.push_str(&o);
                        found = true;
                    }
                }
            }
            if have("apk") {
                let o = run_s(ctx, &["apk", "list", "--installed"]);
                let m: String = grep_ci(&o, &["netdata"]).collect::<Vec<_>>().join("\n");
                if !m.is_empty() {
                    out.push_str(&m);
                    out.push('\n');
                    found = true;
                }
            }
            if !found {
                out.push_str(
                    "(no netdata OS package installed via dpkg/rpm/apk - normal for docker, static and from-source installs; a \"not-installed\" stub above just means another package references the name. See install-type.txt for how this agent was installed.)\n",
                );
            }
            out
        },
    );
    push_native(
        v,
        "02-install/install-type.txt",
        "Install type inference",
        "presence of /opt/netdata, /.dockerenv, /etc/netdata/.environment, netdata in PATH",
        |_| {
            let mut out = String::new();
            if Path::new("/opt/netdata/etc/netdata").is_dir() {
                out.push_str("static build (/opt/netdata)\n");
            }
            if Path::new("/.dockerenv").is_file() {
                out.push_str("docker container (/.dockerenv present)\n");
            }
            if Path::new("/etc/netdata/.environment").is_file() {
                out.push_str("kickstart-managed (/etc/netdata/.environment present)\n");
            }
            if let Some(p) = util::which("netdata") {
                out.push_str(&format!("netdata binary: {}\n", p.display()));
            }
            if out.is_empty() {
                out.push_str("(no netdata installation detected on this system)\n");
            }
            out
        },
    );
    if env.is_container {
        push_native(
            v,
            "02-install/container-context.txt",
            "Container context (pid1, env, cgroup)",
            "/proc/1/comm, /proc/1/cgroup, /proc/1/environ (NETDATA_*/DOCKER_*)",
            |_| {
                let mut out = String::new();
                out.push_str("== /proc/1/comm ==\n");
                out.push_str(&std::fs::read_to_string("/proc/1/comm").unwrap_or_default());
                out.push_str("== /proc/1/cgroup ==\n");
                out.push_str(&std::fs::read_to_string("/proc/1/cgroup").unwrap_or_default());
                out.push_str("== container env (NETDATA_*/DOCKER_*) ==\n");
                let prefixes = ["NETDATA_", "DOCKER_", "DO_NOT"];
                let mut vars = String::new();
                if let Ok(raw) = std::fs::read("/proc/1/environ") {
                    for kv in raw.split(|&b| b == 0) {
                        let s = String::from_utf8_lossy(kv);
                        if prefixes.iter().any(|p| s.starts_with(p)) {
                            vars.push_str(&s);
                            vars.push('\n');
                        }
                    }
                }
                if vars.is_empty() {
                    for (k, v) in std::env::vars() {
                        if prefixes.iter().any(|p| k.starts_with(p)) {
                            vars.push_str(&format!("{k}={v}\n"));
                        }
                    }
                }
                if vars.is_empty() {
                    vars.push_str("(no NETDATA_*/DOCKER_* env vars visible)\n");
                }
                out.push_str(&vars);
                out
            },
        );
    }
    announce_at(v, start, "collecting: install");
}

// --- 03-process --------------------------------------------------------------

fn process_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    push_native(
        v,
        "03-process/ps-netdata.txt",
        "Netdata process tree with CPU/memory",
        "ps aux | grep -E 'netdata|go.d|ebpf|apps.plugin|charts.d|python.d' | head -50",
        |ctx| {
            let t = run_s(ctx, &["ps", "aux"]);
            let mut out = String::new();
            if let Some(h) = t.lines().next() {
                out.push_str(h);
                out.push('\n');
            }
            let needles = [
                "netdata",
                "go.d",
                "ebpf",
                "apps.plugin",
                "charts.d",
                "python.d",
            ];
            let matches: Vec<&str> = t
                .lines()
                .skip(1)
                .filter(|l| needles.iter().any(|n| l.contains(n)))
                .filter(|l| !l.contains("netdata-support-bundle"))
                .take(50)
                .collect();
            out.push_str(&matches.join("\n"));
            if !matches.is_empty() {
                out.push('\n');
            }
            out
        },
    );
    if let Some(pid) = env.netdata_pid {
        let pid_s = pid.to_string();
        push_native(
            v,
            "03-process/threads-cpu.txt",
            "Per-thread CPU of netdata (which thread is hot)",
            "ps -L -o pid,tid,pcpu,pmem,comm (ps -M / ps -H fallback), sorted by cpu",
            move |ctx| {
                let t = run_capped(
                    &["ps", "-L", "-o", "pid,tid,pcpu,pmem,comm", "-p", &pid_s],
                    ctx.cmd_timeout(),
                    (API_CAP * 4) as usize,
                    CaptureMode::Head,
                );
                if t.exit_desc == "0" {
                    let text = String::from_utf8_lossy(&t.output).into_owned();
                    let mut out = String::new();
                    if let Some(h) = text.lines().next() {
                        out.push_str(h);
                        out.push('\n');
                    }
                    let mut rows: Vec<&str> = text.lines().skip(1).collect();
                    rows.sort_by(|a, b| {
                        let cpu = |l: &str| -> f64 {
                            l.split_whitespace()
                                .nth(2)
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0.0)
                        };
                        cpu(b)
                            .partial_cmp(&cpu(a))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    out.push_str(&rows.into_iter().take(40).collect::<Vec<_>>().join("\n"));
                    out.push('\n');
                    out
                } else if let Some(o) = run_stdout_ok(ctx, &["ps", "-M", "-p", &pid_s]) {
                    head_lines(&o, 40)
                } else {
                    head_lines(&run_s(ctx, &["ps", "-H", "-p", &pid_s]), 40)
                }
            },
        );
        let proc_dir = PathBuf::from(format!("/proc/{pid}"));
        if proc_dir.is_dir() {
            push_cmd(
                v,
                "03-process/proc-status.txt",
                "Process status (RSS, threads, ctx switches)",
                &["cat", &format!("/proc/{pid}/status")],
            );
            push_cmd(
                v,
                "03-process/proc-limits.txt",
                "Process limits",
                &["cat", &format!("/proc/{pid}/limits")],
            );
            push_native(
                v,
                "03-process/fd-count.txt",
                "Open file descriptors",
                "ls /proc/PID/fd | wc -l",
                move |_| {
                    std::fs::read_dir(format!("/proc/{pid}/fd"))
                        .map(|d| format!("{}\n", d.count()))
                        .unwrap_or_else(|_| "0\n".to_string())
                },
            );
            push_native(
                v,
                "03-process/process-environ.txt",
                "Netdata process environment (proxy/claim vars; values sanitized)",
                "tr '\\0' '\\n' < /proc/PID/environ (fallback: shell env NETDATA_*/proxy)",
                move |_| {
                    if let Ok(raw) = std::fs::read(format!("/proc/{pid}/environ")) {
                        let mut out = String::new();
                        for kv in raw.split(|&b| b == 0) {
                            if !kv.is_empty() {
                                out.push_str(&String::from_utf8_lossy(kv));
                                out.push('\n');
                            }
                        }
                        out
                    } else {
                        let mut out = format!(
                            "(/proc/{pid}/environ not readable - containers need CAP_SYS_PTRACE for this)\n-- fallback: NETDATA_*/proxy vars visible to this shell (docker exec inherits container env) --\n"
                        );
                        let mut any = false;
                        for (k, v) in std::env::vars() {
                            let kl = k.to_ascii_lowercase();
                            if k.starts_with("NETDATA_")
                                || kl == "http_proxy"
                                || kl == "https_proxy"
                                || kl == "no_proxy"
                                || kl == "all_proxy"
                            {
                                out.push_str(&format!("{k}={v}\n"));
                                any = true;
                            }
                        }
                        if !any {
                            out.push_str("(none)\n");
                        }
                        out.push_str("-- on the docker HOST you can also run: docker inspect -f \"{{.Config.Env}}\" <container> --\n");
                        out
                    }
                },
            );
        }
    }
    push_native(
        v,
        "03-process/zombies.txt",
        "Zombie processes (plugin reaping issues in containers)",
        "ps -eo pid,ppid,stat,comm | awk '$3 ~ /Z/' | head -30",
        |ctx| {
            let t = run_s(ctx, &["ps", "-eo", "pid=,ppid=,stat=,comm="]);
            let z: Vec<&str> = t
                .lines()
                .filter(|l| l.split_whitespace().nth(2).is_some_and(|s| s.contains('Z')))
                .take(30)
                .collect();
            if z.is_empty() {
                "(no zombie processes)\n".to_string()
            } else {
                format!("{}\n", z.join("\n"))
            }
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
    if let Some(confdir) = env.confdir.clone() {
        let confdir_s = confdir.display().to_string();
        push_native(
            v,
            "04-config/config-tree.txt",
            "User config dir tree (files here = user-customized; ssl/ and key material excluded)",
            &format!("ls -laR {confdir_s} (ssl/ contents and .pem/.key withheld)"),
            move |ctx| {
                let t = run_s(ctx, &["ls", "-laR", &confdir_s]);
                let mut out = String::new();
                let mut skip = false;
                for line in head_lines(&t, 2000).lines() {
                    if line.ends_with("/ssl:") {
                        out.push_str(line);
                        out.push_str("\n  [ssl directory contents withheld]\n");
                        skip = true;
                        continue;
                    }
                    if skip {
                        if line.is_empty() {
                            skip = false;
                            out.push('\n');
                        }
                        continue;
                    }
                    if line.ends_with(".pem") || line.ends_with(".key") {
                        continue;
                    }
                    out.push_str(line);
                    out.push('\n');
                }
                out
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
            "04-config/exporting.conf",
            "Exporting engine config (credentials redacted)",
            confdir.join("exporting.conf"),
        );
        push_file(
            v,
            "04-config/go.d.conf",
            "go.d orchestrator config (module enable/disable)",
            confdir.join("go.d.conf"),
        );
        push_file(
            v,
            "04-config/claim.conf",
            "Cloud claim config (token redacted)",
            confdir.join("claim.conf"),
        );
        // every user-customized config, nested dirs included, relative paths
        // preserved; ssl and key material excluded; capped at 200 files
        let mut count = 0;
        for entry in walkdir::WalkDir::new(&confdir)
            .follow_links(false)
            .sort_by_file_name()
        {
            if count >= 200 {
                break;
            }
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext_ok = [".conf", ".yml", ".yaml"].iter().any(|e| name.ends_with(e));
            if !ext_ok {
                continue;
            }
            let rel = p.strip_prefix(&confdir).unwrap_or(p);
            let rel_s = rel.to_string_lossy().to_string();
            if rel_s.split('/').any(|c| c == "ssl")
                || name.ends_with(".pem")
                || name.ends_with(".key")
            {
                continue;
            }
            if [
                "netdata.conf",
                "stream.conf",
                "exporting.conf",
                "go.d.conf",
                "claim.conf",
            ]
            .contains(&rel_s.as_str())
            {
                continue;
            }
            // the 200-file budget counts only files actually collected
            count += 1;
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
        let cloud_conf = libdir.join("cloud.d/cloud.conf");
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

// --- 05-logs -----------------------------------------------------------------

fn logs_items(v: &mut Vec<Item>, env: &Env, since_hours: u64) {
    let start = v.len();
    let since = format!("-{} hours", since_hours);
    if have("journalctl") {
        for (rel, title, unit_args) in [
            (
                "05-logs/journal-netdata.txt",
                "systemd journal for netdata unit",
                vec!["-u", "netdata"],
            ),
            (
                "05-logs/journal-namespace-netdata.txt",
                "netdata journal namespace (some installs log here)",
                vec!["--namespace=netdata"],
            ),
        ] {
            let mut argv: Vec<String> = vec!["journalctl".to_string()];
            argv.extend(unit_args.iter().map(|s| s.to_string()));
            argv.extend(
                ["--no-pager", "-o", "short-iso", "--since"]
                    .iter()
                    .map(|s| s.to_string()),
            );
            argv.push(since.clone());
            let origin = format!("{} | tail -n 20000", argv.join(" "));
            push_native_capped(v, rel, title, &origin, LOG_CAP, move |ctx| {
                let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                let r = run_capped(
                    &refs,
                    ctx.cmd_timeout(),
                    (LOG_CAP * 2) as usize,
                    CaptureMode::Tail,
                );
                fit_tail_to_cap(
                    &tail_lines(&String::from_utf8_lossy(&r.output), 20000),
                    LOG_CAP,
                )
            });
        }
    }
    if let Some(logdir) = env.logdir.clone() {
        for lf in [
            "error.log",
            "daemon.log",
            "collector.log",
            "health.log",
            "aclk.log",
            "debug.log",
        ] {
            push_file_capped(
                v,
                &format!("05-logs/{lf}"),
                &format!("Agent log file: {lf}"),
                logdir.join(lf),
                LOG_CAP,
            );
        }
        push_file_capped(
            v,
            "05-logs/access.log",
            "API access log (clients pseudonymized)",
            logdir.join("access.log"),
            1048576,
        );
        if env.docker_logs_needed {
            push_generated(
                v,
                "05-logs/LOGS-ARE-IN-DOCKER.txt",
                "Instruction: agent logs live in 'docker logs' on the host",
                format!(
                    "This agent logs to the container's stdout/stderr. Its log history is NOT\navailable from inside the container. To complete this bundle, ALSO run on\nthe docker host and attach the output:\n\n    docker logs --since {since_hours}h <netdata-container> > netdata-docker.log 2>&1\n"
                ),
            );
        }
    }
    if have("journalctl") {
        push_native(
            v,
            "05-logs/journal-updater.txt",
            "Auto-updater service journal (updater keeps no persistent log file)",
            "journalctl -u netdata-updater.service | tail -200",
            |ctx| {
                let r = run_capped(
                    &[
                        "journalctl",
                        "-u",
                        "netdata-updater.service",
                        "--no-pager",
                        "-o",
                        "short-iso",
                    ],
                    ctx.cmd_timeout(),
                    (API_CAP * 2) as usize,
                    CaptureMode::Tail,
                );
                tail_lines(&String::from_utf8_lossy(&r.output), 200)
            },
        );
    }
    push_native(
        v,
        "05-logs/coredumps.txt",
        "Recent coredump METADATA for netdata (not the dumps)",
        "coredumpctl list | awk 'NR==1 || /netdata/' | tail -21",
        |ctx| {
            if !have("coredumpctl") {
                return "coredumpctl not available\n".to_string();
            }
            let t = run_s(ctx, &["coredumpctl", "list", "--no-pager"]);
            let mut keep: Vec<&str> = Vec::new();
            for (i, line) in t.lines().enumerate() {
                if i == 0 || line.to_ascii_lowercase().contains("netdata") {
                    keep.push(line);
                }
            }
            tail_lines(&keep.join("\n"), 21)
        },
    );
    announce_at(
        v,
        start,
        &format!("collecting: logs (last {since_hours}h, capped)"),
    );
}

// --- 06-state ----------------------------------------------------------------

fn state_items(v: &mut Vec<Item>, env: &Env) {
    let start = v.len();
    // status file: agent writes to first writable of these; newest mtime wins
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = &env.libdir {
        candidates.push(d.join("status-netdata.json"));
    }
    if let Some(d) = &env.cachedir {
        candidates.push(d.join("status-netdata.json"));
    }
    // transient fallback locations only when netdata is actually on this
    // host, so a no-agent run can't package an unrelated
    // /tmp/status-netdata.json as crash state
    if env.confdir.is_some() || env.libdir.is_some() || env.netdata_pid.is_some() {
        candidates.push(PathBuf::from("/tmp/status-netdata.json"));
        candidates.push(PathBuf::from("/run/status-netdata.json"));
        candidates.push(PathBuf::from("/var/run/status-netdata.json"));
    }
    let newest = candidates
        .iter()
        .filter_map(|p| {
            let m = p.metadata().ok()?;
            if !m.is_file() {
                return None;
            }
            let t = m.modified().ok()?;
            Some((p.clone(), t))
        })
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p);
    if let Some(sf) = newest {
        push_file(
            v,
            crate::consts::PATH_STATUS_FILE,
            "Daemon status file: LAST EXIT/CRASH RECORD incl. fatal stack trace (read this first for crashes)",
            &sf,
        );
    }
    if let Some(libdir) = env.libdir.clone() {
        let libdir_s = libdir.display().to_string();
        push_native(
            v,
            "06-state/state-tree.txt",
            "State dir listing (bearer token filenames withheld - they are live tokens)",
            &format!("ls -laR {libdir_s} (bearer_tokens/ filenames withheld)"),
            {
                let libdir_s = libdir_s.clone();
                move |ctx| {
                    let t = run_s(ctx, &["ls", "-laR", &libdir_s]);
                    let mut out = String::new();
                    let mut skip = false;
                    let mut n = 0usize;
                    for line in head_lines(&t, 2000).lines() {
                        if line.ends_with("/bearer_tokens:") {
                            out.push_str(line);
                            out.push('\n');
                            skip = true;
                            n = 0;
                            continue;
                        }
                        if skip {
                            if line.is_empty() {
                                out.push_str(&format!(
                                "  [{n} token file(s) - names withheld, they ARE the tokens]\n\n"
                            ));
                                skip = false;
                                continue;
                            }
                            if line.starts_with("total") {
                                continue;
                            }
                            let is_dot = line.ends_with(" .") || line.ends_with(" ..");
                            if !is_dot {
                                n += 1;
                            }
                            continue;
                        }
                        out.push_str(line);
                        out.push('\n');
                    }
                    if skip {
                        out.push_str(&format!(
                            "  [{n} token file(s) - names withheld, they ARE the tokens]\n"
                        ));
                    }
                    out
                }
            },
        );
        push_native(
            v,
            crate::consts::PATH_CLOUD_STATE,
            "Cloud claim state (claimed_id is safe; token/private.pem are never collected)",
            &format!("ls -la {libdir_s}/cloud.d; cat {libdir_s}/cloud.d/claimed_id"),
            move |ctx| {
                let mut out = String::new();
                out.push_str("== cloud.d listing ==\n");
                out.push_str(&run_s(ctx, &["ls", "-la", &format!("{libdir_s}/cloud.d/")]));
                out.push_str("== claimed_id ==\n");
                match std::fs::read_to_string(format!("{libdir_s}/cloud.d/claimed_id")) {
                    Ok(id) => out.push_str(&id),
                    Err(_) => out.push_str("(no claimed_id file - agent not claimed)"),
                }
                out.push('\n');
                out.push_str("(token and private.pem intentionally NOT collected)\n");
                out
            },
        );
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
    if env.cachedir.is_some() || env.libdir.is_some() {
        let cachedir = env.cachedir.clone();
        let libdir = env.libdir.clone();
        push_native(
            v,
            "06-state/db-disk-usage.txt",
            "Database disk usage per tier + sqlite sizes + corruption sentinels",
            "du -sh CACHEDIR/* | sort -rh | head -30; ls -la LIBDIR/*.db*; corruption sentinels",
            move |ctx| {
                let mut out = String::new();
                if let Some(cd) = &cachedir {
                    let entries = sorted_dir_entries(cd);
                    if !entries.is_empty() {
                        let mut argv: Vec<String> = vec!["du".to_string(), "-sh".to_string()];
                        argv.extend(entries.iter().map(|p| p.display().to_string()));
                        let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
                        let t = run_s(ctx, &refs);
                        let mut lines: Vec<&str> = t.lines().collect();
                        lines.sort_by_key(|l| {
                            std::cmp::Reverse(human_size_key(
                                l.split_whitespace().next().unwrap_or("0"),
                            ))
                        });
                        out.push_str(&lines.into_iter().take(30).collect::<Vec<_>>().join("\n"));
                        out.push('\n');
                    }
                }
                if let Some(ld) = &libdir {
                    let dbs: Vec<PathBuf> = sorted_dir_entries(ld)
                        .into_iter()
                        .filter(|p| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.contains(".db"))
                        })
                        .collect();
                    if !dbs.is_empty() {
                        let mut argv: Vec<String> = vec!["ls".to_string(), "-la".to_string()];
                        argv.extend(dbs.iter().map(|p| p.display().to_string()));
                        let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
                        out.push_str(&run_s(ctx, &refs));
                    }
                }
                out.push_str(
                    "== sqlite corruption/recovery sentinels (presence = past corruption) ==\n",
                );
                let mut sentinels: Vec<PathBuf> = Vec::new();
                if let Some(cd) = &cachedir {
                    for p in sorted_dir_entries_with_hidden(cd) {
                        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if n.contains(".bad") || n.ends_with(".recover") {
                            sentinels.push(p);
                        }
                    }
                }
                if sentinels.is_empty() {
                    out.push_str("(none found)\n");
                } else {
                    let mut argv: Vec<String> = vec!["ls".to_string(), "-la".to_string()];
                    argv.extend(sentinels.iter().map(|p| p.display().to_string()));
                    let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
                    out.push_str(&run_s(ctx, &refs));
                }
                out
            },
        );
    }
    announce_at(v, start, "collecting: state");
}

fn sorted_dir_entries(dir: &Path) -> Vec<PathBuf> {
    sorted_dir_entries_inner(dir, false)
}

fn sorted_dir_entries_with_hidden(dir: &Path) -> Vec<PathBuf> {
    sorted_dir_entries_inner(dir, true)
}

fn sorted_dir_entries_inner(dir: &Path, hidden: bool) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    hidden
                        || !p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with('.'))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn human_size_key(s: &str) -> u64 {
    // parse du -h sizes ("1.5G", "980K", "0") into comparable bytes
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('K') => (&s[..s.len() - 1], 1u64 << 10),
        Some('M') => (&s[..s.len() - 1], 1u64 << 20),
        Some('G') => (&s[..s.len() - 1], 1u64 << 30),
        Some('T') => (&s[..s.len() - 1], 1u64 << 40),
        _ => (s, 1u64),
    };
    num.parse::<f64>()
        .map(|f| (f * mult as f64) as u64)
        .unwrap_or(0)
}

// --- 07-runtime --------------------------------------------------------------

// --- 08-network --------------------------------------------------------------

fn network_items(v: &mut Vec<Item>, obfuscate: bool) {
    let start = v.len();
    push_native(
        v,
        "08-network/listening-sockets.txt",
        "Listening sockets (netdata-related)",
        "ss -tlnp (sockstat -l / netstat -an fallback), filtered to 19999/netdata",
        |ctx| {
            let filter = |t: &str| -> String {
                let mut out = String::new();
                for (i, line) in t.lines().enumerate() {
                    if i == 0 || line.contains("19999") || line.contains("netdata") {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                out
            };
            if have("ss") {
                filter(&run_s(ctx, &["ss", "-tlnp"]))
            } else if have("sockstat") {
                filter(&run_s(ctx, &["sockstat", "-l"]))
            } else {
                let t = run_s(ctx, &["netstat", "-an"]);
                let mut out = String::new();
                for line in t.lines() {
                    if line.to_ascii_lowercase().contains("listen") && line.contains("19999") {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                out
            }
        },
    );
    if obfuscate {
        // search/domain values are often corporate-internal names outside
        // private TLDs
        push_native(
            v,
            "08-network/resolv-conf.txt",
            "DNS resolver config (search domains withheld)",
            "sed 's/^(search|domain).*/[SEARCH-DOMAINS-WITHHELD]/' /etc/resolv.conf",
            |_| {
                let Ok(t) = std::fs::read_to_string("/etc/resolv.conf") else {
                    return String::new();
                };
                let mut out = String::new();
                for line in t.lines() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("search ")
                        || trimmed.starts_with("search\t")
                        || trimmed.starts_with("domain ")
                        || trimmed.starts_with("domain\t")
                    {
                        let kw = if trimmed.starts_with("search") {
                            "search"
                        } else {
                            "domain"
                        };
                        out.push_str(&format!("{kw} [SEARCH-DOMAINS-WITHHELD]\n"));
                    } else {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                out
            },
        );
    } else {
        push_file(
            v,
            "08-network/resolv-conf.txt",
            "DNS resolver config",
            Path::new("/etc/resolv.conf"),
        );
    }
    push_native(
        v,
        "08-network/proxy-env.txt",
        "Proxy environment (this shell; see 03-process/process-environ.txt for the agent view)",
        "env | grep -iE '^(https?_proxy|no_proxy|all_proxy)='",
        |_| {
            let mut out = String::new();
            for (k, v) in std::env::vars() {
                let kl = k.to_ascii_lowercase();
                if ["http_proxy", "https_proxy", "no_proxy", "all_proxy"].contains(&kl.as_str()) {
                    out.push_str(&format!("{k}={v}\n"));
                }
            }
            if out.is_empty() {
                out.push_str("(no proxy variables set)\n");
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
        "netdata-support-bundle {}",
        crate::consts::TOOL_VERSION
    ));
    info(&format!(
        "agent pid: {} | api: {} | config: {} | container: {}",
        env.netdata_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "not running".to_string()),
        if env.api_ok { "up" } else { "unreachable" },
        env.confdir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".to_string()),
        u8::from(env.is_container),
    ));
}

/// Tier 2: the declared collection plan for this environment, in execution
/// (= triage priority) order. Pure: builds items, runs nothing.
pub fn build_items(env: &Env, opts: &PlanOpts) -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    system_items(&mut v, opts.since_hours);
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
    network_items(&mut v, opts.obfuscate);
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

/// The note appended to "agent running:" when the process appears to run
/// inside a container while no local install exists on this host.
pub fn agent_container_note(env: &Env) -> String {
    if let Some(pid) = env.netdata_pid {
        if !env.is_container && env.confdir.is_none() {
            let in_container = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
                .map(|t| {
                    ["docker", "containerd", "kubepods", "lxc"]
                        .iter()
                        .any(|k| t.contains(k))
                })
                .unwrap_or(false);
            if in_container {
                return " (process appears to run INSIDE a container; no local install found on this host)"
                    .to_string();
            }
        }
    }
    String::new()
}

pub fn summary_inputs(env: &Env) -> SummaryInputs {
    SummaryInputs {
        agent_pid: env.netdata_pid,
        agent_note: agent_container_note(env),
        api_ok: env.api_ok,
        is_container: env.is_container,
        confdir: env.confdir.as_ref().map(|p| p.display().to_string()),
        ran_privileged: ran_privileged(),
        docker_logs_needed: env.docker_logs_needed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Source;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-plan-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_env() -> Env {
        Env {
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
        }
    }

    fn opts() -> PlanOpts {
        PlanOpts {
            since_hours: 24,
            obfuscate: true,
        }
    }

    fn rels(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.rel.as_str()).collect()
    }

    #[test]
    fn plan_starts_with_uname_ends_with_cloud_probe_and_rels_are_unique() {
        let items = build_items(&fixture_env(), &opts());
        let r = rels(&items);
        assert_eq!(r.first(), Some(&"01-system/uname.txt"));
        assert_eq!(r.last(), Some(&"08-network/cloud-connectivity.txt"));
        let mut sorted = r.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), r.len(), "duplicate bundle paths in the plan");
        // section prefixes never go backwards: list order IS triage order
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
        env.is_container = false;
        env.api_ok = false;
        env.netdata_pid = None;
        let r_min: Vec<String> = build_items(&env, &opts())
            .iter()
            .map(|i| i.rel.clone())
            .collect();
        assert!(!r_min.iter().any(|p| p.contains("container-context")));
        assert!(
            !r_min
                .iter()
                .any(|p| p == "04-config/effective-netdata.conf")
        );
        assert!(!r_min.iter().any(|p| p.starts_with("03-process/proc-")));
        assert!(r_min.iter().any(|p| p == "07-runtime/AGENT-WAS-DOWN.txt"));

        env.is_container = true;
        env.api_ok = true;
        env.netdata_pid = Some(42);
        let r_full: Vec<String> = build_items(&env, &opts())
            .iter()
            .map(|i| i.rel.clone())
            .collect();
        assert!(r_full.iter().any(|p| p.contains("container-context")));
        assert!(
            r_full
                .iter()
                .any(|p| p == "04-config/effective-netdata.conf")
        );
        assert!(r_full.iter().any(|p| p == "07-runtime/info-v3.json"));
        assert!(!r_full.iter().any(|p| p == "07-runtime/AGENT-WAS-DOWN.txt"));
    }

    #[test]
    fn config_walk_is_eager_bounded_and_excludes_key_material() {
        let dir = scratch("confwalk");
        std::fs::create_dir_all(dir.join("go.d")).unwrap();
        std::fs::create_dir_all(dir.join("ssl")).unwrap();
        std::fs::write(dir.join("go.d/nginx.conf"), "x\n").unwrap();
        std::fs::write(dir.join("ssl/cert.conf"), "x\n").unwrap();
        std::fs::write(dir.join("key.pem"), "x\n").unwrap();
        std::fs::write(dir.join("host.key"), "x\n").unwrap();
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
        assert_eq!(
            walk.len(),
            200,
            "walk budget must cap at 200: {}",
            walk.len()
        );
        assert!(walk.contains(&"04-config/go.d/nginx.conf"));
        assert!(!walk.iter().any(|p| p.contains("ssl/")));
        assert!(
            !walk
                .iter()
                .any(|p| p.ends_with(".pem") || p.ends_with(".key"))
        );
        // the named top-level configs are separate fixed items, not walk items
        assert!(!walk.contains(&"04-config/netdata.conf"));
        assert!(items.iter().any(|i| i.rel == "04-config/netdata.conf"
            && matches!(&i.source, Source::File { cap, .. } if *cap != CONF_FILE_CAP)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_listing_contains_no_secret_shaped_output() {
        // --list prints rel + describe_source BEFORE any sanitizer exists;
        // origins must be built from paths and tool names only
        let mut env = fixture_env();
        env.confdir = Some(PathBuf::from("/etc/netdata"));
        env.libdir = Some(PathBuf::from("/var/lib/netdata"));
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
