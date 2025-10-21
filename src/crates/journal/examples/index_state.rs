#![allow(unused_imports)]

use journal::index_state::{AppState, HistogramRequest};
use journal::registry::Registry;
use journal::repository::File;

use std::collections::HashSet;

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

    let mut facets = HashSet::new();
    for e in v {
        facets.insert(String::from_utf8_lossy(e).into_owned());
    }

    facets
}

fn main() {
    println!("Hello there!");

    let indexed_fields = get_facets();
    let mut app_state = AppState::new("/var/log/journal", indexed_fields).unwrap();

    use chrono::Utc;

    let before = Utc::now();
    let after = before - chrono::Duration::weeks(52);

    let histogram_request = HistogramRequest {
        after: after.timestamp_micros() as u64,
        before: before.timestamp_micros() as u64,
    };

    let mut iteration = 0;
    loop {
        iteration += 1;
        println!("\n========== Iteration {} ==========", iteration);

        app_state.histogram(histogram_request.clone());

        println!("\nPartial responses: {}", app_state.partial_responses.len());
        for (bucket_request, partial_response) in &app_state.partial_responses {
            println!(
                "  Bucket[{}, +{}) - {} files remaining, {} indexed fields",
                bucket_request.start,
                bucket_request.end - bucket_request.start,
                partial_response.request_metadata.files.len(),
                partial_response.indexed_fields.len()
            );
        }

        println!(
            "\nComplete responses: {}",
            app_state.complete_responses.len()
        );
        for (bucket_request, complete_response) in &app_state.complete_responses {
            println!(
                "  Bucket[{}, +{}) - {} indexed fields, {} unindexed fields",
                bucket_request.start,
                bucket_request.end - bucket_request.start,
                complete_response.indexed_fields.len(),
                complete_response.unindexed_fields.len()
            );
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
