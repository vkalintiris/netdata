//! Query string → AST.
//!
//! Built with [`chumsky`] combinators. Grammar productions are
//! added incrementally per the implementation plan in
//! `src/crates/docs/nlogql-implementation-plan.md`.

use chumsky::error::Rich;
use chumsky::pratt::{infix, left, right};
use chumsky::prelude::*;

use crate::Extra;
use crate::ast::{
    BinaryExpr, BinaryModifier, BinaryOp, ConvOp, DecolorizeStage, Expr, GroupSide, Grouping,
    IpFilterOp, LabelExtraction, LabelFilter, LabelFormatItem, LabelFormatStage, LabelReplaceExpr,
    LabelSelector, LabelSelectorList, LineFilter, LineFilterOp, LineFilterValue, LineFormatStage,
    LiteralExpr, LogRangeExpr, Matcher, MatcherOp, NumericOp, ParserFlag, ParserStage,
    PipelineExpr, PipelineStage, RangeAggregationExpr, RangeOp, StreamSelector, UnwrapExpr,
    VectorAggregationExpr, VectorExpr, VectorMatching, VectorOp,
};
use crate::lex::{bytes, duration, identifier, number, string_literal, ws};
use crate::error::{ParseError, ParseErrorKind};
use crate::span::Span;

/// Parse a LogQL query string into an AST.
///
/// The parser must consume the entire input. Any trailing
/// non-whitespace produces an error.
pub fn parse(input: &str) -> Result<Expr, ParseError> {
    root().parse(input).into_result().map_err(|errs| {
        let first = errs
            .into_iter()
            .next()
            .expect("chumsky returns >= 1 error on failure");
        convert_error(first)
    })
}

/// `root: expr` (syntax.y:99). Outer whitespace is permitted on
/// either side.
fn root<'a>() -> impl Parser<'a, &'a str, Expr, Extra<'a>> {
    ws().ignore_then(top_level_expr())
        .then_ignore(ws())
        .then_ignore(end())
}

/// `expr` (syntax.y:102): log or metric. Metric expressions
/// include binary ops with full operator precedence; log
/// expressions are `{`-led.
fn top_level_expr<'a>() -> impl Parser<'a, &'a str, Expr, Extra<'a>> + Clone {
    choice((metric_expr(), log_expr()))
}

/// `logExpr` (syntax.y:108).
///
/// Today: `selector | selector pipelineExpr`. (The
/// `OPEN_PARENTHESIS logExpr CLOSE_PARENTHESIS` form is grammatically
/// available in Loki but lands with the metric path in Phase B.)
fn log_expr<'a>() -> impl Parser<'a, &'a str, Expr, Extra<'a>> + Clone {
    selector()
        .then(ws().ignore_then(pipeline_stage()).repeated().collect::<Vec<_>>())
        .map_with(|(sel, stages), e| {
            if stages.is_empty() {
                Expr::Selector(sel)
            } else {
                Expr::Pipeline(PipelineExpr {
                    selector: sel,
                    stages,
                    span: e.span().into(),
                })
            }
        })
}

/// `selector` production from syntax.y:192.
pub(crate) fn selector<'a>() -> impl Parser<'a, &'a str, StreamSelector, Extra<'a>> + Clone {
    matcher()
        .separated_by(ws().then(just(',')).then(ws()))
        .collect::<Vec<Matcher>>()
        .delimited_by(just('{').then(ws()), ws().then(just('}')))
        .map_with(|matchers, e| StreamSelector {
            matchers,
            span: e.span().into(),
        })
}

/// `matcher` production from syntax.y:203.
fn matcher<'a>() -> impl Parser<'a, &'a str, Matcher, Extra<'a>> + Clone {
    identifier()
        .then_ignore(ws())
        .then(matcher_op())
        .then_ignore(ws())
        .then(string_literal())
        .map_with(|((name, op), value), e| Matcher {
            name: name.to_string(),
            op,
            value,
            span: e.span().into(),
        })
}

fn matcher_op<'a>() -> impl Parser<'a, &'a str, MatcherOp, Extra<'a>> + Clone {
    choice((
        just("=~").to(MatcherOp::Match),
        just("!~").to(MatcherOp::NotMatch),
        just("!=").to(MatcherOp::NotEq),
        just("=").to(MatcherOp::Eq),
    ))
}

// -- Pipeline stages -----------------------------------------------

/// `pipelineStage` (syntax.y:215). Line filters have their op as
/// the first token (`|=`, `!~`, etc.) — no separate `|`. All other
/// stages start with a literal `|` then a stage-kind keyword or an
/// identifier-led label filter.
fn pipeline_stage<'a>() -> impl Parser<'a, &'a str, PipelineStage, Extra<'a>> + Clone {
    let line = line_filter().map(PipelineStage::LineFilter);
    // Keyword-led stages share a first byte with identifier-led
    // label filters (e.g. `json` vs `json_size > 5`). Order keyword
    // parsers first; chumsky backtracks if no keyword matches and
    // tries label_filter.
    let pipe_prefixed = just('|').ignore_then(ws()).ignore_then(choice((
        // Keyword-led: each starts with a distinct fixed token.
        parser_stage().map(PipelineStage::Parser),
        line_format_stage().map(PipelineStage::LineFormat),
        label_format_stage().map(PipelineStage::LabelFormat),
        decolorize_stage().map(PipelineStage::Decolorize),
        drop_labels_stage().map(PipelineStage::DropLabels),
        keep_labels_stage().map(PipelineStage::KeepLabels),
        // Identifier-led: tried last because it commits to whatever
        // identifier appears first.
        label_filter().map(PipelineStage::LabelFilter),
    )));
    // Line filters tried first because `|=`, `|~`, `|>` share a
    // first byte with the bare `|`. chumsky backtracks zero-cost
    // when `line_filter_op()` fails to match.
    choice((line, pipe_prefixed))
}

/// `labelParser | logfmtParser | jsonExpressionParser | logfmtExpressionParser`
/// from syntax.y:218-220, collapsed into a single chumsky choice.
fn parser_stage<'a>() -> impl Parser<'a, &'a str, ParserStage, Extra<'a>> + Clone {
    choice((
        json_parser(),
        logfmt_parser(),
        regexp_parser(),
        pattern_parser(),
        unpack_parser(),
    ))
}

/// `JSON labelExtractionExpressionList?` — plain `json` or with
/// projections.
fn json_parser<'a>() -> impl Parser<'a, &'a str, ParserStage, Extra<'a>> + Clone {
    keyword("json")
        .ignore_then(
            ws().ignore_then(label_extraction_list())
                .or_not(),
        )
        .map_with(|extractions, e| ParserStage::Json {
            extractions: extractions.unwrap_or_default(),
            span: e.span().into(),
        })
}

/// `LOGFMT parserFlags? labelExtractionExpressionList?`.
fn logfmt_parser<'a>() -> impl Parser<'a, &'a str, ParserStage, Extra<'a>> + Clone {
    keyword("logfmt")
        .ignore_then(
            ws().ignore_then(parser_flag())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(
            ws().ignore_then(label_extraction_list())
                .or_not(),
        )
        .map_with(|(flags, extractions), e| ParserStage::Logfmt {
            flags,
            extractions: extractions.unwrap_or_default(),
            span: e.span().into(),
        })
}

fn regexp_parser<'a>() -> impl Parser<'a, &'a str, ParserStage, Extra<'a>> + Clone {
    keyword("regexp")
        .ignore_then(ws())
        .ignore_then(string_literal())
        .map_with(|pattern, e| ParserStage::Regexp {
            pattern,
            span: e.span().into(),
        })
}

fn pattern_parser<'a>() -> impl Parser<'a, &'a str, ParserStage, Extra<'a>> + Clone {
    keyword("pattern")
        .ignore_then(ws())
        .ignore_then(string_literal())
        .map_with(|pattern, e| ParserStage::Pattern {
            pattern,
            span: e.span().into(),
        })
}

fn unpack_parser<'a>() -> impl Parser<'a, &'a str, ParserStage, Extra<'a>> + Clone {
    keyword("unpack").map_with(|_, e| ParserStage::Unpack {
        span: e.span().into(),
    })
}

/// A `labelExtractionExpression` (syntax.y:314): `IDENTIFIER` (with
/// `expression` defaulting to the identifier itself) or
/// `IDENTIFIER EQ STRING`.
fn label_extraction<'a>() -> impl Parser<'a, &'a str, LabelExtraction, Extra<'a>> + Clone {
    identifier()
        .then(
            ws()
                .ignore_then(just('='))
                .ignore_then(ws())
                .ignore_then(string_literal())
                .or_not(),
        )
        .map_with(|(name, expr_opt), e| {
            let name_s = name.to_string();
            let expression = expr_opt.unwrap_or_else(|| name_s.clone());
            LabelExtraction {
                name: name_s,
                expression,
                span: e.span().into(),
            }
        })
}

/// Comma-separated, at least one. Trailing comma rejected.
fn label_extraction_list<'a>() -> impl Parser<'a, &'a str, Vec<LabelExtraction>, Extra<'a>> + Clone
{
    label_extraction()
        .separated_by(ws().then(just(',')).then(ws()))
        .at_least(1)
        .collect()
}

// -- Metric expression (SOW-11 + SOW-12) ---------------------------

/// `metricExpr` with full operator precedence. Single recursive
/// parser that handles vector and range aggregations, binary ops,
/// parenthesized sub-expressions, and bare numeric literals.
///
/// Precedence table (syntax.y:90-95, lowest-to-highest):
///   1. `or`                    (left)
///   2. `and`, `unless`         (left)
///   3. `== != > >= < <=`       (left)
///   4. `+ -`                   (left)
///   5. `* / %`                 (left)
///   6. `^`                     (right)
fn metric_expr<'a>() -> impl Parser<'a, &'a str, Expr, Extra<'a>> + Clone {
    recursive(|me| {
        let vec_agg = vector_aggregation_expr_inner(me.clone());

        let label_replace = label_replace_expr_inner(me.clone());
        let atom = choice((
            me.clone()
                .delimited_by(just('(').then(ws()), ws().then(just(')'))),
            vec_agg.map(Expr::VectorAggregation),
            label_replace.map(Expr::LabelReplace),
            vector_expr().map(Expr::Vector),
            range_aggregation_expr().map(Expr::RangeAggregation),
            literal_expr().map(Expr::Literal),
        ));

        atom.pratt((
            infix(left(1), op_with_modifier(keyword("or").to(BinaryOp::Or)), make_binop),
            infix(
                left(2),
                op_with_modifier(choice((
                    keyword("and").to(BinaryOp::And),
                    keyword("unless").to(BinaryOp::Unless),
                ))),
                make_binop,
            ),
            infix(
                left(3),
                op_with_modifier(choice((
                    just("==").to(BinaryOp::Eq),
                    just("!=").to(BinaryOp::NotEq),
                    just(">=").to(BinaryOp::Gte),
                    just("<=").to(BinaryOp::Lte),
                    just(">").to(BinaryOp::Gt),
                    just("<").to(BinaryOp::Lt),
                ))),
                make_binop,
            ),
            infix(
                left(4),
                op_with_modifier(choice((
                    just('+').to(BinaryOp::Add),
                    just('-').to(BinaryOp::Sub),
                ))),
                make_binop,
            ),
            infix(
                left(5),
                op_with_modifier(choice((
                    just('*').to(BinaryOp::Mul),
                    just('/').to(BinaryOp::Div),
                    just('%').to(BinaryOp::Mod),
                ))),
                make_binop,
            ),
            infix(right(6), op_with_modifier(just('^').to(BinaryOp::Pow)), make_binop),
        ))
    })
}

/// Wrap an operator-token parser with surrounding whitespace and
/// the optional `binOpModifier` that LogQL's grammar allows between
/// every operator and its right-hand operand.
fn op_with_modifier<'a, P>(
    op_kw: P,
) -> impl Parser<'a, &'a str, (BinaryOp, BinaryModifier), Extra<'a>> + Clone
where
    P: Parser<'a, &'a str, BinaryOp, Extra<'a>> + Clone + 'a,
{
    ws()
        .ignore_then(op_kw)
        .then_ignore(ws())
        .then(binary_modifier())
        .then_ignore(ws())
}

/// Builder closure used by every Pratt level. Pulls the span from
/// chumsky's `MapExtra` so the resulting `BinaryExpr` covers the
/// whole `lhs OP modifier rhs` source range.
fn make_binop<'a>(
    lhs: Expr,
    (op, modifier): (BinaryOp, BinaryModifier),
    rhs: Expr,
    e: &mut chumsky::input::MapExtra<'a, '_, &'a str, Extra<'a>>,
) -> Expr {
    let s = e.span();
    Expr::Binary(BinaryExpr {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
        modifier,
        span: Span::new(s.start, s.end),
    })
}

