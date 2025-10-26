#![allow(unused_imports)]

use journal::index::FilterExpr;
use journal::index_state::{AppState, HistogramRequest};
use journal::registry::Registry;
use journal::repository::File;
use tracing::Instrument;

use std::collections::HashSet;
use std::sync::Arc;

pub fn get_facets() -> HashSet<String> {
    let v: Vec<&[u8]> = vec![
        b"_HOSTNAME",
        b"PRIORITY",
        b"SYSLOG_FACILITY",
        b"ERRNO",
        b"SYSLOG_IDENTIFIER",
        b"UNIT",
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
        b"_SYSTEMD_UNIT",
        b"_NAMESPACE",
        b"_TRANSPORT",
        b"_RUNTIME_SCOPE",
        b"_STREAM_ID",
        b"ND_NIDL_CONTEXT",
        b"ND_ALERT_STATUS",
        b"_SYSTEMD_CGROUP",
        b"ND_NIDL_NODE",
        b"ND_ALERT_COMPONENT",
        b"_COMM",
        b"_SYSTEMD_USER_UNIT",
        b"_SYSTEMD_USER_SLICE",
        b"_SYSTEMD_SESSION",
        b"__logs_sources",
    ];

    // let v: Vec<&[u8]> = vec![b"log.severity_number"];

    let mut facets = HashSet::default();
    for e in v {
        facets.insert(String::from_utf8_lossy(e).into_owned());
    }

    facets
}

fn main() {
    println!("Hello there!");

    let indexed_fields = get_facets();
    // let mut app_state = AppState::new("/var/log/journal", indexed_fields).unwrap();

    let mut app_state =
        AppState::new("/home/vk/repos/tmp/agent-events-journal", indexed_fields).unwrap();

    use chrono::{DateTime, Utc};

    let before = Utc::now();
    let after = before - chrono::Duration::weeks(52);

    let filter_expr = Arc::new(FilterExpr::match_str("PRIORITY=4"));
    let histogram_request = HistogramRequest {
        after: after.timestamp() as u64,
        before: before.timestamp() as u64,
        filter_expr,
    };

    if false {
        const USEC_PER_SEC: u64 = std::time::Duration::from_secs(1).as_micros() as u64;
        let after = u64::from_str_radix("641202f93e665", 16).unwrap() / USEC_PER_SEC;
        let before = u64::from_str_radix("6414242cf1eec", 16).unwrap() / USEC_PER_SEC;

        // Convert to DateTime for verification
        let after_dt = DateTime::from_timestamp_secs(after as i64).unwrap();
        let before_dt = DateTime::from_timestamp_secs(before as i64).unwrap();
        println!("After: {}", after_dt);
        println!("Before: {}", before_dt);

        let filter_expr = Arc::new(FilterExpr::match_str("PRIORITY=4"));

        let filter_expr = Arc::new(FilterExpr::match_str("PRIORITY=4"));
        let histogram_request = HistogramRequest {
            after,
            before,
            filter_expr,
        };
    }

    let mut iteration = 0;
    let loop_start = std::time::Instant::now();
    loop {
        let histogram_result = app_state.get_histogram(histogram_request.clone());

        iteration += 1;
        println!(
            "[Iteration {}] Elapsed: {}, Partial: {}, Complete: {}, Total: {}",
            iteration,
            loop_start.elapsed().as_secs(),
            app_state.partial_responses.len(),
            app_state.complete_responses.len(),
            app_state.partial_responses.len() + app_state.complete_responses.len()
        );

        std::thread::sleep(std::time::Duration::from_secs(1));

        if iteration == 10 {
            // println!("Histogram result: {:#?}", histogram_result);
            //
            let available_histograms = histogram_result.available_histograms();
            println!("{:#?}", available_histograms);
            app_state.print_indexing_stats();
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}
