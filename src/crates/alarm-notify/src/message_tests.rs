use super::*;
use crate::config::Config;

fn paths() -> Paths {
    Paths {
        user_config_dir: "/nonexistent".into(),
        stock_config_dir: "/nonexistent".into(),
        cache_dir: "/nonexistent".into(),
        registry_dir: "/nonexistent".into(),
    }
}

fn args(status: &str, old_status: &str) -> AlertArgs {
    AlertArgs {
        roles: "sysadmin".into(),
        args_host: "node1".into(),
        unique_id: "11".into(),
        alarm_id: "22".into(),
        event_id: "33".into(),
        when: "1700000000".into(),
        name: "disk_space_usage".into(),
        chart: "disk_space./".into(),
        status: status.into(),
        old_status: old_status.into(),
        value: "91.5".into(),
        old_value: "80".into(),
        src: "health.d/disks.conf:12".into(),
        duration: "120".into(),
        non_clear_duration: "3600".into(),
        units: "%".into(),
        info: "disk is almost full".into(),
        value_string: "91.5%".into(),
        old_value_string: "80%".into(),
        calc_expression: "$used > 90".into(),
        calc_param_values: "used = 91.5".into(),
        total_warnings: "2".into(),
        total_critical: "1".into(),
        total_warn_alarms: String::new(),
        total_crit_alarms: String::new(),
        classification: "Utilization".into(),
        edit_command_line: "sudo /etc/netdata/edit-config health.d/disks.conf=12=node1".into(),
        child_machine_guid: "guid-child".into(),
        transition_id: "aaaa-bbbb".into(),
        summary: "disk_space_usage".into(),
        context: "disk.space".into(),
        component: "Disk".into(),
        alert_type: "System".into(),
    }
}

fn cfg() -> Config {
    Config::from_text("")
}

#[test]
fn warning_transition_from_clear() {
    let m = Message::build(&args("WARNING", "CLEAR"), &cfg(), &paths());
    assert_eq!(m.status_message, "needs attention");
    assert_eq!(m.status_email_subject, "Warning");
    assert_eq!(m.color, "#ffc107");
    assert_eq!(m.border_color, "#FFC300");
    assert_eq!(m.background_color, "#FFF8E1");
    assert_eq!(m.text_color, "#536775");
    assert_eq!(m.action_text_color, "#35414A");
    assert_eq!(m.severity, "WARNING");
    assert_eq!(m.rich_status_raised_for, "Raised to warning, for 1 hour");
    assert_eq!(m.alarm, "disk space usage = 91.5%");
    // CLEAR -> WARNING is the fall-through branch, which clears `raised_for`.
    assert_eq!(m.raised_for, "");
    assert_eq!(m.raised_for_html, "");
    assert_eq!(
        m.html_email_subject,
        "Warning, disk_space_usage = 91.5%, on node1"
    );
    assert!(m.image.ends_with("/images/alert-128-orange.png"));
    assert_eq!(
        m.alarm_badge,
        "https://app.netdata.cloud/static/email/img/label_warning.png"
    );
}

#[test]
fn escalation_warning_to_critical() {
    let m = Message::build(&args("CRITICAL", "WARNING"), &cfg(), &paths());
    assert_eq!(m.status_message, "is critical");
    assert_eq!(m.severity, "Escalated to CRITICAL");
    // non_clear_duration (3600) > duration (120), so the longer phrasing is used.
    assert_eq!(m.raised_for, "(alarm is raised for 1 hour)");
    assert_eq!(
        m.rich_status_raised_for,
        "Escalated to critical, (alarm is raised for 1 hour)"
    );
    assert_eq!(m.color, "#ca414b");
    assert_eq!(m.text_color, "#FF4136");
}

#[test]
fn demotion_critical_to_warning() {
    let m = Message::build(&args("WARNING", "CRITICAL"), &cfg(), &paths());
    assert_eq!(m.severity, "Demoted to WARNING");
    assert_eq!(
        m.rich_status_raised_for,
        "Demoted to warning, (alarm is raised for 1 hour)"
    );
}

#[test]
fn clear_drops_the_value_from_the_alarm_text() {
    let m = Message::build(&args("CLEAR", "CRITICAL"), &cfg(), &paths());
    assert_eq!(m.status_message, "recovered");
    assert_eq!(m.severity, "Recovered from CRITICAL");
    assert_eq!(m.raised_for, "(alarm was raised for 1 hour)");
    assert_eq!(
        m.rich_status_raised_for,
        "Recovered from critical, (alarm was raised for 1 hour)"
    );
    // No `= value` on recovery.
    assert_eq!(m.alarm, "disk space usage (alarm was raised for 1 hour)");
    assert_eq!(
        m.html_email_subject,
        "Clear, disk_space_usage (alarm was raised for 1 hour), on node1"
    );
    assert_eq!(m.color, "#77ca6d");
    assert_eq!(
        m.raised_for_html,
        "<br/><small>(alarm was raised for 1 hour)</small>"
    );
}

#[test]
fn clear_keeps_the_short_phrasing_when_non_clear_is_not_longer() {
    let mut a = args("CLEAR", "WARNING");
    a.duration = "3600".into();
    a.non_clear_duration = "120".into();
    let m = Message::build(&a, &cfg(), &paths());
    assert_eq!(m.raised_for, "(was warning for 1 hour)");
    assert_eq!(m.alarm, "disk space usage (was warning for 1 hour)");
}