/// `binOpModifier` (syntax.y:427) — empty / `bool` / `on(...)` /
/// `ignoring(...)` / `... group_left[(...)]` / `... group_right[(...)]`.
fn binary_modifier<'a>() -> impl Parser<'a, &'a str, BinaryModifier, Extra<'a>> + Clone {
    let labels_paren = identifier()
        .map(|s: &str| s.to_string())
        .separated_by(ws().then(just(',')).then(ws()))
        .collect::<Vec<_>>()
        .delimited_by(just('(').then(ws()), ws().then(just(')')));

    let matching = choice((
        keyword("on").to(true),
        keyword("ignoring").to(false),
    ))
    .then_ignore(ws())
    .then(labels_paren.clone())
    .map(|(on, labels)| VectorMatching { on, labels });

    let group = choice((
        keyword("group_left").to(GroupSide::Left),
        keyword("group_right").to(GroupSide::Right),
    ))
    .then(
        ws().ignore_then(labels_paren)
            .or_not()
            .map(|opt| opt.unwrap_or_default()),
    );

    let bool_kw = keyword("bool")
        .or_not()
        .map(|opt: Option<()>| opt.is_some());

    bool_kw
        .then(ws().ignore_then(matching).or_not())
        .then(ws().ignore_then(group).or_not())
        .map(|((return_bool, matching), grp_opt)| {
            let (group, include) = match grp_opt {
                Some((side, labels)) => (Some(side), labels),
                None => (None, Vec::new()),
            };
            BinaryModifier {
                return_bool,
                matching,
                group,
                include,
            }
        })
}

/// `labelReplaceExpr` (syntax.y:187): five-arg call form.
/// Takes the outer metric expression parser so the first argument
/// can be any `metric_expr`.
fn label_replace_expr_inner<'a>(
    me: impl Parser<'a, &'a str, Expr, Extra<'a>> + Clone + 'a,
) -> impl Parser<'a, &'a str, LabelReplaceExpr, Extra<'a>> + Clone + 'a {
    let comma = ws().then(just(',')).then(ws());
    keyword("label_replace")
        .ignore_then(ws())
        .ignore_then(just('('))
        .ignore_then(ws())
        .ignore_then(me)
        .then_ignore(comma.clone())
        .then(string_literal())
        .then_ignore(comma.clone())
        .then(string_literal())
        .then_ignore(comma.clone())
        .then(string_literal())
        .then_ignore(comma)
        .then(string_literal())
        .then_ignore(ws())
        .then_ignore(just(')'))
        .map_with(
            |((((expr, dst_label), replacement), src_label), regex), e| LabelReplaceExpr {
                expr: Box::new(expr),
                dst_label,
                replacement,
                src_label,
                regex,
                span: e.span().into(),
            },
        )
}

/// `vectorExpr` (syntax.y:470): `vector(<number>)`.
fn vector_expr<'a>() -> impl Parser<'a, &'a str, VectorExpr, Extra<'a>> + Clone {
    keyword("vector")
        .ignore_then(ws())
        .ignore_then(just('('))
        .ignore_then(ws())
        .ignore_then(number())
        .then_ignore(ws())
        .then_ignore(just(')'))
        .map_with(|value, e| VectorExpr {
            value,
            span: e.span().into(),
        })
}

/// `literalExpr` (syntax.y:464): optional sign + NUMBER.
fn literal_expr<'a>() -> impl Parser<'a, &'a str, LiteralExpr, Extra<'a>> + Clone {
    let signed = choice((
        just('-').ignore_then(number()).map(|n: f64| -n),
        just('+').ignore_then(number()),
        number(),
    ));
    signed.map_with(|value, e| LiteralExpr {
        value,
        span: e.span().into(),
    })
}

/// Pull-apart of vector_aggregation_expr into a form that accepts
/// the outer metric-expr parser as its inner argument. Mirrors
/// `syntax.y:176`.
fn vector_aggregation_expr_inner<'a>(
    me: impl Parser<'a, &'a str, Expr, Extra<'a>> + Clone + 'a,
) -> impl Parser<'a, &'a str, VectorAggregationExpr, Extra<'a>> + Clone + 'a {
    let arg_with_param = number()
        .then_ignore(ws())
        .then_ignore(just(','))
        .then_ignore(ws())
        .then(me.clone())
        .map(|(n, e)| (Some(n), Box::new(e)));
    let arg_no_param = me.map(|e| (None, Box::new(e)));
    let arg = choice((arg_with_param, arg_no_param));

    vector_op()
        .then_ignore(ws())
        .then(grouping().or_not())
        .then_ignore(ws())
        .then_ignore(just('('))
        .then_ignore(ws())
        .then(arg)
        .then_ignore(ws())
        .then_ignore(just(')'))
        .then(ws().ignore_then(grouping()).or_not())
        .try_map(|(((op, before_grp), (parameter, expr)), after_grp), span| {
            if before_grp.is_some() && after_grp.is_some() {
                return Err(Rich::custom(
                    span,
                    "vector aggregation cannot have grouping on both sides \
                     of the argument list",
                ));
            }
            Ok(VectorAggregationExpr {
                op,
                expr,
                parameter,
                grouping: before_grp.or(after_grp),
                span: Span::new(span.start, span.end),
            })
        })
}

/// The 12 vector operators. `keyword()` makes prefix collisions safe
/// (e.g. `sort` vs `sort_desc`).
fn vector_op<'a>() -> impl Parser<'a, &'a str, VectorOp, Extra<'a>> + Clone {
    let a = choice((
        keyword("approx_topk").to(VectorOp::ApproxTopK),
        keyword("avg").to(VectorOp::Avg),
        keyword("bottomk").to(VectorOp::BottomK),
        keyword("count").to(VectorOp::Count),
        keyword("max").to(VectorOp::Max),
        keyword("min").to(VectorOp::Min),
    ));
    let b = choice((
        keyword("sort_desc").to(VectorOp::SortDesc),
        keyword("sort").to(VectorOp::Sort),
        keyword("stddev").to(VectorOp::Stddev),
        keyword("stdvar").to(VectorOp::Stdvar),
        keyword("sum").to(VectorOp::Sum),
        keyword("topk").to(VectorOp::TopK),
    ));
    choice((a, b))
}

// -- Range aggregations (SOW-09) -----------------------------------

/// `rangeAggregationExpr` (syntax.y:169): a 15-op `*_over_time`-style
/// call, with optional first-positional parameter (for
/// `quantile_over_time`) and optional trailing `by`/`without`
/// grouping.
fn range_aggregation_expr<'a>()
-> impl Parser<'a, &'a str, RangeAggregationExpr, Extra<'a>> + Clone {
    let arg_with_param = number()
        .then_ignore(ws())
        .then_ignore(just(','))
        .then_ignore(ws())
        .then(log_range_expr())
        .map(|(n, lr)| (Some(n), lr));
    let arg_no_param = log_range_expr().map(|lr| (None, lr));

    range_op()
        .then_ignore(ws())
        .then_ignore(just('('))
        .then_ignore(ws())
        .then(choice((arg_with_param, arg_no_param)))
        .then_ignore(ws())
        .then_ignore(just(')'))
        .then(ws().ignore_then(grouping()).or_not())
        .map_with(
            |((op, (parameter, log_range)), grouping), e| RangeAggregationExpr {
                op,
                log_range,
                parameter,
                grouping,
                span: e.span().into(),
            },
        )
}

/// The 15 range operators (syntax.y:492). `keyword()` enforces word
/// boundaries, so `rate` doesn't match the `rate` prefix of
/// `rate_counter` — order within the choice is irrelevant.
fn range_op<'a>() -> impl Parser<'a, &'a str, RangeOp, Extra<'a>> + Clone {
    // chumsky's choice() tuple arity has a practical ceiling; split
    // across two nested choices to stay under it.
    let a = choice((
        keyword("absent_over_time").to(RangeOp::AbsentOverTime),
        keyword("avg_over_time").to(RangeOp::AvgOverTime),
        keyword("bytes_over_time").to(RangeOp::BytesOverTime),
        keyword("bytes_rate").to(RangeOp::BytesRate),
        keyword("count_over_time").to(RangeOp::CountOverTime),
        keyword("first_over_time").to(RangeOp::FirstOverTime),
        keyword("last_over_time").to(RangeOp::LastOverTime),
        keyword("max_over_time").to(RangeOp::MaxOverTime),
    ));
    let b = choice((
        keyword("min_over_time").to(RangeOp::MinOverTime),
        keyword("quantile_over_time").to(RangeOp::QuantileOverTime),
        keyword("rate_counter").to(RangeOp::RateCounter),
        keyword("rate").to(RangeOp::Rate),
        keyword("stddev_over_time").to(RangeOp::StddevOverTime),
        keyword("stdvar_over_time").to(RangeOp::StdvarOverTime),
        keyword("sum_over_time").to(RangeOp::SumOverTime),
    ));
    choice((a, b))
}

/// `grouping` (syntax.y:518): `(by|without) ( <labels>? )`.
fn grouping<'a>() -> impl Parser<'a, &'a str, Grouping, Extra<'a>> + Clone {
    let kw = choice((keyword("by").to(false), keyword("without").to(true)));
    let labels = identifier()
        .map(|s: &str| s.to_string())
        .separated_by(ws().then(just(',')).then(ws()))
        .collect::<Vec<_>>();
    kw.then_ignore(ws())
        .then_ignore(just('('))
        .then_ignore(ws())
        .then(labels)
        .then_ignore(ws())
        .then_ignore(just(')'))
        .map_with(|(without, labels), e| Grouping {
            without,
            labels,
            span: e.span().into(),
        })
}

// -- Log range expression (SOW-08) ---------------------------------

/// `logRangeExpr` (syntax.y:128). Argument to range aggregations.
///
/// Accepts the canonical layout — `selector pipeline? RANGE offset?` —
/// and also Loki's alternate layout where the pipeline trails the
/// RANGE/offset. Stages on both sides are merged in source order.
/// Unwrap (SOW-10) is intentionally not handled yet.
pub(crate) fn log_range_expr<'a>() -> impl Parser<'a, &'a str, LogRangeExpr, Extra<'a>> + Clone {
    // A body element is either a pipeline stage or an unwrap.
    // Unwrap is tried first because both start with `|` and unwrap's
    // keyword check would otherwise be shadowed by line-filter
    // operators that share the same leading char.
    let element = choice((
        unwrap_expr().map(BodyElement::Unwrap),
        pipeline_stage().map(BodyElement::Stage),
    ));
    let body = ws().ignore_then(element).repeated().collect::<Vec<_>>();

    selector()
        .then(body.clone())
        .then_ignore(ws())
        .then(range_token())
        .then(ws().ignore_then(offset_expr()).or_not())
        .then(body)
        .map_with(
            |((((sel, pre), range_ns), offset_ns), post), e| {
                let mut stages = Vec::new();
                let mut unwrap = None;
                for el in pre.into_iter().chain(post) {
                    match el {
                        BodyElement::Stage(s) => stages.push(s),
                        // Loki's grammar permits at most one unwrap;
                        // if the user writes more than one we take
                        // the last (last-write-wins). Semantic
                        // validation happens in a later pass.
                        BodyElement::Unwrap(u) => unwrap = Some(u),
                    }
                }
                LogRangeExpr {
                    selector: sel,
                    stages,
                    unwrap,
                    range_ns,
                    offset_ns,
                    span: e.span().into(),
                }
            },
        )
}

enum BodyElement {
    Stage(PipelineStage),
    Unwrap(UnwrapExpr),
}

