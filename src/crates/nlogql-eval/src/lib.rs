//! LogQL evaluator: parsed AST → executable plan → query results.
//!
//! Companion to the [`nlogql`] parser crate. This crate owns the
//! lowering layer, the `Backend` storage abstraction, the
//! evaluator that walks plans against backends, and the output
//! formatter. A thin `nlogql-query` binary on top of this library
//! lands in SOW-G1 (see `src/crates/docs/nlogql-evaluator-plan.md`).
//!
//! ## Architecture
//!
//! ```text
//!                            ┌───────────────┐
//!         LogQL string  ───> │  nlogql       │  parse()
//!                            │  (parser)     │
//!                            └───────┬───────┘
//!                                    ▼ Expr (AST)
//!                            ┌───────────────┐
//!                            │  lowering     │  lower()
//!                            │  (this crate) │
//!                            └───────┬───────┘
//!                                    ▼ Plan (IR)
//!                            ┌───────────────┐
//!                            │  evaluator    │  eval()
//!                            │  (this crate) │  ◄──── Backend trait
//!                            └───────┬───────┘             │
//!                                    ▼                     ├─ MemBackend
//!                            QueryResult                   └─ SfstBackend
//!                                    ▼
//!                            ┌───────────────┐
//!                            │  output       │  ndjson
//!                            │  (this crate) │
//!                            └───────────────┘
//!                                    ▼
//!                                  stdout
//! ```
//!
//! ## Module layout
//!
//! - [`plan`] — `Plan` IR types (populated in SOW-D2, SOW-D3).
//! - [`lower`] — AST → Plan lowering with semantic type checks.
//! - [`storage`] — `Backend` trait + concrete impls (Mem, Sfst).
//! - [`eval`] — `eval(plan, &backend)` driver.
//! - [`output`] — result serialization (NDJSON for now).
//! - [`error`] — error types used across the crate.
//!
//! ## Current status
//!
//! Scaffold only — `lower()` returns `Err(LowerError::Unimplemented)`
//! for every input. Productions land progressively across the SOWs
//! in `docs/nlogql-evaluator-plan.md`.

pub mod error;
pub mod eval;
pub mod lower;
pub mod output;
pub mod plan;
pub mod storage;

pub use error::{EvalError, LowerError};
pub use lower::lower;
pub use plan::Plan;
