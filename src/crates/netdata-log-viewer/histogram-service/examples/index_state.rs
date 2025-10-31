use histogram_service::{AppState, HistogramRequest};
use journal::index::FilterExpr;

use std::collections::HashSet;

pub fn get_facets() -> HashSet<String> {
    let v: Vec<&[u8]> = vec![
        b"_HOSTNAME",
        b"PRIORITY",
        b"SYSLOG_FACILITY",
        b"ERRNO",
        b"SYSLOG_IDENTIFIER",
        // b"UNIT",
        b"USER_UNIT",
        b"MESSAGE_ID",
        b"_BOOT_ID",
        b"_SYSTEMD_OWNER_UID",
        b"_UID",
        b"OBJECT_SYSTEMD_OWNER_UID",
        b"OBJECT_UID",
        b"_GID",
        b"OBJECT_GID",
        b"_CAP_EFFECTIVE",
        b"_AUDIT_LOGINUID",
        b"OBJECT_AUDIT_LOGINUID",
        b"CODE_FUNC",
        b"ND_LOG_SOURCE",
        b"CODE_FILE",
        b"ND_ALERT_NAME",
        b"ND_ALERT_CLASS",
        b"_SELINUX_CONTEXT",
        b"_MACHINE_ID",
        b"ND_ALERT_TYPE",
        b"_SYSTEMD_SLICE",
        b"_EXE",
        // b"_SYSTEMD_UNIT",
        b"_NAMESPACE",
        b"_TRANSPORT",
        b"_RUNTIME_SCOPE",
        b"_STREAM_ID",
        b"ND_NIDL_CONTEXT",
        b"ND_ALERT_STATUS",
        // b"_SYSTEMD_CGROUP",
        b"ND_NIDL_NODE",
        b"ND_ALERT_COMPONENT",
        b"_COMM",
        b"_SYSTEMD_USER_UNIT",
        b"_SYSTEMD_USER_SLICE",
        // b"_SYSTEMD_SESSION",
        b"__logs_sources",
    ];

    // let v: Vec<&[u8]> = vec![b"log.severity_number"];

    let mut facets = HashSet::default();
    for e in v {
        facets.insert(String::from_utf8_lossy(e).into_owned());
    }

    facets
}

#[tokio::main]
async fn main() {
    println!("Hello there!");

    // let path = "/home/vk/repos/tmp/aws";
    // let path = "/var/log/journal";
    let path = "/home/vk/repos/tmp/agent-events-journal";
    let mut app_state = AppState::new(path, tokio::runtime::Handle::current())
        .await
        .unwrap();

    use chrono::Utc;

    let before = Utc::now();
    let after = before - chrono::Duration::weeks(8);

    let filter_expr = FilterExpr::None;
    let histogram_request = HistogramRequest::new(
        after.timestamp() as u64,
        before.timestamp() as u64,
        &[],
        &filter_expr,
    );

    use tokio::time::{Duration, interval};
    let loop_start = std::time::Instant::now();

    let mut iteration = 0;
    let mut interval = interval(Duration::from_secs(1));
    loop {
        interval.tick().await;

        let instant = std::time::Instant::now();
        let histogram_result = app_state.get_histogram(histogram_request.clone()).await;
        iteration += 1;
        println!(
            "[Iteration {}] Elapsed: {}/{}, Partial: {}, Complete: {}, Total: {}",
            iteration,
            instant.elapsed().as_millis(),
            loop_start.elapsed().as_secs(),
            app_state.partial_responses.len(),
            app_state.complete_responses.len(),
            app_state.partial_responses.len() + app_state.complete_responses.len()
        );

        if iteration > 15 {
            use std::fs::File;
            use std::io::Write;

            let mut file = File::create("/home/vk/output.json").unwrap();
            let ui_response = histogram_result.ui_response("log.severity_number");
            let s = serde_json::to_string_pretty(&ui_response).unwrap();
            file.write_all(s.as_bytes()).unwrap();

            println!("ui response length: {} MiB", s.len() / (1024 * 1024));
            return;
        }
    }

    // app_state.close().await.expect("Failed to close cache");
}
