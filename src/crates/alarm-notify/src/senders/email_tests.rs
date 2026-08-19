use super::*;

#[test]
fn sender_parsing_covers_every_documented_form() {
    assert_eq!(
        parse_email_sender("netdata@example.com"),
        ("netdata@example.com".to_string(), String::new())
    );
    assert_eq!(
        parse_email_sender("Netdata <netdata@example.com>"),
        ("netdata@example.com".to_string(), "Netdata".to_string())
    );
    assert_eq!(
        parse_email_sender("\"Netdata Agent\" <netdata@example.com>"),
        (
            "netdata@example.com".to_string(),
            "Netdata Agent".to_string()
        )
    );
    assert_eq!(
        parse_email_sender("'Netdata Agent' <netdata@example.com>"),
        (
            "netdata@example.com".to_string(),
            "Netdata Agent".to_string()
        )
    );
    assert_eq!(parse_email_sender(""), (String::new(), String::new()));
    // A malformed value is used verbatim rather than dropped.
    assert_eq!(
        parse_email_sender("Netdata <broken"),
        ("Netdata <broken".to_string(), String::new())
    );
}

#[test]
fn header_values_cannot_break_out_of_their_header() {
    assert_eq!(header_value("plain"), "plain");
    assert_eq!(
        header_value("evil\r\nBcc: attacker@example.com"),
        "evil  Bcc: attacker@example.com"
    );
    assert_eq!(header_value("a\nb"), "a b");
    // A body-separating blank line cannot be forged either.
    assert!(!header_value("x\r\n\r\ninjected body").contains('\n'));
}
