//! LogQL AST.
//!
//! AST node families, grouped by the grammar production they
//! correspond to in Loki's `syntax.y` (kept locally at
//! `~/.cache/nlogql-loki-reference/`).
//!
//! Every node carries a [`Span`] into the original query string.
//! Every node implements [`Display`](std::fmt::Display) such that
//! `parse(input)?.to_string()` produces a canonical (normalized)
//! LogQL string that re-parses to an equivalent AST — see the
//! `roundtrip_*` tests in [`crate::parser`].

use std::fmt::{self, Write as _};

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

// ============================================================
// Display impls — canonical (normalized) LogQL serialization.
// ============================================================

// -- Operator enums ------------------------------------------------

impl fmt::Display for MatcherOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MatcherOp::Eq => "=",
            MatcherOp::NotEq => "!=",
            MatcherOp::Match => "=~",
            MatcherOp::NotMatch => "!~",
        })
    }
}

impl fmt::Display for LineFilterOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LineFilterOp::Eq => "|=",
            LineFilterOp::NotEq => "!=",
            LineFilterOp::Match => "|~",
            LineFilterOp::NotMatch => "!~",
            LineFilterOp::Pattern => "|>",
            LineFilterOp::NotPattern => "!>",
        })
    }
}

impl fmt::Display for NumericOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NumericOp::Eq => "==",
            NumericOp::NotEq => "!=",
            NumericOp::Gt => ">",
            NumericOp::Gte => ">=",
            NumericOp::Lt => "<",
            NumericOp::Lte => "<=",
        })
    }
}

impl fmt::Display for IpFilterOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IpFilterOp::Eq => "=",
            IpFilterOp::NotEq => "!=",
        })
    }
}

impl fmt::Display for ParserFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParserFlag::Strict => "--strict",
            ParserFlag::KeepEmpty => "--keep-empty",
        })
    }
}

impl fmt::Display for ConvOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConvOp::Bytes => "bytes",
            ConvOp::Duration => "duration",
            ConvOp::DurationSeconds => "duration_seconds",
        })
    }
}

impl fmt::Display for RangeOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RangeOp::AbsentOverTime => "absent_over_time",
            RangeOp::AvgOverTime => "avg_over_time",
            RangeOp::BytesOverTime => "bytes_over_time",
            RangeOp::BytesRate => "bytes_rate",
            RangeOp::CountOverTime => "count_over_time",
            RangeOp::FirstOverTime => "first_over_time",
            RangeOp::LastOverTime => "last_over_time",
            RangeOp::MaxOverTime => "max_over_time",
            RangeOp::MinOverTime => "min_over_time",
            RangeOp::QuantileOverTime => "quantile_over_time",
            RangeOp::Rate => "rate",
            RangeOp::RateCounter => "rate_counter",
            RangeOp::StddevOverTime => "stddev_over_time",
            RangeOp::StdvarOverTime => "stdvar_over_time",
            RangeOp::SumOverTime => "sum_over_time",
        })
    }
}

impl fmt::Display for VectorOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            VectorOp::Sum => "sum",
            VectorOp::Avg => "avg",
            VectorOp::Min => "min",
            VectorOp::Max => "max",
            VectorOp::Stddev => "stddev",
            VectorOp::Stdvar => "stdvar",
            VectorOp::Count => "count",
            VectorOp::BottomK => "bottomk",
            VectorOp::TopK => "topk",
            VectorOp::Sort => "sort",
            VectorOp::SortDesc => "sort_desc",
            VectorOp::ApproxTopK => "approx_topk",
        })
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BinaryOp::Or => "or",
            BinaryOp::And => "and",
            BinaryOp::Unless => "unless",
            BinaryOp::Eq => "==",
            BinaryOp::NotEq => "!=",
            BinaryOp::Gt => ">",
            BinaryOp::Gte => ">=",
            BinaryOp::Lt => "<",
            BinaryOp::Lte => "<=",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "^",
        })
    }
}

impl fmt::Display for GroupSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GroupSide::Left => "group_left",
            GroupSide::Right => "group_right",
        })
    }
}

// -- Helpers -------------------------------------------------------

/// Backtick the value if it contains a `"`, otherwise double-quote it.
/// Matches Loki's choice for raw strings.
fn fmt_string(s: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if s.contains('"') && !s.contains('`') {
        write!(f, "`{s}`")
    } else {
        f.write_char('"')?;
        for c in s.chars() {
            match c {
                '\\' => f.write_str("\\\\")?,
                '"' => f.write_str("\\\"")?,
                '\n' => f.write_str("\\n")?,
                '\t' => f.write_str("\\t")?,
                '\r' => f.write_str("\\r")?,
                c => f.write_char(c)?,
            }
        }
        f.write_char('"')
    }
}

