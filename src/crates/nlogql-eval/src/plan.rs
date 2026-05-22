//! Plan IR — the post-lowering representation that the evaluator
//! consumes.
//!
//! The IR is *not* a structural rewrite of the AST — it is the AST
//! with two refinements:
//!
//! 1. **Variants that can't be executed are absent.** Today that
//!    means `line_format` and `label_format` stages, which the
//!    parser accepts but which require a Go-template engine
//!    (deferred — see the evaluator plan).
//! 2. **Construction is gated by [`crate::lower`].** A value of
//!    type `Plan` is a load-bearing claim that semantic checks
//!    have passed: e.g. `topk` carries its count parameter,
//!    `quantile_over_time` has a quantile in `[0, 1]`, etc. The
//!    evaluator can trust these invariants without re-checking.
//!
//! The metric-path variants land in SOW-D3.

use nlogql::ast::{LabelFilter, LabelSelectorList, LineFilter, Matcher, ParserStage};

/// A lowered, semantically valid LogQL query.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Query returns log lines.
    Log(LogPlan),
    // Metric(MetricPlan) — landed in SOW-D3.
}

/// A log-path query: stream selector plus an ordered list of
/// pipeline stages that filter / parse / project.
#[derive(Debug, Clone, PartialEq)]
pub struct LogPlan {
    /// Matchers from the stream selector. Each matcher narrows
    /// the candidate stream set; the evaluator AND-s them.
    pub matchers: Vec<Matcher>,
    /// Pipeline stages applied in order, left to right.
    /// May be empty for a bare-selector query (`{foo="bar"}`).
    pub stages: Vec<LogStage>,
}

/// One pipeline stage in a log-path plan. Subset of
/// [`nlogql::ast::PipelineStage`]: the `LineFormat` and
/// `LabelFormat` variants are intentionally absent because the
/// evaluator can't render Go templates yet (deferred to a
/// follow-up plan).
#[derive(Debug, Clone, PartialEq)]
pub enum LogStage {
    /// `|= "x"`, `!~ "y"`, etc. with optional `or`-chained values.
    LineFilter(LineFilter),
    /// `| json`, `| logfmt --strict`, `| regexp "..."`, etc.
    Parser(ParserStage),
    /// `| status >= 400`, `| host = ip("10/8")`, compound and/or.
    LabelFilter(LabelFilter),
    /// `| decolorize` — strip ANSI escape codes.
    Decolorize,
    /// `| drop a, b="c", ...`
    DropLabels(LabelSelectorList),
    /// `| keep a, b="c", ...`
    KeepLabels(LabelSelectorList),
}
