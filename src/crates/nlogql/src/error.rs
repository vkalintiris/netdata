//! Parse error type.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub kind: ParseErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// Generic "couldn't match" — populated with what the parser
    /// was expecting at that point.
    Expected(&'static str),
    /// Input ended before the parser was done.
    UnexpectedEof,
    /// Parser succeeded but did not consume the full input.
    TrailingInput,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ParseErrorKind::Expected(what) => {
                write!(f, "parse error at byte {}: expected {what}", self.span.start)
            }
            ParseErrorKind::UnexpectedEof => {
                write!(f, "parse error at byte {}: unexpected end of input", self.span.start)
            }
            ParseErrorKind::TrailingInput => {
                write!(f, "parse error at byte {}: unexpected trailing input", self.span.start)
            }
        }
    }
}

impl std::error::Error for ParseError {}
