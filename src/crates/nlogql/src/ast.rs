//! LogQL AST.
//!
//! Populated incrementally as we mirror productions from Loki's
//! `syntax.y` (kept locally at `~/.cache/nlogql-loki-reference/`).

use crate::span::Span;

/// Top-level expression. Grows as we add productions.
///
/// Note: only `PartialEq` is derived — `LabelFilter::Numeric` holds
/// an `f64`, which has no total equality (NaN).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `{label="value", ...}` — log stream selector standing alone,
    /// no pipeline.
    Selector(StreamSelector),
    /// `{...} <stage> <stage> ...` — selector followed by one or
    /// more pipeline stages. Mirrors Loki's `PipelineExpr`.
    Pipeline(PipelineExpr),
    /// `rate({...}[5m])`, `count_over_time({...}[5m])`,
    /// `quantile_over_time(0.99, {...}[5m])`, etc. (SOW-09)
    RangeAggregation(RangeAggregationExpr),
    /// `sum(...)`, `avg by (job) (...)`, `topk(5, ...)`, etc. (SOW-11)
    VectorAggregation(VectorAggregationExpr),
    /// `<lhs> <op> <modifier>? <rhs>` — arithmetic, comparison, or
    /// logical binary operator. (SOW-12)
    Binary(BinaryExpr),
    /// Bare numeric literal (`1`, `2.5`, `-3`). (SOW-12)
    Literal(LiteralExpr),
    /// `label_replace(expr, dst, replacement, src, regex)`. (SOW-13)
    LabelReplace(LabelReplaceExpr),
    /// `vector(<number>)` — a scalar wrapped as a 0-d vector. (SOW-13)
    Vector(VectorExpr),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Selector(s) => s.span,
            Expr::Pipeline(p) => p.span,
            Expr::RangeAggregation(r) => r.span,
            Expr::VectorAggregation(v) => v.span,
            Expr::Binary(b) => b.span,
            Expr::Literal(l) => l.span,
            Expr::LabelReplace(l) => l.span,
            Expr::Vector(v) => v.span,
        }
    }
}

/// Selector composed with one or more pipeline stages.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineExpr {
    pub selector: StreamSelector,
    /// Non-empty; an empty pipeline collapses to `Expr::Selector`.
    pub stages: Vec<PipelineStage>,
    pub span: Span,
}

/// One pipeline stage. Variants land progressively across SOW-03+.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineStage {
    /// `|= "x"`, `!~ "y"`, etc. (SOW-03).
    LineFilter(LineFilter),
    /// `| json`, `| logfmt --strict`, `| regexp "..."`, etc. (SOW-04).
    Parser(ParserStage),
    /// `| status >= 400`, `| host = ip("10/8")`, `| a > 1 and b < 2`,
    /// etc. (SOW-05).
    LabelFilter(LabelFilter),
    /// `| line_format "{{ .ip }}"` — rewrite the log line. (SOW-06)
    LineFormat(LineFormatStage),
    /// `| label_format new=old, x="{{ .y }}"` — rename or template
    /// labels. (SOW-06)
    LabelFormat(LabelFormatStage),
    /// `| decolorize` — strip ANSI color codes from the log line. (SOW-07)
    Decolorize(DecolorizeStage),
    /// `| drop label, label2, foo="bar"` — drop labels (or label-value
    /// conditional drop). (SOW-07)
    DropLabels(LabelSelectorList),
    /// `| keep label, label2` — opposite of drop. (SOW-07)
    KeepLabels(LabelSelectorList),
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

