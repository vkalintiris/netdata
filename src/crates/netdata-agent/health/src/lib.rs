//! Health: alerts (C: `src/health/`).
//!
//! The crate grows with milestone 9. So far it holds the configuration path: the parsers of a `health.d` line
//! (`health_config.c`), the rule types (`health_prototypes.h`) and their tables. `tests/oracle/` runs C's own
//! reader, prototype store and hash over `tests/corpus/` and the stock files, and C's unit tables, into the vectors
//! under `tests/vectors/` that this code is written against.

#![forbid(unsafe_code)]

pub mod keywords;
pub mod prototype;
pub mod tables;
