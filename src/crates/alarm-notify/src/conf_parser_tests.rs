use super::*;
use std::path::Path;

fn parse(text: &str) -> ConfigData {
    parse_str(text, Path::new("test.conf"), &HashMap::new())
}

#[test]
fn plain_assignments_and_quoting_styles() {
    let d = parse(
        r##"
# a comment
SEND_EMAIL="YES"
EMAIL_SENDER='netdata@example.com'
BARE=value
EMPTY=
QUOTED_SPACES="a b c"
        "##,
    );
    assert_eq!(d.str("SEND_EMAIL"), "YES");
    assert_eq!(d.str("EMAIL_SENDER"), "netdata@example.com");
    assert_eq!(d.str("BARE"), "value");
    assert_eq!(d.str("EMPTY"), "");
    assert_eq!(d.str("QUOTED_SPACES"), "a b c");
    assert!(d.unsupported.is_empty(), "{:?}", d.unsupported);
}

#[test]
fn trailing_comments_are_stripped_but_hashes_in_values_survive() {
    let d = parse(
        r##"
SEND_SLACK="YES"   # enable slack
DEFAULT_RECIPIENT_SLACK=#alerts
CHANNEL="#ops"
        "##,
    );
    assert_eq!(d.str("SEND_SLACK"), "YES");
    assert_eq!(d.str("DEFAULT_RECIPIENT_SLACK"), "#alerts");
    assert_eq!(d.str("CHANNEL"), "#ops");
}

#[test]
fn variable_references_expand_from_earlier_lines() {
    let d = parse(
        r##"
DEFAULT_RECIPIENT_EMAIL="root"
OTHER="${DEFAULT_RECIPIENT_EMAIL}"
BARE_REF=$DEFAULT_RECIPIENT_EMAIL
FALLBACK="${NOT_SET:-default-value}"
KEEP="${DEFAULT_RECIPIENT_EMAIL:-ignored}"
        "##,
    );
    assert_eq!(d.str("OTHER"), "root");
    assert_eq!(d.str("BARE_REF"), "root");
    assert_eq!(d.str("FALLBACK"), "default-value");
    assert_eq!(d.str("KEEP"), "root");
}

#[test]
fn role_recipient_arrays() {
    let d = parse(
        r##"
DEFAULT_RECIPIENT_EMAIL="root"
role_recipients_email[sysadmin]="${DEFAULT_RECIPIENT_EMAIL}"
role_recipients_email[webmaster]="web@example.com admin@example.com"
role_recipients_slack[sysadmin]="#alerts"
        "##,
    );
    let email = d.array("role_recipients_email").expect("array present");
    assert_eq!(email.get("sysadmin").unwrap(), "root");
    assert_eq!(
        email.get("webmaster").unwrap(),
        "web@example.com admin@example.com"
    );
    assert_eq!(
        d.array("role_recipients_slack")
            .unwrap()
            .get("sysadmin")
            .unwrap(),
        "#alerts"
    );
}

#[test]
fn declarations_and_unset_are_handled() {
    let d = parse(
        r##"
declare -A role_recipients_custom
export SEND_CUSTOM="YES"
SEND_MSTEAM="YES"
unset -v SEND_MSTEAM
        "##,
    );
    assert_eq!(d.str("SEND_CUSTOM"), "YES");
    assert_eq!(d.get("SEND_MSTEAM"), None);
    assert!(d.unsupported.is_empty(), "{:?}", d.unsupported);
}

#[test]
fn line_continuations_join() {
    let d = parse(
        "LONG=\"first \\\n\
         second\"\n\
         NEXT=ok\n",
    );
    assert_eq!(d.str("LONG"), "first second");
    assert_eq!(d.str("NEXT"), "ok");
}

#[test]
fn multiline_quoted_values_keep_newlines() {
    let d = parse("MSG=\"line1\nline2\"\nAFTER=1\n");
    assert_eq!(d.str("MSG"), "line1\nline2");
    assert_eq!(d.str("AFTER"), "1");
}

#[test]
fn custom_sender_body_is_captured_and_not_flagged() {
    let d = parse(
        r##"
SEND_CUSTOM="YES"
custom_sender() {
    local msg="${host} ${status_message}"
    info "sent ${msg} to ${1}"
}
DEFAULT_RECIPIENT_CUSTOM="ops"
        "##,
    );
    let body = d.custom_sender_body.as_deref().expect("captured");
    assert!(body.contains("custom_sender()"));
    assert!(body.contains("info \"sent ${msg} to ${1}\""));
    // Parsing must continue past the function.
    assert_eq!(d.str("DEFAULT_RECIPIENT_CUSTOM"), "ops");
    assert!(d.unsupported.is_empty(), "{:?}", d.unsupported);
}

#[test]
fn other_functions_are_reported_not_silently_dropped() {
    let d = parse("helper() {\n  echo hi\n}\nSEND_EMAIL=YES\n");
    assert_eq!(d.str("SEND_EMAIL"), "YES");
    assert_eq!(d.unsupported.len(), 1);
    assert!(d.unsupported[0].2.contains("helper()"));
}

#[test]
fn unparseable_statements_are_reported() {
    let d = parse("if [ -f /tmp/x ]; then\nSEND_EMAIL=YES\n");
    assert_eq!(d.str("SEND_EMAIL"), "YES");
    assert!(!d.unsupported.is_empty());
}

#[test]
fn escaped_characters_inside_double_quotes() {
    let d = parse(r#"V="a\"b\\c\$d""#);
    assert_eq!(d.str("V"), r#"a"b\c$d"#);
}

#[test]
fn single_quotes_are_literal() {
    let d = parse(r#"V='${NOT_EXPANDED} $x'"#);
    assert_eq!(d.str("V"), "${NOT_EXPANDED} $x");
}

#[test]
fn merge_gives_precedence_to_the_later_file() {
    let mut stock = parse("SEND_EMAIL=YES\nrole_recipients_email[sysadmin]=root\n");
    let user = parse("SEND_EMAIL=NO\nrole_recipients_email[dba]=dba@example.com\n");
    stock.merge(user);
    assert_eq!(stock.str("SEND_EMAIL"), "NO");
    let arr = stock.array("role_recipients_email").unwrap();
    assert_eq!(arr.get("sysadmin").unwrap(), "root");
    assert_eq!(arr.get("dba").unwrap(), "dba@example.com");
}

#[test]
#[cfg(unix)]
fn command_substitution_runs_through_a_shell() {
    let d = parse("V=\"$(echo hello)\"\nW=`echo world`\n");
    assert_eq!(d.str("V"), "hello");
    assert_eq!(d.str("W"), "world");
}

#[test]
fn the_real_stock_config_parses_without_complaints() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../health/notifications/health_alarm_notify.conf");
    let data = parse_file(&path, &HashMap::new()).expect("stock config readable");
    assert!(
        data.unsupported.is_empty(),
        "stock config produced unsupported constructs: {:?}",
        data.unsupported
    );
    // Spot-check values that the senders depend on.
    // The stock file ships AUTO, which resolves against a real MTA later.
    assert_eq!(data.str("SEND_EMAIL"), "AUTO");
    assert_eq!(data.str("SEND_CUSTOM"), "YES");
    assert_eq!(data.str("SMSEAGLE_MSG_TYPE"), "sms");
    assert_eq!(data.str("IRC_PORT"), "6667");
    // The stock file ships a documented `custom_sender()` stub.
    assert!(data.custom_sender_body.is_some());
}
