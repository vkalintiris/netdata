use std::ffi::CString;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, SystemTime};

use super::io::{self, Locations};
use super::*;

/// What `daemon_status_file_init()` recomputes after a load: the texts the file carries only as times.
fn with_texts(mut ds: StatusFile) -> StatusFile {
    ds.timestamp_rfc3339 = rfc3339(ds.timestamp_ut);
    if ds.host_id.last_modified_ut != 0 {
        ds.host_id.last_modified_rfc3339 = rfc3339(ds.host_id.last_modified_ut);
    }
    if ds.disk_footprint.last_updated_ut != 0 {
        ds.disk_footprint.last_updated_rfc3339 = rfc3339(ds.disk_footprint.last_updated_ut);
    }
    ds
}

fn full() -> StatusFile {
    let uuid = |b: u8| [b; 16];
    let mut ds = StatusFile {
        v: VERSION,
        version: FixedStr::from("v2.11.0-458-g1e97a0fc9e"),
        status: DaemonStatus::Running,
        exit_reason: exit_reason::SIGSEGV | exit_reason::FATAL,
        profile: profile::PARENT | profile::IOT,
        os_type: OsType::Linux,
        db_mode: db_mode::ALLOC,
        cloud_status: cloud_status::INDIRECT,
        db_tiers: 3,
        kubernetes: true,
        sentry_available: true,
        boottime: 39623,
        uptime: 21,
        timestamp_ut: 1_790_199_184_050_000,
        restarts: 7,
        crashes: 2,
        posts: 5,
        reliability: -3,
        pid: 4242,
        host_id: HostId {
            uuid: uuid(0x11),
            last_modified_ut: 1_790_199_163_330_000,
            ..Default::default()
        },
        boot_id: uuid(0x22),
        invocation: uuid(0x33),
        node_id: uuid(0x44),
        claim_id: uuid(0x55),
        machine_id: uuid(0x66),
        timings: Timings {
            init: 6,
            exit: 2,
            ..Default::default()
        },
        oom_protection: 3_360_811_909,
        netdata_max_rss: 73_666_560,
        memory: Memory {
            total: 33_659_879_424,
            available: 21_564_096_512,
        },
        var_cache: DiskSpace {
            total_bytes: 422_533_181_440,
            free_bytes: 366_935_478_272,
            total_inodes: 26_206_208,
            free_inodes: 25_699_929,
            read_only: true,
        },
        disk_footprint: DiskFootprint {
            dbengine: 1,
            sqlite: 2,
            other: 36,
            last_updated_ut: 1_790_199_163_370_000,
            ..Default::default()
        },
        metrics: Metrics {
            nodes_total: 4,
            nodes_receiving: 3,
            nodes_sending: 1,
            nodes_archived: 2,
            metrics: Counts {
                collected: 2429,
                available: 2483,
            },
            instances: Counts {
                collected: 214,
                available: 965,
            },
            contexts: Counts {
                collected: 291,
                available: 294,
            },
        },
        install_type: FixedStr::from("custom"),
        architecture: FixedStr::from("x86_64"),
        virtualization: FixedStr::from("kvm"),
        container: FixedStr::from("none"),
        kernel_version: FixedStr::from("6.12.107"),
        os_name: FixedStr::from("Debian GNU/Linux"),
        os_version: FixedStr::from("13 (trixie)"),
        os_id: FixedStr::from("debian"),
        os_id_like: FixedStr::from("unknown"),
        timezone: FixedStr::from("Etc/UTC"),
        cloud_provider_type: FixedStr::from("GCP"),
        cloud_instance_type: FixedStr::from("e2-small"),
        cloud_instance_region: FixedStr::from("region-a"),
        system_cpus: 16,
        stack_traces: FixedStr::from("libbacktrace"),
        ..Default::default()
    };
    let h = &mut ds.hw;
    h.sys_vendor.set("QEMU");
    h.sys_uuid.set("sys-uuid");
    h.product_id.set("pid");
    h.product_name.set("Standard PC");
    h.product_version.set("pc-9.2");
    h.product_sku.set("sku");
    h.product_family.set("family");
    h.board_name.set("board");
    h.board_version.set("1");
    h.board_vendor.set("vendor");
    h.chassis_type.set("1");
    h.chassis_vendor.set("QEMU");
    h.chassis_version.set("pc-9.2");
    h.bios_date.set("04/01/2014");
    h.bios_release.set("0.0");
    h.bios_version.set("rel-1.16.3");
    h.bios_vendor.set("SeaBIOS");
    ds.product = Product {
        vendor: FixedStr::from("KVM"),
        id: FixedStr::from("pid"),
        name: FixedStr::from("Standard PC"),
        kind: FixedStr::from("vm"),
    };
    ds.fatal = Fatal {
        line: 123,
        filename: FixedStr::from("daemon.c"),
        function: FixedStr::from("main"),
        errno: FixedStr::from("2, No such file or directory"),
        message: FixedStr::from("a \"quoted\"\nmessage"),
        stack_trace: FixedStr::from("#1 main"),
        thread: FixedStr::from("MAIN"),
        thread_id: 99,
        signal_code: signal_code::create(11, 1),
        fault_address: 0xDEAD_BEEF,
        worker_job_id: 8,
        sentry: true,
    };
    with_texts(ds)
}

