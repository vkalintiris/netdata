//! Text primitives of the Netdata Agent, ported from the C implementation
//! (`src/libnetdata`) with byte-identical results.
//!
//! The C code is the specification; every public function names the C
//! function it mirrors. Inputs are bytes (`&[u8]`) because the C code works
//! on NUL-terminated byte strings that need not be UTF-8: the end of a slice,
//! or its first NUL byte, plays the role of the terminator.
//!
//! Behaviour that depends on the C runtime matches the Agent's environment:
//! glibc on x86-64 in the "C" locale (the Agent never calls `setlocale()`).
//! `pow()`, `log10()` and `strtod()` results are glibc's; out-of-range
//! float-to-integer conversions reproduce the x86-64 instructions gcc emits.
//!
//! Modules:
//! - [`print`](mod@print): `print_netdata_double()` and the integer / hex / base64 printers.
//! - [`parse`]: `str2ndd()`, `str2ull_encoded()` and friends, and `strtod()`.
//! - [`duration`], [`size`]: duration, size and entry-count parsing and rendering.
//! - [`simple_pattern`]: Netdata simple patterns.
//! - [`sanitize`]: `text_sanitize()` and the chart, label and function sanitizers.
//! - [`json`]: the `buffer_json_*()` streaming JSON writer.
//! - [`datetime`]: RFC 3339 timestamps (UTC).

pub mod c;

pub mod datetime;
pub mod duration;
pub mod json;
pub mod line_splitter;
pub mod parse;
pub mod print;
pub mod sanitize;
pub mod simple_pattern;
pub mod size;
