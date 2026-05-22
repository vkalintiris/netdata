//! Fuzz target: any byte string either parses or returns Err — no
//! panics, no unbounded recursion, no infinite loops.
//!
//! Run with:
//!     cargo +nightly fuzz run parse_no_panic
//! (cargo-fuzz currently requires nightly).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = nlogql::parse(s);
    }
});
