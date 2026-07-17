//! summary.txt and README.md generation — the human entry points into the
//! bundle. Ported verbatim from the scripts; keep the read-order lists in
//! sync with SUPPORT-BUNDLE.md.

use crate::collect::Ctx;
use crate::consts::TOOL_VERSION;
use crate::util;

pub struct SummaryInputs {
    pub agent_pid: Option<u32>,
    pub agent_note: String,
    pub api_ok: bool,
    pub is_container: bool,
    pub confdir: Option<String>,
    pub ran_privileged: bool,
    pub docker_logs_needed: bool,
}

fn json_field_scan(path: &std::path::Path, keys: &[&str]) -> Option<String> {
    // the awk -F'"' '/key/{print $4}' equivalent: first line containing a key,
    // fourth double-quote-separated field
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if keys.iter().any(|k| line.contains(k)) {
            if let Some(v) = line.split('"').nth(3) {
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

pub fn write_summary(ctx: &mut Ctx, inp: &SummaryInputs) {
    let work = ctx.work().to_path_buf();
    let agent_version = json_field_scan(&work.join(crate::consts::PATH_INFO_V1), &["\"version\""])
        .unwrap_or_default();

    let mut claimed = "unknown";
    for f in [crate::consts::PATH_ACLK_STATE, crate::consts::PATH_ACLK] {
        if let Ok(text) = std::fs::read_to_string(work.join(f)) {
            if text.contains("\"agent-claimed\":true") {
                claimed = "yes";
                break;
            }
            if text.contains("\"agent-claimed\":false") {
                claimed = "no";
                break;
            }
        }
    }
    if claimed == "unknown" {
        if let Ok(text) = std::fs::read_to_string(work.join(crate::consts::PATH_CLOUD_STATE)) {
            // a bare guid line means a claimed_id was present
            let guid_line = text.lines().any(|l| {
                l.len() == 36
                    && l.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                    && l.matches('-').count() == 4
            });
            if guid_line {
                claimed = "yes";
            }
        }
    }

    // deliberately coarse (any line containing "error", like the sh script):
    // a triage signal, not an accurate error tally
    let err_count = std::fs::read_to_string(work.join(crate::consts::PATH_ERROR_LOG))
        .map(|t| {
            t.lines()
                .filter(|l| l.to_ascii_lowercase().contains("error"))
                .count()
        })
        .ok();
    // prefer structured extraction (.agent.exit_reason may be an array); fall
    // back to the field scan the sh script used
    let status_path = work.join(crate::consts::PATH_STATUS_FILE);
    let crash_hint = std::fs::read_to_string(&status_path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            let er = &v["agent"]["exit_reason"];
            match er {
                serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
                serde_json::Value::Array(a) => {
                    let joined = a
                        .iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(",");
                    (!joined.is_empty()).then_some(joined)
                }
                _ => v["exit_reason"].as_str().map(|s| s.to_string()),
            }
        })
        .or_else(|| json_field_scan(&status_path, &["\"exit_reason\"", "\"cause\""]));

    let status_says_running = std::fs::read_to_string(work.join(crate::consts::PATH_STATUS_FILE))
        .map(|t| t.contains("\"status\":\"running\""))
        .unwrap_or(false);

    let mut s = String::new();
    s.push_str("NETDATA SUPPORT BUNDLE SUMMARY\n");
    s.push_str(&format!("generated:        {}\n", util::utc_now_iso()));
    s.push_str(&format!("tool version:     {TOOL_VERSION}\n"));
    s.push_str(&format!("runtime seconds:  {}\n", ctx.runtime_seconds()));
    let priv_label = if cfg!(windows) {
        "ran elevated:  "
    } else {
        "ran as root:  "
    };
    s.push_str(&format!(
        "{priv_label}    {}\n",
        if inp.ran_privileged { "yes" } else { "no" }
    ));
    s.push_str(&format!(
        "pii obfuscation:  {}\n",
        if ctx.obfuscate() { "on" } else { "OFF" }
    ));
    s.push('\n');
    s.push_str(&format!(
        "agent version:    {}\n",
        if agent_version.is_empty() {
            "unknown"
        } else {
            &agent_version
        }
    ));
    match inp.agent_pid {
        Some(pid) => {
            s.push_str(&format!(
                "agent running:    yes (pid {pid}){}\n",
                inp.agent_note
            ));
        }
        None => {
            s.push_str("agent running:    NO\n");
            if status_says_running {
                s.push_str(
                    "WARNING: status file still says 'running' but no netdata process exists -\n\
                     \x20        unclean termination (SIGKILL / OOM kill / power loss); the agent\n\
                     \x20        could not update the file at death. Check 01-system/kernel-messages.txt.\n",
                );
            }
        }
    }
    s.push_str(&format!(
        "agent api:        {}\n",
        if inp.api_ok {
            "reachable"
        } else {
            "UNREACHABLE"
        }
    ));
    s.push_str(&format!(
        "container:        {}\n",
        if inp.is_container { "yes" } else { "no" }
    ));
    s.push_str(&format!(
        "config dir:       {}\n",
        inp.confdir.as_deref().unwrap_or("NOT FOUND")
    ));
    s.push_str(&format!("claimed to cloud: {claimed}\n"));
    if let Some(h) = crash_hint {
        s.push_str(&format!(
            "last exit reason: {h}   <-- check 06-state/status-file.json\n"
        ));
    }
    if let Some(n) = err_count {
        s.push_str(&format!("error.log 'error' lines: {n}\n"));
    }
    let (skip_details, skip_total) = ctx.skipped();
    if skip_total > 0 {
        s.push_str(&format!(
            "captures skipped (no marker written): {skip_total}\n"
        ));
        for d in skip_details {
            s.push_str(&format!("  - {d}\n"));
        }
        if skip_total > skip_details.len() {
            s.push_str(&format!(
                "  - ... and {} more\n",
                skip_total - skip_details.len()
            ));
        }
    }
    if inp.docker_logs_needed {
        s.push_str(
            "NOTE: agent log HISTORY is not in this bundle - it lives in 'docker logs' on the host (see 05-logs/LOGS-ARE-IN-DOCKER.txt)\n",
        );
    }
    s.push('\n');
    s.push_str("READ ORDER FOR TRIAGE:\n");
    s.push_str("  crashes/won't start -> 06-state/status-file.json, 05-logs/, 01-system/kernel-messages.txt\n");
    s.push_str("  collector issues    -> 04-config/go.d*, 05-logs/collector.log\n");
    s.push_str("  streaming issues    -> 04-config/stream.conf, 07-runtime/node-instances.json, 01-system/clock-timesync.txt\n");
    s.push_str("  cloud/claiming      -> 06-state/cloud-state.txt, 07-runtime/aclk-state.json, 08-network/\n");
    s.push_str("  performance         -> 03-process/threads-cpu.txt, 06-state/db-disk-usage.txt, 07-runtime/node-instances.json\n");

    ctx.write_generated("summary.txt", "Human summary", &s);
}

pub fn write_readme(ctx: &mut Ctx) {
    let readme = r#"# Netdata Support Bundle

Generated by `netdata-support-bundle`. Contents are SANITIZED:
secrets (tokens, api keys, passwords) are always redacted; by default IPs,
MACs, emails and hostnames are replaced with stable pseudonyms (`ip-1`,
`redacted-host`, `[EMAIL]`, `[MAC]`) - consistent across all files, so
cross-referencing still works. The pseudonym map stays on the user's machine,
next to the bundle - it is NOT in this bundle.

## Layout (triage order)

| dir | contents |
|---|---|
| `summary.txt` | one-page overview - start here |
| `MANIFEST.json` | machine-readable index of every file (origin, size, sanitization) |
| `01-system/` | OS, kernel, memory, disks, virtualization, clock sync, OOM/segfault evidence |
| `02-install/` | install method, packages, .environment, container context |
| `03-process/` | netdata process tree, per-thread CPU, limits, fds, environment |
| `04-config/` | effective (running) config + every user-customized config file |
| `05-logs/` | journal + agent log files (window-capped), updater log, coredump metadata |
| `06-state/` | daemon status file (crash record), state/db disk usage, claim state |
| `07-runtime/` | live API captures: info, node instances, alerts, aclk state, buildinfo |
| `08-network/` | listening sockets, DNS, proxy, Netdata Cloud reachability |

## Conventions

- Command captures (`*.txt`) begin with a `# netdata-support-bundle | command: ...`
  provenance header and end with `# exit: N`.
- Copied files (configs, logs, json) are pristine (no injected headers);
  their origin is recorded in `MANIFEST.json`.
- `07-runtime/AGENT-WAS-DOWN.txt` exists only when the agent was not running.
"#;
    ctx.write_generated("README.md", "Bundle documentation", readme);
}
