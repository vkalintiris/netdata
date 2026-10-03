//! Error codes and their texts (`eval.h`; `eval-utils.c` `expression_strerror()`), and what a failed parse reports
//! (`eval-parser-legacy.c` `expression_parse()`).

use netdata_agent_text::c::c_str;

/// `EVAL_ERROR_OK`.
pub const ERROR_OK: i32 = 0;
/// `EVAL_ERROR_MISSING_CLOSE_SUBEXPRESSION`.
pub const ERROR_MISSING_CLOSE_SUBEXPRESSION: i32 = 1;
/// `EVAL_ERROR_UNKNOWN_OPERAND`.
pub const ERROR_UNKNOWN_OPERAND: i32 = 2;
/// `EVAL_ERROR_MISSING_OPERAND`.
pub const ERROR_MISSING_OPERAND: i32 = 3;
/// `EVAL_ERROR_MISSING_OPERATOR`.
pub const ERROR_MISSING_OPERATOR: i32 = 4;
/// `EVAL_ERROR_REMAINING_GARBAGE`: the code of every failed parse.
pub const ERROR_REMAINING_GARBAGE: i32 = 5;
/// `EVAL_ERROR_IF_THEN_ELSE_MISSING_ELSE`.
pub const ERROR_IF_THEN_ELSE_MISSING_ELSE: i32 = 6;
/// `EVAL_ERROR_INVALID_VALUE`.
pub const ERROR_INVALID_VALUE: i32 = 101;
/// `EVAL_ERROR_INVALID_NUMBER_OF_OPERANDS`.
pub const ERROR_INVALID_NUMBER_OF_OPERANDS: i32 = 102;
/// `EVAL_ERROR_VALUE_IS_NAN`.
pub const ERROR_VALUE_IS_NAN: i32 = 103;
/// `EVAL_ERROR_VALUE_IS_INFINITE`.
pub const ERROR_VALUE_IS_INFINITE: i32 = 104;
/// `EVAL_ERROR_UNKNOWN_VARIABLE`.
pub const ERROR_UNKNOWN_VARIABLE: i32 = 105;

/// `expression_strerror()`.
pub fn strerror(code: i32) -> &'static str {
    match code {
        ERROR_OK => "success",
        ERROR_MISSING_CLOSE_SUBEXPRESSION => "missing closing parenthesis",
        ERROR_UNKNOWN_OPERAND => "unknown operand",
        ERROR_MISSING_OPERAND => "expected operand",
        ERROR_MISSING_OPERATOR => "expected operator",
        ERROR_REMAINING_GARBAGE => "remaining characters after expression",
        ERROR_INVALID_VALUE => "invalid value structure - internal error",
        ERROR_INVALID_NUMBER_OF_OPERANDS => "wrong number of operands for operation - internal error",
        ERROR_VALUE_IS_NAN => "value is unset",
        ERROR_VALUE_IS_INFINITE => "computed value is infinite",
        ERROR_UNKNOWN_VARIABLE => "undefined variable",
        ERROR_IF_THEN_ELSE_MISSING_ELSE => "missing second sub-expression of inline conditional",
        _ => "unknown error",
    }
}

/// Why `Expression::parse` returned no expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// An empty input: C returns NULL and touches neither `*error` nor `*failed_at`.
    Empty,
    /// Every other failure. `failed_at` is the offset of the last token the lexer returned, or 0 when it returned
    /// none: never the position of the offending byte.
    Syntax { failed_at: usize },
}

impl ParseError {
    /// What C stores in `*error`. Always `ERROR_REMAINING_GARBAGE`: the parser's own code is replaced whenever
    /// `failed_at` points at a byte of the input, and it always does.
    pub fn code(&self) -> Option<i32> {
        match self {
            ParseError::Empty => None,
            ParseError::Syntax { .. } => Some(ERROR_REMAINING_GARBAGE),
        }
    }

    /// The offset C stores in `*failed_at`.
    pub fn failed_at(&self) -> Option<usize> {
        match self {
            ParseError::Empty => None,
            ParseError::Syntax { failed_at } => Some(*failed_at),
        }
    }

    /// The line `expression_parse()` logs for this failure of `input` (daemon source, error priority); the caller
    /// logs it.
    pub fn log_text(&self, input: &[u8]) -> Option<Vec<u8>> {
        let ParseError::Syntax { failed_at } = *self else {
            return None;
        };
        let input = c_str(input);
        let mut line = Vec::with_capacity(2 * input.len() + 96);
        line.extend_from_slice(b"failed to parse expression '");
        line.extend_from_slice(input);
        line.extend_from_slice(b"': ");
        line.extend_from_slice(strerror(ERROR_REMAINING_GARBAGE).as_bytes());
        line.extend_from_slice(format!(" at character {} (i.e.: '", failed_at + 1).as_bytes());
        line.extend_from_slice(&input[failed_at.min(input.len())..]);
        line.extend_from_slice(b"').");
        Some(line)
    }
}