#[test]
fn goto_url_carries_every_redirect_parameter() {
    let m = Message::build(&args("WARNING", "CLEAR"), &cfg(), &paths());
    assert!(m.goto_url.starts_with(
        "https://registry.my-netdata.io/registry-alert-redirect.html?agent_machine_guid="
    ));
    for expected in [
        "host_machine_guid=guid-child",
        "transition_id=aaaa-bbbb",
        "host=node1",
        "chart=disk_space.%2f",
        "alarm=disk_space_usage",
        "alarm_unique_id=11",
        "alarm_id=22",
        "alarm_event_id=33",
        "alarm_when=1700000000",
        "alarm_status=WARNING",
        // `alarm_chart` is deliberately the unencoded chart id, as before.
        "alarm_chart=disk_space./",
        "alarm_value=91.5%25",
    ] {
        assert!(
            m.goto_url.contains(expected),
            "missing {expected} in {}",
            m.goto_url
        );
    }
}

#[test]
fn info_html_is_omitted_when_there_is_no_info() {
    let mut a = args("WARNING", "CLEAR");
    a.info = String::new();
    let m = Message::build(&a, &cfg(), &paths());
    assert_eq!(m.info_html, "");

    let m2 = Message::build(&args("WARNING", "CLEAR"), &cfg(), &paths());
    assert_eq!(m2.info_html, " <small><br/>disk is almost full</small>");
}

#[test]
fn edit_command_line_is_split_into_three_fields() {
    let m = Message::build(&args("WARNING", "CLEAR"), &cfg(), &paths());
    assert_eq!(
        m.edit_command,
        "sudo /etc/netdata/edit-config health.d/disks.conf"
    );
    assert_eq!(m.line, "12");
    assert_eq!(m.s_host, "node1");
}

#[test]
fn extra_alarms_caption_appears_past_fifteen() {
    let mut a = args("WARNING", "CLEAR");
    a.total_warnings = "10".into();
    a.total_critical = "6".into();
    let m = Message::build(&a, &cfg(), &paths());
    assert_eq!(m.extra_alarms_list_text, "(Showing latest 15 alerts)");

    a.total_critical = "5".into();
    let m2 = Message::build(&a, &cfg(), &paths());
    assert_eq!(m2.extra_alarms_list_text, "");
}

#[test]
fn alarm_rows_are_rendered_for_each_listed_alert() {
    let mut a = args("CRITICAL", "WARNING");
    let now = datefmt::now_secs();
    a.total_crit_alarms = format!("cpu_usage={},ram_usage={}", now - 3600, now - 120);
    a.total_warn_alarms = format!("disk_space={}", now - 60);
    let m = Message::build(&a, &cfg(), &paths());

    assert!(m.crit_alarms_html.contains("cpu_usage"));
    assert!(m.crit_alarms_html.contains("ram_usage"));
    assert!(m.crit_alarms_html.contains("Critical for 1 hour"));
    assert!(m.crit_alarms_html.contains("Critical for 2 minutes"));
    assert!(m.warn_alarms_html.contains("Warning for 1 minute"));
    // Colour scheme differs between the two row types.
    assert!(m.crit_alarms_html.contains("#FFEBEF"));
    assert!(m.warn_alarms_html.contains("#FFF8E1"));
    // No leftover placeholders.
    assert!(!m.crit_alarms_html.contains("${"), "unexpanded placeholder");
    assert!(!m.warn_alarms_html.contains("${"), "unexpanded placeholder");
}

#[test]
fn empty_alarm_lists_render_nothing() {
    let m = Message::build(&args("WARNING", "CLEAR"), &cfg(), &paths());
    assert_eq!(m.warn_alarms_html, "");
    assert_eq!(m.crit_alarms_html, "");
}

#[test]
fn images_base_url_is_configurable() {
    let c = Config::from_text("images_base_url=\"http://my.server:19999\"");
    let m = Message::build(&args("WARNING", "CLEAR"), &c, &paths());
    assert_eq!(
        m.image,
        "http://my.server:19999/images/alert-128-orange.png"
    );
}

#[test]
fn notification_description_matches_the_log_wording() {
    let m = Message::build(&args("WARNING", "CLEAR"), &cfg(), &paths());
    assert_eq!(
        m.notification_description,
        "notification to 'sysadmin' for transition from CLEAR to WARNING, of alert \
         'disk_space_usage' = '91.5%', of instance 'disk_space./', context 'disk.space' on host 'node1'"
    );
}

#[test]
fn template_vars_cover_every_placeholder_in_the_email_templates() {
    let a = args("CRITICAL", "WARNING");
    let c = cfg();
    let m = Message::build(&a, &c, &paths());
    let vars = m.template_vars(&a, &c);

    let html = include_str!("../templates/email_html.tpl");
    let plain = include_str!("../templates/email_plaintext.tpl");
    for template in [html, plain] {
        let mut i = 0;
        let bytes = template.as_bytes();
        while i + 1 < bytes.len() {
            if bytes[i] == b'$' && bytes[i + 1] == b'{' {
                let end = template[i + 2..].find('}').expect("closed placeholder");
                let key = &template[i + 2..i + 2 + end];
                assert!(
                    vars.contains_key(key),
                    "template placeholder ${{{key}}} has no value"
                );
                i += 2 + end + 1;
            } else {
                i += 1;
            }
        }
    }
}
