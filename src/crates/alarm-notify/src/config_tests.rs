use super::*;

#[test]
fn defaults_are_present_before_any_file_is_read() {
    let c = Config::from_text("");
    assert_eq!(c.str("images_base_url"), "https://registry.my-netdata.io");
    assert_eq!(c.str("use_fqdn"), "NO");
    assert_eq!(c.str("EMAIL_CHARSET"), "UTF-8");
    assert_eq!(c.str("IRC_PORT"), "6667");
    assert_eq!(c.str("SMSEAGLE_MSG_TYPE"), "sms");
    assert_eq!(c.str("OPSGENIE_API_URL"), "https://api.opsgenie.com");
    assert_eq!(c.str("TELEGRAM_API_URL"), "https://api.telegram.org");
}

#[test]
fn methods_without_credentials_are_disabled() {
    let c = Config::from_text("");
    // Every webhook/token method starts enabled but is screened out with no secret.
    for method in [
        "slack",
        "rocketchat",
        "alerta",
        "flock",
        "discord",
        "pushover",
        "pushbullet",
        "twilio",
        "hipchat",
        "messagebird",
        "smseagle",
        "kavenegar",
        "telegram",
        "kafka",
        "irc",
        "fleep",
        "dynatrace",
        "opsgenie",
        "matrix",
        "gotify",
        "ntfy",
        "msteams",
        "pd",
        "prowl",
        "custom",
        "ilert",
        "signl4",
    ] {
        assert!(
            !c.enabled(method),
            "{method} should be disabled without config"
        );
    }
    // These need no credential of their own.
    assert!(c.enabled("email"));
    assert!(c.enabled("syslog"));
    assert!(c.enabled("sms"));
    assert!(c.enabled("awssns"));
}

#[test]
fn a_configured_credential_enables_its_method() {
    let c = Config::from_text("SLACK_WEBHOOK_URL=\"https://hooks.example/x\"");
    assert!(c.enabled("slack"));
    assert!(!c.enabled("discord"));
}

#[test]
fn every_required_key_must_be_present() {
    // Twilio needs all three.
    let partial = Config::from_text("TWILIO_ACCOUNT_SID=sid\nTWILIO_ACCOUNT_TOKEN=token\n");
    assert!(!partial.enabled("twilio"));
    let complete = Config::from_text(
        "TWILIO_ACCOUNT_SID=sid\nTWILIO_ACCOUNT_TOKEN=token\nTWILIO_NUMBER=+100\n",
    );
    assert!(complete.enabled("twilio"));
}

#[test]
fn send_no_overrides_a_present_credential() {
    let c = Config::from_text("SLACK_WEBHOOK_URL=\"https://hooks.example/x\"\nSEND_SLACK=\"NO\"");
    assert!(!c.enabled("slack"));
}

#[test]
fn dump_methods_lists_enabled_variables_alphabetically() {
    let c = Config::from_text(
        "SLACK_WEBHOOK_URL=\"https://hooks.example/x\"\nSEND_EMAIL=\"NO\"\nSEND_SYSLOG=\"NO\"\nSEND_SMS=\"NO\"\nSEND_AWSSNS=\"NO\"",
    );
    let names = c.enabled_send_variables();
    assert!(names.contains(&"SEND_SLACK".to_string()));
    assert!(!names.contains(&"SEND_EMAIL".to_string()));
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "dump_methods output must be sorted");
}

#[test]
fn dynatrace_is_off_by_default_and_kafka_is_on() {
    // The shell declared these two outside its enable-everything loop.
    let c = Config::from_text("");
    assert!(!c.enabled("dynatrace"));
    // kafka defaults to YES but is screened out for want of a URL.
    let with_kafka = Config::from_text("KAFKA_URL=http://k\nKAFKA_SENDER_IP=1.2.3.4");
    assert!(with_kafka.enabled("kafka"));
}

#[test]
fn any_enabled_reflects_the_send_map() {
    let all_off = Config::from_text("SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO");
    assert!(!all_off.any_enabled());
    assert!(Config::from_text("").any_enabled());
}

#[test]
fn msteams_legacy_keys_are_migrated() {
    let c = Config::from_text(
        "MSTEAM_WEBHOOK_URL=\"https://teams.example/hook\"\nDEFAULT_RECIPIENT_MSTEAM=\"alerts\"\nMSTEAM_ICON_WARNING=\"@\"\n",
    );
    assert_eq!(c.str("MSTEAMS_WEBHOOK_URL"), "https://teams.example/hook");
    assert_eq!(c.str("DEFAULT_RECIPIENT_MSTEAMS"), "alerts");
    assert_eq!(c.str("MSTEAMS_ICON_WARNING"), "@");
    assert!(c.enabled("msteams"));
    // The legacy enable flag is consumed, not left behind.
    assert_eq!(c.get("SEND_MSTEAM"), None);
}

#[test]
fn msteams_legacy_role_recipients_are_migrated() {
    let c = Config::from_text(
        "MSTEAMS_WEBHOOK_URL=\"https://teams.example/hook\"\nrole_recipients_msteam[dba]=\"dba-channel\"\n",
    );
    assert_eq!(c.role_recipients("msteams", "dba"), Some("dba-channel"));
    assert!(c.data.array("role_recipients_msteam").is_none());
}

#[test]
fn legacy_send_flag_wins_when_set() {
    let c = Config::from_text(
        "MSTEAMS_WEBHOOK_URL=\"https://x\"\nSEND_MSTEAM=\"NO\"\nSEND_MSTEAMS=\"YES\"\n",
    );
    assert!(!c.enabled("msteams"));
}

#[test]
fn email_auto_resolves_against_a_real_mta() {
    let c = Config::from_text("SEND_EMAIL=\"AUTO\"");
    // AUTO now probes sendmail, which is what e-mail actually needs.
    assert_eq!(c.enabled("email"), crate::exec::which("sendmail").is_some());
}

#[test]
fn default_recipient_lookup_is_case_correct() {
    let c = Config::from_text("DEFAULT_RECIPIENT_EMAIL=\"root\"");
    assert_eq!(c.default_recipient("email"), "root");
    assert_eq!(c.default_recipient("slack"), "");
}

#[test]
fn dedup_preserves_first_seen_order() {
    let out = dedup_preserving_order(["b", "a", "b", "c", "a"].iter().map(|s| s.to_string()));
    assert_eq!(out, vec!["b", "a", "c"]);
}

#[test]
fn method_lists_match_the_shell_script() {
    // 27 methods with per-role recipients, 4 host-level ones.
    assert_eq!(METHOD_NAMES.len(), 27);
    assert_eq!(HOST_LEVEL_METHODS.len(), 4);
    for m in HOST_LEVEL_METHODS {
        assert!(!METHOD_NAMES.contains(m), "{m} must not be in both lists");
    }
}
