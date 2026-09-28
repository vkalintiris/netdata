//! `post_status_file()` (`status-file.c:1159-1264`): the last run's record reported to agent-events, through libcurl as
//! C links it (D96.2): the body, the POST and what C tells about it. The gate and the steps around it are `session`'s.

use std::time::Duration;

use netdata_agent_log::Priority;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use super::{StatusFile, json, session};
use crate::build;

/// Where C posts the reports.
pub const AGENT_EVENTS_URL: &str = "https://agent-events.netdata.cloud/agent-events";

/// `ci_env_vars[]` (`os/ci.c`): set, even empty, it means a CI run.
const CI_VARS: [&str; 26] = [
    "CI",
    "CONTINUOUS_INTEGRATION",
    "BUILD_NUMBER",
    "RUN_ID",
    "TRAVIS",
    "GITHUB_ACTIONS",
    "GITHUB_TOKEN",
    "GITLAB_CI",
    "CIRCLECI",
    "APPVEYOR",
    "BITBUCKET_BUILD_NUMBER",
    "SYSTEM_TEAMFOUNDATIONCOLLECTIONURI",
    "TF_BUILD",
    "BAMBOO_BUILDKEY",
    "GO_PIPELINE_NAME",
    "HUDSON_URL",
    "TEAMCITY_VERSION",
    "CI_NAME",
    "CI_WORKER",
    "CI_SERVER",
    "HEROKU_TEST_RUN_ID",
    "BUILDKITE",
    "DRONE",
    "SEMAPHORE",
    "NETLIFY",
    "NOW_BUILDER",
];

/// `nd_is_running_under_ci()`.
pub fn running_under_ci() -> bool {
    CI_VARS.iter().any(|name| std::env::var_os(name).is_some())
}

/// `agent_health()`: how the record's run ended against its streak.
fn agent_health(ds: &StatusFile) -> &'static str {
    if session::has_crashed(ds) {
        match ds.restarts {
            1 => "crash-first",
            _ if ds.reliability <= -2 => "crash-loop",
            _ if ds.reliability < 0 => "crash-repeated",
            _ => "crash-entered",
        }
    } else {
        match ds.restarts {
            1 => "healthy-first",
            _ if ds.reliability >= 2 => "healthy-loop",
            _ if ds.reliability > 0 => "healthy-repeated",
            _ => "healthy-recovered",
        }
    }
}

/// `os_system_memory_available_percent()` rounded by `parser_round_number_to_uint64()`: 100 without a total, the
/// largest integer for what does not round to one.
fn free_percent(ds: &StatusFile) -> u64 {
    let percent = if ds.memory.total == 0 {
        100.0
    } else {
        100.0 * ds.memory.available as f64 / ds.memory.total as f64
    };
    let rounded = percent.round();
    if (0.0..18_446_744_073_709_551_616.0).contains(&rounded) {
        rounded as u64
    } else {
        u64::MAX
    }
}

/// The report's JSON, minified: why the run ended and how the agent is now, then the record as the file keeps it.
pub fn body(ds: &StatusFile, cause: &str, msg: &str, priority: Priority) -> Vec<u8> {
    let mut w = JsonWriter::new(JsonOptions::MINIFY);
    w.member_add_string("exit_cause", cause);
    w.member_add_string("message", msg);
    w.member_add_uint64("priority", priority as u64);
    w.member_add_uint64("version_saved", u64::from(ds.v));
    w.member_add_string("agent_version_now", build::NETDATA_VERSION);
    w.member_add_uint64("agent_pid_now", u64::from(std::process::id()));
    w.member_add_boolean(
        "host_memory_critical",
        ds.memory.total > 0 && ds.memory.available <= ds.oom_protection,
    );
    w.member_add_uint64("host_memory_free_percent", free_percent(ds));
    w.member_add_string("agent_health", agent_health(ds));
    json::record_members(&mut w, ds);
    w.finalize();
    w.into_bytes()
}

