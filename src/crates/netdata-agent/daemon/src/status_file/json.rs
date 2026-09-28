//! `daemon_status_file_to_json()` (`status-file.c:297-587`): the record as C writes it, the members gated on the
//! record's own version, `"version"` always 29.

use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::print::HEX_DIGITS;

use super::{
    StatusFile, VERSION, bitmap_names, cloud_status, db_mode, exit_reason, profile, signal_code,
};

fn bitmap(w: &mut JsonWriter, key: &str, names: &[(u32, &'static str)], bits: u32) {
    w.member_add_array(Some(key.as_bytes()));
    for name in bitmap_names(names, bits) {
        w.add_array_item_string(name);
    }
    w.array_close();
}

fn agent(w: &mut JsonWriter, ds: &StatusFile) {
    w.member_add_object("agent");
    w.member_add_uuid("id", &ds.host_id.uuid);
    if ds.v >= 24 && ds.host_id.last_modified_ut != 0 {
        w.member_add_string("since", ds.host_id.last_modified_rfc3339.as_bytes());
    }
    w.member_add_uuid_compact("ephemeral_id", &ds.invocation);
    w.member_add_string("version", ds.version.as_bytes());
    w.member_add_time_t("uptime", ds.uptime);
    w.member_add_uuid("node_id", &ds.node_id);
    w.member_add_uuid("claim_id", &ds.claim_id);
    w.member_add_uint64("restarts", ds.restarts);
    if ds.v >= 24 {
        w.member_add_uint64("crashes", ds.crashes);
    }
    if ds.v >= 27 {
        w.member_add_uint64("pid", ds.pid as u64);
    }
    if ds.v >= 22 {
        w.member_add_uint64("posts", ds.posts);
        w.member_add_string("aclk", cloud_status::name(ds.cloud_status));
    }
    bitmap(w, "profile", &profile::NAMES, ds.profile);
    w.member_add_string("status", ds.status.name());
    bitmap(w, "exit_reason", &exit_reason::NAMES, ds.exit_reason);
    w.member_add_string("install_type", ds.install_type.as_bytes());
    if ds.v >= 14 {
        w.member_add_string("db_mode", db_mode::name(ds.db_mode));
        w.member_add_uint64("db_tiers", u64::from(ds.db_tiers));
        w.member_add_boolean("kubernetes", ds.kubernetes);
    }
    if ds.v >= 16 {
        w.member_add_boolean("sentry_available", ds.sentry_available);
    }
    if ds.v >= 18 {
        w.member_add_int64("reliability", ds.reliability);
        w.member_add_string("stack_traces", ds.stack_traces.as_bytes());
    }
    w.member_add_object("timings");
    w.member_add_time_t("init", ds.timings.init);
    w.member_add_time_t("exit", ds.timings.exit);
    w.object_close();
    w.object_close();
}

fn metrics(w: &mut JsonWriter, ds: &StatusFile) {
    let m = &ds.metrics;
    w.member_add_object("metrics");
    w.member_add_object("nodes");
    w.member_add_uint64("total", m.nodes_total);
    w.member_add_uint64("receiving", m.nodes_receiving);
    w.member_add_uint64("sending", m.nodes_sending);
    w.member_add_uint64("archived", m.nodes_archived);
    w.object_close();
    for (key, counts) in [
        ("metrics", m.metrics),
        ("instances", m.instances),
        ("contexts", m.contexts),
    ] {
        w.member_add_object(key);
        w.member_add_uint64("collected", counts.collected);
        w.member_add_uint64("available", counts.available);
        w.object_close();
    }
    w.object_close();
}

fn host(w: &mut JsonWriter, ds: &StatusFile) {
    w.member_add_object("host");
    w.member_add_uuid_compact("id", &ds.machine_id);
    w.member_add_string("architecture", ds.architecture.as_bytes());
    w.member_add_string("virtualization", ds.virtualization.as_bytes());
    w.member_add_string("container", ds.container.as_bytes());
    w.member_add_time_t("uptime", ds.boottime);
    if ds.v >= 20 {
        w.member_add_string("timezone", ds.timezone.as_bytes());
        w.member_add_string("cloud_provider", ds.cloud_provider_type.as_bytes());
        w.member_add_string("cloud_instance", ds.cloud_instance_type.as_bytes());
        w.member_add_string("cloud_region", ds.cloud_instance_region.as_bytes());
    }
    w.member_add_uint64("system_cpus", ds.system_cpus);
    w.member_add_object("boot");
    w.member_add_uuid_compact("id", &ds.boot_id);
    w.object_close();
    w.member_add_object("memory");
    if ds.memory.total > 0 {
        w.member_add_uint64("total", ds.memory.total);
        w.member_add_uint64("free", ds.memory.available);
        if ds.v >= 21 {
            w.member_add_uint64("netdata", ds.netdata_max_rss);
            w.member_add_uint64("oom_protection", ds.oom_protection);
        }
    }
    w.object_close();
    w.member_add_object("disk");
    w.member_add_object("db");
    let db = &ds.var_cache;
    if db.total_bytes > 0 {
        w.member_add_uint64("total", db.total_bytes);
        w.member_add_uint64("free", db.free_bytes);
        w.member_add_uint64("inodes_total", db.total_inodes);
        w.member_add_uint64("inodes_free", db.free_inodes);
        w.member_add_boolean("read_only", db.read_only);
    }
    w.object_close();
    let f = &ds.disk_footprint;
    w.member_add_object("netdata");
    w.member_add_uint64("dbengine", f.dbengine);
    w.member_add_uint64("sqlite", f.sqlite);
    w.member_add_uint64("other", f.other);
    w.member_add_string("last_updated", f.last_updated_rfc3339.as_bytes());
    w.object_close();
    w.object_close();
    w.object_close();
}

fn os(w: &mut JsonWriter, ds: &StatusFile) {
    w.member_add_object("os");
    w.member_add_string("type", ds.os_type.name());
    w.member_add_string("kernel", ds.kernel_version.as_bytes());
    w.member_add_string("name", ds.os_name.as_bytes());
    w.member_add_string("version", ds.os_version.as_bytes());
    w.member_add_string("family", ds.os_id.as_bytes());
    w.member_add_string("platform", ds.os_id_like.as_bytes());
    w.object_close();
}

fn hw(w: &mut JsonWriter, ds: &StatusFile) {
    let h = &ds.hw;
    let object = |w: &mut JsonWriter, key: &str, members: &[(&str, &[u8])]| {
        w.member_add_object(key);
        for (k, v) in members {
            w.member_add_string(k, v);
        }
        w.object_close();
    };
    w.member_add_object("hw");
    object(
        w,
        "sys",
        &[
            ("vendor", h.sys_vendor.as_bytes()),
            ("uuid", h.sys_uuid.as_bytes()),
        ],
    );
    object(
        w,
        "product",
        &[
            ("id", h.product_id.as_bytes()),
            ("name", h.product_name.as_bytes()),
            ("version", h.product_version.as_bytes()),
            ("sku", h.product_sku.as_bytes()),
            ("family", h.product_family.as_bytes()),
        ],
    );
    object(
        w,
        "board",
        &[
            ("name", h.board_name.as_bytes()),
            ("version", h.board_version.as_bytes()),
            ("vendor", h.board_vendor.as_bytes()),
        ],
    );
    object(
        w,
        "chassis",
        &[
            ("type", h.chassis_type.as_bytes()),
            ("vendor", h.chassis_vendor.as_bytes()),
            ("version", h.chassis_version.as_bytes()),
        ],
    );
    object(
        w,
        "bios",
        &[
            ("date", h.bios_date.as_bytes()),
            ("release", h.bios_release.as_bytes()),
            ("version", h.bios_version.as_bytes()),
            ("vendor", h.bios_vendor.as_bytes()),
        ],
    );
    w.object_close();
    w.member_add_object("product");
    w.member_add_string("vendor", ds.product.vendor.as_bytes());
    w.member_add_string("id", ds.product.id.as_bytes());
    w.member_add_string("name", ds.product.name.as_bytes());
    w.member_add_string("type", ds.product.kind.as_bytes());
    w.object_close();
}

fn fatal(w: &mut JsonWriter, ds: &StatusFile) {
    let f = &ds.fatal;
    w.member_add_object("fatal");
    w.member_add_uint64("line", f.line as u64);
    w.member_add_string("filename", f.filename.as_bytes());
    w.member_add_string("function", f.function.as_bytes());
    w.member_add_string("message", f.message.as_bytes());
    w.member_add_string("errno", f.errno.as_bytes());
    w.member_add_string("thread", f.thread.as_bytes());
    w.member_add_uint64("thread_id", f.thread_id as u64);
    w.member_add_string("stack_trace", f.stack_trace.as_bytes());
    if ds.v >= 16 {
        w.member_add_string("signal_code", signal_code::text(f.signal_code, &mut [0; 24]));
    }
    if ds.v >= 17 {
        w.member_add_boolean("sentry", f.sentry);
    }
    if ds.v >= 18 {
        let mut address = [0u8; 18];
        let text = if f.signal_code != 0 { hex(f.fault_address, &mut address) } else { &address[..0] };
        w.member_add_string("fault_address", text);
    }
    if ds.v >= 23 {
        w.member_add_uint64("worker_job_id", u64::from(f.worker_job_id));
    }
    w.object_close();
}

/// `print_uint64_hex()` on the stack: `0x` and uppercase digits, no padding.
fn hex(mut value: u64, out: &mut [u8; 18]) -> &[u8] {
    let mut at = out.len();
    loop {
        at -= 1;
        out[at] = HEX_DIGITS[(value & 0xf) as usize];
        value >>= 4;
        if value == 0 {
            break;
        }
    }
    out[at - 2..at].copy_from_slice(b"0x");
    &out[at - 2..]
}

/// The file's text: `buffer_json_initialize()`, the members, `buffer_json_finalize()`.
pub fn to_json(ds: &StatusFile) -> Vec<u8> {
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    members(&mut w, ds);
    w.into_bytes()
}

/// The file's text in `w`, reset first: nothing is allocated while it fits the writer's capacity (the signal
/// handler's save, D91.3).
pub fn to_json_in(w: &mut JsonWriter, ds: &StatusFile) {
    w.reset(JsonOptions::DEFAULT);
    members(w, ds);
}

fn members(w: &mut JsonWriter, ds: &StatusFile) {
    w.member_add_string("@timestamp", ds.timestamp_rfc3339.as_bytes());
    w.member_add_uint64("version", u64::from(VERSION));
    agent(w, ds);
    metrics(w, ds);
    host(w, ds);
    os(w, ds);
    hw(w, ds);
    fatal(w, ds);
    w.finalize();
}
