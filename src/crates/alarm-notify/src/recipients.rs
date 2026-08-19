//! Turning roles into recipient lists, and the `|critical` severity filter.

use std::collections::BTreeMap;

use crate::args::Status;
use crate::config::{Config, METHOD_NAMES};
use crate::paths::Paths;
use crate::textutil::split_list;

/// Resolved recipients per method, in the order the configuration listed them.
///
/// The shell used an associative array, so its output order was bash's hash order;
/// first-seen order is deterministic and otherwise equivalent.
pub struct Recipients {
    per_method: BTreeMap<String, Vec<String>>,
    /// True when at least one method resolved a non-empty list. The script used this
    /// to decide between a silent exit and a fatal error.
    pub have_to_send_something: bool,
}

impl Recipients {
    pub fn get(&self, method: &str) -> &[String] {
        self.per_method
            .get(method)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Space-separated list, the form every sender consumed.
    pub fn joined(&self, method: &str) -> String {
        self.get(method).join(" ")
    }
}

/// Modifiers a recipient may carry after a `|`.
#[derive(Default, Debug, PartialEq, Eq)]
struct Modifiers {
    critical: bool,
    noclear: bool,
    nowarn: bool,
}

/// Resolve every method's recipients, disabling methods that end up with none.
pub fn resolve(
    cfg: &mut Config,
    roles: &str,
    status: Status,
    alarm_id: &str,
    paths: &Paths,
) -> Recipients {
    let mut per_method = BTreeMap::new();
    let mut have_to_send_something = false;

    for method in METHOD_NAMES {
        if !cfg.enabled(method) {
            continue;
        }

        let mut chosen: Vec<String> = Vec::new();
        for role in split_list(roles) {
            // These two role names mean "notify nobody for this role".
            if role == "silent" || role == "disabled" {
                continue;
            }

            let configured = cfg
                .role_recipients(method, role)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| cfg.default_recipient(method).to_string());

            for entry in split_list(&configured) {
                if entry == "disabled" {
                    continue;
                }
                if !allowed_by_criticality(method, entry, status, alarm_id, paths) {
                    continue;
                }
                let bare = bare_recipient(entry).to_string();
                if !chosen.contains(&bare) {
                    chosen.push(bare);
                }
            }
        }

        if chosen.is_empty() {
            cfg.disable(method);
        } else {
            have_to_send_something = true;
        }
        per_method.insert((*method).to_string(), chosen);
    }

    Recipients {
        per_method,
        have_to_send_something,
    }
}

/// The recipient without its `|modifier` suffix.
fn bare_recipient(entry: &str) -> &str {
    entry.split('|').next().unwrap_or(entry)
}

fn parse_modifiers(entry: &str) -> Option<Modifiers> {
    let (_, rest) = entry.split_once('|')?;
    let mut m = Modifiers::default();
    for token in rest.split('|') {
        match token.to_ascii_lowercase().as_str() {
            "critical" => m.critical = true,
            "noclear" => m.noclear = true,
            "nowarn" => m.nowarn = true,
            "" => {}
            other => {
                tracing::error!("SEVERITY FILTERING for {entry}: invalid modifier '{other}'.");
                // An unrecognised modifier fails open, exactly as before: better a
                // surplus notification than a silently dropped alert.
                return None;
            }
        }
    }
    Some(m)
}

/// Decide whether a recipient should receive this transition.
///
/// State lives in one file per (method, recipient, alarm) under the cache directory;
/// its presence records "this recipient has already been told about this alarm".
fn allowed_by_criticality(
    method: &str,
    entry: &str,
    status: Status,
    alarm_id: &str,
    paths: &Paths,
) -> bool {
    let Some(m) = parse_modifiers(entry) else {
        // No modifiers, or an invalid one: no filtering.
        return true;
    };

    let recipient = bare_recipient(entry);
    let dir = paths.criticality_tracking_dir(method, recipient);
    let tracking_file = dir.join(alarm_id);

    if m.critical && !dir.is_dir() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::error!(dir = %dir.display(), "cannot create severity tracking directory: {e}");
        }
    }

    match status {
        Status::Critical => {
            if m.critical {
                // Create if absent, never truncate: the script used `touch`, and a
                // recipient name that collided with an existing file must not empty it.
                if let Err(e) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&tracking_file)
                {
                    tracing::error!(
                        file = %tracking_file.display(),
                        "cannot create severity tracking file: {e}"
                    );
                }
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: ALLOW: the alarm is CRITICAL (will now receive next status change)"
                );
            } else {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: ALLOW: the alarm is CRITICAL"
                );
            }
            true
        }

        Status::Warning => {
            if m.nowarn {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: BLOCK: recipient should not receive this notification (nowarn modifier set)"
                );
                return false;
            }
            if !m.critical {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: ALLOW: the alarm is WARNING"
                );
                return true;
            }
            if tracking_file.is_file() {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: ALLOW: recipient has been notified for this alarm in the past (will still receive next status change)"
                );
                return true;
            }
            block(entry, method)
        }

        Status::Clear => {
            // The file is consumed here whether or not the notification goes out,
            // which is what the shell did: CLEAR closes the tracking window.
            let existed = tracking_file.is_file();
            if existed {
                if let Err(e) = std::fs::remove_file(&tracking_file) {
                    tracing::error!(
                        file = %tracking_file.display(),
                        "cannot remove severity tracking file: {e}"
                    );
                }
            }
            if m.noclear {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: BLOCK: recipient should not receive this notification (noclear modifier set)"
                );
                return false;
            }
            if !m.critical {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: ALLOW: the alarm is CLEAR"
                );
                return true;
            }
            if existed {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: ALLOW: recipient has been notified for this alarm in the past (no status change will be sent from now)"
                );
                return true;
            }
            block(entry, method)
        }

        Status::Other => {
            if m.critical && tracking_file.is_file() {
                tracing::debug!(
                    "SEVERITY FILTERING for {entry} VIA {method}: ALLOW: recipient has been notified for this alarm in the past (will still receive next status change)"
                );
                return true;
            }
            block(entry, method)
        }
    }
}

fn block(entry: &str, method: &str) -> bool {
    tracing::debug!(
        "SEVERITY FILTERING for {entry} VIA {method}: BLOCK: recipient should not receive this notification"
    );
    false
}

#[cfg(test)]
#[path = "recipients_tests.rs"]
mod tests;