/// The POST of `body` to `url` with C's options and none other: libcurl's environment proxies and CA defaults, no
/// signals, 10 s, the answer discarded. Any answer counts (C reads no status code); only a transfer failure does not.
pub fn post(url: &str, body: &[u8]) -> bool {
    let mut easy = curl::easy::Easy::new();
    let mut headers = curl::easy::List::new();
    let set = headers.append("Content-Type: application/json").is_ok()
        && easy.url(url).is_ok()
        && easy.post(true).is_ok()
        && easy.post_fields_copy(body).is_ok()
        && easy.signal(false).is_ok()
        && easy.http_headers(headers).is_ok()
        && easy.timeout(Duration::from_secs(10)).is_ok()
        && easy.write_function(|data| Ok(data.len())).is_ok();
    set && easy.perform().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_as_c() {
        let ds = |restarts, reliability, crashed: bool| StatusFile {
            restarts,
            reliability,
            status: if crashed { super::super::DaemonStatus::Running } else { super::super::DaemonStatus::Exited },
            ..Default::default()
        };
        let got: Vec<_> = [
            ds(1, 0, true),
            ds(3, -2, true),
            ds(3, -1, true),
            ds(3, 0, true),
            ds(1, 5, false),
            ds(3, 2, false),
            ds(3, 1, false),
            ds(3, 0, false),
        ]
        .iter()
        .map(agent_health)
        .collect();
        assert_eq!(
            got,
            [
                "crash-first",
                "crash-loop",
                "crash-repeated",
                "crash-entered",
                "healthy-first",
                "healthy-loop",
                "healthy-repeated",
                "healthy-recovered"
            ]
        );
    }

    #[test]
    fn the_body_leads_with_the_report() {
        let mut ds = StatusFile { v: 29, ..Default::default() };
        ds.memory.total = 3;
        ds.memory.available = 2;
        ds.oom_protection = 2;
        let body = String::from_utf8(body(&ds, "killed hard", "Netdata was last killed", Priority::Warning)).unwrap();
        let head = format!(
            r#"{{"exit_cause":"killed hard","message":"Netdata was last killed","priority":4,"version_saved":29,"agent_version_now":"{}","agent_pid_now":{},"host_memory_critical":true,"host_memory_free_percent":67,"agent_health":"healthy-recovered","@timestamp":"#,
            build::NETDATA_VERSION,
            std::process::id()
        );
        assert!(body.starts_with(&head), "{body}");
        assert!(body.ends_with('}') && !body.contains('\n'), "{body}");
    }

    /// libcurl is the system's, as C links it: OpenSSL underneath.
    #[test]
    fn libcurl_is_the_systems() {
        let v = curl::Version::get();
        assert!(v.ssl_version().is_some_and(|s| s.starts_with("OpenSSL/")), "{:?}", v.ssl_version());
    }

    /// Any HTTP answer is a success, a refused connection is not; the body goes out as given.
    #[test]
    fn a_post_counts_any_answer() {
        use std::io::{Read, Write};
        // a proxy in the environment would take the request elsewhere
        if ["http_proxy", "all_proxy", "ALL_PROXY"].iter().any(|v| std::env::var_os(v).is_some()) {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.windows(7).any(|w| w == b"{\"a\":1}") {
                let n = s.read(&mut buf).unwrap();
                assert!(n > 0);
                got.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"HTTP/1.1 500 No\r\nContent-Length: 2\r\n\r\nno").unwrap();
            got
        });
        assert!(post(&format!("http://{addr}/agent-events"), b"{\"a\":1}"));
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        assert!(request.starts_with("POST /agent-events HTTP/1.1\r\n"), "{request}");
        assert!(request.contains("\r\nContent-Type: application/json\r\n"), "{request}");
        // the listener is gone with its thread
        assert!(!post(&format!("http://{addr}/agent-events"), b"{}"));
    }
}
