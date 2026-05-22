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
    /// Range window length in nanoseconds (always positive).
    pub range_ns: i64,
    /// Optional offset; may be negative.
    pub offset_ns: Option<i64>,
    pub span: Span,
}
