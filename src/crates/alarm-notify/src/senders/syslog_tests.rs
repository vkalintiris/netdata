use super::*;

fn parse(raw: &str) -> Target {
    Target::parse(raw, "local6", "info")
}

#[test]
fn a_bare_value_is_only_a_prefix() {
    let t = parse("netdata");
    assert_eq!(
        t,
        Target {
            facility: "local6".into(),
            level: "info".into(),
            server: None,
            port: 514,
            prefix: "netdata".into(),
        }
    );
}

#[test]
fn facility_and_level_override() {
    let t = parse("daemon.err/netdata");
    assert_eq!(t.facility, "daemon");
    assert_eq!(t.level, "err");
    assert_eq!(t.prefix, "netdata");
    assert!(t.server.is_none());

    // A bare word is a facility.
    let t = parse("local1/nd");
    assert_eq!(t.facility, "local1");
    assert_eq!(t.level, "info");
}

#[test]
fn remote_targets_with_and_without_a_port() {
    let t = parse("local5.warning@logs.example.com:1514/nd");
    assert_eq!(t.facility, "local5");
    assert_eq!(t.level, "warning");
    assert_eq!(t.server.as_deref(), Some("logs.example.com"));
    assert_eq!(t.port, 1514);
    assert_eq!(t.prefix, "nd");

    let t = parse("@logs.example.com/nd");
    assert_eq!(t.server.as_deref(), Some("logs.example.com"));
    assert_eq!(t.port, 514);
    assert_eq!(t.facility, "local6");
}

#[test]
fn ipv6_targets() {
    let t = parse("@[2001:db8::1]:5514/nd");
    assert_eq!(t.server.as_deref(), Some("2001:db8::1"));
    assert_eq!(t.port, 5514);

    let t = parse("@[2001:db8::1]/nd");
    assert_eq!(t.server.as_deref(), Some("2001:db8::1"));
    assert_eq!(t.port, 514);

    // Unbracketed IPv6 must not be mistaken for host:port.
    let t = parse("@2001:db8::1/nd");
    assert_eq!(t.server.as_deref(), Some("2001:db8::1"));
    assert_eq!(t.port, 514);
}

#[test]
fn priority_values_follow_rfc3164() {
    // local6.info = 22 * 8 + 6
    assert_eq!(parse("netdata").priority_value(), 182);
    assert_eq!(parse("local0.emerg/x").priority_value(), 128);
    assert_eq!(parse("kern.debug/x").priority_value(), 7);
    assert_eq!(parse("daemon.crit/x").priority_value(), 3 * 8 + 2);
}

#[test]
fn unknown_names_fall_back_to_documented_defaults() {
    assert_eq!(facility_code("nonsense"), 22);
    assert_eq!(severity_code("nonsense"), 6);
}

#[test]
fn the_local_record_carries_no_timestamp_or_host() {
    // What `logger` puts on /dev/log: journald supplies the rest, and a timestamp
    // here would become part of the message and cost the record its identifier.
    assert_eq!(
        format_local(182, "netdata", "hello world"),
        "<182>netdata: hello world"
    );
}

#[test]
fn the_remote_record_is_rfc5424() {
    let record = format_rfc5424(182, "node1", "netdata", "hello world");
    assert!(record.starts_with("<182>1 "), "{record}");
    assert!(
        record.ends_with(" node1 netdata - - hello world"),
        "{record}"
    );
    // VERSION, TIMESTAMP, HOSTNAME, APP-NAME, PROCID, MSGID, then the message.
    let fields: Vec<&str> = record.splitn(7, ' ').collect();
    assert_eq!(fields.len(), 7);
    assert_eq!(fields[0], "<182>1");
    assert!(fields[1].contains('T'), "timestamp: {}", fields[1]);
    assert_eq!(fields[4], "-");
    assert_eq!(fields[5], "-");
    assert_eq!(fields[6], "hello world");
}

#[test]
fn a_missing_hostname_becomes_the_nil_value() {
    assert!(format_rfc5424(182, "", "netdata", "m").contains(" - netdata - - m"));
}

#[test]
fn tags_are_sanitised() {
    assert_eq!(sanitize_tag("netdata"), "netdata");
    assert_eq!(sanitize_tag("net data!"), "netdata");
    assert_eq!(sanitize_tag("!!!"), "netdata");
    assert_eq!(sanitize_tag("nd-1.2_x"), "nd-1.2_x");
}

#[test]
fn host_port_splitting() {
    assert_eq!(split_host_port("h"), ("h".to_string(), None));
    assert_eq!(split_host_port("h:1"), ("h".to_string(), Some(1)));
    assert_eq!(split_host_port("h:bad"), ("h".to_string(), None));
}
