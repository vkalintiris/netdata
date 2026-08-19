use super::*;
use std::path::{Path, PathBuf};

fn config(text: &str) -> Config {
    Config::from_text(text)
}

fn paths_in(dir: &Path) -> Paths {
    Paths {
        user_config_dir: dir.to_path_buf(),
        stock_config_dir: dir.to_path_buf(),
        cache_dir: dir.to_path_buf(),
        registry_dir: dir.to_path_buf(),
    }
}

#[test]
fn role_recipients_override_the_default() {
    let mut cfg = config(
        r##"
SLACK_WEBHOOK_URL="https://hooks.example/x"
DEFAULT_RECIPIENT_SLACK="#default"
role_recipients_slack[dba]="#dba"
        "##,
    );
    let tmp = tempfile::tempdir().unwrap();
    let r = resolve(&mut cfg, "dba", Status::Warning, "1", &paths_in(tmp.path()));
    assert_eq!(r.get("slack"), ["#dba"]);

    let mut cfg2 = config(
        r##"
SLACK_WEBHOOK_URL="https://hooks.example/x"
DEFAULT_RECIPIENT_SLACK="#default"
        "##,
    );
    let r2 = resolve(
        &mut cfg2,
        "sysadmin",
        Status::Warning,
        "1",
        &paths_in(tmp.path()),
    );
    assert_eq!(r2.get("slack"), ["#default"]);
}

#[test]
fn multiple_roles_are_merged_and_deduplicated() {
    let mut cfg = config(
        r##"
SLACK_WEBHOOK_URL="https://hooks.example/x"
role_recipients_slack[a]="#one #two"
role_recipients_slack[b]="#two,#three"
        "##,
    );
    let tmp = tempfile::tempdir().unwrap();
    let r = resolve(&mut cfg, "a,b", Status::Warning, "1", &paths_in(tmp.path()));
    assert_eq!(r.get("slack"), ["#one", "#two", "#three"]);
}

#[test]
fn silent_and_disabled_suppress_delivery() {
    let mut cfg = config(
        r##"
SLACK_WEBHOOK_URL="https://hooks.example/x"
DEFAULT_RECIPIENT_SLACK="#default"
        "##,
    );
    let tmp = tempfile::tempdir().unwrap();
    let r = resolve(
        &mut cfg,
        "silent",
        Status::Warning,
        "1",
        &paths_in(tmp.path()),
    );
    assert!(r.get("slack").is_empty());
    assert!(!r.have_to_send_something);
    // The method is switched off once nothing is addressable.
    assert!(!cfg.enabled("slack"));

    let mut cfg2 = config(
        r##"
SLACK_WEBHOOK_URL="https://hooks.example/x"
role_recipients_slack[x]="disabled"
        "##,
    );
    let r2 = resolve(&mut cfg2, "x", Status::Warning, "1", &paths_in(tmp.path()));
    assert!(r2.get("slack").is_empty());
}

#[test]
fn modifiers_are_stripped_from_the_outgoing_list() {
    let mut cfg = config(
        r##"
SLACK_WEBHOOK_URL="https://hooks.example/x"
role_recipients_slack[x]="#ops|critical"
        "##,
    );
    let tmp = tempfile::tempdir().unwrap();
    let r = resolve(&mut cfg, "x", Status::Critical, "7", &paths_in(tmp.path()));
    assert_eq!(r.get("slack"), ["#ops"]);
}

#[test]
fn critical_modifier_gates_warnings_until_a_critical_was_seen() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    let entry = "#ops|critical";

    // WARNING first: blocked, because this recipient has not seen a CRITICAL yet.
    assert!(!allowed_by_criticality(
        "slack",
        entry,
        Status::Warning,
        "42",
        &paths
    ));

    // A CRITICAL is always delivered and opens the window.
    assert!(allowed_by_criticality(
        "slack",
        entry,
        Status::Critical,
        "42",
        &paths
    ));
    assert!(
        paths
            .criticality_tracking_dir("slack", "#ops")
            .join("42")
            .is_file()
    );

    // Now WARNING passes.
    assert!(allowed_by_criticality(
        "slack",
        entry,
        Status::Warning,
        "42",
        &paths
    ));

    // CLEAR passes once and closes the window.
    assert!(allowed_by_criticality(
        "slack",
        entry,
        Status::Clear,
        "42",
        &paths
    ));
    assert!(
        !paths
            .criticality_tracking_dir("slack", "#ops")
            .join("42")
            .is_file()
    );
    assert!(!allowed_by_criticality(
        "slack",
        entry,
        Status::Clear,
        "42",
        &paths
    ));
}

#[test]
fn nowarn_and_noclear_block_their_transitions() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    assert!(!allowed_by_criticality(
        "email",
        "a@b|nowarn",
        Status::Warning,
        "1",
        &paths
    ));
    assert!(allowed_by_criticality(
        "email",
        "a@b|nowarn",
        Status::Critical,
        "1",
        &paths
    ));
    assert!(allowed_by_criticality(
        "email",
        "a@b|nowarn",
        Status::Clear,
        "1",
        &paths
    ));

    assert!(!allowed_by_criticality(
        "email",
        "a@b|noclear",
        Status::Clear,
        "1",
        &paths
    ));
    assert!(allowed_by_criticality(
        "email",
        "a@b|noclear",
        Status::Warning,
        "1",
        &paths
    ));
}

#[test]
fn an_invalid_modifier_fails_open() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    assert!(allowed_by_criticality(
        "email",
        "a@b|bogus",
        Status::Warning,
        "1",
        &paths
    ));
}

#[test]
fn no_modifier_means_no_filtering() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    for status in [
        Status::Clear,
        Status::Warning,
        Status::Critical,
        Status::Other,
    ] {
        assert!(allowed_by_criticality("email", "a@b", status, "1", &paths));
    }
}

#[test]
fn parse_modifiers_reads_every_combination() {
    assert_eq!(parse_modifiers("x"), None);
    assert_eq!(
        parse_modifiers("x|critical"),
        Some(Modifiers {
            critical: true,
            ..Default::default()
        })
    );
    assert_eq!(
        parse_modifiers("x|CRITICAL|noclear"),
        Some(Modifiers {
            critical: true,
            noclear: true,
            nowarn: false
        })
    );
    assert_eq!(parse_modifiers("x|nope"), None);
}

#[test]
fn tracking_paths_are_namespaced_per_method_and_recipient() {
    let paths = paths_in(&PathBuf::from("/cache"));
    assert_eq!(
        paths.criticality_tracking_dir("email", "root"),
        PathBuf::from("/cache/alarm-notify/email/root")
    );
}
