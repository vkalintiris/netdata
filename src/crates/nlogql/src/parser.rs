//! Query string → AST.
//!
//! Built with [`chumsky`] combinators. Grammar productions are
//! added incrementally per the implementation plan in
//! `src/crates/docs/nlogql-implementation-plan.md`.

use chumsky::error::Rich;
use chumsky::prelude::*;

use crate::Extra;
use crate::ast::{
    DecolorizeStage, Expr, IpFilterOp, LabelExtraction, LabelFilter, LabelFormatItem,
    LabelFormatStage, LabelSelector, LabelSelectorList, LineFilter, LineFilterOp, LineFilterValue,
    LineFormatStage, LogRangeExpr, Matcher, MatcherOp, NumericOp, ParserFlag, ParserStage,
    PipelineExpr, PipelineStage, StreamSelector,
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
    ws().ignore_then(log_expr())
        .then_ignore(ws())
        .then_ignore(end())
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

// -- Log range expression (SOW-08) ---------------------------------

/// `logRangeExpr` (syntax.y:128). Argument to range aggregations.
///
/// Accepts the canonical layout — `selector pipeline? RANGE offset?` —
/// and also Loki's alternate layout where the pipeline trails the
/// RANGE/offset. Stages on both sides are merged in source order.
/// Unwrap (SOW-10) is intentionally not handled yet.
///
/// `#[allow(dead_code)]` until SOW-09 wires range aggregations.
#[allow(dead_code)]
pub(crate) fn log_range_expr<'a>() -> impl Parser<'a, &'a str, LogRangeExpr, Extra<'a>> + Clone {
    let pre_stages = ws()
        .ignore_then(pipeline_stage())
        .repeated()
        .collect::<Vec<_>>();
    let post_stages = ws()
        .ignore_then(pipeline_stage())
        .repeated()
        .collect::<Vec<_>>();

    selector()
        .then(pre_stages)
        .then_ignore(ws())
        .then(range_token())
        .then(ws().ignore_then(offset_expr()).or_not())
        .then(post_stages)
        .map_with(|((((sel, pre), range_ns), offset_ns), post), e| {
            let mut stages = pre;
            stages.extend(post);
            LogRangeExpr {
                selector: sel,
                stages,
                range_ns,
                offset_ns,
                span: e.span().into(),
            }
        })
}

/// `RANGE` token: `[<duration>]`. In Loki this is one lexer token;
/// here we parse the brackets explicitly with internal whitespace
/// allowed for readability.
#[allow(dead_code)]
fn range_token<'a>() -> impl Parser<'a, &'a str, i64, Extra<'a>> + Clone {
    just('[')
        .ignore_then(ws())
        .ignore_then(duration())
        .then_ignore(ws())
        .then_ignore(just(']'))
}

/// `offsetExpr` (syntax.y:510): `offset <duration>`. The duration
/// may be negative (Loki returns DURATION with a negative value).
#[allow(dead_code)]
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
            Expr::Pipeline(_) => panic!("expected bare selector for {input:?}"),
        }
    }

    fn expect_pipeline(input: &str) -> PipelineExpr {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::Pipeline(p) => p,
            Expr::Selector(_) => panic!("expected pipeline for {input:?}"),
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
