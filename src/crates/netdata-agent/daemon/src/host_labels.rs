//! Localhost's host labels, ported from `reload_host_labels()` (`src/database/rrdhost-labels.c`): `[host labels]`
//! from netdata.conf with `${VAR}` expansion, the Kubernetes script's labels, then the automatic `_*` labels.
//! Decisions D49 in the status repository.

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use netdata_agent_inicfg::{Config, SECTION_GLOBAL, SECTION_HOST_LABEL};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::host::{Hosts, meta_flags};
use netdata_agent_rrd::labels::{self, MAX_VALUE_LENGTH};
use netdata_agent_text::c::fgets_chunks;

use crate::build;
use crate::cloud_proxy;
use crate::spawn::Popen;

/// `env_expand_labels_value()` into a buffer of `size` bytes: `${VAR}` and `${VAR:-default}` (an empty variable is an
/// unset one), the first `}` closing; no closing brace copies the rest; nothing is expanded twice.
fn expand(value: &[u8], size: usize, lookup: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> Vec<u8> {
    let max = size.saturating_sub(1);
    let mut out = Vec::new();
    let mut s = value;
    while !s.is_empty() && out.len() < max {
        if !s.starts_with(b"${") {
            out.push(s[0]);
            s = &s[1..];
            continue;
        }
        let Some(close) = s[2..].iter().position(|&c| c == b'}') else {
            let room = max - out.len();
            out.extend_from_slice(&s[..s.len().min(room)]);
            break;
        };
        let content = &s[2..2 + close];
        let (name, default) = match content.windows(2).position(|w| w == b":-") {
            Some(sep) => (&content[..sep], Some(&content[sep + 2..])),
            None => (content, None),
        };
        let resolved = match (lookup(name).filter(|v| !v.is_empty()), default) {
            (Some(v), _) => v,
            (None, Some(d)) => d.to_vec(),
            (None, None) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Warning,
                    "RRDLABEL: environment variable '{}' is not set and no default provided",
                    String::from_utf8_lossy(name)
                );
                Vec::new()
            }
        };
        let room = max - out.len();
        out.extend_from_slice(&resolved[..resolved.len().min(room)]);
        s = &s[2 + close + 1..];
    }
    out
}

/// `getenv()`.
fn getenv(name: &[u8]) -> Option<Vec<u8>> {
    std::env::var_os(OsStr::from_bytes(name)).map(|v| v.as_bytes().to_vec())
}

/// `rrdhost_load_kubernetes_labels()`: the script's lines (added even when it fails), and whether it ran cleanly.
fn kubernetes_labels(plugins_dir: &str) -> (Vec<Vec<u8>>, bool) {
    let script = format!("{plugins_dir}/get-kubernetes-labels.sh");
    if let Err(errno) = nix::unistd::access(script.as_str(), nix::unistd::AccessFlags::R_OK) {
        nd_log!(Source::Daemon, Priority::Err, errno = errno as i32;
            "Kubernetes pod label fetching script {script} not found.");
        return (Vec::new(), false);
    }
    let Ok(mut child) = Popen::run(&script) else {
        return (Vec::new(), false);
    };
    let mut stdout = Vec::new();
    let _ = child.stdout().read_to_end(&mut stdout);
    let lines = fgets_chunks(&stdout, 1000).map(<[u8]>::to_vec).collect();
    if child.wait().map_or(true, |rc| rc != 0) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "{script} exited abnormally. Failed to get kubernetes labels."
        );
        return (lines, false);
    }
    (lines, true)
}

