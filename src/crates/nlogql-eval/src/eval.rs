//! Plan IR evaluator.
//!
//! Walks a [`crate::plan::Plan`] against a [`crate::storage::Backend`]
//! impl, producing a result stream. Log-path evaluation lands in
//! SOW-F1; metric-path follows in SOW-F6 / F7 / F8.
