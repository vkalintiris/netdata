//! LogQL AST.
//!
//! Populated incrementally as we mirror productions from Loki's
//! `syntax.y` (kept locally at `~/.cache/nlogql-loki-reference/`).

use crate::span::Span;

/// Top-level expression. Grows as we add productions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// `{label="value", ...}` — log stream selector standing alone,
    /// no pipeline.
    Selector(StreamSelector),
    /// `{...} <stage> <stage> ...` — selector followed by one or
    /// more pipeline stages. Mirrors Loki's `PipelineExpr`.
    Pipeline(PipelineExpr),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Selector(s) => s.span,
            Expr::Pipeline(p) => p.span,
        }
    }
}

/// Selector composed with one or more pipeline stages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineExpr {
    pub selector: StreamSelector,
    /// Non-empty; an empty pipeline collapses to `Expr::Selector`.
    pub stages: Vec<PipelineStage>,
    pub span: Span,
}

/// One pipeline stage. Variants land progressively across SOW-03+.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineStage {
    /// `|= "x"`, `!~ "y"`, etc. (SOW-03).
    LineFilter(LineFilter),
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

/// A line filter: `<op> <value> [or <value>]*`.
///
/// `values` is non-empty. Multiple values are an `or`-chain — all
/// share the parent's `op`. E.g. `|= "a" or "b"` becomes
/// `LineFilter { op: Eq, values: [Literal("a"), Literal("b")] }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineFilter {
    pub op: LineFilterOp,
    pub values: Vec<LineFilterValue>,
    pub span: Span,
}

/// Operand of a line filter — either a literal string or a CIDR
/// passed to `ip(...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineFilterValue {
    /// `|= "literal"`
    Literal(String),
    /// `|= ip("10.0.0.0/8")`
    Ip(String),
}

/// Line-filter comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineFilterOp {
    /// `|=` — contains exact substring.
    Eq,
    /// `!=` — does not contain.
    NotEq,
    /// `|~` — regex match.
    Match,
    /// `!~` — regex non-match.
    NotMatch,
    /// `|>` — pattern match (Loki 2.9+).
    Pattern,
    /// `!>` — pattern non-match (Loki 2.9+).
    NotPattern,
}
