//! Query string → AST.
//!
//! Built with [`chumsky`] combinators. Grammar productions are
//! added incrementally per the implementation plan in
//! `src/crates/docs/nlogql-implementation-plan.md`.

use chumsky::error::Rich;
use chumsky::prelude::*;

use crate::Extra;
use crate::ast::{
    Expr, LineFilter, LineFilterOp, LineFilterValue, Matcher, MatcherOp, PipelineExpr,
    PipelineStage, StreamSelector,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lex::{identifier, string_literal, ws};
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

/// `pipelineStage` (syntax.y:215). Today: line filters only. Other
/// stages join in SOW-04+.
fn pipeline_stage<'a>() -> impl Parser<'a, &'a str, PipelineStage, Extra<'a>> + Clone {
    line_filter().map(PipelineStage::LineFilter)
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
}
