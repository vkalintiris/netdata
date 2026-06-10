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
//! ## The AST/IR boundary
//!
//! The [`nlogql`] parser produces an [`Expr`](nlogql::ast::Expr)
//! tree that mirrors Loki's `syntax.y` grammar one-to-one. That's
//! the right shape for *grammar* fidelity but the wrong shape for
//! *execution*:
//!
//! - The AST records spans on every node — the evaluator doesn't
//!   need them at runtime, but it would have to drag them through
//!   every computation.
//! - The AST keeps surface-syntax variants the evaluator can't run
//!   (today: `line_format` and `label_format`, both deferred to a
//!   follow-up plan).
//! - Some queries are *syntactically* valid but *semantically*
//!   broken — `topk` without its count, `quantile_over_time(2, ...)`
//!   with a quantile outside `[0, 1]`. The parser is permissive
//!   here so that error reporting is layered: the parser deals in
//!   syntax, the lowering layer deals in semantics.
//!
//! The [`Plan`] IR is the post-lowering shape that the evaluator
//! consumes. It is reachable only via [`lower`], which:
//!
//! 1. Re-uses AST sub-types (`Matcher`, `LineFilter`, `LabelFilter`,
//!    etc.) where they're already in the right shape — the IR is
//!    deliberately *not* a wholesale rewrite.
//! 2. Omits variants the evaluator can't execute, so the type
//!    itself encodes "this is runnable today."
//! 3. Surfaces semantic errors as [`LowerError`] with the source
//!    [`Span`](nlogql::span::Span) of the offending AST node, so
//!    callers can resolve to line/column against the original query.
//!
//! ## Current status
//!
//! Lowering is complete (Phase D landed). The evaluator (`eval`),
//! storage backends, and output formatter are stubs filled in
//! during Phases E–G of `docs/nlogql-evaluator-plan.md`.

pub mod error;
pub mod eval;
pub mod lower;
pub mod output;
pub mod plan;
pub mod storage;

pub use error::{EvalError, LowerError};
pub use lower::lower;
pub use plan::Plan;
