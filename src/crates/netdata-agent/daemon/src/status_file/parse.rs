//! `daemon_status_file_from_json()` (`status-file.c:592-866`): every member optional but `version`, the pre-v18
//! keys with their `ND_` prefix, C's coercions (`ingest::jsonc`), and C's partial fill: a hard failure stops the parse
//! and leaves what was read (D88.3).

use netdata_agent_ingest::jsonc::{self, Presence};
use netdata_agent_text::parse::str2ull_encoded;
use serde_json::{Map, Value};

use super::{
    DaemonStatus, FixedStr, OsType, StatusFile, cloud_status, db_mode, exit_reason, profile,
    signal_code,
};

type Obj = Map<String, Value>;

const OPT: Presence = Presence::Optional;

/// `JSONC_PARSE_TXT2CHAR_OR_ERROR_AND_RETURN`: the text (json-c's for numbers and booleans), empty for null, for
/// another type or when missing.
fn txt<const N: usize>(o: &Obj, member: &str, dst: &mut FixedStr<N>, e: &mut String) -> Option<()> {
    let text = jsonc::txt(o, "", member, OPT, e)?;
    dst.set(text.as_deref().unwrap_or_default());
    Some(())
}

fn uint64(o: &Obj, member: &str, e: &mut String) -> Option<u64> {
    jsonc::uint64(o, "", member, OPT, e)
}

/// `JSONC_PARSE_TXT2ENUM_OR_ERROR_AND_RETURN`: `dst` changes only for a string.
fn enum_of<T>(
    o: &Obj,
    member: &str,
    convert: fn(&[u8]) -> T,
    dst: &mut T,
    e: &mut String,
) -> Option<()> {
    if let Some(text) = jsonc::enum_text(o, "", member, OPT, e)? {
        *dst = convert(text);
    }
    Some(())
}

/// `JSONC_PARSE_SUBOBJECT`: `block` over the member's object, when it is one.
fn object(
    o: &Obj,
    member: &str,
    e: &mut String,
    block: impl FnOnce(&Obj, &mut String) -> Option<()>,
) -> Option<()> {
    match jsonc::object(o, "", member, OPT, e)? {
        Some(sub) => block(sub, e),
        None => Some(()),
    }
}

/// Fills `ds` from a status file's text as C does, true when it parsed through.
pub fn from_json(text: &[u8], ds: &mut StatusFile) -> bool {
    let Some(Value::Object(root)) = jsonc::tokener_parse(text) else {
        return false;
    };
    let mut error = String::new();
    parse(&root, ds, &mut error).is_some()
}