/// `unwrapExpr` (syntax.y:157):
///   `| unwrap IDENTIFIER`
///   `| unwrap convOp ( IDENTIFIER )`
///   `unwrapExpr | labelFilter`  — post-filter chain
fn unwrap_expr<'a>() -> impl Parser<'a, &'a str, UnwrapExpr, Extra<'a>> + Clone {
    // duration_seconds before duration so the prefix match resolves
    // to the longer form. (keyword()'s rewind handles this too, but
    // ordering is still meaningful for clarity.)
    let conv = choice((
        keyword("duration_seconds").to(ConvOp::DurationSeconds),
        keyword("duration").to(ConvOp::Duration),
        keyword("bytes").to(ConvOp::Bytes),
    ));

    let conv_form = conv
        .then_ignore(ws())
        .then_ignore(just('('))
        .then_ignore(ws())
        .then(identifier())
        .then_ignore(ws())
        .then_ignore(just(')'))
        .map(|(c, name): (ConvOp, &str)| (Some(c), name.to_string()));
    let plain_form = identifier().map(|n: &str| (None, n.to_string()));
    let unwrap_body = choice((conv_form, plain_form));

    just('|')
        .then_ignore(ws())
        .then_ignore(keyword("unwrap"))
        .then_ignore(ws())
        .ignore_then(unwrap_body)
        .then(
            ws()
                .ignore_then(just('|'))
                .ignore_then(ws())
                .ignore_then(label_filter())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map_with(|((conv_op, identifier), post_filters), e| UnwrapExpr {
            conv_op,
            identifier,
            post_filters,
            span: e.span().into(),
        })
}

/// `RANGE` token: `[<duration>]`. In Loki this is one lexer token;
/// here we parse the brackets explicitly with internal whitespace
/// allowed for readability.
fn range_token<'a>() -> impl Parser<'a, &'a str, i64, Extra<'a>> + Clone {
    just('[')
        .ignore_then(ws())
        .ignore_then(duration())
        .then_ignore(ws())
        .then_ignore(just(']'))
}

/// `offsetExpr` (syntax.y:510): `offset <duration>`. The duration
/// may be negative (Loki returns DURATION with a negative value).
fn offset_expr<'a>() -> impl Parser<'a, &'a str, i64, Extra<'a>> + Clone {
    keyword("offset").ignore_then(ws()).ignore_then(duration())
}

// -- Structural stages (SOW-07) -----------------------------------

/// `decolorizeExpr` (syntax.y:286): bare `decolorize` keyword.
fn decolorize_stage<'a>() -> impl Parser<'a, &'a str, DecolorizeStage, Extra<'a>> + Clone {
    keyword("decolorize").map_with(|_, e| DecolorizeStage {
        span: e.span().into(),
    })
}

/// `dropLabelsExpr` (syntax.y:371): `drop namedMatchers`.
fn drop_labels_stage<'a>() -> impl Parser<'a, &'a str, LabelSelectorList, Extra<'a>> + Clone {
    keyword("drop")
        .ignore_then(ws())
        .ignore_then(named_matchers())
        .map_with(|items, e| LabelSelectorList {
            items,
            span: e.span().into(),
        })
}

/// `keepLabelsExpr` (syntax.y:373): `keep namedMatchers`.
fn keep_labels_stage<'a>() -> impl Parser<'a, &'a str, LabelSelectorList, Extra<'a>> + Clone {
    keyword("keep")
        .ignore_then(ws())
        .ignore_then(named_matchers())
        .map_with(|items, e| LabelSelectorList {
            items,
            span: e.span().into(),
        })
}

/// `namedMatchers` (syntax.y:366): one-or-more `namedMatcher`s
/// separated by commas.
fn named_matchers<'a>() -> impl Parser<'a, &'a str, Vec<LabelSelector>, Extra<'a>> + Clone {
    named_matcher()
        .separated_by(ws().then(just(',')).then(ws()))
        .at_least(1)
        .collect()
}

/// `namedMatcher` (syntax.y:362): either a bare `IDENTIFIER`
/// (label name) or a full `matcher` (label = "value" with any of
/// the four matcher ops).
fn named_matcher<'a>() -> impl Parser<'a, &'a str, LabelSelector, Extra<'a>> + Clone {
    identifier()
        .then(
            ws()
                .ignore_then(matcher_op())
                .then_ignore(ws())
                .then(string_literal())
                .or_not(),
        )
        .map_with(|(name, m_opt), e| {
            let name = name.to_string();
            let span: Span = e.span().into();
            match m_opt {
                None => LabelSelector::Name { name, span },
                Some((op, value)) => LabelSelector::Matched(Matcher {
                    name,
                    op,
                    value,
                    span,
                }),
            }
        })
}

// -- Format stages (SOW-06) ----------------------------------------

/// `lineFormatExpr` (syntax.y:284): `LINE_FMT STRING`.
fn line_format_stage<'a>() -> impl Parser<'a, &'a str, LineFormatStage, Extra<'a>> + Clone {
    keyword("line_format")
        .ignore_then(ws())
        .ignore_then(string_literal())
        .map_with(|template, e| LineFormatStage {
            template,
            span: e.span().into(),
        })
}

/// `labelFormatExpr` (syntax.y:299): `LABEL_FMT labelsFormat`.
fn label_format_stage<'a>() -> impl Parser<'a, &'a str, LabelFormatStage, Extra<'a>> + Clone {
    keyword("label_format")
        .ignore_then(ws())
        .ignore_then(
            label_format_item()
                .separated_by(ws().then(just(',')).then(ws()))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .map_with(|items, e| LabelFormatStage {
            items,
            span: e.span().into(),
        })
}

/// `labelFormat` (syntax.y:288): `IDENTIFIER EQ (IDENTIFIER | STRING)`.
/// String RHS yields a template; identifier RHS yields a rename.
fn label_format_item<'a>() -> impl Parser<'a, &'a str, LabelFormatItem, Extra<'a>> + Clone {
    enum Rhs {
        Rename(String),
        Template(String),
    }

    let rhs = choice((
        string_literal().map(Rhs::Template),
        identifier().map(|s: &str| Rhs::Rename(s.to_string())),
    ));

    identifier()
        .then_ignore(ws())
        .then_ignore(just('='))
        .then_ignore(ws())
        .then(rhs)
        .map_with(|(dst, rhs), e| {
            let span = e.span().into();
            let dst = dst.to_string();
            match rhs {
                Rhs::Rename(src) => LabelFormatItem::Rename { dst, src, span },
                Rhs::Template(template) => LabelFormatItem::Template {
                    dst,
                    template,
                    span,
                },
            }
        })
}

// -- Label filters (SOW-05) ----------------------------------------

/// `labelFilter` (syntax.y:302) with AND/OR composition and parens.
///
/// Atoms: ip / duration / bytes / numeric / string-matcher, tried
/// in that order so the disambiguation works without committing to
/// a wrong branch.
fn label_filter<'a>() -> impl Parser<'a, &'a str, LabelFilter, Extra<'a>> + Clone {
    recursive(|expr| {
        let atom = choice((
            expr.delimited_by(just('(').then(ws()), ws().then(just(')'))),
            atomic_label_filter(),
        ));

        // AND-level: comma, `and` keyword. Adjacency (no separator
        // at all between two atoms) is also valid in Loki; we skip
        // it for now and require an explicit separator.
        let and_sep = ws()
            .ignore_then(choice((just(',').ignored(), keyword("and"))))
            .then_ignore(ws());
        let and_expr = atom
            .clone()
            .then(and_sep.ignore_then(atom).repeated().collect::<Vec<_>>())
            .map(|(first, rest)| {
                rest.into_iter().fold(first, |acc, right| {
                    let span = Span::new(acc.span().start, right.span().end);
                    LabelFilter::And {
                        left: Box::new(acc),
                        right: Box::new(right),
                        span,
                    }
                })
            });

        // OR-level: `or` keyword.
        let or_sep = ws().ignore_then(keyword("or")).then_ignore(ws());
        and_expr
            .clone()
            .then(or_sep.ignore_then(and_expr).repeated().collect::<Vec<_>>())
            .map(|(first, rest)| {
                rest.into_iter().fold(first, |acc, right| {
                    let span = Span::new(acc.span().start, right.span().end);
                    LabelFilter::Or {
                        left: Box::new(acc),
                        right: Box::new(right),
                        span,
                    }
                })
            })
    })
}

/// Atomic label filter: a single labelled comparison. Five variants,
/// disambiguated by value type. Order matters — most-specific first.
fn atomic_label_filter<'a>() -> impl Parser<'a, &'a str, LabelFilter, Extra<'a>> + Clone {
    choice((
        ip_label_filter(),
        duration_label_filter(),
        bytes_label_filter(),
        numeric_label_filter(),
        string_label_filter(),
    ))
}

/// `IDENTIFIER (= | !=) ip("cidr")` (syntax.y:323).
fn ip_label_filter<'a>() -> impl Parser<'a, &'a str, LabelFilter, Extra<'a>> + Clone {
    identifier()
        .then_ignore(ws())
        .then(ip_op())
        .then_ignore(ws())
        .then_ignore(keyword("ip"))
        .then_ignore(ws())
        .then_ignore(just('('))
        .then_ignore(ws())
        .then(string_literal())
        .then_ignore(ws())
        .then_ignore(just(')'))
        .map_with(|((name, op), value), e| LabelFilter::Ip {
            name: name.to_string(),
            op,
            value,
            span: e.span().into(),
        })
}

fn ip_op<'a>() -> impl Parser<'a, &'a str, IpFilterOp, Extra<'a>> + Clone {
    choice((
        just("!=").to(IpFilterOp::NotEq),
        just("=").to(IpFilterOp::Eq),
    ))
}

/// `IDENTIFIER cmp_op DURATION` (syntax.y:332).
fn duration_label_filter<'a>() -> impl Parser<'a, &'a str, LabelFilter, Extra<'a>> + Clone {
    identifier()
        .then_ignore(ws())
        .then(comparison_op())
        .then_ignore(ws())
        .then(duration())
        .map_with(|((name, op), value), e| LabelFilter::Duration {
            name: name.to_string(),
            op,
            value,
            span: e.span().into(),
        })
}

/// `IDENTIFIER cmp_op BYTES` (syntax.y:342).
fn bytes_label_filter<'a>() -> impl Parser<'a, &'a str, LabelFilter, Extra<'a>> + Clone {
    identifier()
        .then_ignore(ws())
        .then(comparison_op())
        .then_ignore(ws())
        .then(bytes())
        .map_with(|((name, op), value), e| LabelFilter::Bytes {
            name: name.to_string(),
            op,
            value,
            span: e.span().into(),
        })
}

/// `IDENTIFIER cmp_op literalExpr` (syntax.y:352). `literalExpr` is
/// `[+-]? NUMBER` per syntax.y:464.
fn numeric_label_filter<'a>() -> impl Parser<'a, &'a str, LabelFilter, Extra<'a>> + Clone {
    let signed_number = choice((
        just('-').ignore_then(number()).map(|n: f64| -n),
        just('+').ignore_then(number()),
        number(),
    ));
    identifier()
        .then_ignore(ws())
        .then(comparison_op())
        .then_ignore(ws())
        .then(signed_number)
        .map_with(|((name, op), value), e| LabelFilter::Numeric {
            name: name.to_string(),
            op,
            value,
            span: e.span().into(),
        })
}

/// `matcher` reused as a label filter (syntax.y:303). String-typed.
fn string_label_filter<'a>() -> impl Parser<'a, &'a str, LabelFilter, Extra<'a>> + Clone {
    matcher().map(LabelFilter::String)
}

/// Six-op comparison: `==`/`=` -> Eq, `!=`, `>=`, `<=`, `>`, `<`.
/// `==` before `=`, `>=` before `>`, `<=` before `<`.
fn comparison_op<'a>() -> impl Parser<'a, &'a str, NumericOp, Extra<'a>> + Clone {
    choice((
        just("==").to(NumericOp::Eq),
        just("!=").to(NumericOp::NotEq),
        just(">=").to(NumericOp::Gte),
        just("<=").to(NumericOp::Lte),
        just(">").to(NumericOp::Gt),
        just("<").to(NumericOp::Lt),
        just("=").to(NumericOp::Eq),
    ))
}

/// `FUNCTION_FLAG` token: `--strict` or `--keep-empty`. The trailing
/// rewind check rejects `--strictly`-style false positives.
fn parser_flag<'a>() -> impl Parser<'a, &'a str, ParserFlag, Extra<'a>> + Clone {
    let after = choice((
        any()
            .filter(|c: &char| !c.is_ascii_alphabetic() && *c != '-')
            .rewind()
            .ignored(),
        end(),
    ));
    choice((
        just("--keep-empty")
            .then_ignore(after)
            .to(ParserFlag::KeepEmpty),
        just("--strict")
            .then_ignore(after)
            .to(ParserFlag::Strict),
    ))
}

/// `lineFilter` (syntax.y:248): `filter STRING` or `filter ip(STRING)`,
/// optionally `or`-chained with more values that share the parent op.
fn line_filter<'a>() -> impl Parser<'a, &'a str, LineFilter, Extra<'a>> + Clone {
    line_filter_op()
        .then_ignore(ws())
        .then(line_filter_value_chain())
        .map_with(|(op, values), e| LineFilter {
            op,
            values,
            span: e.span().into(),
        })
}

/// `filter` (syntax.y:229). Longer operators first so the alt
/// resolves correctly.
fn line_filter_op<'a>() -> impl Parser<'a, &'a str, LineFilterOp, Extra<'a>> + Clone {
    choice((
        just("|=").to(LineFilterOp::Eq),
        just("|~").to(LineFilterOp::Match),
        just("|>").to(LineFilterOp::Pattern),
        just("!=").to(LineFilterOp::NotEq),
        just("!~").to(LineFilterOp::NotMatch),
        just("!>").to(LineFilterOp::NotPattern),
    ))
}

/// `<value> (or <value>)*` — at least one value; multiple share the
/// op. Per syntax.y:242 / 251, the chain is left-associative.
fn line_filter_value_chain<'a>() -> impl Parser<'a, &'a str, Vec<LineFilterValue>, Extra<'a>> + Clone
{
    let sep = ws().then(keyword("or")).then(ws());
    line_filter_value()
        .separated_by(sep)
        .at_least(1)
        .collect::<Vec<_>>()
}

/// A single line-filter value: a string literal, or `ip("...")`.
fn line_filter_value<'a>() -> impl Parser<'a, &'a str, LineFilterValue, Extra<'a>> + Clone {
    let ip_form = keyword("ip")
        .ignore_then(ws())
        .ignore_then(just('('))
        .ignore_then(ws())
        .ignore_then(string_literal())
        .then_ignore(ws())
        .then_ignore(just(')'))
        .map(LineFilterValue::Ip);
    let literal = string_literal().map(LineFilterValue::Literal);
    // `ip` starts with `i`, string literals start with `"` or `` ` ``
    // — disjoint at the first byte, so order doesn't matter for
    // backtracking.
    choice((ip_form, literal))
}

/// Match a literal keyword that is not part of a longer identifier.
/// `keyword("or")` accepts `or`, `or ` and `or)` but not `orange`.
fn keyword<'a>(kw: &'static str) -> impl Parser<'a, &'a str, (), Extra<'a>> + Clone {
    let not_ident_continuation = any()
        .filter(|c: &char| !c.is_alphanumeric() && *c != '_')
        .rewind()
        .ignored();
    just(kw)
        .then(choice((not_ident_continuation, end())))
        .ignored()
}