fn fmt_labels(labels: &[String], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_char('(')?;
    for (i, l) in labels.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        f.write_str(l)?;
    }
    f.write_char(')')
}

// -- Selectors / matchers ------------------------------------------

impl fmt::Display for Matcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.name, self.op)?;
        fmt_string(&self.value, f)
    }
}

impl fmt::Display for StreamSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('{')?;
        for (i, m) in self.matchers.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{m}")?;
        }
        f.write_char('}')
    }
}

// -- Line filters --------------------------------------------------

impl fmt::Display for LineFilterValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LineFilterValue::Literal(s) => fmt_string(s, f),
            LineFilterValue::Ip(s) => {
                f.write_str("ip(")?;
                fmt_string(s, f)?;
                f.write_char(')')
            }
        }
    }
}

impl fmt::Display for LineFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ", self.op)?;
        for (i, v) in self.values.iter().enumerate() {
            if i > 0 {
                f.write_str(" or ")?;
            }
            write!(f, "{v}")?;
        }
        Ok(())
    }
}

// -- Parser stages -------------------------------------------------

impl fmt::Display for LabelExtraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.expression == self.name {
            f.write_str(&self.name)
        } else {
            write!(f, "{}=", self.name)?;
            fmt_string(&self.expression, f)
        }
    }
}

fn fmt_extractions(items: &[LabelExtraction], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for (i, e) in items.iter().enumerate() {
        if i == 0 {
            f.write_char(' ')?;
        } else {
            f.write_str(", ")?;
        }
        write!(f, "{e}")?;
    }
    Ok(())
}

impl fmt::Display for ParserStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParserStage::Json { extractions, .. } => {
                f.write_str("json")?;
                if !extractions.is_empty() {
                    fmt_extractions(extractions, f)?;
                }
                Ok(())
            }
            ParserStage::Logfmt {
                flags,
                extractions,
                ..
            } => {
                f.write_str("logfmt")?;
                for fl in flags {
                    write!(f, " {fl}")?;
                }
                if !extractions.is_empty() {
                    fmt_extractions(extractions, f)?;
                }
                Ok(())
            }
            ParserStage::Regexp { pattern, .. } => {
                f.write_str("regexp ")?;
                fmt_string(pattern, f)
            }
            ParserStage::Pattern { pattern, .. } => {
                f.write_str("pattern ")?;
                fmt_string(pattern, f)
            }
            ParserStage::Unpack { .. } => f.write_str("unpack"),
        }
    }
}

// -- Label filters -------------------------------------------------

impl fmt::Display for LabelFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LabelFilter::String(m) => write!(f, "{m}"),
            LabelFilter::Ip {
                name, op, value, ..
            } => {
                write!(f, "{name}{op}ip(")?;
                fmt_string(value, f)?;
                f.write_char(')')
            }
            LabelFilter::Numeric {
                name, op, value, ..
            } => write!(f, "{name}{op}{value}"),
            LabelFilter::Duration {
                name, op, value, ..
            } => write!(f, "{name}{op}{value}ns"),
            LabelFilter::Bytes {
                name, op, value, ..
            } => write!(f, "{name}{op}{value}B"),
            LabelFilter::And { left, right, .. } => write!(f, "{left} and {right}"),
            LabelFilter::Or { left, right, .. } => write!(f, "{left} or {right}"),
        }
    }
}

// -- Format / structural stages -----------------------------------

impl fmt::Display for LineFormatStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("line_format ")?;
        fmt_string(&self.template, f)
    }
}

impl fmt::Display for LabelFormatItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LabelFormatItem::Rename { dst, src, .. } => write!(f, "{dst}={src}"),
            LabelFormatItem::Template { dst, template, .. } => {
                write!(f, "{dst}=")?;
                fmt_string(template, f)
            }
        }
    }
}

impl fmt::Display for LabelFormatStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("label_format ")?;
        for (i, it) in self.items.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{it}")?;
        }
        Ok(())
    }
}

impl fmt::Display for DecolorizeStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("decolorize")
    }
}

impl fmt::Display for LabelSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LabelSelector::Name { name, .. } => f.write_str(name),
            LabelSelector::Matched(m) => write!(f, "{m}"),
        }
    }
}

fn fmt_drop_keep_list(
    kw: &str,
    list: &LabelSelectorList,
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    write!(f, "{kw} ")?;
    for (i, it) in list.items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{it}")?;
    }
    Ok(())
}

// -- Pipeline stages -----------------------------------------------