/// A parser stage: `| json`, `| logfmt`, `| regexp "..."`, etc.
///
/// `json` and `logfmt` accept an optional `labelExtractionExpressionList`
/// to project specific fields. `logfmt` additionally accepts
/// `--strict` and `--keep-empty` flags before the extractions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParserStage {
    /// `| json` (plain) or `| json a="b.c", c, ...` (with projections).
    Json {
        extractions: Vec<LabelExtraction>,
        span: Span,
    },
    /// `| logfmt`, `| logfmt --strict`, `| logfmt a="b"`, or any combination.
    Logfmt {
        flags: Vec<ParserFlag>,
        extractions: Vec<LabelExtraction>,
        span: Span,
    },
    /// `| regexp "pattern"` — extracts named capture groups.
    Regexp { pattern: String, span: Span },
    /// `| pattern "<ip> - <_> - <method>"` — extracts via Loki pattern syntax.
    Pattern { pattern: String, span: Span },
    /// `| unpack` — unpacks a JSON-encoded log line shipped by promtail.
    Unpack { span: Span },
}

/// A `name [= "<expression>"]` projection in a json or logfmt parser.
///
/// When the input is bare `name` (no `=`), `expression` defaults to
/// `name` itself (syntax.y:316).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelExtraction {
    pub name: String,
    pub expression: String,
    pub span: Span,
}

/// `--strict` and `--keep-empty` flags for the logfmt parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParserFlag {
    /// `--strict` — fail the stage when any field can't be parsed.
    Strict,
    /// `--keep-empty` — emit labels even when the value is empty.
    KeepEmpty,
}

/// A label filter expression. Atomic variants compare a single
/// label against a typed value; `And`/`Or` compose them.
#[derive(Debug, Clone, PartialEq)]
pub enum LabelFilter {
    /// `name (= | != | =~ | !~) "value"` — a reused
    /// [`Matcher`]. (`syntax.y:303`)
    String(Matcher),
    /// `name (= | !=) ip("cidr")`. (`syntax.y:323`)
    Ip {
        name: String,
        op: IpFilterOp,
        value: String,
        span: Span,
    },
    /// `name OP <number>` — numeric comparison. (`syntax.y:352`)
    Numeric {
        name: String,
        op: NumericOp,
        value: f64,
        span: Span,
    },
    /// `name OP <duration>` — duration value in nanoseconds.
    /// (`syntax.y:332`)
    Duration {
        name: String,
        op: NumericOp,
        /// Signed nanoseconds.
        value: i64,
        span: Span,
    },
    /// `name OP <bytes>` — byte-quantity comparison. (`syntax.y:342`)
    Bytes {
        name: String,
        op: NumericOp,
        value: u64,
        span: Span,
    },
    /// `left AND right`, expressed as `,`, `and`, or by adjacency.
    And {
        left: Box<LabelFilter>,
        right: Box<LabelFilter>,
        span: Span,
    },
    /// `left OR right`.
    Or {
        left: Box<LabelFilter>,
        right: Box<LabelFilter>,
        span: Span,
    },
}

impl LabelFilter {
    pub fn span(&self) -> Span {
        match self {
            LabelFilter::String(m) => m.span,
            LabelFilter::Ip { span, .. }
            | LabelFilter::Numeric { span, .. }
            | LabelFilter::Duration { span, .. }
            | LabelFilter::Bytes { span, .. }
            | LabelFilter::And { span, .. }
            | LabelFilter::Or { span, .. } => *span,
        }
    }
}

/// Numeric / duration / bytes comparison operator. Six variants; `==`
/// and `=` both yield `Eq` (`syntax.y:339,349,359`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericOp {
    Eq,
    NotEq,
    Gt,
    Gte,
    Lt,
    Lte,
}

/// Operator for an `ip(...)` label filter — only equality variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpFilterOp {
    Eq,
    NotEq,
}

/// `line_format "<template>"` — Go template rewrite of the log line.
/// We treat the template body as an opaque string; template parsing
/// happens at evaluation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineFormatStage {
    pub template: String,
    pub span: Span,
}

/// `label_format <item> (, <item>)*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelFormatStage {
    /// Non-empty.
    pub items: Vec<LabelFormatItem>,
    pub span: Span,
}