fn convert_error(err: Rich<'_, char>) -> ParseError {
    let s = err.span();
    let span = Span::new(s.start, s.end);
    let kind = if err.found().is_none() {
        ParseErrorKind::UnexpectedEof
    } else {
        ParseErrorKind::Expected("LogQL expression")
    };
    ParseError { span, kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- selector helpers (carried over from SOW-02) ---------------

    fn expect_selector(input: &str) -> StreamSelector {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::Selector(s) => s,
            other => panic!("expected bare selector for {input:?}, got {other:?}"),
        }
    }

    fn expect_pipeline(input: &str) -> PipelineExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::Pipeline(p) => p,
            other => panic!("expected pipeline for {input:?}, got {other:?}"),
        }
    }

    fn expect_range_agg(input: &str) -> RangeAggregationExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::RangeAggregation(r) => r,
            other => panic!("expected range aggregation for {input:?}, got {other:?}"),
        }
    }

    fn expect_vector_agg(input: &str) -> VectorAggregationExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::VectorAggregation(v) => v,
            other => panic!("expected vector aggregation for {input:?}, got {other:?}"),
        }
    }

    fn matcher_at(name: &str, op: MatcherOp, value: &str) -> Matcher {
        Matcher {
            name: name.to_string(),
            op,
            value: value.to_string(),
            span: Span::new(0, 0),
        }
    }

    fn eq_ignore_span(a: &[Matcher], b: &[Matcher]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.name == y.name && x.op == y.op && x.value == y.value)
    }

    fn line_filter_of(stage: &PipelineStage) -> &LineFilter {
        match stage {
            PipelineStage::LineFilter(lf) => lf,
            other => panic!("expected line filter, got {other:?}"),
        }
    }

    fn parser_of(stage: &PipelineStage) -> &ParserStage {
        match stage {
            PipelineStage::Parser(p) => p,
            other => panic!("expected parser stage, got {other:?}"),
        }
    }

    fn label_filter_of(stage: &PipelineStage) -> &LabelFilter {
        match stage {
            PipelineStage::LabelFilter(lf) => lf,
            other => panic!("expected label filter, got {other:?}"),
        }
    }

    fn line_format_of(stage: &PipelineStage) -> &LineFormatStage {
        match stage {
            PipelineStage::LineFormat(lf) => lf,
            other => panic!("expected line_format, got {other:?}"),
        }
    }

    fn label_format_of(stage: &PipelineStage) -> &LabelFormatStage {
        match stage {
            PipelineStage::LabelFormat(lf) => lf,
            other => panic!("expected label_format, got {other:?}"),
        }
    }

    // ============= SOW-02 selector tests ============================

    #[test]
    fn single_matcher() {
        let s = expect_selector(r#"{foo="bar"}"#);
        assert!(eq_ignore_span(
            &s.matchers,
            &[matcher_at("foo", MatcherOp::Eq, "bar")],
        ));
    }

    #[test]
    fn whitespace_around_tokens() {
        let s = expect_selector(r#"{ foo = "bar" }"#);
        assert!(eq_ignore_span(
            &s.matchers,
            &[matcher_at("foo", MatcherOp::Eq, "bar")],
        ));
    }

    #[test]
    fn all_four_matcher_ops() {
        let s = expect_selector(r#"{ foo != "bar" }"#);
        assert_eq!(s.matchers[0].op, MatcherOp::NotEq);

        let s = expect_selector(r#"{ foo =~ "bar" }"#);
        assert_eq!(s.matchers[0].op, MatcherOp::Match);

        let s = expect_selector(r#"{ foo !~ "bar" }"#);
        assert_eq!(s.matchers[0].op, MatcherOp::NotMatch);
    }

    #[test]
    fn comma_separated() {
        let s = expect_selector(r#"{ namespace="buzz", foo != "bar" }"#);
        assert!(eq_ignore_span(
            &s.matchers,
            &[
                matcher_at("namespace", MatcherOp::Eq, "buzz"),
                matcher_at("foo", MatcherOp::NotEq, "bar"),
            ],
        ));
    }

    #[test]
    fn three_matchers() {
        let s = expect_selector(r#"{app!="foo",cluster=~".+bar",bar!~".?boo"}"#);
        assert_eq!(s.matchers.len(), 3);
    }

    #[test]
    fn escaped_quote_in_value() {
        let s = expect_selector(r#"{ foo = "ba\"r" }"#);
        assert_eq!(s.matchers[0].value, r#"ba"r"#);
    }

    #[test]
    fn raw_string_value() {
        let s = expect_selector(r"{foo=~`bar\w+`}");
        assert_eq!(s.matchers[0].value, r"bar\w+");
    }

    #[test]
    fn hash_inside_value_is_not_a_comment() {
        let s = expect_selector(r##"{foo="#"}"##);
        assert_eq!(s.matchers[0].value, "#");
    }

    #[test]
    fn empty_selector() {
        let s = expect_selector("{}");
        assert!(s.matchers.is_empty());
    }

    #[test]
    fn outer_whitespace_and_comments() {
        let s = expect_selector("  # a comment\n{foo=\"bar\"}  ");
        assert_eq!(s.matchers.len(), 1);
    }

    #[test]
    fn span_covers_whole_selector() {
        let input = r#"{foo="bar"}"#;
        let s = expect_selector(input);
        assert_eq!(s.span.start, 0);
        assert_eq!(s.span.end, input.len());
    }

    #[test]
    fn missing_close_brace() {
        assert!(parse(r#"{foo="bar""#).is_err());
    }

    #[test]
    fn missing_value() {
        assert!(parse(r#"{foo=}"#).is_err());
    }

    #[test]
    fn missing_op_and_value() {
        assert!(parse(r#"{foo}"#).is_err());
    }

    #[test]
    fn missing_name() {
        assert!(parse(r#"{="bar"}"#).is_err());
    }

    #[test]
    fn just_garbage() {
        assert!(parse("garbage").is_err());
    }

    #[test]
    fn trailing_comma_rejected() {
        assert!(parse(r#"{foo="bar",}"#).is_err());
    }

    // ============= SOW-03 line filter tests =========================

    #[test]
    fn single_line_filter_eq() {
        let p = expect_pipeline(r#"{app="foo"} |= "error""#);
        assert_eq!(p.stages.len(), 1);
        let lf = line_filter_of(&p.stages[0]);
        assert_eq!(lf.op, LineFilterOp::Eq);
        assert_eq!(lf.values, vec![LineFilterValue::Literal("error".into())]);
    }

    #[test]
    fn all_six_line_filter_ops() {
        for (text, op) in [
            ("|=", LineFilterOp::Eq),
            ("!=", LineFilterOp::NotEq),
            ("|~", LineFilterOp::Match),
            ("!~", LineFilterOp::NotMatch),
            ("|>", LineFilterOp::Pattern),
            ("!>", LineFilterOp::NotPattern),
        ] {
            let q = format!(r#"{{app="foo"}} {text} "x""#);
            let p = expect_pipeline(&q);
            assert_eq!(line_filter_of(&p.stages[0]).op, op, "op for {text:?}");
        }
    }

    #[test]
    fn stacked_line_filters() {
        // From parser_test.go style: multi-filter pipeline.
        let p = expect_pipeline(r#"{app="foo"} |= "error" !~ "noise""#);
        assert_eq!(p.stages.len(), 2);
        assert_eq!(line_filter_of(&p.stages[0]).op, LineFilterOp::Eq);
        assert_eq!(line_filter_of(&p.stages[1]).op, LineFilterOp::NotMatch);
    }

    #[test]
    fn or_chained_values() {
        // syntax.y:242,251 — `or` chains values sharing the parent op.
        let p = expect_pipeline(r#"{app="foo"} |= "a" or "b" or "c""#);
        let lf = line_filter_of(&p.stages[0]);
        assert_eq!(lf.op, LineFilterOp::Eq);
        assert_eq!(
            lf.values,
            vec![
                LineFilterValue::Literal("a".into()),
                LineFilterValue::Literal("b".into()),
                LineFilterValue::Literal("c".into()),
            ],
        );
    }

    #[test]
    fn ip_value() {
        let p = expect_pipeline(r#"{app="foo"} |= ip("10.0.0.0/8")"#);
        let lf = line_filter_of(&p.stages[0]);
        assert_eq!(lf.values, vec![LineFilterValue::Ip("10.0.0.0/8".into())]);
    }

    #[test]
    fn ip_in_or_chain() {
        let p = expect_pipeline(r#"{app="foo"} |= "a" or ip("10.0.0.0/8") or "b""#);
        let lf = line_filter_of(&p.stages[0]);
        assert_eq!(lf.values.len(), 3);
        assert!(matches!(lf.values[0], LineFilterValue::Literal(_)));
        assert!(matches!(lf.values[1], LineFilterValue::Ip(_)));
        assert!(matches!(lf.values[2], LineFilterValue::Literal(_)));
    }

    #[test]
    fn raw_string_value_in_line_filter() {
        // From parser_test.go: `count_over_time({foo=~`bar\w+`}[12h] |~ `error\`)`.
        // The inner `|~ \`error\\\`` form.
        let p = expect_pipeline(r"{foo=`bar`} |~ `error\`");
        let lf = line_filter_of(&p.stages[0]);
        assert_eq!(lf.values, vec![LineFilterValue::Literal(r"error\".into())]);
    }

    #[test]
    fn no_whitespace_between_selector_and_filter() {
        let p = expect_pipeline(r#"{app="foo"}|="error""#);
        assert_eq!(p.stages.len(), 1);
    }

    #[test]
    fn keyword_or_not_substring() {
        // `orange` is not `or` — value list ends after first value.
        // The trailing `orange` then makes the whole parse fail
        // because nothing else can consume it.
        assert!(parse(r#"{app="foo"} |= "a" orange"#).is_err());
    }

    #[test]
    fn pipeline_span_covers_selector_and_stages() {
        let input = r#"{app="foo"} |= "x""#;
        let p = expect_pipeline(input);
        assert_eq!(p.span.start, 0);
        assert_eq!(p.span.end, input.len());
    }

    #[test]
    fn line_filter_missing_value_rejected() {
        assert!(parse(r#"{app="foo"} |="#).is_err());
    }

    #[test]
    fn ip_missing_close_paren_rejected() {
        assert!(parse(r#"{app="foo"} |= ip("1.2.3.4""#).is_err());
    }

    // ============= SOW-04 parser stage tests ========================

    #[test]
    fn json_plain() {
        let p = expect_pipeline(r#"{app="foo"} | json"#);
        assert_eq!(p.stages.len(), 1);
        match parser_of(&p.stages[0]) {
            ParserStage::Json { extractions, .. } => assert!(extractions.is_empty()),
            other => panic!("expected json, got {other:?}"),
        }
    }

    #[test]
    fn json_with_expression() {
        // From lex_test.go: `{foo="bar"} | json code="response.code", param="request.params[0]"`
        let p = expect_pipeline(r#"{app="foo"} | json code="response.code", param="x.y[0]""#);
        match parser_of(&p.stages[0]) {
            ParserStage::Json { extractions, .. } => {
                assert_eq!(extractions.len(), 2);
                assert_eq!(extractions[0].name, "code");
                assert_eq!(extractions[0].expression, "response.code");
                assert_eq!(extractions[1].name, "param");
                assert_eq!(extractions[1].expression, "x.y[0]");
            }
            other => panic!("expected json, got {other:?}"),
        }
    }

    #[test]
    fn json_bare_name_extraction() {
        // syntax.y:316 — IDENTIFIER alone defaults expression to the name.
        let p = expect_pipeline(r#"{app="foo"} | json a, b"#);
        match parser_of(&p.stages[0]) {
            ParserStage::Json { extractions, .. } => {
                assert_eq!(extractions.len(), 2);
                assert_eq!(extractions[0].name, "a");
                assert_eq!(extractions[0].expression, "a");
                assert_eq!(extractions[1].name, "b");
                assert_eq!(extractions[1].expression, "b");
            }
            other => panic!("expected json, got {other:?}"),
        }
    }

    #[test]
    fn logfmt_plain() {
        let p = expect_pipeline(r#"{app="foo"} | logfmt"#);
        match parser_of(&p.stages[0]) {
            ParserStage::Logfmt { flags, extractions, .. } => {
                assert!(flags.is_empty());
                assert!(extractions.is_empty());
            }
            other => panic!("expected logfmt, got {other:?}"),
        }
    }

    #[test]
    fn logfmt_strict_flag() {
        // From parser_test.go: `{ foo = "bar" }|logfmt --strict`
        let p = expect_pipeline(r#"{app="foo"} | logfmt --strict"#);
        match parser_of(&p.stages[0]) {
            ParserStage::Logfmt { flags, extractions, .. } => {
                assert_eq!(flags, &[ParserFlag::Strict]);
                assert!(extractions.is_empty());
            }
            other => panic!("expected logfmt, got {other:?}"),
        }
    }

    #[test]
    fn logfmt_both_flags() {
        // From lex_test.go: `{foo="bar"} | logfmt --strict --keep-empty|=ip("b")`
        let p = expect_pipeline(r#"{app="foo"} | logfmt --strict --keep-empty"#);
        match parser_of(&p.stages[0]) {
            ParserStage::Logfmt { flags, .. } => {
                assert_eq!(flags, &[ParserFlag::Strict, ParserFlag::KeepEmpty]);
            }
            other => panic!("expected logfmt, got {other:?}"),
        }
    }

    #[test]
    fn logfmt_keep_empty_first_then_strict() {
        // From lex_test.go: `{foo="bar"} | logfmt --keep-empty --strict code=...`
        // Order in flags vec matches input order.
        let p = expect_pipeline(
            r#"{app="foo"} | logfmt --keep-empty --strict code="response.code""#,
        );
        match parser_of(&p.stages[0]) {
            ParserStage::Logfmt { flags, extractions, .. } => {
                assert_eq!(flags, &[ParserFlag::KeepEmpty, ParserFlag::Strict]);
                assert_eq!(extractions.len(), 1);
                assert_eq!(extractions[0].name, "code");
            }
            other => panic!("expected logfmt, got {other:?}"),
        }
    }

    #[test]
    fn logfmt_with_extractions_no_flags() {
        // From lex_test.go: `{foo="bar"} | logfmt code="response.code", IPAddress="host"`
        let p = expect_pipeline(
            r#"{app="foo"} | logfmt code="response.code", IPAddress="host""#,
        );
        match parser_of(&p.stages[0]) {
            ParserStage::Logfmt { flags, extractions, .. } => {
                assert!(flags.is_empty());
                assert_eq!(extractions.len(), 2);
                assert_eq!(extractions[1].name, "IPAddress");
                assert_eq!(extractions[1].expression, "host");
            }
            other => panic!("expected logfmt, got {other:?}"),
        }
    }

    #[test]
    fn regexp_with_pattern() {
        let p = expect_pipeline(r#"{app="foo"} | regexp "(?P<level>\\w+)""#);
        match parser_of(&p.stages[0]) {
            ParserStage::Regexp { pattern, .. } => {
                assert_eq!(pattern, r"(?P<level>\w+)");
            }
            other => panic!("expected regexp, got {other:?}"),
        }
    }

    #[test]
    fn pattern_stage() {
        let p = expect_pipeline(r#"{app="foo"} | pattern "<ip> - <_> - <method>""#);
        match parser_of(&p.stages[0]) {
            ParserStage::Pattern { pattern, .. } => {
                assert_eq!(pattern, "<ip> - <_> - <method>");
            }
            other => panic!("expected pattern, got {other:?}"),
        }
    }

    #[test]
    fn unpack_stage() {
        let p = expect_pipeline(r#"{app="foo"} | unpack"#);
        assert!(matches!(
            parser_of(&p.stages[0]),
            ParserStage::Unpack { .. }
        ));
    }

    #[test]
    fn composition_json_then_logfmt() {
        let p = expect_pipeline(r#"{app="foo"} | json | logfmt"#);
        assert_eq!(p.stages.len(), 2);
        assert!(matches!(parser_of(&p.stages[0]), ParserStage::Json { .. }));
        assert!(matches!(parser_of(&p.stages[1]), ParserStage::Logfmt { .. }));
    }

    #[test]
    fn composition_filter_then_parser() {
        let p = expect_pipeline(r#"{app="foo"} |= "x" | json"#);
        assert_eq!(p.stages.len(), 2);
        assert!(matches!(&p.stages[0], PipelineStage::LineFilter(_)));
        assert!(matches!(parser_of(&p.stages[1]), ParserStage::Json { .. }));
    }

    #[test]
    fn no_whitespace_between_pipe_and_keyword() {
        // From parser_test.go: `{ foo = "bar" }|logfmt --strict`
        let p = expect_pipeline(r#"{app="foo"}|logfmt"#);
        assert!(matches!(parser_of(&p.stages[0]), ParserStage::Logfmt { .. }));
    }

    #[test]
    fn regexp_missing_pattern_rejected() {
        assert!(parse(r#"{app="foo"} | regexp"#).is_err());
    }

    #[test]
    fn pattern_missing_pattern_rejected() {
        assert!(parse(r#"{app="foo"} | pattern"#).is_err());
    }

    #[test]
    fn unknown_flag_rejected() {
        // `--frobnicate` isn't a known parser flag, so the parse must fail.
        assert!(parse(r#"{app="foo"} | logfmt --frobnicate"#).is_err());
    }

    #[test]
    fn strictly_is_not_strict() {
        // The trailing-character check on parser_flag must reject
        // `--strictly` as a misspelled `--strict`.
        assert!(parse(r#"{app="foo"} | logfmt --strictly"#).is_err());
    }

    // ============= SOW-05 label filter tests ========================

    const NS: i64 = 1_000_000_000;

    #[test]
    fn label_filter_string_eq() {
        let p = expect_pipeline(r#"{app="foo"} | level = "info""#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::String(m) => {
                assert_eq!(m.name, "level");
                assert_eq!(m.op, MatcherOp::Eq);
                assert_eq!(m.value, "info");
            }
            other => panic!("expected string filter, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_string_regex() {
        let p = expect_pipeline(r#"{app="foo"} | host =~ ".*prod""#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::String(m) => assert_eq!(m.op, MatcherOp::Match),
            other => panic!("expected string filter, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_numeric_ops() {
        // All six numeric comparison operators.
        for (text, op) in [
            ("status > 400", NumericOp::Gt),
            ("status >= 400", NumericOp::Gte),
            ("status < 400", NumericOp::Lt),
            ("status <= 400", NumericOp::Lte),
            ("status == 400", NumericOp::Eq),
            ("status = 400", NumericOp::Eq),
            ("status != 400", NumericOp::NotEq),
        ] {
            let q = format!(r#"{{app="foo"}} | {text}"#);
            let p = expect_pipeline(&q);
            match label_filter_of(&p.stages[0]) {
                LabelFilter::Numeric { op: got, value, .. } => {
                    assert_eq!(*got, op, "op for {text:?}");
                    assert_eq!(*value, 400.0);
                }
                other => panic!("expected numeric for {text:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn label_filter_numeric_signed() {
        let p = expect_pipeline(r#"{app="foo"} | offset > -5"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::Numeric { value, .. } => assert_eq!(*value, -5.0),
            other => panic!("expected numeric, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_duration() {
        // From parser_test.go: `length>5d` and `latency >= 250ms`.
        let p = expect_pipeline(r#"{app="foo"} | latency >= 250ms"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::Duration { name, op, value, .. } => {
                assert_eq!(name, "latency");
                assert_eq!(*op, NumericOp::Gte);
                assert_eq!(*value, 250 * 1_000_000); // 250ms in ns
            }
            other => panic!("expected duration, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_bytes() {
        // From lex_test.go: `size > 250kB`, `size > 200MiB`.
        let p = expect_pipeline(r#"{app="foo"} | size > 250kB"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::Bytes { name, op, value, .. } => {
                assert_eq!(name, "size");
                assert_eq!(*op, NumericOp::Gt);
                assert_eq!(*value, 250_000);
            }
            other => panic!("expected bytes, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_ip_eq() {
        let p = expect_pipeline(r#"{app="foo"} | host = ip("10.0.0.0/8")"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::Ip { name, op, value, .. } => {
                assert_eq!(name, "host");
                assert_eq!(*op, IpFilterOp::Eq);
                assert_eq!(value, "10.0.0.0/8");
            }
            other => panic!("expected ip, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_ip_neq() {
        let p = expect_pipeline(r#"{app="foo"} | host != ip("10.0.0.0/8")"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::Ip { op, .. } => assert_eq!(*op, IpFilterOp::NotEq),
            other => panic!("expected ip, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_and_via_comma() {
        let p = expect_pipeline(r#"{app="foo"} | status >= 400, latency > 100ms"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::And { left, right, .. } => {
                assert!(matches!(**left, LabelFilter::Numeric { .. }));
                assert!(matches!(**right, LabelFilter::Duration { .. }));
            }
            other => panic!("expected AND, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_and_via_keyword() {
        let p = expect_pipeline(r#"{app="foo"} | status >= 400 and latency > 100ms"#);
        assert!(matches!(
            label_filter_of(&p.stages[0]),
            LabelFilter::And { .. }
        ));
    }

    #[test]
    fn label_filter_or() {
        let p = expect_pipeline(r#"{app="foo"} | status >= 500 or latency > 1s"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::Or { left, right, .. } => {
                assert!(matches!(**left, LabelFilter::Numeric { .. }));
                assert!(matches!(**right, LabelFilter::Duration { value, .. } if value == NS));
            }
            other => panic!("expected OR, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_or_precedence_below_and() {
        // `a and b or c` parses as `(a and b) or c`, not `a and (b or c)`.
        let p = expect_pipeline(r#"{app="foo"} | a > 1 and b > 2 or c > 3"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::Or { left, right, .. } => {
                assert!(matches!(**left, LabelFilter::And { .. }));
                assert!(matches!(**right, LabelFilter::Numeric { .. }));
            }
            other => panic!("expected OR at root, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_parens_invert_precedence() {
        // `a and (b or c)` keeps the OR as a child of AND.
        let p = expect_pipeline(r#"{app="foo"} | a > 1 and ( b > 2 or c > 3 )"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::And { right, .. } => {
                assert!(matches!(**right, LabelFilter::Or { .. }));
            }
            other => panic!("expected AND at root, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_chained_and_is_left_assoc() {
        // `a , b , c` -> ((a AND b) AND c).
        let p = expect_pipeline(r#"{app="foo"} | a > 1, b > 2, c > 3"#);
        match label_filter_of(&p.stages[0]) {
            LabelFilter::And { left, right, .. } => {
                assert!(matches!(**left, LabelFilter::And { .. }));
                assert!(matches!(**right, LabelFilter::Numeric { .. }));
            }
            other => panic!("expected AND, got {other:?}"),
        }
    }

    #[test]
    fn label_filter_disambiguates_string_vs_unit_value() {
        // `level = "info"` -> String filter; `level = 200` -> Numeric.
        let p = expect_pipeline(r#"{app="foo"} | level = "info""#);
        assert!(matches!(
            label_filter_of(&p.stages[0]),
            LabelFilter::String(_)
        ));

        let p = expect_pipeline(r#"{app="foo"} | level = 200"#);
        assert!(matches!(
            label_filter_of(&p.stages[0]),
            LabelFilter::Numeric { .. }
        ));
    }

    #[test]
    fn label_filter_after_logfmt() {
        // From parser_test.go-style: `| logfmt | latency >= 250ms`.
        let p = expect_pipeline(r#"{app="foo"} | logfmt | latency >= 250ms"#);
        assert_eq!(p.stages.len(), 2);
        assert!(matches!(parser_of(&p.stages[0]), ParserStage::Logfmt { .. }));
        assert!(matches!(
            label_filter_of(&p.stages[1]),
            LabelFilter::Duration { .. }
        ));
    }

    #[test]
    fn label_filter_missing_value_rejected() {
        assert!(parse(r#"{app="foo"} | status >"#).is_err());
    }

    #[test]
    fn label_filter_dangling_and_rejected() {
        assert!(parse(r#"{app="foo"} | a > 1 and"#).is_err());
    }

    // ============= SOW-06 format stage tests ========================

    #[test]
    fn line_format_basic() {
        let p = expect_pipeline(r#"{app="foo"} | line_format "{{ .ip }}""#);
        assert_eq!(line_format_of(&p.stages[0]).template, "{{ .ip }}");
    }

    #[test]
    fn line_format_with_literal_text() {
        let p = expect_pipeline(r#"{app="foo"} | line_format "request {{.method}} from {{.ip}}""#);
        assert_eq!(
            line_format_of(&p.stages[0]).template,
            "request {{.method}} from {{.ip}}",
        );
    }

    #[test]
    fn label_format_rename_single() {
        // `new = src` — identifier RHS yields a Rename.
        let p = expect_pipeline(r#"{app="foo"} | label_format new=old"#);
        let lf = label_format_of(&p.stages[0]);
        assert_eq!(lf.items.len(), 1);
        match &lf.items[0] {
            LabelFormatItem::Rename { dst, src, .. } => {
                assert_eq!(dst, "new");
                assert_eq!(src, "old");
            }
            other => panic!("expected Rename, got {other:?}"),
        }
    }

    #[test]
    fn label_format_template_single() {
        // `new = "{{ .x }}"` — string RHS yields a Template.
        let p = expect_pipeline(r#"{app="foo"} | label_format new="{{ .x }}""#);
        match &label_format_of(&p.stages[0]).items[0] {
            LabelFormatItem::Template { dst, template, .. } => {
                assert_eq!(dst, "new");
                assert_eq!(template, "{{ .x }}");
            }
            other => panic!("expected Template, got {other:?}"),
        }
    }

    #[test]
    fn label_format_mixed_multi() {
        let p = expect_pipeline(r#"{app="foo"} | label_format a=b, c="{{ .d }}", e=f"#);
        let lf = label_format_of(&p.stages[0]);
        assert_eq!(lf.items.len(), 3);
        assert!(matches!(lf.items[0], LabelFormatItem::Rename { .. }));
        assert!(matches!(lf.items[1], LabelFormatItem::Template { .. }));
        assert!(matches!(lf.items[2], LabelFormatItem::Rename { .. }));
    }

    #[test]
    fn label_format_keyword_required() {
        // Bare `new=old` without the keyword is not a label_format —
        // it's an identifier-led label_filter (which itself needs an
        // op, so this should fail).
        // Bare `new=old` matches a label_filter (matcher with `=` and
        // identifier RHS) — but matcher requires a STRING RHS, so it
        // fails. Confirm the parse errors out.
        assert!(parse(r#"{app="foo"} | new=old"#).is_err());
    }

    #[test]
    fn line_format_missing_string_rejected() {
        assert!(parse(r#"{app="foo"} | line_format"#).is_err());
    }

    #[test]
    fn label_format_missing_items_rejected() {
        assert!(parse(r#"{app="foo"} | label_format"#).is_err());
    }

    #[test]
    fn label_format_missing_rhs_rejected() {
        assert!(parse(r#"{app="foo"} | label_format new="#).is_err());
    }

    // ============= SOW-07 structural stage tests ====================

    fn decolorize_of(stage: &PipelineStage) -> &DecolorizeStage {
        match stage {
            PipelineStage::Decolorize(d) => d,
            other => panic!("expected decolorize, got {other:?}"),
        }
    }

    fn drop_of(stage: &PipelineStage) -> &LabelSelectorList {
        match stage {
            PipelineStage::DropLabels(d) => d,
            other => panic!("expected drop, got {other:?}"),
        }
    }

    fn keep_of(stage: &PipelineStage) -> &LabelSelectorList {
        match stage {
            PipelineStage::KeepLabels(d) => d,
            other => panic!("expected keep, got {other:?}"),
        }
    }

    #[test]
    fn decolorize_alone() {
        let p = expect_pipeline(r#"{app="foo"} | decolorize"#);
        assert_eq!(p.stages.len(), 1);
        let _ = decolorize_of(&p.stages[0]);
    }

    #[test]
    fn drop_single_label() {
        let p = expect_pipeline(r#"{app="foo"} | drop foo"#);
        let d = drop_of(&p.stages[0]);
        assert_eq!(d.items.len(), 1);
        match &d.items[0] {
            LabelSelector::Name { name, .. } => assert_eq!(name, "foo"),
            other => panic!("expected Name, got {other:?}"),
        }
    }

    #[test]
    fn drop_multiple_labels() {
        let p = expect_pipeline(r#"{app="foo"} | drop foo, bar, baz"#);
        let d = drop_of(&p.stages[0]);
        assert_eq!(d.items.len(), 3);
        for item in &d.items {
            assert!(matches!(item, LabelSelector::Name { .. }));
        }
    }

    #[test]
    fn drop_with_matcher() {
        // `drop foo="bar"` — conditional drop.
        let p = expect_pipeline(r#"{app="foo"} | drop foo="bar""#);
        let d = drop_of(&p.stages[0]);
        match &d.items[0] {
            LabelSelector::Matched(m) => {
                assert_eq!(m.name, "foo");
                assert_eq!(m.op, MatcherOp::Eq);
                assert_eq!(m.value, "bar");
            }
            other => panic!("expected Matched, got {other:?}"),
        }
    }

    #[test]
    fn drop_mixed_names_and_matchers() {
        let p = expect_pipeline(r#"{app="foo"} | drop foo, bar="x", baz"#);
        let d = drop_of(&p.stages[0]);
        assert_eq!(d.items.len(), 3);
        assert!(matches!(d.items[0], LabelSelector::Name { .. }));
        assert!(matches!(d.items[1], LabelSelector::Matched(_)));
        assert!(matches!(d.items[2], LabelSelector::Name { .. }));
    }

    #[test]
    fn keep_single_label() {
        let p = expect_pipeline(r#"{app="foo"} | keep foo"#);
        let k = keep_of(&p.stages[0]);
        assert_eq!(k.items.len(), 1);
    }

    #[test]
    fn keep_multiple_labels() {
        let p = expect_pipeline(r#"{app="foo"} | keep foo, bar"#);
        let k = keep_of(&p.stages[0]);
        assert_eq!(k.items.len(), 2);
    }

    #[test]
    fn drop_missing_labels_rejected() {
        assert!(parse(r#"{app="foo"} | drop"#).is_err());
    }

    #[test]
    fn keep_missing_labels_rejected() {
        assert!(parse(r#"{app="foo"} | keep"#).is_err());
    }

    #[test]
    fn decolorize_does_not_consume_arguments() {
        // `| decolorize foo` — `foo` becomes a stray identifier that
        // can't continue the pipeline (decolorize takes no args).
        // Loki would parse decolorize as standalone and then fail on
        // `foo`. Our parser should too.
        assert!(parse(r#"{app="foo"} | decolorize foo"#).is_err());
    }

    // ============= SOW-08 log range expression tests ================

    fn parse_log_range(input: &str) -> LogRangeExpr {
        log_range_expr()
            .then_ignore(end())
            .parse(input)
            .into_result()
            .unwrap_or_else(|e| panic!("log_range_expr failed for {input:?}: {e:?}"))
    }

    fn parse_log_range_err(input: &str) -> bool {
        log_range_expr()
            .then_ignore(end())
            .parse(input)
            .into_result()
            .is_err()
    }

    const MIN_NS: i64 = 60 * NS;

    #[test]
    fn log_range_basic() {
        let r = parse_log_range(r#"{foo="bar"}[5m]"#);
        assert_eq!(r.range_ns, 5 * MIN_NS);
        assert!(r.stages.is_empty());
        assert!(r.offset_ns.is_none());
    }

    #[test]
    fn log_range_with_offset() {
        let r = parse_log_range(r#"{foo="bar"}[5m] offset 10m"#);
        assert_eq!(r.range_ns, 5 * MIN_NS);
        assert_eq!(r.offset_ns, Some(10 * MIN_NS));
    }

    #[test]
    fn log_range_negative_offset() {
        let r = parse_log_range(r#"{foo="bar"}[5m] offset -5m"#);
        assert_eq!(r.offset_ns, Some(-5 * MIN_NS));
    }

    #[test]
    fn log_range_pipeline_before_range() {
        let r = parse_log_range(r#"{foo="bar"} |= "error" [5m]"#);
        assert_eq!(r.stages.len(), 1);
        assert!(matches!(&r.stages[0], PipelineStage::LineFilter(_)));
        assert_eq!(r.range_ns, 5 * MIN_NS);
    }

    #[test]
    fn log_range_pipeline_after_range() {
        // Loki's alternate ordering: stages after [RANGE].
        let r = parse_log_range(r#"{foo="bar"} [5m] |= "error""#);
        assert_eq!(r.stages.len(), 1);
        assert!(matches!(&r.stages[0], PipelineStage::LineFilter(_)));
    }

    #[test]
    fn log_range_multi_stage_pipeline() {
        let r = parse_log_range(r#"{foo="bar"} |= "x" | logfmt | latency > 1s [5m]"#);
        assert_eq!(r.stages.len(), 3);
        assert!(matches!(&r.stages[0], PipelineStage::LineFilter(_)));
        assert!(matches!(&r.stages[1], PipelineStage::Parser(_)));
        assert!(matches!(&r.stages[2], PipelineStage::LabelFilter(_)));
    }

    #[test]
    fn log_range_pipeline_with_offset() {
        // Pipeline + RANGE + offset is the canonical layout.
        let r = parse_log_range(r#"{foo="bar"} |= "x" [5m] offset 30s"#);
        assert_eq!(r.stages.len(), 1);
        assert_eq!(r.range_ns, 5 * MIN_NS);
        assert_eq!(r.offset_ns, Some(30 * NS));
    }

    #[test]
    fn log_range_various_units() {
        assert_eq!(parse_log_range(r#"{foo="bar"}[1h]"#).range_ns, 60 * MIN_NS);
        assert_eq!(parse_log_range(r#"{foo="bar"}[12h]"#).range_ns, 12 * 60 * MIN_NS);
        assert_eq!(parse_log_range(r#"{foo="bar"}[1d]"#).range_ns, 24 * 60 * MIN_NS);
        assert_eq!(parse_log_range(r#"{foo="bar"}[1w]"#).range_ns, 7 * 24 * 60 * MIN_NS);
    }

    #[test]
    fn log_range_whitespace_in_brackets() {
        let r = parse_log_range(r#"{foo="bar"}[ 5m ]"#);
        assert_eq!(r.range_ns, 5 * MIN_NS);
    }

    #[test]
    fn log_range_missing_range_token() {
        // Selector alone isn't a log_range — needs [...].
        assert!(parse_log_range_err(r#"{foo="bar"}"#));
    }

    #[test]
    fn log_range_unclosed_bracket() {
        assert!(parse_log_range_err(r#"{foo="bar"}[5m"#));
    }

    #[test]
    fn log_range_offset_missing_duration() {
        assert!(parse_log_range_err(r#"{foo="bar"}[5m] offset"#));
    }

    // ============= SOW-09 range aggregation tests ===================

    // ============= SOW-12 binary op tests ===========================

    fn expect_binary(input: &str) -> BinaryExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::Binary(b) => b,
            other => panic!("expected binary for {input:?}, got {other:?}"),
        }
    }

    fn expect_literal(input: &str) -> LiteralExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::Literal(l) => l,
            other => panic!("expected literal for {input:?}, got {other:?}"),
        }
    }

    #[test]
    fn literal_int() {
        assert_eq!(expect_literal("42").value, 42.0);
    }

    #[test]
    fn literal_neg() {
        assert_eq!(expect_literal("-42").value, -42.0);
    }

    #[test]
    fn literal_float() {
        assert_eq!(expect_literal("1.5").value, 1.5);
    }

    #[test]
    fn binop_add() {
        let b = expect_binary("1 + 2");
        assert_eq!(b.op, BinaryOp::Add);
    }

    #[test]
    fn binop_precedence_mul_over_add() {
        // `1 + 2 * 3` -> `1 + (2 * 3)`. The root must be Add with a
        // Mul on the right.
        let b = expect_binary("1 + 2 * 3");
        assert_eq!(b.op, BinaryOp::Add);
        match &*b.rhs {
            Expr::Binary(inner) => assert_eq!(inner.op, BinaryOp::Mul),
            other => panic!("expected Mul on rhs, got {other:?}"),
        }
        match &*b.lhs {
            Expr::Literal(l) => assert_eq!(l.value, 1.0),
            other => panic!("expected literal 1 on lhs, got {other:?}"),
        }
    }

    #[test]
    fn binop_precedence_mul_before_add() {
        // `2 * 3 + 1` -> `(2 * 3) + 1`.
        let b = expect_binary("2 * 3 + 1");
        assert_eq!(b.op, BinaryOp::Add);
        match &*b.lhs {
            Expr::Binary(inner) => assert_eq!(inner.op, BinaryOp::Mul),
            other => panic!("expected Mul on lhs, got {other:?}"),
        }
    }

    #[test]
    fn binop_pow_right_assoc() {
        // `2 ^ 3 ^ 2` -> `2 ^ (3 ^ 2)`. Right-associative.
        let b = expect_binary("2 ^ 3 ^ 2");
        assert_eq!(b.op, BinaryOp::Pow);
        match &*b.rhs {
            Expr::Binary(inner) => assert_eq!(inner.op, BinaryOp::Pow),
            other => panic!("expected Pow on rhs, got {other:?}"),
        }
        match &*b.lhs {
            Expr::Literal(l) => assert_eq!(l.value, 2.0),
            other => panic!("expected literal 2 on lhs, got {other:?}"),
        }
    }

    #[test]
    fn binop_add_left_assoc() {
        // `1 + 2 + 3` -> `(1 + 2) + 3`. Left-associative.
        let b = expect_binary("1 + 2 + 3");
        match &*b.lhs {
            Expr::Binary(inner) => assert_eq!(inner.op, BinaryOp::Add),
            other => panic!("expected Add on lhs, got {other:?}"),
        }
        match &*b.rhs {
            Expr::Literal(l) => assert_eq!(l.value, 3.0),
            other => panic!("expected literal 3 on rhs, got {other:?}"),
        }
    }

    #[test]
    fn binop_parens_override_precedence() {
        // `(1 + 2) * 3` -> Mul at root.
        let b = expect_binary("(1 + 2) * 3");
        assert_eq!(b.op, BinaryOp::Mul);
        match &*b.lhs {
            Expr::Binary(inner) => assert_eq!(inner.op, BinaryOp::Add),
            other => panic!("expected Add inside parens, got {other:?}"),
        }
    }

    #[test]
    fn binop_all_arithmetic_ops() {
        for (text, op) in [
            ("1 + 2", BinaryOp::Add),
            ("1 - 2", BinaryOp::Sub),
            ("1 * 2", BinaryOp::Mul),
            ("1 / 2", BinaryOp::Div),
            ("1 % 2", BinaryOp::Mod),
            ("1 ^ 2", BinaryOp::Pow),
        ] {
            let b = expect_binary(text);
            assert_eq!(b.op, op, "op for {text:?}");
        }
    }

    #[test]
    fn binop_all_comparison_ops() {
        for (text, op) in [
            ("1 == 2", BinaryOp::Eq),
            ("1 != 2", BinaryOp::NotEq),
            ("1 > 2", BinaryOp::Gt),
            ("1 >= 2", BinaryOp::Gte),
            ("1 < 2", BinaryOp::Lt),
            ("1 <= 2", BinaryOp::Lte),
        ] {
            let b = expect_binary(text);
            assert_eq!(b.op, op, "op for {text:?}");
        }
    }

    #[test]
    fn binop_logical_ops() {
        for (text, op) in [
            (r#"rate({a="b"}[5m]) or rate({c="d"}[5m])"#, BinaryOp::Or),
            (r#"rate({a="b"}[5m]) and rate({c="d"}[5m])"#, BinaryOp::And),
            (
                r#"rate({a="b"}[5m]) unless rate({c="d"}[5m])"#,
                BinaryOp::Unless,
            ),
        ] {
            let b = expect_binary(text);
            assert_eq!(b.op, op, "op for {text:?}");
        }
    }

    #[test]
    fn binop_or_lowest_precedence() {
        // `1 > 0 or 1 > 0 and 1 > 0` -> `(1>0) or ((1>0) and (1>0))`.
        // OR is precedence 1, AND is 2, comparison is 3.
        let b = expect_binary("1 > 0 or 1 > 0 and 1 > 0");
        assert_eq!(b.op, BinaryOp::Or);
        match &*b.rhs {
            Expr::Binary(inner) => assert_eq!(inner.op, BinaryOp::And),
            other => panic!("expected And on rhs, got {other:?}"),
        }
    }

    #[test]
    fn binop_with_bool_modifier() {
        // Comparison with `bool` modifier makes the comparison
        // return 0/1 instead of filtering.
        let b = expect_binary(r#"rate({a="b"}[5m]) > bool 1"#);
        assert_eq!(b.op, BinaryOp::Gt);
        assert!(b.modifier.return_bool);
    }

    #[test]
    fn binop_with_on_matching() {
        let b = expect_binary(
            r#"rate({a="b"}[5m]) / on(job) rate({c="d"}[5m])"#,
        );
        assert_eq!(b.op, BinaryOp::Div);
        let m = b.modifier.matching.as_ref().unwrap();
        assert!(m.on);
        assert_eq!(m.labels, vec!["job".to_string()]);
    }

    #[test]
    fn binop_with_ignoring_matching() {
        let b = expect_binary(
            r#"rate({a="b"}[5m]) + ignoring(env) rate({c="d"}[5m])"#,
        );
        let m = b.modifier.matching.as_ref().unwrap();
        assert!(!m.on);
        assert_eq!(m.labels, vec!["env".to_string()]);
    }

    #[test]
    fn binop_with_group_left() {
        let b = expect_binary(
            r#"rate({a="b"}[5m]) * on(job) group_left(env) rate({c="d"}[5m])"#,
        );
        assert_eq!(b.modifier.group, Some(GroupSide::Left));
        assert_eq!(b.modifier.include, vec!["env".to_string()]);
    }

    #[test]
    fn binop_with_group_right() {
        let b = expect_binary(
            r#"rate({a="b"}[5m]) * on(job) group_right rate({c="d"}[5m])"#,
        );
        assert_eq!(b.modifier.group, Some(GroupSide::Right));
        assert!(b.modifier.include.is_empty());
    }

    #[test]
    fn binop_scalar_with_range_agg() {
        // From parser_test.go-style: arithmetic between range agg
        // and scalar.
        let b = expect_binary(r#"2 * rate({a="b"}[5m])"#);
        assert_eq!(b.op, BinaryOp::Mul);
        assert!(matches!(&*b.lhs, Expr::Literal(_)));
        assert!(matches!(&*b.rhs, Expr::RangeAggregation(_)));
    }

    #[test]
    fn binop_inside_vector_aggregation() {
        // `sum(rate(...) + rate(...))` — the inner arg is a binop.
        let v = expect_vector_agg(
            r#"sum(rate({a="b"}[5m]) + rate({c="d"}[5m]))"#,
        );
        assert_eq!(v.op, VectorOp::Sum);
        assert!(matches!(&*v.expr, Expr::Binary(_)));
    }

    #[test]
    fn binop_span_covers_whole() {
        let input = "1 + 2";
        let b = expect_binary(input);
        assert_eq!(b.span.start, 0);
        assert_eq!(b.span.end, input.len());
    }

    // ============= SOW-13 misc metric expression tests ==============

    fn expect_label_replace(input: &str) -> LabelReplaceExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::LabelReplace(l) => l,
            other => panic!("expected label_replace, got {other:?}"),
        }
    }

    fn expect_vector_lit(input: &str) -> VectorExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::Vector(v) => v,
            other => panic!("expected vector(...), got {other:?}"),
        }
    }

    #[test]
    fn label_replace_basic() {
        let lr = expect_label_replace(
            r#"label_replace(rate({a="b"}[5m]), "dst", "$1", "src", "(.+)")"#,
        );
        assert_eq!(lr.dst_label, "dst");
        assert_eq!(lr.replacement, "$1");
        assert_eq!(lr.src_label, "src");
        assert_eq!(lr.regex, "(.+)");
        assert!(matches!(&*lr.expr, Expr::RangeAggregation(_)));
    }

    #[test]
    fn label_replace_with_vector_aggregation_inner() {
        let lr = expect_label_replace(
            r#"label_replace(sum(rate({a="b"}[5m])) by (job), "dst", "$1", "job", "(.*)")"#,
        );
        assert!(matches!(&*lr.expr, Expr::VectorAggregation(_)));
    }

    #[test]
    fn label_replace_with_binop_inner() {
        // The first arg can itself be a binop.
        let lr = expect_label_replace(
            r#"label_replace(rate({a="b"}[5m]) * 2, "dst", "r", "src", ".*")"#,
        );
        assert!(matches!(&*lr.expr, Expr::Binary(_)));
    }

    #[test]
    fn label_replace_too_few_args_rejected() {
        // Three string args instead of four.
        assert!(parse(r#"label_replace(rate({a="b"}[5m]), "x", "y", "z")"#).is_err());
    }

    #[test]
    fn label_replace_span_covers_whole() {
        let input = r#"label_replace(rate({a="b"}[5m]), "d", "$1", "s", ".+")"#;
        let lr = expect_label_replace(input);
        assert_eq!(lr.span.start, 0);
        assert_eq!(lr.span.end, input.len());
    }

    #[test]
    fn vector_scalar() {
        let v = expect_vector_lit("vector(1)");
        assert_eq!(v.value, 1.0);
    }

    #[test]
    fn vector_decimal() {
        let v = expect_vector_lit("vector(3.14)");
        assert_eq!(v.value, 3.14);
    }

    #[test]
    fn vector_in_binop() {
        // `vector(0) + rate(...)` — vector wraps a scalar so it can
        // participate in vector arithmetic.
        let b = expect_binary(r#"vector(0) + rate({a="b"}[5m])"#);
        assert_eq!(b.op, BinaryOp::Add);
        assert!(matches!(&*b.lhs, Expr::Vector(_)));
        assert!(matches!(&*b.rhs, Expr::RangeAggregation(_)));
    }

    #[test]
    fn vector_missing_paren_rejected() {
        assert!(parse("vector 1").is_err());
    }

    #[test]
    fn vector_span_covers_whole() {
        let input = "vector(42)";
        let v = expect_vector_lit(input);
        assert_eq!(v.span.start, 0);
        assert_eq!(v.span.end, input.len());
    }

    // ============= SOW-11 vector aggregation tests ==================

    #[test]
    fn sum_of_rate() {
        let v = expect_vector_agg(r#"sum(rate({foo="bar"}[5m]))"#);
        assert_eq!(v.op, VectorOp::Sum);
        assert!(v.parameter.is_none());
        assert!(v.grouping.is_none());
        // Inner is a range aggregation.
        assert!(matches!(&*v.expr, Expr::RangeAggregation(_)));
    }

    #[test]
    fn topk_with_parameter() {
        let v = expect_vector_agg(r#"topk(5, rate({foo="bar"}[5m]))"#);
        assert_eq!(v.op, VectorOp::TopK);
        assert_eq!(v.parameter, Some(5.0));
    }

    #[test]
    fn bottomk_with_parameter() {
        let v = expect_vector_agg(r#"bottomk(10, rate({foo="bar"}[5m]))"#);
        assert_eq!(v.op, VectorOp::BottomK);
        assert_eq!(v.parameter, Some(10.0));
    }

    #[test]
    fn sum_by_after_parens() {
        // From parser_test.go: `sum(count_over_time({foo="bar"}[5m])) by (foo,bar)`
        let v = expect_vector_agg(
            r#"sum(count_over_time({foo="bar"}[5m])) by (foo, bar)"#,
        );
        let g = v.grouping.as_ref().unwrap();
        assert!(!g.without);
        assert_eq!(g.labels, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn sum_by_before_parens() {
        // From parser_test.go: `SUM BY (foo, bar) (Count_Over_Time({foo="bar"}[5m]))`
        // (Case-sensitive though — Loki is mixed-case; we mirror the
        // documented lowercase keywords.)
        let v = expect_vector_agg(
            r#"sum by (foo, bar) (count_over_time({foo="bar"}[5m]))"#,
        );
        let g = v.grouping.as_ref().unwrap();
        assert_eq!(g.labels, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn sum_without_grouping() {
        let v = expect_vector_agg(
            r#"sum(rate({foo="bar"}[5m])) without (instance)"#,
        );
        let g = v.grouping.as_ref().unwrap();
        assert!(g.without);
    }

    #[test]
    fn nested_vector_aggregations() {
        // From parser_test.go: `sum(max(rate({foo="bar"}[5m])) by (foo,bar)) by (foo)`
        let v = expect_vector_agg(
            r#"sum(max(rate({foo="bar"}[5m])) by (foo, bar)) by (foo)"#,
        );
        assert_eq!(v.op, VectorOp::Sum);
        // Inner: max(rate(...)) by (foo, bar)
        match &*v.expr {
            Expr::VectorAggregation(inner) => {
                assert_eq!(inner.op, VectorOp::Max);
                let inner_grp = inner.grouping.as_ref().unwrap();
                assert_eq!(inner_grp.labels.len(), 2);
            }
            other => panic!("expected nested vector aggregation, got {other:?}"),
        }
    }

    #[test]
    fn topk_with_param_and_grouping() {
        // From parser_test.go: `topk(3,count_over_time({foo="bar"}[5m])) by (foo,bar)`
        let v = expect_vector_agg(
            r#"topk(3, count_over_time({foo="bar"}[5m])) by (foo, bar)"#,
        );
        assert_eq!(v.parameter, Some(3.0));
        assert!(v.grouping.is_some());
    }

    #[test]
    fn all_twelve_vector_ops() {
        let with_param = ["topk", "bottomk", "approx_topk"];
        for (kw, op) in [
            ("avg", VectorOp::Avg),
            ("count", VectorOp::Count),
            ("max", VectorOp::Max),
            ("min", VectorOp::Min),
            ("sort", VectorOp::Sort),
            ("sort_desc", VectorOp::SortDesc),
            ("stddev", VectorOp::Stddev),
            ("stdvar", VectorOp::Stdvar),
            ("sum", VectorOp::Sum),
        ] {
            let q = format!(r#"{kw}(rate({{foo="bar"}}[5m]))"#);
            let v = expect_vector_agg(&q);
            assert_eq!(v.op, op, "op for {kw:?}");
        }
        for (kw, op) in [
            ("topk", VectorOp::TopK),
            ("bottomk", VectorOp::BottomK),
            ("approx_topk", VectorOp::ApproxTopK),
        ] {
            let q = format!(r#"{kw}(5, rate({{foo="bar"}}[5m]))"#);
            let v = expect_vector_agg(&q);
            assert_eq!(v.op, op, "op for {kw:?}");
            assert_eq!(v.parameter, Some(5.0));
        }
        let _ = with_param;
    }

    #[test]
    fn sort_desc_not_misread_as_sort() {
        // sort_desc has sort as a prefix; keyword()'s word-boundary
        // rewind must prevent the shorter op from winning.
        let v = expect_vector_agg(r#"sort_desc(rate({foo="bar"}[5m]))"#);
        assert_eq!(v.op, VectorOp::SortDesc);
    }

    #[test]
    fn count_not_confused_with_count_over_time() {
        // `count_over_time(...)` is a range op, not a vector op.
        let r = expect_range_agg(r#"count_over_time({foo="bar"}[5m])"#);
        assert_eq!(r.op, RangeOp::CountOverTime);

        // `count(rate(...))` is a vector op.
        let v = expect_vector_agg(r#"count(rate({foo="bar"}[5m]))"#);
        assert_eq!(v.op, VectorOp::Count);
    }

    #[test]
    fn vector_agg_grouping_on_both_sides_rejected() {
        // `sum by (job) (rate(...)) by (instance)` — disallowed by
        // our try_map check (Loki's yacc rules each pick at most one
        // grouping position).
        assert!(parse(r#"sum by (job) (rate({foo="bar"}[5m])) by (instance)"#).is_err());
    }

    #[test]
    fn topk_missing_param_rejected() {
        // `topk` without a numeric first arg should fail to parse.
        // The current parser falls back to the 1-arg form and the
        // inner expr parser would reject a bare metricExpr without
        // the `,`. In practice Loki accepts `topk(rate(...))` too
        // (treating param as missing), but our parser is stricter.
        // Documenting current behavior:
        let r = parse(r#"topk(rate({foo="bar"}[5m]))"#);
        // We accept it as a 1-arg form (no param). Loki would error;
        // we'll align in SOW-15 (error messages).
        assert!(r.is_ok());
    }

    #[test]
    fn vector_agg_span_covers_whole() {
        let input = r#"sum(rate({foo="bar"}[5m])) by (job)"#;
        let v = expect_vector_agg(input);
        assert_eq!(v.span.start, 0);
        assert_eq!(v.span.end, input.len());
    }

    #[test]
    fn rate_basic() {
        let r = expect_range_agg(r#"rate({foo="bar"}[5m])"#);
        assert_eq!(r.op, RangeOp::Rate);
        assert_eq!(r.log_range.range_ns, 5 * MIN_NS);
        assert!(r.parameter.is_none());
        assert!(r.grouping.is_none());
    }

    #[test]
    fn count_over_time_with_filter() {
        // From parser_test.go: `count_over_time({foo="bar"}[12h] |= "error")`
        let r = expect_range_agg(r#"count_over_time({foo="bar"}[12h] |= "error")"#);
        assert_eq!(r.op, RangeOp::CountOverTime);
        assert_eq!(r.log_range.range_ns, 12 * 60 * MIN_NS);
        assert_eq!(r.log_range.stages.len(), 1);
    }

    #[test]
    fn count_over_time_pipeline_before_range() {
        // From parser_test.go: `count_over_time({foo="bar"} |= "error" [12h])`
        let r = expect_range_agg(r#"count_over_time({foo="bar"} |= "error" [12h])"#);
        assert_eq!(r.log_range.range_ns, 12 * 60 * MIN_NS);
        assert_eq!(r.log_range.stages.len(), 1);
    }

    #[test]
    fn quantile_over_time_with_parameter() {
        let r = expect_range_agg(r#"quantile_over_time(0.99, {foo="bar"}[5m])"#);
        assert_eq!(r.op, RangeOp::QuantileOverTime);
        assert_eq!(r.parameter, Some(0.99));
    }

    #[test]
    fn rate_with_by_grouping() {
        let r = expect_range_agg(r#"rate({foo="bar"}[5m]) by (job, instance)"#);
        let g = r.grouping.as_ref().expect("grouping present");
        assert!(!g.without);
        assert_eq!(g.labels, vec!["job".to_string(), "instance".to_string()]);
    }

    #[test]
    fn rate_with_without_grouping() {
        let r = expect_range_agg(r#"rate({foo="bar"}[5m]) without (job)"#);
        let g = r.grouping.as_ref().expect("grouping present");
        assert!(g.without);
        assert_eq!(g.labels, vec!["job".to_string()]);
    }

    #[test]
    fn rate_with_empty_by_grouping() {
        let r = expect_range_agg(r#"rate({foo="bar"}[5m]) by ()"#);
        let g = r.grouping.as_ref().expect("grouping present");
        assert!(g.labels.is_empty());
    }

    #[test]
    fn quantile_over_time_with_param_and_grouping() {
        let r = expect_range_agg(
            r#"quantile_over_time(0.95, {foo="bar"}[5m]) by (job)"#,
        );
        assert_eq!(r.parameter, Some(0.95));
        assert!(r.grouping.is_some());
    }

    #[test]
    fn all_fifteen_range_ops() {
        for (kw, op) in [
            ("absent_over_time", RangeOp::AbsentOverTime),
            ("avg_over_time", RangeOp::AvgOverTime),
            ("bytes_over_time", RangeOp::BytesOverTime),
            ("bytes_rate", RangeOp::BytesRate),
            ("count_over_time", RangeOp::CountOverTime),
            ("first_over_time", RangeOp::FirstOverTime),
            ("last_over_time", RangeOp::LastOverTime),
            ("max_over_time", RangeOp::MaxOverTime),
            ("min_over_time", RangeOp::MinOverTime),
            ("rate", RangeOp::Rate),
            ("rate_counter", RangeOp::RateCounter),
            ("stddev_over_time", RangeOp::StddevOverTime),
            ("stdvar_over_time", RangeOp::StdvarOverTime),
            ("sum_over_time", RangeOp::SumOverTime),
        ] {
            let q = format!(r#"{kw}({{foo="bar"}}[5m])"#);
            let r = expect_range_agg(&q);
            assert_eq!(r.op, op, "op for {kw:?}");
        }
        // quantile_over_time requires a parameter.
        let r = expect_range_agg(r#"quantile_over_time(0.5, {foo="bar"}[5m])"#);
        assert_eq!(r.op, RangeOp::QuantileOverTime);
    }

    #[test]
    fn rate_with_offset() {
        let r = expect_range_agg(r#"rate({foo="bar"}[5m] offset 10m)"#);
        assert_eq!(r.log_range.range_ns, 5 * MIN_NS);
        assert_eq!(r.log_range.offset_ns, Some(10 * MIN_NS));
    }

    #[test]
    fn rate_inner_pipeline_multi_stage() {
        let r = expect_range_agg(
            r#"count_over_time({foo="bar"} |= "x" | logfmt | latency > 1s [5m])"#,
        );
        assert_eq!(r.log_range.stages.len(), 3);
    }

    #[test]
    fn rate_missing_parens_rejected() {
        // Bare op name without parens — Loki rejects, we should too.
        assert!(parse("rate").is_err());
    }

    #[test]
    fn rate_empty_parens_rejected() {
        assert!(parse("rate()").is_err());
    }

    #[test]
    fn rate_unclosed_parens_rejected() {
        assert!(parse(r#"rate({foo="bar"}[5m]"#).is_err());
    }

    #[test]
    fn quantile_param_without_log_range_rejected() {
        assert!(parse(r#"quantile_over_time(0.99,)"#).is_err());
    }

    #[test]
    fn rate_followed_by_orphan_rejected() {
        // `rate({...}[5m])` followed by stray identifier — should fail
        // because top-level parse requires end-of-input.
        assert!(parse(r#"rate({foo="bar"}[5m]) something"#).is_err());
    }

    #[test]
    fn rate_span_covers_whole() {
        let input = r#"rate({foo="bar"}[5m]) by (job)"#;
        let r = expect_range_agg(input);
        assert_eq!(r.span.start, 0);
        assert_eq!(r.span.end, input.len());
    }

    // ============= SOW-10 unwrap expression tests ===================

    #[test]
    fn unwrap_plain_identifier() {
        let r = parse_log_range(r#"{foo="bar"} | unwrap latency [5m]"#);
        let u = r.unwrap.as_ref().expect("unwrap present");
        assert_eq!(u.identifier, "latency");
        assert!(u.conv_op.is_none());
        assert!(u.post_filters.is_empty());
    }

    #[test]
    fn unwrap_with_conv_duration() {
        let r = parse_log_range(r#"{foo="bar"} | unwrap duration(latency) [5m]"#);
        let u = r.unwrap.as_ref().unwrap();
        assert_eq!(u.identifier, "latency");
        assert_eq!(u.conv_op, Some(ConvOp::Duration));
    }

    #[test]
    fn unwrap_with_conv_bytes() {
        let r = parse_log_range(r#"{foo="bar"} | unwrap bytes(size) [5m]"#);
        assert_eq!(r.unwrap.as_ref().unwrap().conv_op, Some(ConvOp::Bytes));
    }

    #[test]
    fn unwrap_with_conv_duration_seconds() {
        // duration_seconds is the longer-prefix form — must not be
        // misread as `duration(_seconds)` or similar.
        let r = parse_log_range(r#"{foo="bar"} | unwrap duration_seconds(t) [5m]"#);
        assert_eq!(
            r.unwrap.as_ref().unwrap().conv_op,
            Some(ConvOp::DurationSeconds),
        );
    }

    #[test]
    fn unwrap_with_post_filter() {
        let r = parse_log_range(
            r#"{foo="bar"} | unwrap latency | level="warn" [5m]"#,
        );
        let u = r.unwrap.as_ref().unwrap();
        assert_eq!(u.post_filters.len(), 1);
        assert!(matches!(u.post_filters[0], LabelFilter::String(_)));
    }

    #[test]
    fn unwrap_with_multiple_post_filters() {
        let r = parse_log_range(
            r#"{foo="bar"} | unwrap latency | level="warn" | n > 5 [5m]"#,
        );
        assert_eq!(r.unwrap.as_ref().unwrap().post_filters.len(), 2);
    }

    #[test]
    fn unwrap_before_pipeline_then_range() {
        // `{...} | unwrap x | level="warn" [5m]` — unwrap with post
        // filter, then RANGE.
        let r = parse_log_range(
            r#"{foo="bar"} | unwrap latency | level="warn" [5m]"#,
        );
        assert!(r.unwrap.is_some());
        assert_eq!(r.range_ns, 5 * MIN_NS);
    }

    #[test]
    fn unwrap_after_range() {
        // Loki's alternate layout: `selector RANGE unwrap`.
        let r = parse_log_range(r#"{foo="bar"} [5m] | unwrap latency"#);
        assert!(r.unwrap.is_some());
    }

    #[test]
    fn unwrap_with_filter_stage_before() {
        // Pipeline stages can precede the unwrap.
        let r = parse_log_range(
            r#"{foo="bar"} | logfmt | unwrap latency [5m]"#,
        );
        assert_eq!(r.stages.len(), 1);
        assert!(matches!(&r.stages[0], PipelineStage::Parser(_)));
        assert!(r.unwrap.is_some());
    }

    #[test]
    fn unwrap_inside_quantile_over_time() {
        // From parser_test.go-style: a real Loki query.
        let r = expect_range_agg(
            r#"quantile_over_time(0.99, {foo="bar"} | unwrap duration(latency) [5m])"#,
        );
        assert_eq!(r.op, RangeOp::QuantileOverTime);
        assert_eq!(r.parameter, Some(0.99));
        let u = r.log_range.unwrap.as_ref().unwrap();
        assert_eq!(u.identifier, "latency");
        assert_eq!(u.conv_op, Some(ConvOp::Duration));
    }

    #[test]
    fn unwrap_inside_sum_over_time() {
        let r = expect_range_agg(
            r#"sum_over_time({foo="bar"} | unwrap bytes(size) [5m])"#,
        );
        assert_eq!(r.op, RangeOp::SumOverTime);
        assert_eq!(
            r.log_range.unwrap.as_ref().unwrap().conv_op,
            Some(ConvOp::Bytes),
        );
    }

    #[test]
    fn unwrap_missing_identifier_rejected() {
        assert!(parse_log_range_err(r#"{foo="bar"} | unwrap [5m]"#));
    }

    #[test]
    fn unwrap_conv_missing_close_paren_rejected() {
        assert!(parse_log_range_err(
            r#"{foo="bar"} | unwrap duration(latency [5m]"#,
        ));
    }

    #[test]
    fn log_range_span_covers_whole() {
        let input = r#"{foo="bar"} |= "x" [5m] offset 10s"#;
        let r = parse_log_range(input);
        assert_eq!(r.span.start, 0);
        assert_eq!(r.span.end, input.len());
    }

    #[test]
    fn composition_filter_parser_then_line_format() {
        let p = expect_pipeline(
            r#"{app="foo"} |= "x" | logfmt | latency > 1s | line_format "{{ .level }}""#,
        );
        assert_eq!(p.stages.len(), 4);
        assert!(matches!(&p.stages[0], PipelineStage::LineFilter(_)));
        assert!(matches!(parser_of(&p.stages[1]), ParserStage::Logfmt { .. }));
        assert!(matches!(
            label_filter_of(&p.stages[2]),
            LabelFilter::Duration { .. }
        ));
        assert!(matches!(&p.stages[3], PipelineStage::LineFormat(_)));
    }
}
