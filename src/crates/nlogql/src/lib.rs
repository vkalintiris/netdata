//! Netdata's LogQL parser and evaluator.
//!
//! The reference grammar is Loki's `syntax.y`
//! (`~/repos/loki/pkg/logql/syntax/syntax.y`). We do not vendor or
//! copy Loki source; the yacc file is treated as a specification
//! oracle. A vendored reference of the (incomplete) third-party
//! `logql` crate lives at `~/repos/crates/logql/` for comparison.
//!
//! ## Design goals
//!
//! - **Full-input consumption**: a successful parse must consume the
//!   entire input. No silent suffix-drop.
//! - **Operator precedence**: binary operators follow Loki's yacc
//!   precedence directives (Pratt-style).
//! - **Source spans**: every AST node carries a byte range into the
//!   original query, so error messages and downstream tooling can
//!   point at the offending text.
//! - **Useful errors**: an unparseable input returns a structured
//!   error with location and an "expected" hint, not a generic
//!   combinator failure.
//!
//! ## Crate layout
//!
//! - [`ast`]    — typed AST nodes (post-parse).
//! - [`parser`] — query string → AST.
//! - [`span`]   — source-position primitives shared by AST + errors.
//! - [`error`]  — parse error type.
//!
//! Lowering to an evaluator IR and the evaluator itself will live
//! in sibling modules added in later passes.

pub mod ast;
pub mod error;
pub mod parser;
pub mod span;

pub use error::ParseError;
pub use parser::parse;
