//! Parse error type.

use crate::span::Span;

/// A LogQL parse error.
///
/// Always carries a byte-range [`Span`] into the original input, the
/// 1-based line/column where the failure was located, and a
/// [`ParseErrorKind`] describing the failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    /// 1-based line number where the error starts.
    pub line: usize,
    /// 1-based column (character count, not bytes).
    pub col: usize,
    pub kind: ParseErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The parser was looking for one of `expected` but found
    /// something else. `found` is `None` at end of input.
    Expected {
        /// Free-form descriptions of what could have appeared here.
        /// May be empty if chumsky didn't surface a useful label.
        expected: Vec<String>,
        /// What was actually found, formatted for display
        /// (e.g. `'*'`, `"foo"`, etc.). `None` means EOF.
        found: Option<String>,
    },
    /// A custom failure, typically from a `try_map` guard inside
    /// the parser (e.g. "vector aggregation cannot have grouping
    /// on both sides").
    Custom(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parse error at line {}, col {}: ", self.line, self.col)?;
        match &self.kind {
            ParseErrorKind::Expected { expected, found } => {
                match found {
                    Some(c) => write!(f, "unexpected {c}")?,
                    None => write!(f, "unexpected end of input")?,
                }
                if !expected.is_empty() {
                    write!(f, ", expected ")?;
                    if expected.len() == 1 {
                        write!(f, "{}", expected[0])?;
                    } else {
                        write!(f, "one of: ")?;
                        for (i, e) in expected.iter().enumerate() {
                            if i > 0 {
                                write!(f, ", ")?;
                            }
                            write!(f, "{e}")?;
                        }
                    }
                }
                Ok(())
            }
            ParseErrorKind::Custom(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Convert a 0-based byte offset into 1-based (line, col).
///
/// `col` counts Unicode scalar values (chars), not bytes, so a `é`
/// counts as one column regardless of UTF-8 byte width.
pub(crate) fn line_col(input: &str, byte_offset: usize) -> (usize, usize) {
    let offset = byte_offset.min(input.len());
    let prefix = &input[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
    let last_line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = prefix[last_line_start..].chars().count() + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_first_line() {
        assert_eq!(line_col("abc", 0), (1, 1));
        assert_eq!(line_col("abc", 1), (1, 2));
        assert_eq!(line_col("abc", 3), (1, 4));
    }

    #[test]
    fn line_col_after_newline() {
        assert_eq!(line_col("abc\ndef", 4), (2, 1));
        assert_eq!(line_col("abc\ndef", 7), (2, 4));
    }

    #[test]
    fn line_col_multibyte_chars() {
        // `é` is two bytes in UTF-8 but one column.
        let s = "héllo";
        assert_eq!(line_col(s, s.len()), (1, 6));
    }

    #[test]
    fn line_col_out_of_bounds() {
        // Clamp to len.
        assert_eq!(line_col("abc", 100), (1, 4));
    }
}
