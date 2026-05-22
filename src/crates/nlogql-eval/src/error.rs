//! Error types for lowering and evaluation.

use nlogql::span::Span;

/// Failure to lower a parsed AST into a [`crate::plan::Plan`].
///
/// These are *semantic* errors — the AST was syntactically valid
/// (the parser accepted it) but couldn't be turned into something
/// the evaluator can run. Every variant carries the source [`Span`]
/// of the offending AST node so callers can resolve byte offsets
/// to line/column against the original query string.
///
/// `Eq` is intentionally not derived — [`LowerError::QuantileOutOfRange`]
/// carries an `f64`, which has no total equality (NaN).
#[derive(Debug, Clone, PartialEq)]
pub enum LowerError {
    /// A pipeline stage the parser accepts but that we deliberately
    /// don't lower yet — the evaluator can't execute it. Today
    /// this is `line_format` and `label_format`, both deferred to
    /// a follow-up plan that introduces a Go-template engine.
    DeferredStage {
        stage: &'static str,
        span: Span,
    },
    /// An aggregation operator was called with a missing first
    /// argument (`topk(rate(...))` instead of `topk(5, rate(...))`,
    /// or `quantile_over_time({...}[5m])` instead of
    /// `quantile_over_time(0.95, {...}[5m])`).
    MissingParameter {
        op: &'static str,
        span: Span,
    },
    /// An aggregation operator was called with a numeric first
    /// argument it doesn't accept (e.g. `sum(3, rate(...))`).
    /// Closes the leniency documented in `nlogql/EXPECTED_FAILS.md`.
    UnexpectedParameter {
        op: &'static str,
        span: Span,
    },
    /// `quantile_over_time` called with a quantile outside `[0, 1]`.
    QuantileOutOfRange {
        value: f64,
        span: Span,
    },
    /// A log-shaped expression (`Selector` / `Pipeline`) appeared
    /// where a metric expression was expected. This can't happen
    /// via the parser's top-level entry — log queries don't compose
    /// with binops — but the error exists so the pattern match in
    /// `lower_metric` stays total against hand-constructed ASTs.
    LogInMetricPosition {
        span: Span,
    },
}

impl LowerError {
    /// Source span of the offending AST node.
    pub fn span(&self) -> Span {
        match self {
            LowerError::DeferredStage { span, .. }
            | LowerError::MissingParameter { span, .. }
            | LowerError::UnexpectedParameter { span, .. }
            | LowerError::QuantileOutOfRange { span, .. }
            | LowerError::LogInMetricPosition { span } => *span,
        }
    }
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.span();
        write!(f, "lower at byte {}..{}: ", s.start, s.end)?;
        match self {
            LowerError::DeferredStage { stage, .. } => write!(
                f,
                "pipeline stage `{stage}` is not supported in this build \
                 (deferred to a follow-up plan)"
            ),
            LowerError::MissingParameter { op, .. } => {
                write!(f, "`{op}` requires a numeric first argument")
            }
            LowerError::UnexpectedParameter { op, .. } => {
                write!(f, "`{op}` does not take a numeric first argument")
            }
            LowerError::QuantileOutOfRange { value, .. } => {
                write!(f, "quantile_over_time requires q in [0, 1], got {value}")
            }
            LowerError::LogInMetricPosition { .. } => {
                f.write_str("log-shaped expression in metric-only position")
            }
        }
    }
}

impl std::error::Error for LowerError {}

/// Failure during plan evaluation against a backend.
///
/// Populated in Phase F. The `Unimplemented` placeholder exists so
/// downstream consumers can pattern-match defensively today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// Evaluator feature not yet implemented.
    Unimplemented { what: &'static str },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::Unimplemented { what } => {
                write!(f, "eval: not yet implemented: {what}")
            }
        }
    }
}

impl std::error::Error for EvalError {}
