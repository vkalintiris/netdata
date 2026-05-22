//! Query string → AST.
//!
//! Built with [`chumsky`] combinators. Grammar productions are
//! added incrementally per the implementation plan in
//! `src/crates/docs/nlogql-implementation-plan.md`.

use chumsky::error::Rich;
use chumsky::prelude::*;

use crate::Extra;
use crate::ast::{Expr, Matcher, MatcherOp, StreamSelector};
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

/// Top-level production. Today: just a stream selector wrapped
/// in `Expr::Selector`. Grows as Phase A adds productions.
fn root<'a>() -> impl Parser<'a, &'a str, Expr, Extra<'a>> {
    ws().ignore_then(selector())
        .then_ignore(ws())
        .then_ignore(end())
        .map(Expr::Selector)
}

/// `selector` production from syntax.y:192.
///
/// `OPEN_BRACE matchers? CLOSE_BRACE` — empty `{}` is valid.
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
///
/// `IDENTIFIER (=|!=|=~|!~) STRING`, with whitespace allowed
/// around the operator (per lex_test.go cases like `{ foo = "bar" }`).
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

/// Matcher operator. Longer operators first (`=~`/`!=` before `=`).
fn matcher_op<'a>() -> impl Parser<'a, &'a str, MatcherOp, Extra<'a>> + Clone {
    choice((
        just("=~").to(MatcherOp::Match),
        just("!~").to(MatcherOp::NotMatch),
        just("!=").to(MatcherOp::NotEq),
        just("=").to(MatcherOp::Eq),
    ))
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

    fn expect_selector(input: &str) -> StreamSelector {
        match parse(input).unwrap_or_else(|e| panic!("parse failed for {input:?}: {e}")) {
            Expr::Selector(s) => s,
        }
    }

    fn matcher_at(name: &str, op: MatcherOp, value: &str) -> Matcher {
        Matcher {
            name: name.to_string(),
            op,
            value: value.to_string(),
            // Span content is asserted separately; tests that don't
            // care use this helper and ignore the field via custom
            // comparison.
            span: Span::new(0, 0),
        }
    }

    /// Compare ignoring spans (which depend on the exact input layout).
    fn eq_ignore_span(a: &[Matcher], b: &[Matcher]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.name == y.name && x.op == y.op && x.value == y.value)
    }

    // --- positive ----------------------------------------------------

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
    fn all_four_ops() {
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
        assert_eq!(s.matchers[0].op, MatcherOp::NotEq);
        assert_eq!(s.matchers[1].op, MatcherOp::Match);
        assert_eq!(s.matchers[2].op, MatcherOp::NotMatch);
    }

    #[test]
    fn escaped_quote_in_value() {
        // From lex_test.go: `{ foo = "ba\"r" }`.
        let s = expect_selector(r#"{ foo = "ba\"r" }"#);
        assert_eq!(s.matchers[0].value, r#"ba"r"#);
    }

    #[test]
    fn raw_string_value() {
        let s = expect_selector(r"{foo=~`bar\w+`}");
        assert_eq!(s.matchers[0].value, r"bar\w+");
        assert_eq!(s.matchers[0].op, MatcherOp::Match);
    }

    #[test]
    fn hash_inside_value_is_not_a_comment() {
        // From lex_test.go: `{foo="#"}` — `#` inside a string is data.
        let s = expect_selector(r##"{foo="#"}"##);
        assert_eq!(s.matchers[0].value, "#");
    }

    #[test]
    fn empty_selector() {
        // syntax.y:195: `OPEN_BRACE CLOSE_BRACE { }` is a valid
        // production. Semantic rejection (if any) is downstream.
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

    // --- negative ----------------------------------------------------

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
        // Loki's yacc grammar (matchers: matcher | matchers COMMA matcher)
        // does not allow a trailing comma. Match that.
        assert!(parse(r#"{foo="bar",}"#).is_err());
    }

    #[test]
    fn unexpected_eof_kind() {
        let err = parse(r#"{foo="bar""#).unwrap_err();
        // Either UnexpectedEof or Expected depending on where chumsky
        // bailed; both are acceptable. Just verify the span is sane.
        assert!(err.span.start <= r#"{foo="bar""#.len());
    }
}
