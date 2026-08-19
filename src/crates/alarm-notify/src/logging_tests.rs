use super::*;

#[test]
fn priorities_match_the_syslog_scale() {
    assert_eq!(priority_of(&Level::ERROR), 3);
    assert_eq!(priority_of(&Level::WARN), 4);
    assert_eq!(priority_of(&Level::INFO), 6);
    assert_eq!(priority_of(&Level::DEBUG), 7);
}

#[test]
fn debug_flag_wins_over_the_environment() {
    assert_eq!(level_from_env(true), Level::DEBUG);
}

#[test]
fn record_carries_the_documented_field_set() {
    let ctx = LogContext {
        invocation_id: "inv".into(),
        program_name: "alarm-notify".into(),
        node: "node1".into(),
        alert_name: "disk_space_usage".into(),
        transition_id: "aabb".into(),
        status: "WARNING".into(),
        ..Default::default()
    };
    let fields = ctx.fields(6, "sent slack notification");
    let by_name: std::collections::HashMap<_, _> = fields.iter().cloned().collect();

    assert_eq!(by_name["MESSAGE_ID"], "6db0018e83e34320ae2a659d78019fb7");
    assert_eq!(by_name["PRIORITY"], "6");
    assert_eq!(by_name["THREAD_TAG"], "alarm-notify");
    assert_eq!(by_name["ND_LOG_SOURCE"], "health");
    assert_eq!(by_name["SYSLOG_IDENTIFIER"], "alarm-notify");
    assert_eq!(by_name["ND_NIDL_NODE"], "node1");
    assert_eq!(by_name["ND_ALERT_NAME"], "disk_space_usage");
    assert_eq!(by_name["ND_ALERT_TRANSITION_ID"], "aabb");
    assert_eq!(by_name["ND_ALERT_STATUS"], "WARNING");
    assert_eq!(
        by_name["MESSAGE"],
        "[ALERT NOTIFICATION]: sent slack notification"
    );

    // Every field the shell emitted must still be present.
    for required in [
        "INVOCATION_ID",
        "SYSLOG_IDENTIFIER",
        "PRIORITY",
        "THREAD_TAG",
        "ND_LOG_SOURCE",
        "ND_NIDL_NODE",
        "ND_NIDL_INSTANCE",
        "ND_NIDL_CONTEXT",
        "ND_ALERT_NAME",
        "ND_ALERT_ID",
        "ND_ALERT_UNIQUE_ID",
        "ND_ALERT_EVENT_ID",
        "ND_ALERT_TRANSITION_ID",
        "ND_ALERT_CLASS",
        "ND_ALERT_COMPONENT",
        "ND_ALERT_TYPE",
        "ND_ALERT_RECIPIENT",
        "ND_ALERT_VALUE",
        "ND_ALERT_VALUE_OLD",
        "ND_ALERT_STATUS",
        "ND_ALERT_STATUS_OLD",
        "ND_ALERT_UNITS",
        "ND_ALERT_SUMMARY",
        "ND_ALERT_INFO",
        "ND_ALERT_DURATION",
        "ND_REQUEST",
        "MESSAGE_ID",
        "MESSAGE",
    ] {
        assert!(by_name.contains_key(required), "missing {required}");
    }
}

#[test]
fn newlines_are_escaped_in_the_message_field() {
    let ctx = LogContext::default();
    let fields = ctx.fields(3, "line one\nline two");
    let message = fields.iter().find(|(k, _)| *k == "MESSAGE").unwrap();
    assert_eq!(message.1, "[ALERT NOTIFICATION]: line one\\nline two");
}

#[test]
fn context_is_taken_from_the_alert_arguments() {
    let argv: Vec<String> = (0..33).map(|i| format!("a{i}")).collect();
    let ctx = LogContext::from_args(&argv, "alarm-notify");
    assert_eq!(ctx.recipient, "a0");
    assert_eq!(ctx.node, "a1");
    assert_eq!(ctx.alert_name, "a6");
    assert_eq!(ctx.status, "a8");
    assert!(ctx.request.starts_with("'alarm-notify' 'a0' 'a1'"));
}
