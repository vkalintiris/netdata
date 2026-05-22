//! LogQL AST.
//!
//! Populated incrementally as we mirror productions from Loki's
//! `syntax.y` (kept locally at `~/.cache/nlogql-loki-reference/`).

use crate::span::Span;

/// Top-level expression. Grows as we add productions; today it's
/// just a bare stream selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// `{label="value", ...}` — log stream selector standing alone,
    /// no pipeline. Pipeline composition lands in SOW-03.
    Selector(StreamSelector),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Selector(s) => s.span,
        }
    }
}

/// A LogQL stream selector: `{name1 op "val1", name2 op "val2", ...}`.
///
/// Empty `{}` is syntactically valid; semantic rejection (e.g.
/// "must have at least one matcher") is the evaluator's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSelector {
    pub matchers: Vec<Matcher>,
    pub span: Span,
}

/// A single label matcher: `name op "value"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matcher {
    pub name: String,
    pub op: MatcherOp,
    pub value: String,
    pub span: Span,
}

/// Matcher comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatcherOp {
    /// `=` — exact equality.
    Eq,
    /// `!=` — exact inequality.
    NotEq,
    /// `=~` — regex match.
    Match,
    /// `!~` — regex non-match.
    NotMatch,
}
