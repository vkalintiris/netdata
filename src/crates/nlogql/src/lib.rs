//! Netdata's LogQL parser.
//!
//! ## Quick start
//!
//! ```
//! use nlogql::{parse, ast::Expr};
//!
//! let expr = parse(r#"sum(rate({app="foo"}[5m])) by (job)"#).unwrap();
//! assert!(matches!(expr, Expr::VectorAggregation(_)));
//!
//! // Every AST node implements Display, producing canonical LogQL:
//! let canonical = expr.to_string();
//! assert!(parse(&canonical).is_ok());
//! ```
//!
//! ## Reference
//!
//! The grammar mirrors Loki's `syntax.y` (kept locally at
//! `~/.cache/nlogql-loki-reference/`). Loki is AGPL-3.0; we read it
//! as a spec but vendor no code. A cleaned copy of the third-party
//! MIT `logql` crate lives at `~/repos/crates/logql/` for AST-shape
//! comparison only.
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
//! - [`lex`]    — leaf parsers for primitives (identifiers, strings,
//!   numbers, durations, bytes, whitespace).
//! - [`parser`] — query string → AST.
//! - [`span`]   — source-position primitives shared by AST + errors.
//! - [`error`]  — parse error type.
//!
//! Lowering to an evaluator IR and the evaluator itself will live
//! in sibling modules added in later passes.

pub mod ast;
pub mod error;
pub mod lex;
pub mod parser;
pub mod span;

pub use error::ParseError;
pub use parser::parse;

/// Shared chumsky extra-config: rich per-character errors, no
/// state, no context. Used by every parser in the crate.
pub(crate) type Extra<'a> = chumsky::extra::Err<chumsky::error::Rich<'a, char>>;
