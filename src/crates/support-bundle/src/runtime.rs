//! 07-runtime: live agent API captures plus binary buildinfo. Identical on
//! every platform — the API set is the bundle contract's most valuable
//! section and must not drift between OSes.

use crate::collect::Ctx;
use crate::item::{Item, announce_first};
use std::path::Path;
use std::time::Duration;

/// Pre-seed child/mirrored node hostnames so they pseudonymize consistently
/// in EVERY file (node_instances, stream configs, logs). Queries both the v2
/// endpoint and the v1 fallback (`mirrored_hosts`) so older agents seed too.
pub fn seed_child_hostnames(ctx: &mut Ctx) {
    for path in ["/api/v2/node_instances", "/api/v1/info"] {
        let Ok(resp) = crate::http::local_get(
            crate::consts::ND_PORT,
            path,
            Duration::from_secs(5),
            crate::consts::API_CAP as usize,
        ) else {
            continue;
        };
        if !(200..300).contains(&resp.status) || !resp.complete {
            continue;
        }
        let text = String::from_utf8_lossy(&resp.body);
        let mut hosts: Vec<String> = Vec::new();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            collect_json_hostnames(&v, &mut hosts);
        }
        hosts.sort();
        hosts.dedup();
        for h in hosts {
            ctx.seed_fqdn(&h);
        }
    }
}

fn collect_json_hostnames(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map {
                if k == "nm" || k == "hostname" {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_string());
                    }
                }
                if k == "mirrored_hosts" {
                    if let Some(a) = val.as_array() {
                        for h in a {
                            if let Some(s) = h.as_str() {
                                out.push(s.to_string());
                            }
                        }
                    }
                }
                collect_json_hostnames(val, out);
            }
        }
        serde_json::Value::Array(a) => {
            for val in a {
                collect_json_hostnames(val, out);
            }
        }
        _ => {}
    }
}

pub fn runtime_items(
    api_ok: bool,
    agent_running: bool,
    netdata_bin: Option<&Path>,
    netdatacli: Option<&Path>,
) -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    if api_ok {
        v.push(Item::api("07-runtime/info-v3.json", "BEST SINGLE CALL: buildinfo, features, cloud status, per-tier retention (works even under bearer protection)", "/api/v3/info"));
        v.push(Item::api(
            crate::consts::PATH_INFO_V1,
            "Agent info v1: version, cloud/stream booleans, mirrored hosts",
            "/api/v1/info",
        ));
        v.push(Item::api(
            "07-runtime/node-instances.json",
            "Node instances: children, streaming state, db_size per tier, metric counts",
            "/api/v2/node_instances",
        ));
        v.push(Item::api(
            "07-runtime/stream-info.json",
            "Streaming diagnostics",
            "/api/v3/stream_info",
        ));
        v.push(Item::api(
            crate::consts::PATH_ACLK,
            "Cloud/ACLK connection state",
            "/api/v1/aclk",
        ));
        v.push(Item::api(
            "07-runtime/alerts-active.json",
            "Currently raised alerts",
            "/api/v3/alerts?options=active",
        ));
        v.push(Item::api(
            "07-runtime/alerts-all.json",
            "All alert instances (summary)",
            "/api/v1/alarms?all",
        ));
        v.push(Item::api(
            "07-runtime/functions.json",
            "Registered functions (which plugins expose what)",
            "/api/v1/functions",
        ));
        v.push(Item::api(
            "07-runtime/ml-info.json",
            "Machine learning status",
            "/api/v1/ml_info",
        ));
        // netdata's own resource usage, bounded windows (perf triage without screenshots)
        v.push(Item::api(
            "07-runtime/self-cpu.csv",
            "Netdata CPU last 10min (csv)",
            "/api/v1/data?chart=netdata.server_cpu&after=-600&points=60&format=csv",
        ));
        v.push(Item::api(
            "07-runtime/self-memory.csv",
            "Netdata memory last 10min (csv)",
            "/api/v1/data?chart=netdata.memory&after=-600&points=60&format=csv",
        ));
        v.push(Item::api(
            "07-runtime/self-api-clients.csv",
            "Netdata API clients last 10min (csv)",
            "/api/v1/data?chart=netdata.clients&after=-600&points=60&format=csv",
        ));
        announce_first(&mut v, "collecting: runtime (agent is up)");
    } else {
        v.push(Item::generated(
            "07-runtime/AGENT-WAS-DOWN.txt",
            "Marker: agent API unreachable at collection time",
            format!(
                "Agent API at 127.0.0.1:{} was unreachable when this bundle was created (the agent may be down, or its API bound away from 127.0.0.1 / bearer-protected). See 05-logs and 06-state/status-file.json for why.\n",
                crate::consts::ND_PORT
            ),
        ));
        announce_first(&mut v, "agent API unreachable - skipping runtime section");
    }
    if let Some(bin) = netdata_bin {
        let bin_s = bin.display().to_string();
        v.push(Item::cmd(
            "07-runtime/buildinfo.txt",
            "netdata -W buildinfo (verbatim - paths section matters; works with daemon down)",
            &[&bin_s, "-W", "buildinfo"],
        ));
        v.push(Item::cmd_raw(
            "07-runtime/buildinfo.json",
            "netdata -W buildinfojson (machine-readable; no header so it parses as JSON)",
            &[&bin_s, "-W", "buildinfojson"],
        ));
    }
    if agent_running {
        if let Some(cli) = netdatacli {
            let cli_s = cli.display().to_string();
            v.push(Item::cmd_raw(
                crate::consts::PATH_ACLK_STATE,
                "Cloud connectivity state (netdatacli aclk-state json; no header so it parses as JSON)",
                &[&cli_s, "aclk-state", "json"],
            ));
        }
    }
    v
}
