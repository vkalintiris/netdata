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

use nlogql::ast::{
    BinaryModifier, BinaryOp, ConvOp, Grouping, LabelFilter, LabelSelectorList, LineFilter,
    Matcher, ParserStage, RangeOp, VectorOp,
};

/// A lowered, semantically valid LogQL query.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Query returns log lines.
    Log(LogPlan),
    /// Query returns a time-series sample stream.
    Metric(MetricPlan),
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

// ===========================================================
// Metric path (SOW-D3)
// ===========================================================

/// A metric-path expression. Recursive: vector aggregations, binary
/// ops, and `label_replace` wrap inner [`MetricPlan`] children.
#[derive(Debug, Clone, PartialEq)]
pub enum MetricPlan {
    /// `rate({...}[5m])`, `count_over_time(...)`,
    /// `quantile_over_time(0.99, ...)`, etc.
    RangeAgg(RangeAggPlan),
    /// `sum(...)`, `topk(5, ...)`, `avg by (job) (...)`, etc.
    VectorAgg(VectorAggPlan),
    /// `lhs OP modifier? rhs`.
    Binary(BinaryPlan),
    /// `label_replace(inner, dst, replacement, src, regex)`.
    LabelReplace(LabelReplacePlan),
    /// `vector(N)` — wraps a scalar as a 0-d vector.
    Vector(f64),
    /// Bare numeric literal.
    Literal(f64),
}

/// A range aggregation: an op applied to a [`LogRange`] window,
/// optionally taking a first-positional parameter (only
/// `quantile_over_time` uses one) and an optional `by`/`without`
/// grouping.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeAggPlan {
    pub op: RangeOp,
    pub log: LogRange,
    /// Always `Some` for `quantile_over_time`, always `None`
    /// otherwise (enforced at lower time).
    pub parameter: Option<f64>,
    pub grouping: Option<Grouping>,
}

/// The log-window argument of a range aggregation: a selector with
/// its pipeline, an optional unwrap, the range duration, and an
/// optional offset.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRange {
    pub matchers: Vec<Matcher>,
    pub stages: Vec<LogStage>,
    pub unwrap: Option<Unwrap>,
    /// Range window length in nanoseconds (always positive).
    pub range_ns: i64,
    /// Offset in nanoseconds; may be negative.
    pub offset_ns: Option<i64>,
}

/// `| unwrap [conv_op(]identifier[)] [| label_filter]*`.
#[derive(Debug, Clone, PartialEq)]
pub struct Unwrap {
    pub conv_op: Option<ConvOp>,
    pub identifier: String,
    pub post_filters: Vec<LabelFilter>,
}

/// A vector aggregation: an op over a child metric expression,
/// optionally taking a first-positional parameter (only
/// `topk`/`bottomk`/`approx_topk` do) and an optional grouping.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorAggPlan {
    pub op: VectorOp,
    /// Always `Some` for `topk`/`bottomk`/`approx_topk`, always
    /// `None` otherwise (enforced at lower time).
    pub parameter: Option<f64>,
    pub grouping: Option<Grouping>,
    pub inner: Box<MetricPlan>,
}

/// A binary operator with both operands fully lowered.
#[derive(Debug, Clone, PartialEq)]
pub struct BinaryPlan {
    pub op: BinaryOp,
    pub lhs: Box<MetricPlan>,
    pub rhs: Box<MetricPlan>,
    pub modifier: BinaryModifier,
}

/// `label_replace(inner, dst, replacement, src, regex)`.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelReplacePlan {
    pub inner: Box<MetricPlan>,
    pub dst_label: String,
    pub replacement: String,
    pub src_label: String,
    pub regex: String,
}