/// `reload_host_labels()` at startup (the step `localhost labels`). The side effects run in C's order: netdata.conf's
/// `[host labels]` reloaded (from the compile-time path, as C does), their expansion, the Kubernetes script, the
/// proxy, then the `[global]` options the automatic labels read.
pub fn reload(netdata: &mut Config, cloud: &mut Config, plugins_dir: &str, hosts: &Hosts) {
    let filename = format!("{}/netdata.conf", build::CONFIG_DIR);
    if let Err(err) = netdata.load(Path::new(&filename), true, Some(SECTION_HOST_LABEL)) {
        nd_log!(Source::Daemon, Priority::Warning, errno = netdata_agent_inicfg::load_errno(&err);
            "RRDLABEL: Cannot reload the configuration file '{filename}', using labels in memory");
    }
    let mut configured = Vec::new();
    netdata.foreach_value_in_section(SECTION_HOST_LABEL, |name, value| {
        configured.push((name.to_vec(), value.to_vec()));
        true
    });
    let configured: Vec<(Vec<u8>, Vec<u8>)> = configured
        .into_iter()
        .map(|(name, value)| {
            let value = if value.windows(2).any(|w| w == b"${") {
                expand(&value, MAX_VALUE_LENGTH + 1, &getenv)
            } else {
                value
            };
            (name, value)
        })
        .collect();
    let (k8s, k8s_loaded) = kubernetes_labels(plugins_dir);
    let proxy = cloud_proxy::get(netdata, cloud);
    let is_ephemeral = netdata.get_boolean(SECTION_GLOBAL, "is ephemeral node", false);
    let has_unstable_connection =
        netdata.get_boolean(SECTION_GLOBAL, "has unstable connection", false);
    let is_parent = hosts.is_parent_label();
    let localhost = hosts.localhost();
    let info = localhost.info();
    let bool_text = |b: bool| if b { &b"true"[..] } else { &b"false"[..] };
    localhost.update_labels(|l| {
        use labels::{SRC_ACLK, SRC_AUTO, SRC_CONFIG, SRC_K8S};
        l.unmark_all();
        for (name, value) in &configured {
            l.add(name, value, SRC_CONFIG);
        }
        for line in &k8s {
            l.add_pair(line, SRC_AUTO | SRC_K8S);
        }
        // rrdhost_load_auto_labels()
        info.system_info.to_labels(l);
        l.add(b"_aclk_available", b"true", SRC_AUTO | SRC_ACLK);
        l.add(b"_mqtt_version", b"5", SRC_AUTO);
        l.add(b"_aclk_proxy", proxy.label().as_bytes(), SRC_AUTO);
        l.add(b"_aclk_ng_new_cloud_protocol", b"true", SRC_AUTO | SRC_ACLK);
        l.add(b"_is_ephemeral", bool_text(is_ephemeral), SRC_CONFIG);
        l.add(
            b"_has_unstable_connection",
            bool_text(has_unstable_connection),
            SRC_AUTO,
        );
        l.add(b"_is_parent", is_parent, SRC_AUTO);
        l.add(b"_hostname", info.hostname.as_bytes(), SRC_AUTO);
        l.add(b"_os", info.os.as_bytes(), SRC_AUTO);
        if let Some(send) = &info.stream_send {
            l.add(b"_streams_to", send.destination.as_bytes(), SRC_AUTO);
        }
        l.add(b"_timezone", info.timezone.as_bytes(), SRC_AUTO);
        l.add(
            b"_abbrev_timezone",
            info.abbrev_timezone.as_bytes(),
            SRC_AUTO,
        );
        // a failed Kubernetes run keeps the labels an earlier one loaded
        if !k8s_loaded {
            l.mark_source_as_old(SRC_K8S);
        }
        l.remove_all_unmarked();
    });
    localhost.set_meta_flags(meta_flags::LABELS | meta_flags::UPDATE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &[u8]) -> Option<Vec<u8>> {
        let value: &[u8] = match name {
            b"ND_TEST_VAR" => b"hello",
            b"ND_TEST_DC" => b"us-east",
            b"ND_TEST_RACK" => b"rack42",
            b"ND_TEST_EMPTY" => b"",
            b"ND_TEST_NESTED" => b"${ND_TEST_VAR}",
            _ => return None,
        };
        Some(value.to_vec())
    }

    /// C's `rrdhost_labels_unittest()` expansion cases.
    #[test]
    fn expansion_as_c() {
        let cases: [(&str, &str); 30] = [
            ("plain value", "plain value"),
            ("", ""),
            ("${ND_TEST_VAR}", "hello"),
            ("prefix-${ND_TEST_VAR}", "prefix-hello"),
            ("${ND_TEST_VAR}-suffix", "hello-suffix"),
            ("pre-${ND_TEST_VAR}-post", "pre-hello-post"),
            ("${ND_TEST_DC}-${ND_TEST_RACK}", "us-east-rack42"),
            (
                "${ND_TEST_DC}/${ND_TEST_RACK}/${ND_TEST_VAR}",
                "us-east/rack42/hello",
            ),
            (
                "dc=${ND_TEST_DC} rack=${ND_TEST_RACK}",
                "dc=us-east rack=rack42",
            ),
            ("${ND_TEST_VAR:-fallback}", "hello"),
            ("${ND_TEST_DC:-other}", "us-east"),
            ("${ND_TEST_UNSET:-fallback}", "fallback"),
            ("pre-${ND_TEST_UNSET:-fallback}-post", "pre-fallback-post"),
            ("${ND_TEST_EMPTY:-fallback}", "fallback"),
            ("${ND_TEST_UNSET}", ""),
            ("pre-${ND_TEST_UNSET}-post", "pre--post"),
            ("${ND_TEST_UNSET:-}", ""),
            ("pre-${ND_TEST_UNSET:-}-post", "pre--post"),
            ("${ND_TEST_UNCLOSED", "${ND_TEST_UNCLOSED"),
            ("pre-${ND_TEST_UNCLOSED", "pre-${ND_TEST_UNCLOSED"),
            ("$notavar", "$notavar"),
            ("price is $5", "price is $5"),
            ("$$", "$$"),
            ("$", "$"),
            ("${}", ""),
            ("${:-fallback}", "fallback"),
            ("${ND_TEST_UNSET:-a:-b}", "a:-b"),
            ("${ND_TEST_NESTED}", "${ND_TEST_VAR}"),
            ("${ND_TEST_UNSET:-${ND_TEST_VAR}}", "${ND_TEST_VAR}"),
            ("x${ND_TEST_DC}", "x"),
        ];
        for (i, (value, want)) in cases.iter().enumerate() {
            // the last case: a buffer of 2 bytes holds one
            let size = if i == cases.len() - 1 {
                2
            } else {
                MAX_VALUE_LENGTH + 1
            };
            let (got, _) = netdata_agent_log::capture(|| expand(value.as_bytes(), size, &env));
            assert_eq!(String::from_utf8(got).unwrap(), *want, "{value:?}");
        }
        assert_eq!(expand(b"${ND_TEST_DC}", 8, &env), b"us-east");
        assert_eq!(expand(b"${ND_TEST_DC}", 5, &env), b"us-e");
        let (_, records) = netdata_agent_log::capture(|| expand(b"${ND_TEST_UNSET}", 801, &env));
        assert_eq!(
            records
                .into_iter()
                .filter_map(|r| r.message)
                .collect::<Vec<_>>(),
            ["RRDLABEL: environment variable 'ND_TEST_UNSET' is not set and no default provided"]
        );
    }
}
