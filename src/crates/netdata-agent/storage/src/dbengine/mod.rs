//! The dbengine storage engine, ported from `src/database/engine/`: on-disk compatible with C's files both ways
//! (GOAL I4). Spec `knowledge/spec-dbengine.md` in the status repository; decisions D29, D30, D50, D62.
//!
//! `format` is the file-format layer (step S0): pure encoders, decoders and validators of C's on-disk structures.
//! `engine` is the running engine (step S2 on): the metric registry, the tiers' files, the read path.

pub mod engine;
pub mod format;
