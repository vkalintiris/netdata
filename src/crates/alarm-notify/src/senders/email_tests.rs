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
