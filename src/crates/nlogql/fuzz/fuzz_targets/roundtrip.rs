//! Fuzz target: for any input that parses, the canonical display
//! form re-parses and re-displaying it is a fixed point.
//!
//! Run with:
//!     cargo +nightly fuzz run roundtrip

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(first) = nlogql::parse(s) else {
        return;
    };
    let displayed = first.to_string();
    let reparsed = nlogql::parse(&displayed)
        .expect("canonical display must re-parse");
    let redisplayed = reparsed.to_string();
    assert_eq!(
        displayed, redisplayed,
        "display is not idempotent: {displayed:?} -> {redisplayed:?}"
    );
});
