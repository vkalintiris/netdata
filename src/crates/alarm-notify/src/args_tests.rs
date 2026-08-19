use super::*;

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn recognises_test_mode_in_both_orders() {
    for argv in [
        v(&["test"]),
        v(&["test", "sysadmin"]),
        v(&["sysadmin", "test"]),
    ] {
        match parse(&argv) {
            Invocation::Test { role } => assert_eq!(role, "sysadmin"),
            _ => panic!("expected test mode for {argv:?}"),
        }
    }
    match parse(&v(&["webmaster", "test"])) {
        Invocation::Test { role } => assert_eq!(role, "webmaster"),
        _ => panic!("expected test mode"),
    }
}

#[test]
fn three_or_more_args_is_never_test_mode() {
    // A real alert whose role happens to be "test" must dispatch normally.
    let argv = v(&["test", "host", "1", "2", "3"]);
    assert!(matches!(parse(&argv), Invocation::Notify(_)));
}

#[test]
fn recognises_unittest_and_dump_methods() {
    match parse(&v(&["unittest", "sysadmin", "/tmp/c", "WARNING", "CLEAR"])) {
        Invocation::UnitTest {
            role,
            config_file,
            status,
            old_status,
        } => {
            assert_eq!(
                (role.as_str(), config_file.as_str()),
                ("sysadmin", "/tmp/c")
            );
            assert_eq!((status.as_str(), old_status.as_str()), ("WARNING", "CLEAR"));
        }
        _ => panic!("expected unittest"),
    }
    assert!(matches!(
        parse(&v(&["dump_methods"])),
        Invocation::DumpMethods
    ));
}

#[test]
fn maps_all_33_positions() {
    let argv: Vec<String> = (0..33).map(|i| format!("a{i}")).collect();
    let Invocation::Notify(a) = parse(&argv) else {
        panic!("expected notify")
    };
    assert_eq!(a.roles, "a0");
    assert_eq!(a.args_host, "a1");
    assert_eq!(a.status, "a8");
    assert_eq!(a.value_string, "a17");
    assert_eq!(a.summary, "a29");
    assert_eq!(a.context, "a30");
    assert_eq!(a.alert_type, "a32");
}

#[test]
fn missing_trailing_args_are_empty() {
    // The script's own `test` mode passes only 30 arguments.
    let argv: Vec<String> = (0..30).map(|i| format!("a{i}")).collect();
    let Invocation::Notify(a) = parse(&argv) else {
        panic!("expected notify")
    };
    assert_eq!(a.summary, "a29");
    assert_eq!(a.context, "");
    assert_eq!(a.component, "");
    assert_eq!(a.alert_type, "");
}

#[test]
fn numeric_views_tolerate_garbage() {
    let a = AlertArgs {
        duration: "90".into(),
        non_clear_duration: String::new(),
        when: "1700000000".into(),
        value: "nan".into(),
        transition_id: "aa-bb-cc".into(),
        ..Default::default()
    };
    assert_eq!(a.duration_secs(), 90);
    assert_eq!(a.non_clear_duration_secs(), 0);
    assert_eq!(a.when_secs(), 1_700_000_000);
    assert_eq!(a.transition_id_compact(), "aabbcc");
    // `value` stays verbatim so `nan` reaches the payload unchanged.
    assert_eq!(a.value, "nan");
}

#[test]
fn status_parsing() {
    assert_eq!(Status::parse("CLEAR"), Status::Clear);
    assert_eq!(Status::parse("REMOVED"), Status::Other);
    assert!(Status::Warning.is_notifiable());
    assert!(!Status::Other.is_notifiable());
}
