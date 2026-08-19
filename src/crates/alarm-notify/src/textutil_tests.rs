use super::*;

#[test]
fn urlencode_matches_shell_unreserved_set() {
    assert_eq!(urlencode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    assert_eq!(urlencode("a b"), "a%20b");
    assert_eq!(urlencode("system.cpu"), "system.cpu");
    assert_eq!(urlencode("10%"), "10%25");
    assert_eq!(urlencode("a/b?c=d&e"), "a%2fb%3fc%3dd%26e");
    // Multi-byte input is encoded byte by byte, as the LC_ALL=C shell loop did.
    assert_eq!(urlencode("é"), "%c3%a9");
}

#[test]
fn duration4human_matches_shell() {
    let cases = [
        (0, "0 second"),
        (1, "1 second"),
        (2, "2 seconds"),
        (59, "59 seconds"),
        (60, "1 minute"),
        (61, "1 minute and 1 second"),
        (122, "2 minutes and 2 seconds"),
        (3600, "1 hour"),
        // seconds >= 30 rounds the minutes up inside the hours branch
        (3600 + 30, "1 hour and 1 minute"),
        (3600 + 120, "1 hour and 2 minutes"),
        (7200, "2 hours"),
        (86400, "1 day"),
        (86400 + 3600, "1 day and 1 hour"),
        // minutes >= 30 rounds the hours up inside the days branch
        (86400 + 1800, "1 day and 1 hour"),
        (2 * 86400 + 2 * 3600, "2 days and 2 hours"),
    ];
    for (secs, want) in cases {
        assert_eq!(duration4human(secs), want, "for {secs}s");
    }
}

#[test]
fn truncation_helpers() {
    assert_eq!(truncate_with_ellipsis("short", 250, 247), "short");
    let long = "x".repeat(300);
    let got = truncate_with_ellipsis(&long, 250, 247);
    assert_eq!(got.chars().count(), 250);
    assert!(got.ends_with("..."));
    assert_eq!(truncate("abcdef", 3), "abc");
    assert_eq!(truncate("ab", 3), "ab");
}

#[test]
fn split_list_handles_commas_and_spaces() {
    assert_eq!(split_list("a,b c,,d  e"), vec!["a", "b", "c", "d", "e"]);
    assert_eq!(split_list("").len(), 0);
}

#[test]
fn expand_substitutes_and_preserves_unknown() {
    let out = expand("a=${a} b=${b} lit=${nope}", |k| match k {
        "a" => Some("1"),
        "b" => Some("2"),
        _ => None,
    });
    assert_eq!(out, "a=1 b=2 lit=${nope}");
}

#[test]
fn expand_is_utf8_safe() {
    let out = expand("héllo ${x} wörld", |k| {
        if k == "x" { Some("✓") } else { None }
    });
    assert_eq!(out, "héllo ✓ wörld");
}

#[test]
fn name_rewrites() {
    assert_eq!(
        underscores_to_spaces("disk_space_usage"),
        "disk space usage"
    );
    assert_eq!(
        dots_and_underscores_to_dashes("system.cpu_usage"),
        "system-cpu-usage"
    );
}