#[test]
fn round_trips_as_c() {
    let ds = full();
    let text = to_json(&ds);
    let mut back = StatusFile::default();
    assert!(
        from_json(&text, &mut back),
        "{}",
        String::from_utf8_lossy(&text)
    );

    let mut expected = ds;
    // C writes `fatal.sentry` as a boolean and reads it as a signal code's text, so it never comes back
    expected.fatal.sentry = false;
    assert_eq!(with_texts(back), expected);
    // the times' texts are not in the file
    assert!(back.timestamp_rfc3339.is_empty() && back.host_id.last_modified_rfc3339.is_empty());
    assert_eq!(to_json(&with_texts(back)), to_json(&expected));
}

#[test]
fn reads_and_writes_the_oracle_file_byte_for_byte() {
    let golden = include_bytes!("../../tests/golden/status-netdata-master-prod.json");
    let mut ds = StatusFile::default();
    assert!(from_json(golden, &mut ds));
    assert_eq!(
        (
            ds.v,
            ds.status,
            ds.exit_reason,
            ds.profile,
            ds.restarts,
            ds.cloud_status,
            ds.db_tiers
        ),
        (
            29,
            DaemonStatus::Exited,
            exit_reason::SIGTERM,
            profile::STANDALONE,
            1,
            cloud_status::ONLINE,
            3
        )
    );
    assert_eq!(
        ds.fatal
            .stack_trace
            .as_bytes()
            .split(|&c| c == b'\n')
            .count(),
        23
    );
    let written = to_json(&with_texts(ds));
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(golden)
    );
}

/// C's file after `kill -SEGV` while running: its signal members read, and the file written back byte for byte.
#[test]
fn reads_and_writes_a_c_crash_file_byte_for_byte() {
    let golden = include_bytes!("../../tests/golden/status-netdata-master-prod-sigsegv.json");
    let mut ds = StatusFile::default();
    assert!(from_json(golden, &mut ds));
    let f = &ds.fatal;
    assert_eq!(ds.exit_reason, exit_reason::SIGSEGV);
    assert_eq!(ds.status, DaemonStatus::Running);
    assert_eq!(f.signal_code, signal_code::create(11, 0));
    assert_eq!(f.fault_address, 0x3E8_002E_6A04);
    assert_eq!((f.thread.as_bytes(), f.function.as_bytes()), (&b"NO_NAME"[..], &b"nd_process_signals"[..]));
    assert_eq!((f.thread_id, f.worker_job_id, f.line), (4242, 0, 0));
    assert!(f.stack_trace.as_bytes().starts_with(b"#0 <unknown> ["));
    let written = to_json(&with_texts(ds));
    assert_eq!(String::from_utf8_lossy(&written), String::from_utf8_lossy(golden));
}