impl fmt::Display for PipelineStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineStage::LineFilter(lf) => write!(f, "{lf}"),
            PipelineStage::Parser(p) => write!(f, "| {p}"),
            PipelineStage::LabelFilter(lf) => write!(f, "| {lf}"),
            PipelineStage::LineFormat(s) => write!(f, "| {s}"),
            PipelineStage::LabelFormat(s) => write!(f, "| {s}"),
            PipelineStage::Decolorize(s) => write!(f, "| {s}"),
            PipelineStage::DropLabels(l) => {
                f.write_str("| ")?;
                fmt_drop_keep_list("drop", l, f)
            }
            PipelineStage::KeepLabels(l) => {
                f.write_str("| ")?;
                fmt_drop_keep_list("keep", l, f)
            }
        }
    }
}

impl fmt::Display for PipelineExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.selector)?;
        for s in &self.stages {
            write!(f, " {s}")?;
        }
        Ok(())
    }
}

// -- Log range / unwrap -------------------------------------------

impl fmt::Display for UnwrapExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("| unwrap ")?;
        match &self.conv_op {
            Some(c) => write!(f, "{c}({})", self.identifier)?,
            None => f.write_str(&self.identifier)?,
        }
        for pf in &self.post_filters {
            write!(f, " | {pf}")?;
        }
        Ok(())
    }
}

impl fmt::Display for LogRangeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.selector)?;
        for s in &self.stages {
            write!(f, " {s}")?;
        }
        if let Some(u) = &self.unwrap {
            write!(f, " {u}")?;
        }
        write!(f, " [{}ns]", self.range_ns)?;
        if let Some(off) = self.offset_ns {
            write!(f, " offset {off}ns")?;
        }
        Ok(())
    }
}

// -- Range / vector aggregations ----------------------------------

impl fmt::Display for Grouping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.without { "without " } else { "by " })?;
        fmt_labels(&self.labels, f)
    }
}

impl fmt::Display for RangeAggregationExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.op)?;
        if let Some(p) = self.parameter {
            write!(f, "{p}, ")?;
        }
        write!(f, "{})", self.log_range)?;
        if let Some(g) = &self.grouping {
            write!(f, " {g}")?;
        }
        Ok(())
    }
}

impl fmt::Display for VectorAggregationExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.op)?;
        if let Some(p) = self.parameter {
            write!(f, "{p}, ")?;
        }
        write!(f, "{})", self.expr)?;
        if let Some(g) = &self.grouping {
            write!(f, " {g}")?;
        }
        Ok(())
    }
}

// -- Binary ops ---------------------------------------------------

impl fmt::Display for VectorMatching {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.on { "on" } else { "ignoring" })?;
        fmt_labels(&self.labels, f)
    }
}

impl fmt::Display for BinaryModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        if self.return_bool {
            f.write_str("bool")?;
            first = false;
        }
        if let Some(m) = &self.matching {
            if !first {
                f.write_char(' ')?;
            }
            write!(f, "{m}")?;
            first = false;
        }
        if let Some(g) = &self.group {
            if !first {
                f.write_char(' ')?;
            }
            write!(f, "{g}")?;
            if !self.include.is_empty() {
                fmt_labels(&self.include, f)?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for BinaryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.lhs, self.op)?;
        let mod_str = self.modifier.to_string();
        if !mod_str.is_empty() {
            write!(f, " {mod_str}")?;
        }
        write!(f, " {}", self.rhs)
    }
}

// -- Literal / label_replace / vector -----------------------------

impl fmt::Display for LiteralExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl fmt::Display for LabelReplaceExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "label_replace({}, ", self.expr)?;
        fmt_string(&self.dst_label, f)?;
        f.write_str(", ")?;
        fmt_string(&self.replacement, f)?;
        f.write_str(", ")?;
        fmt_string(&self.src_label, f)?;
        f.write_str(", ")?;
        fmt_string(&self.regex, f)?;
        f.write_char(')')
    }
}

impl fmt::Display for VectorExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vector({})", self.value)
    }
}

// -- Top-level Expr -----------------------------------------------

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Selector(s) => write!(f, "{s}"),
            Expr::Pipeline(p) => write!(f, "{p}"),
            Expr::RangeAggregation(r) => write!(f, "{r}"),
            Expr::VectorAggregation(v) => write!(f, "{v}"),
            Expr::Binary(b) => write!(f, "{b}"),
            Expr::Literal(l) => write!(f, "{l}"),
            Expr::LabelReplace(lr) => write!(f, "{lr}"),
            Expr::Vector(v) => write!(f, "{v}"),
        }
    }
}
