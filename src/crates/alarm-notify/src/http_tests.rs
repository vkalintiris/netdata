use super::*;

#[test]
fn curl_options_subset_is_understood() {
    let o =
        CurlOptions::parse("--connect-timeout 10 --insecure --max-time 20 --proxy http://p:3128");
    assert_eq!(
        o,
        CurlOptions {
            connect_timeout: Some(10),
            max_time: Some(20),
            insecure: true,
            proxy: Some("http://p:3128".to_string()),
        }
    );
    assert_eq!(CurlOptions::parse(""), CurlOptions::default());
}

#[test]
fn urls_are_redacted_for_logging() {
    assert_eq!(
        redact_url("https://api.telegram.org/bot123:ABC/sendMessage?chat_id=1"),
        "https://api.telegram.org/bot[REDACTED_TOKEN]/sendMessage?[REDACTED_QUERY]"
    );
    assert_eq!(
        redact_url("https://gotify.example/message?token=secret"),
        "https://gotify.example/message?[REDACTED_QUERY]"
    );
    assert_eq!(
        redact_url("https://hooks.slack.com/services/T/B/X"),
        "https://hooks.slack.com/services/T/B/X"
    );
}

#[test]
fn response_helpers() {
    let r = Response {
        status: Some(202),
        body: String::new(),
    };
    assert!(r.is(202));
    assert!(r.is_any(&[200, 201, 202]));
    assert!(!r.is_any(&[200]));
    assert_eq!(r.code_str(), "202");
    let none = Response {
        status: None,
        body: String::new(),
    };
    assert_eq!(none.code_str(), "none");
    assert!(!none.is(200));
}

#[test]
fn raw_bodies_default_to_form_urlencoded_like_curl() {
    let req = Request::post("http://x").raw("{\"a\":1}");
    match req.body {
        Body::Raw { content_type, data } => {
            assert!(content_type.is_none());
            assert_eq!(data, "{\"a\":1}");
        }
        _ => panic!("expected raw body"),
    }
    let req = Request::post("http://x").json("{}");
    match req.body {
        Body::Raw { content_type, .. } => {
            assert_eq!(content_type.as_deref(), Some("application/json"))
        }
        _ => panic!("expected raw body"),
    }
}
