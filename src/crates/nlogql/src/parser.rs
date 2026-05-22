//! Query string → AST.
//!
//! Built with [`chumsky`] combinators. Grammar productions are
//! added incrementally; this module currently holds only the
//! top-level entry-point and a placeholder root that accepts
//! whitespace-only input.

use chumsky::error::Rich;
use chumsky::prelude::*;

use crate::Extra;
use crate::error::{ParseError, ParseErrorKind};
use crate::span::Span;

/// Parse a LogQL query string into an AST.
///
/// The parser must consume the entire input. Any trailing
/// non-whitespace produces a `TrailingInput` error.
pub fn parse(input: &str) -> Result<(), ParseError> {
    root().parse(input).into_result().map_err(|errs| {
        // chumsky always reports at least one error on failure;
        // we surface the first.
        let first = errs.into_iter().next().expect("chumsky returns >= 1 error on failure");
        convert_error(first)
    })
}

/// Top-level production. Placeholder: accepts whitespace-only
/// input. Replaced as we port `syntax.y` productions.
fn root<'a>() -> impl Parser<'a, &'a str, (), Extra<'a>> {
    text::whitespace().then_ignore(end())
}

fn convert_error(err: Rich<'_, char>) -> ParseError {
    let s = err.span();
    let span = Span::new(s.start, s.end);
    // Until we have rich production-level errors, classify by
    // chumsky's `found()`: `None` means the input ended; anything
    // else is an "expected something else here" failure.
    let kind = if err.found().is_none() {
        ParseErrorKind::UnexpectedEof
    } else {
        ParseErrorKind::Expected("LogQL expression (parser not yet implemented)")
    };
    ParseError { span, kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_parses() {
        assert!(parse("").is_ok());
    }

    #[test]
    fn whitespace_only_parses() {
        assert!(parse("   \t\n  ").is_ok());
    }

    #[test]
    fn non_whitespace_errors() {
        let err = parse("garbage").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::Expected(_)));
        assert_eq!(err.span.start, 0);
    }
}