/// One `label_format` item — either a label-to-label rename or a
/// Go-template that produces the new label's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelFormatItem {
    /// `dst = src` — copy `src`'s value into `dst`. (`syntax.y:289`)
    Rename {
        dst: String,
        src: String,
        span: Span,
    },
    /// `dst = "<template>"` — set `dst` to the template's expansion.
    /// (`syntax.y:290`)
    Template {
        dst: String,
        template: String,
        span: Span,
    },
}

/// `| decolorize`. (`syntax.y:286`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecolorizeStage {
    pub span: Span,
}

/// Comma-separated list of `namedMatcher`s used by `drop` and
/// `keep` stages. (`syntax.y:366`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSelectorList {
    /// Non-empty.
    pub items: Vec<LabelSelector>,
    pub span: Span,
}

/// One entry in a `drop`/`keep` list — either a bare label name
/// (drop the label unconditionally) or a full matcher (drop only
/// when the condition holds). (`syntax.y:362`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelSelector {
    /// `drop foo` — bare label name.
    Name { name: String, span: Span },
    /// `drop foo="bar"` — drop only when `foo="bar"`.
    Matched(Matcher),
}

/// A range aggregation: `rangeOp(logRange)`, with optional first
/// argument (e.g. `quantile_over_time(0.99, ...)`) and optional
/// trailing `by(...)`/`without(...)` grouping.
///
/// `syntax.y:169-174`.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeAggregationExpr {
    pub op: RangeOp,
    pub log_range: LogRangeExpr,
    /// First positional argument when present (currently only used
    /// by `quantile_over_time`).
    pub parameter: Option<f64>,
    pub grouping: Option<Grouping>,
    pub span: Span,
}

/// A vector aggregation: `vectorOp [grouping] ( [param,] expr ) [grouping]`.
///
/// `syntax.y:176`. The grouping may appear either before or after
/// the parentheses, but not in both positions on a single call.
/// The optional first-positional parameter is used by `topk`,
/// `bottomk`, and `approx_topk`.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorAggregationExpr {
    pub op: VectorOp,
    pub expr: Box<Expr>,
    pub parameter: Option<f64>,
    pub grouping: Option<Grouping>,
    pub span: Span,
}

/// One of 12 vector operators (`syntax.y:vectorOp`, populated from
/// `lex.go` `functionTokens`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VectorOp {
    Sum,
    Avg,
    Min,
    Max,
    Stddev,
    Stdvar,
    Count,
    BottomK,
    TopK,
    Sort,
    SortDesc,
    ApproxTopK,
}

/// A binary operator expression. (`syntax.y:376`)
///
/// Loki's grammar attaches the same optional `binOpModifier` to
/// every binary op (including arithmetic), so we carry it here
/// even when it's only meaningful for comparison / set operators.
#[derive(Debug, Clone, PartialEq)]
pub struct BinaryExpr {
    pub op: BinaryOp,
    pub lhs: Box<Expr>,
    pub rhs: Box<Expr>,
    pub modifier: BinaryModifier,
    pub span: Span,
}

/// 15 binary operators across 6 precedence levels (`syntax.y:90-95`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    // Precedence 1, left
    Or,
    // Precedence 2, left
    And,
    Unless,
    // Precedence 3, left
    Eq,
    NotEq,
    Gt,
    Gte,
    Lt,
    Lte,
    // Precedence 4, left
    Add,
    Sub,
    // Precedence 5, left
    Mul,
    Div,
    Mod,
    // Precedence 6, right
    Pow,
}

/// Modifier attached to a binary op (`syntax.y:394-462`):
/// - `bool` makes comparison ops return 0/1 instead of filtering.
/// - `on(...)` / `ignoring(...)` controls vector matching.
/// - `group_left` / `group_right` declares many-to-one cardinality.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BinaryModifier {
    pub return_bool: bool,
    pub matching: Option<VectorMatching>,
    pub group: Option<GroupSide>,
    pub include: Vec<String>,
}

