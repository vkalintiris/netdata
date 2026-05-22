//! Error types for lowering and evaluation.

/// Failure to lower a parsed AST into a [`crate::plan::Plan`].
///
/// These are *semantic* errors — the AST was syntactically valid
/// (the parser accepted it) but couldn't be turned into something
/// the evaluator can run. Examples: a `quantile_over_time` with
/// `q < 0`, a `topk` without its count argument.
// `Eq` deliberately not derived — `QuantileOutOfRange` carries an
// `f64` so total equality is undefined (NaN).
#[derive(Debug, Clone, PartialEq)]
pub enum LowerError {
    /// Feature not yet wired through to the lowering layer.
    Unimplemented { what: &'static str },
    /// A pipeline stage the parser accepts but that we deliberately
    /// don't lower yet — the evaluator can't execute it. Today
    /// this is `line_format` and `label_format`, both deferred to
    /// a follow-up plan that introduces a Go-template engine.
    DeferredStage { stage: &'static str },
    /// An aggregation operator was called with a missing first
    /// argument (`topk(rate(...))` instead of `topk(5, rate(...))`,
    /// or `quantile_over_time({...}[5m])` instead of
    /// `quantile_over_time(0.95, {...}[5m])`).
    MissingParameter { op: &'static str },
    /// An aggregation operator was called with a numeric first
    /// argument it doesn't accept (e.g. `sum(3, rate(...))`).
    /// Closes the leniency documented in `nlogql/EXPECTED_FAILS.md`.
    UnexpectedParameter { op: &'static str },
    /// `quantile_over_time` called with a quantile outside `[0, 1]`.
    QuantileOutOfRange { value: f64 },
    /// A log-shaped expression (`Selector` / `Pipeline`) showed up
    /// where a metric expression was expected. Today this can't
    /// happen through the parser's top-level entry — log queries
    /// don't compose with binops — but the error exists so the
    /// pattern match in `lower_metric` is total.
    LogInMetricPosition,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LowerError::Unimplemented { what } => {
                write!(f, "lower: not yet implemented: {what}")
            }
            LowerError::DeferredStage { stage } => write!(
                f,
                "lower: pipeline stage `{stage}` is not supported in \
                 this build (deferred to a follow-up plan)"
            ),
            LowerError::MissingParameter { op } => write!(
                f,
                "lower: `{op}` requires a numeric first argument"
            ),
            LowerError::UnexpectedParameter { op } => write!(
                f,
                "lower: `{op}` does not take a numeric first argument"
            ),
            LowerError::QuantileOutOfRange { value } => write!(
                f,
                "lower: quantile_over_time requires q in [0, 1], got {value}"
            ),
            LowerError::LogInMetricPosition => f.write_str(
                "lower: log-shaped expression in metric-only position",
            ),
        }
    }
}

impl std::error::Error for LowerError {}

/// Failure during plan evaluation against a backend.
///
/// Populated in Phase F. The `Unimplemented` placeholder exists so
/// downstream consumers can pattern-match defensively from SOW-D1
/// onwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// Evaluator feature not yet implemented (e.g. `line_format`
    /// stage — see the plan doc for the deferred items).
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