fn parse(root: &Obj, ds: &mut StatusFile, e: &mut String) -> Option<()> {
    let version = jsonc::uint64(root, "", "version", Presence::Required, e)?;
    ds.v = version as u32;
    ds.timestamp_ut = jsonc::rfc3339(root, "", "@timestamp", OPT, e)?;
    let key = |current: &'static str, old: &'static str| if version >= 18 { current } else { old };

    object(root, "agent", e, |o, e| {
        ds.host_id.uuid = jsonc::uuid(o, "", "id", OPT, e)?;
        if version >= 24 {
            ds.host_id.last_modified_ut = jsonc::rfc3339(o, "", "since", OPT, e)?;
        }
        ds.invocation = jsonc::uuid(o, "", "ephemeral_id", OPT, e)?;
        txt(o, "version", &mut ds.version, e)?;
        ds.uptime = uint64(o, "uptime", e)? as i64;
        jsonc::bitmap_into(o, "", key("profile", "ND_profile"), profile::from_name, OPT, e, &mut ds.profile)?;
        enum_of(
            o,
            key("status", "ND_status"),
            DaemonStatus::from_name,
            &mut ds.status,
            e,
        )?;
        jsonc::bitmap_into(
            o, "",
            key("exit_reason", "ND_exit_reason"),
            exit_reason::from_name,
            OPT,
            e,
            &mut ds.exit_reason,
        )?;
        ds.node_id = jsonc::uuid(o, "", key("node_id", "ND_node_id"), OPT, e)?;
        ds.claim_id = jsonc::uuid(o, "", key("claim_id", "ND_claim_id"), OPT, e)?;
        txt(
            o,
            key("install_type", "ND_install_type"),
            &mut ds.install_type,
            e,
        )?;
        object(o, key("timings", "ND_timings"), e, |o, e| {
            ds.timings.init = uint64(o, "init", e)? as i64;
            ds.timings.exit = uint64(o, "exit", e)? as i64;
            Some(())
        })?;
        if version >= 4 {
            ds.restarts = uint64(o, key("restarts", "ND_restarts"), e)?;
        }
        if version >= 24 {
            ds.crashes = uint64(o, "crashes", e)?;
        }
        if version >= 27 {
            ds.pid = uint64(o, "pid", e)? as i32;
        }
        if version >= 22 {
            ds.posts = uint64(o, "posts", e)?;
            enum_of(o, "aclk", cloud_status::from_name, &mut ds.cloud_status, e)?;
        }
        if version >= 14 {
            enum_of(
                o,
                key("db_mode", "ND_db_mode"),
                db_mode::from_name,
                &mut ds.db_mode,
                e,
            )?;
            ds.db_tiers = uint64(o, key("db_tiers", "ND_db_tiers"), e)? as u8;
            ds.kubernetes = jsonc::boolean(o, "", key("kubernetes", "ND_kubernetes"), OPT, e)?;
        } else {
            // C's compile default, and `nd_profile.storage_tiers` before the profile's setup
            ds.db_mode = db_mode::DBENGINE;
            ds.db_tiers = 0;
            ds.kubernetes = false;
        }
        if version >= 17 {
            ds.sentry_available =
                jsonc::boolean(o, "", key("sentry_available", "ND_sentry_available"), OPT, e)?;
        } else if version == 16 {
            ds.sentry_available = jsonc::boolean(o, "", "ND_sentry", OPT, e)?;
        }
        if version >= 18 {
            ds.reliability = jsonc::int64(o, "", "reliability", OPT, e)?;
            txt(o, "stack_traces", &mut ds.stack_traces, e)?;
        }
        Some(())
    })?;

    object(root, "host", e, |o, e| {
        ds.machine_id = jsonc::uuid(o, "", "id", OPT, e)?;
        txt(o, "architecture", &mut ds.architecture, e)?;
        txt(o, "virtualization", &mut ds.virtualization, e)?;
        txt(o, "container", &mut ds.container, e)?;
        ds.boottime = uint64(o, "uptime", e)? as i64;
        ds.system_cpus = uint64(o, "system_cpus", e)?;
        object(o, "boot", e, |o, e| {
            ds.boot_id = jsonc::uuid(o, "", "id", OPT, e)?;
            Some(())
        })?;
        object(o, "memory", e, |o, e| {
            ds.memory.total = uint64(o, "total", e)?;
            ds.memory.available = uint64(o, "free", e)?;
            if ds.memory.total == 0 {
                ds.memory = Default::default();
            }
            if version >= 21 {
                ds.netdata_max_rss = uint64(o, "netdata", e)?;
                ds.oom_protection = uint64(o, "oom_protection", e)?;
            }
            Some(())
        })?;
        object(o, "disk", e, |o, e| {
            object(o, "db", e, |o, e| {
                let db = &mut ds.var_cache;
                db.total_bytes = uint64(o, "total", e)?;
                db.free_bytes = uint64(o, "free", e)?;
                db.total_inodes = uint64(o, "inodes_total", e)?;
                db.free_inodes = uint64(o, "inodes_free", e)?;
                db.read_only = jsonc::boolean(o, "", "read_only", OPT, e)?;
                if db.total_bytes == 0 {
                    *db = Default::default();
                }
                Some(())
            })?;
            object(o, "netdata", e, |o, e| {
                let f = &mut ds.disk_footprint;
                f.dbengine = uint64(o, "dbengine", e)?;
                f.sqlite = uint64(o, "sqlite", e)?;
                f.other = uint64(o, "other", e)?;
                f.last_updated_ut = jsonc::rfc3339(o, "", "last_updated", OPT, e)?;
                Some(())
            })
        })?;
        if version >= 20 {
            txt(o, "timezone", &mut ds.timezone, e)?;
            txt(o, "cloud_provider", &mut ds.cloud_provider_type, e)?;
            txt(o, "cloud_instance", &mut ds.cloud_instance_type, e)?;
            txt(o, "cloud_region", &mut ds.cloud_instance_region, e)?;
        }
        Some(())
    })?;

    object(root, "metrics", e, |o, e| {
        let m = &mut ds.metrics;
        object(o, "nodes", e, |o, e| {
            m.nodes_total = uint64(o, "total", e)?;
            m.nodes_receiving = uint64(o, "receiving", e)?;
            m.nodes_sending = uint64(o, "sending", e)?;
            m.nodes_archived = uint64(o, "archived", e)?;
            Some(())
        })?;
        for (key, counts) in [
            ("metrics", &mut m.metrics),
            ("instances", &mut m.instances),
            ("contexts", &mut m.contexts),
        ] {
            object(o, key, e, |o, e| {
                counts.collected = uint64(o, "collected", e)?;
                counts.available = uint64(o, "available", e)?;
                Some(())
            })?;
        }
        Some(())
    })?;

    object(root, "os", e, |o, e| {
        enum_of(o, "type", OsType::from_name, &mut ds.os_type, e)?;
        txt(o, "kernel", &mut ds.kernel_version, e)?;
        txt(o, "name", &mut ds.os_name, e)?;
        txt(o, "version", &mut ds.os_version, e)?;
        txt(o, "family", &mut ds.os_id, e)?;
        txt(o, "platform", &mut ds.os_id_like, e)
    })?;

    let h = &mut ds.hw;
    object(root, "hw", e, |o, e| {
        object(o, "sys", e, |o, e| {
            txt(o, "vendor", &mut h.sys_vendor, e)?;
            txt(o, "uuid", &mut h.sys_uuid, e)
        })?;
        object(o, "product", e, |o, e| {
            txt(o, "id", &mut h.product_id, e)?;
            txt(o, "name", &mut h.product_name, e)?;
            txt(o, "version", &mut h.product_version, e)?;
            txt(o, "sku", &mut h.product_sku, e)?;
            txt(o, "family", &mut h.product_family, e)
        })?;
        object(o, "board", e, |o, e| {
            txt(o, "name", &mut h.board_name, e)?;
            txt(o, "version", &mut h.board_version, e)?;
            txt(o, "vendor", &mut h.board_vendor, e)
        })?;
        object(o, "chassis", e, |o, e| {
            txt(o, "type", &mut h.chassis_type, e)?;
            txt(o, "vendor", &mut h.chassis_vendor, e)?;
            txt(o, "version", &mut h.chassis_version, e)
        })?;
        object(o, "bios", e, |o, e| {
            txt(o, "date", &mut h.bios_date, e)?;
            txt(o, "release", &mut h.bios_release, e)?;
            txt(o, "version", &mut h.bios_version, e)?;
            txt(o, "vendor", &mut h.bios_vendor, e)
        })
    })?;

    let p = &mut ds.product;
    object(root, "product", e, |o, e| {
        txt(o, "vendor", &mut p.vendor, e)?;
        txt(o, "id", &mut p.id, e)?;
        txt(o, "name", &mut p.name, e)?;
        txt(o, "type", &mut p.kind, e)
    })?;

    let f = &mut ds.fatal;
    object(root, "fatal", e, |o, e| {
        txt(o, "filename", &mut f.filename, e)?;
        txt(o, "function", &mut f.function, e)?;
        txt(o, "message", &mut f.message, e)?;
        txt(o, "stack_trace", &mut f.stack_trace, e)?;
        f.line = uint64(o, "line", e)? as i64;
        txt(o, "errno", &mut f.errno, e)?;
        txt(o, "thread", &mut f.thread, e)?;
        if version >= 16 {
            enum_of(
                o,
                "signal_code",
                signal_code::from_text,
                &mut f.signal_code,
                e,
            )?;
        }
        if version >= 17 {
            // C reads it through the signal code's parser: a string that parses to non-zero is true, a boolean is
            // left as it was
            enum_of(
                o,
                "sentry",
                |t| signal_code::from_text(t) != 0,
                &mut f.sentry,
                e,
            )?;
        }
        if version >= 18 {
            f.thread_id = uint64(o, "thread_id", e)? as i32;
            // into a UINT64_HEX_MAX_LENGTH buffer
            let mut address = FixedStr::<19>::default();
            txt(o, "fault_address", &mut address, e)?;
            f.fault_address = if address.is_empty() {
                0
            } else {
                str2ull_encoded(address.as_bytes())
            };
        }
        if version >= 23 {
            f.worker_job_id = uint64(o, "worker_job_id", e)? as u32;
        }
        Some(())
    })
}
