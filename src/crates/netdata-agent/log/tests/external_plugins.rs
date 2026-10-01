//! A process the daemon starts (its spawn server) logs as an external plugin does
//! (`nd_log_initialize_for_external_plugins()`): every record as COLLECTORS, and a warning with C's fallback when the
//! exported method is not one an external plugin may use.

use netdata_agent_log::{Priority, Source, capture, initialize_for_external_plugins, nd_log};

#[test]
fn an_external_plugin_logs_every_record_as_collectors() {
    let env = |key: &str| match key {
        // a file is the daemon's method, not an external plugin's
        "NETDATA_LOG_METHOD" => Some("file".to_string()),
        "NETDATA_LOG_LEVEL" => Some("debug".to_string()),
        _ => None,
    };
    let ((), records) = capture(|| {
        initialize_for_external_plugins("spawn-plugins", &env);
        nd_log!(Source::Daemon, Priority::Info, "from the daemon's source");
    });
    let got: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
    // stderr may be a journal stream where the test runs under systemd
    let fallback = if got.first().is_some_and(|r| r.2.ends_with("journal.")) { "journal" } else { "stderr" };
    assert_eq!(
        got,
        [
            (Source::Collector, Priority::Warning, format!("NETDATA_LOG_METHOD is not set. Using {fallback}.")),
            (Source::Collector, Priority::Info, "from the daemon's source".to_string()),
        ]
    );
}