/// `on(labels)` vs `ignoring(labels)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorMatching {
    /// `true` for `on`, `false` for `ignoring`.
    pub on: bool,
    /// May be empty for `on()` / `ignoring()`.
    pub labels: Vec<String>,
}

/// `group_left` vs `group_right`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupSide {
    Left,
    Right,
}

/// Numeric literal expression (`syntax.y:464`). The value carries
/// any leading sign.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteralExpr {
    pub value: f64,
    pub span: Span,
}

/// `label_replace(expr, dst, replacement, src, regex)`
/// (`syntax.y:187`). Rewrites the `dst` label on `expr`'s output
/// using a regex match against `src`. Mirrors Prometheus's
/// `label_replace`.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelReplaceExpr {
    pub expr: Box<Expr>,
    pub dst_label: String,
    pub replacement: String,
    pub src_label: String,
    pub regex: String,
    pub span: Span,
}

/// `vector(<number>)` — a 0-dimensional vector carrying a single
/// scalar value (`syntax.y:470`).
#[derive(Debug, Clone, PartialEq)]
pub struct VectorExpr {
    pub value: f64,
    pub span: Span,
}

/// One of 15 range operators (`syntax.y:492`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeOp {
    AbsentOverTime,
    AvgOverTime,
    BytesOverTime,
    BytesRate,
    CountOverTime,
    FirstOverTime,
    LastOverTime,
    MaxOverTime,
    MinOverTime,
    QuantileOverTime,
    Rate,
    RateCounter,
    StddevOverTime,
    StdvarOverTime,
    SumOverTime,
}

/// `by(labels)` / `without(labels)` / `by()` / `without()`.
/// (`syntax.y:518`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grouping {
    /// `false` for `by`, `true` for `without`.
    pub without: bool,
    /// May be empty for the bare `by()` / `without()` forms.
    pub labels: Vec<String>,
    pub span: Span,
}

/// A log range expression: `selector pipeline? RANGE offset? pipeline?`.
///
/// Used as the argument to range aggregations (`rate(<logRange>)`,
/// `count_over_time(<logRange>)`, etc.) in SOW-09. Not surfaced at
/// the top level — `{foo="bar"}[5m]` alone is not a valid LogQL
/// query (it must be wrapped in a range aggregation).
///
/// `syntax.y:128-155`. The grammar permits the pipeline either
/// before or after the `[...]` RANGE token; we accept stages on
/// both sides and merge them in the AST (preserving source order).
/// Unwrap is handled in SOW-10.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRangeExpr {
    pub selector: StreamSelector,
    pub stages: Vec<PipelineStage>,
    /// At-most-one unwrap. Loki's yacc allows it before or after
    /// the RANGE token; we record only its presence and the
    /// caller can inspect source positions via the span if needed.
    pub unwrap: Option<UnwrapExpr>,
    /// Range window length in nanoseconds (always positive).
    pub range_ns: i64,
    /// Optional offset; may be negative.
    pub offset_ns: Option<i64>,
    pub span: Span,
}

/// `unwrapExpr` (syntax.y:157): the `| unwrap` modifier that turns
/// a string-valued label into a numeric value for a range agg.
///
/// - `| unwrap latency`         → bare identifier
/// - `| unwrap duration(latency)` / `bytes(size)` / `duration_seconds(t)`
/// - Trailing post-filters: `| unwrap latency | level="warn" | n>5`
#[derive(Debug, Clone, PartialEq)]
pub struct UnwrapExpr {
    pub conv_op: Option<ConvOp>,
    pub identifier: String,
    pub post_filters: Vec<LabelFilter>,
    pub span: Span,
}

/// Conversion operator on an unwrap (`syntax.y:163`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConvOp {
    /// `bytes(...)` — parse value as bytes.
    Bytes,
    /// `duration(...)` — parse value as Go duration string.
    Duration,
    /// `duration_seconds(...)` — parse value as a float seconds count.
    DurationSeconds,
}
