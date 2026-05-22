//! Error types for lowering and evaluation.

/// Failure to lower a parsed AST into a [`crate::plan::Plan`].
///
/// These are *semantic* errors — the AST was syntactically valid
/// (the parser accepted it) but couldn't be turned into something
/// the evaluator can run. Examples: a `quantile_over_time` with
/// `q < 0`, a `topk` without its count argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// Feature not yet wired through to the lowering layer.
    Unimplemented { what: &'static str },
    /// A pipeline stage the parser accepts but that we deliberately
    /// don't lower yet — the evaluator can't execute it. Today
    /// this is `line_format` and `label_format`, both deferred to
    /// a follow-up plan that introduces a Go-template engine.
    DeferredStage { stage: &'static str },
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LowerError::Unimplemented { what } => {
                write!(f, "lower: not yet implemented: {what}")
            }
            LowerError::DeferredStage { stage } => {
                write!(
                    f,
                    "lower: pipeline stage `{stage}` is not supported in \
                     this build (deferred to a follow-up plan)"
                )
            }
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
