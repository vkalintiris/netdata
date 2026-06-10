//! OTLP log-record normalization helpers.
//!
//! [`normalize::normalize_body`] detects JSON-object strings in log bodies
//! and converts them to structured `KvlistValue`s so the downstream
//! flattener (`otel-ingestor`'s `arrow_bridge`) indexes their fields.
//!
//! This crate previously also carried its own record flattener
//! (`flatten_resource_logs` / `Frame` / `FlatVisitor`); it had no consumers
//! and flattened arrays with different semantics than the live flattener in
//! `arrow_bridge`, so it was removed rather than left as a trap.

pub mod normalize;