#[test]
fn reads_the_pre_v18_keys() {
    let text = br#"{
        "version": 17,
        "@timestamp": "2025-01-02T03:04:05.06Z",
        "agent": {
            "ND_status": "exiting",
            "ND_exit_reason": ["signal-abort", "no-such-reason"],
            "ND_profile": ["child"],
            "ND_restarts": 9,
            "ND_db_mode": "alloc",
            "ND_db_tiers": 2,
            "ND_kubernetes": "yes",
            "ND_sentry_available": 1,
            "ND_install_type": "kickstart",
            "ND_timings": {"init": 4, "exit": 5},
            "status": "running",
            "reliability": 5
        },
        "fatal": {"signal_code": "SIGSEGV/SEGV_ACCERR", "sentry": "SIGBUS", "thread_id": 7}
    }"#;
    let mut ds = StatusFile::default();
    assert!(from_json(text, &mut ds));
    let expected = StatusFile {
        v: 17,
        timestamp_ut: 1_735_787_045_060_000,
        status: DaemonStatus::Exiting,
        exit_reason: exit_reason::SIGABRT,
        profile: profile::CHILD,
        restarts: 9,
        db_mode: db_mode::ALLOC,
        db_tiers: 2,
        kubernetes: true,
        sentry_available: true,
        install_type: FixedStr::from("kickstart"),
        timings: Timings {
            init: 4,
            exit: 5,
            ..Default::default()
        },
        fatal: Fatal {
            signal_code: signal_code::create(11, 2),
            sentry: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(ds, expected);

    // before v14 the mode and tiers were not saved: C assumes its compile-time default
    let mut ds = StatusFile::default();
    assert!(from_json(
        br#"{"version": 13, "agent": {"ND_db_mode": "ram", "ND_db_tiers": 5}}"#,
        &mut ds
    ));
    assert_eq!((ds.db_mode, ds.db_tiers), (db_mode::DBENGINE, 0));
}

#[test]
fn a_hard_failure_keeps_what_was_read() {
    let mut ds = StatusFile::default();
    let text = br#"{"version": 29, "agent": {"restarts": 3, "crashes": "many", "pid": 5}, "host": {"system_cpus": 2}}"#;
    assert!(!from_json(text, &mut ds));
    assert_eq!(
        (ds.v, ds.restarts, ds.crashes, ds.pid, ds.system_cpus),
        (29, 3, 0, 0, 0)
    );

    // no version, or not an object
    for text in [&br#"{"agent": {}}"#[..], b"[]", b"{", b""] {
        assert!(
            !from_json(text, &mut StatusFile::default()),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
}

/// The members the writer gates on the record's version, with the version each needs.
const WRITER_GATES: [(&str, u32); 21] = [
    (".agent.since", 24),
    (".agent.crashes", 24),
    (".agent.pid", 27),
    (".agent.posts", 22),
    (".agent.aclk", 22),
    (".agent.db_mode", 14),
    (".agent.db_tiers", 14),
    (".agent.kubernetes", 14),
    (".agent.sentry_available", 16),
    (".agent.reliability", 18),
    (".agent.stack_traces", 18),
    (".host.timezone", 20),
    (".host.cloud_provider", 20),
    (".host.cloud_instance", 20),
    (".host.cloud_region", 20),
    (".host.memory.netdata", 21),
    (".host.memory.oom_protection", 21),
    (".fatal.signal_code", 16),
    (".fatal.sentry", 17),
    (".fatal.fault_address", 18),
    (".fatal.worker_job_id", 23),
];

fn written_keys(v: u32) -> Vec<String> {
    let text = to_json(&StatusFile { v, ..full() });
    let value: serde_json::Value = serde_json::from_slice(&text).unwrap();
    fn walk(prefix: &str, v: &serde_json::Value, keys: &mut Vec<String>) {
        if let serde_json::Value::Object(o) = v {
            for (k, v) in o {
                let path = format!("{prefix}.{k}");
                keys.push(path.clone());
                walk(&path, v, keys);
            }
        }
    }
    let mut keys = Vec::new();
    walk("", &value, &mut keys);
    keys
}

#[test]
fn the_record_version_gates_the_members() {
    for (member, v) in WRITER_GATES {
        assert!(written_keys(v).iter().any(|k| k == member), "{member} at {v}");
        assert!(!written_keys(v - 1).iter().any(|k| k == member), "{member} at {}", v - 1);
    }
    // the rest are always written
    let (old, new) = (written_keys(0), written_keys(VERSION));
    let mut added: Vec<_> = new.iter().filter(|k| !old.contains(k)).map(String::as_str).collect();
    let mut gated: Vec<_> = WRITER_GATES.iter().map(|(m, _)| *m).collect();
    added.sort_unstable();
    gated.sort_unstable();
    assert_eq!(added, gated);
    // "version" is always the writer's own
    let text = to_json(&StatusFile { v: 13, ..full() });
    assert!(String::from_utf8_lossy(&text).contains("\"version\":29,"));
}

/// The full record's file as a record of version `v` would have it: its version, and the agent's keys with the
/// `ND_` prefix before 18.
fn file_of_version(v: u32) -> Vec<u8> {
    let mut text = String::from_utf8(to_json(&full())).unwrap().replace("\"version\":29,", &format!("\"version\":{v},"));
    if v < 18 {
        for key in [
            "profile", "status", "exit_reason", "node_id", "claim_id", "install_type", "timings", "restarts",
            "db_mode", "db_tiers", "kubernetes", "sentry_available",
        ] {
            text = text.replace(&format!("\"{key}\":"), &format!("\"ND_{key}\":"));
        }
    }
    text.into_bytes()
}

#[test]
fn the_file_version_gates_what_is_read() {
    let read = |v: u32| {
        let mut ds = StatusFile::default();
        assert!(from_json(&file_of_version(v), &mut ds), "{v}");
        ds
    };
    let all = full();
    type Get = fn(&StatusFile) -> String;
    let gates: [(u32, &str, Get); 17] = [
        (4, "restarts", |d| d.restarts.to_string()),
        (14, "db_mode", |d| d.db_mode.to_string()),
        (14, "kubernetes", |d| d.kubernetes.to_string()),
        (16, "signal_code", |d| d.fatal.signal_code.to_string()),
        (17, "sentry_available", |d| d.sentry_available.to_string()),
        (18, "reliability", |d| d.reliability.to_string()),
        (18, "stack_traces", |d| format!("{:?}", d.stack_traces)),
        (18, "thread_id", |d| d.fatal.thread_id.to_string()),
        (18, "fault_address", |d| d.fatal.fault_address.to_string()),
        (20, "timezone", |d| format!("{:?}", d.timezone)),
        (20, "cloud_region", |d| format!("{:?}", d.cloud_instance_region)),
        (21, "netdata", |d| d.netdata_max_rss.to_string()),
        (22, "posts", |d| d.posts.to_string()),
        (22, "aclk", |d| d.cloud_status.to_string()),
        (23, "worker_job_id", |d| d.fatal.worker_job_id.to_string()),
        (24, "since", |d| d.host_id.last_modified_ut.to_string()),
        (27, "pid", |d| d.pid.to_string()),
    ];
    for (v, name, get) in gates {
        assert_eq!(get(&read(v)), get(&all), "{name} read at {v}");
        assert_ne!(get(&read(v - 1)), get(&all), "{name} not read at {}", v - 1);
    }
}

#[test]
fn reads_what_json_c_reads() {
    // a multi-byte character cut at a text's limit: the record still loads (D89)
    let mut ds = full();
    ds.fatal.message.set(format!("{}é", "x".repeat(510)));
    assert_eq!(ds.fatal.message.as_bytes().len(), 511);
    let mut back = StatusFile::default();
    assert!(from_json(&to_json(&ds), &mut back));
    assert_eq!(back.restarts, ds.restarts);
    // U+FFFD in the lone byte's place, itself cut at the limit: 0xEF where C keeps 0xC3
    let mut want = vec![b'x'; 510];
    want.push(0xef);
    assert_eq!(back.fatal.message.as_bytes(), &want[..]);
    // text after the root object is ignored
    let mut ds = StatusFile::default();
    assert!(from_json(b"{\"version\":29,\"agent\":{\"restarts\":7}} xyz", &mut ds));
    assert_eq!(ds.restarts, 7);
    // a bitmap keeps the bits read before a bad item
    let mut ds = StatusFile::default();
    assert!(!from_json(br#"{"version":29,"agent":{"exit_reason":["signal-segmentation-fault",5]}}"#, &mut ds));
    assert_eq!(ds.exit_reason, exit_reason::SIGSEGV);
}

/// A name no other test or process uses: a save unlinks it from `/tmp`, `/run`, `/var/run` and `.` too.
fn unique_name(tag: &str) -> CString {
    CString::new(format!(
        "status-netdata-test-{}-{tag}.json",
        std::process::id()
    ))
    .unwrap()
}

fn read(path: &std::path::Path) -> Option<Vec<u8>> {
    io::read_text(path, 65536)
}

#[test]
fn saves_and_loads_through_the_primary_directory() {
    let (lib, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let loc = Locations::new(lib.path().to_str().unwrap(), cache.path().to_str().unwrap());
    let name = unique_name("primary");
    let file = name.to_str().unwrap();

    // an older copy in a fallback is removed by the first save into the primary directory
    std::fs::write(cache.path().join(file), b"old").unwrap();
    assert!(io::save(&loc, &name, b"{\"version\":29}", false));
    assert_eq!(
        std::fs::read(lib.path().join(file)).unwrap(),
        b"{\"version\":29}"
    );
    assert!(!cache.path().join(file).exists());
    let mode = std::fs::metadata(lib.path().join(file))
        .unwrap()
        .permissions()
        .mode();
    // `fchmod()` sets it whatever the umask
    assert_eq!(mode & 0o777, 0o664);
    // no temporary file is left behind
    assert_eq!(std::fs::read_dir(lib.path()).unwrap().count(), 1);

    // later saves replace it whole
    assert!(io::save(&loc, &name, b"{}", false));
    let mut loaded = None;
    assert!(io::load(&loc, &name, false, |path| {
        loaded = read(path);
        loaded.is_some()
    }));
    assert_eq!(loaded.as_deref(), Some(&b"{}"[..]));
}

#[test]
fn falls_back_when_the_primary_directory_refuses() {
    let (base, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let missing = base.path().join("missing");
    let loc = Locations::new(missing.to_str().unwrap(), cache.path().to_str().unwrap());
    let name = unique_name("fallback");
    assert!(io::save(&loc, &name, b"data", false));
    assert_eq!(
        std::fs::read(cache.path().join(name.to_str().unwrap())).unwrap(),
        b"data"
    );
    assert!(!missing.exists());
}

#[test]
fn loads_the_newest_copy() {
    let (lib, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let loc = Locations::new(lib.path().to_str().unwrap(), cache.path().to_str().unwrap());
    let name = unique_name("newest");
    let file = name.to_str().unwrap();
    let put = |dir: &std::path::Path, content: &[u8], age: u64| {
        let path = dir.join(file);
        std::fs::write(&path, content).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    };
    let loaded = |loc: &Locations| {
        let mut got = None;
        io::load(loc, &name, false, |path| {
            got = read(path);
            got.is_some()
        });
        got
    };

    put(lib.path(), b"primary", 100);
    put(cache.path(), b"cache", 10);
    assert_eq!(loaded(&loc).as_deref(), Some(&b"cache"[..]));

    // the same time keeps the primary copy
    put(cache.path(), b"cache", 100);
    let t = std::fs::metadata(lib.path().join(file))
        .unwrap()
        .modified()
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(cache.path().join(file))
        .unwrap()
        .set_modified(t)
        .unwrap();
    assert_eq!(loaded(&loc).as_deref(), Some(&b"primary"[..]));

    // an empty file is not a candidate; a refused parse is C's "not found"
    std::fs::write(lib.path().join(file), b"").unwrap();
    assert_eq!(loaded(&loc).as_deref(), Some(&b"cache"[..]));
    assert!(!io::load(&loc, &name, false, |_| false));

    // a file over 64 KiB is not read
    std::fs::write(cache.path().join(file), vec![b' '; 65537]).unwrap();
    assert_eq!(loaded(&loc), None);
    std::fs::write(cache.path().join(file), vec![b' '; 65536]).unwrap();
    assert_eq!(loaded(&loc).map(|c| c.len()), Some(65536));
}

#[test]
fn reuses_a_regular_leftover_and_refuses_anything_else() {
    let dir = tempfile::tempdir().unwrap();
    let d = CString::new(dir.path().to_str().unwrap()).unwrap();
    let name = unique_name("leftover");
    let file = name.to_str().unwrap();
    let temp = |n: u64| dir.path().join(format!("{file}-{n}"));
    // an interrupted save's temporary file is reused, truncated first
    std::fs::write(temp(1), vec![b'z'; 100]).unwrap();
    assert!(io::save_attempt(&d, &name, b"new", 1));
    assert_eq!(std::fs::read(dir.path().join(file)).unwrap(), b"new");
    assert!(!temp(1).exists());
    // a symlink in its place is not followed, a directory not used
    let target = dir.path().join("target");
    std::fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, temp(2)).unwrap();
    assert!(!io::save_attempt(&d, &name, b"evil", 2));
    assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    std::fs::create_dir(temp(3)).unwrap();
    assert!(!io::save_attempt(&d, &name, b"x", 3));
    assert_eq!(std::fs::read(dir.path().join(file)).unwrap(), b"new");
}

/// The save a signal handler or a failed allocation makes (D91.3): the largest record, escaped the most, written in
/// the reserved writer and saved, allocates nothing, and gives `to_json()`'s bytes.
#[test]
fn the_signal_path_saves_without_allocating() {
    let mut ds = full();
    ds.fatal.stack_trace.set(vec![0x01u8; 4095]);
    ds.fatal.message.set(vec![b'"'; 511]);
    ds.fatal.signal_code = signal_code::create(11, 1);
    ds.fatal.fault_address = u64::MAX;
    let mut w = netdata_agent_text::json::JsonWriter::new(netdata_agent_text::json::JsonOptions::DEFAULT);
    w.reserve(64 * 1024);
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations::new(dir.path().to_str().unwrap(), dir.path().to_str().unwrap());
    let name = unique_name("no-alloc");
    let before = netdata_agent_sys::allocations();
    super::json::to_json_in(&mut w, &ds);
    let saved = io::save(&loc, &name, w.as_bytes(), false);
    let allocations = netdata_agent_sys::allocations() - before;
    assert!(saved);
    assert_eq!(allocations, 0);
    // the counter counts: the allocating writer moves it
    let before = netdata_agent_sys::allocations();
    let _ = to_json(&ds);
    assert!(netdata_agent_sys::allocations() > before);
    assert_eq!(w.as_bytes(), &to_json(&ds)[..]);
    assert_eq!(std::fs::read(dir.path().join(name.to_str().unwrap())).unwrap(), to_json(&ds));
}
