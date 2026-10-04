//! Health: alerts (C: `src/health/`).
//!
//! The crate grows with milestone 9. What is here so far is the oracle of the configuration path under
//! `tests/oracle/`: C's `health.d` reader, prototype store and hash, run over `tests/corpus/` and the stock files,
//! with C's own unit tables, as the vectors under `tests/vectors/` that the Rust reader is written against.

#![forbid(unsafe_code)]
