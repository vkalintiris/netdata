use super::*;

#[test]
fn explicit_formats_are_honoured_with_or_without_the_leading_plus() {
    // 2023-11-14T22:13:20Z
    let t = 1_700_000_000;
    assert_eq!(format(t, "+%Y-%m-%d", true), "2023-11-14");
    assert_eq!(format(t, "%Y-%m-%d", true), "2023-11-14");
    assert_eq!(format(t, "%Y-%m-%dT%H:%M:%S", true), "2023-11-14T22:13:20");
}

#[test]
fn an_empty_format_uses_the_date_default_shape() {
    let out = format(1_700_000_000, "", true);
    // "Tue Nov 14 22:13:20 UTC 2023" - weekday, month, day, time, zone, year.
    let parts: Vec<&str> = out.split_whitespace().collect();
    assert_eq!(parts.len(), 6, "unexpected default format: {out}");
    assert_eq!(parts[0], "Tue");
    assert_eq!(parts[1], "Nov");
    assert_eq!(parts[2], "14");
    assert_eq!(parts[3], "22:13:20");
    assert_eq!(parts[5], "2023");
}

#[test]
fn utc_and_local_are_distinct_selectors() {
    let t = 1_700_000_000;
    let utc = format(t, "%H:%M:%S", true);
    assert_eq!(utc, "22:13:20");
    // Local may equal UTC on a UTC machine; it must at least parse to a time.
    let local = format(t, "%H:%M:%S", false);
    assert_eq!(local.len(), 8, "unexpected local time: {local}");
}

#[test]
fn pagerduty_timestamp_shape() {
    let s = pagerduty_timestamp(1_700_000_000);
    assert_eq!(s.len(), "2023-11-14T22:13:20.000".len());
    assert!(s.ends_with(".000"), "{s}");
    assert_eq!(&s[10..11], "T");
}

#[test]
fn current_year_is_four_digits() {
    let y = current_year();
    assert_eq!(y.len(), 4);
    assert!(y.chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn now_is_after_2020() {
    assert!(now_secs() > 1_577_836_800);
}
