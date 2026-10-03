//! The alert expression evaluator (C: `src/libnetdata/eval/`: `re2c_lemon/lexer.re` and `parser.y` as the production
//! build compiles their generated `lexer.c` and `parser.c`, `eval-evaluate.c`, `eval-utils.c`, and
//! `expression_parse()` in `eval-parser-legacy.c`, whose own recursive-descent parser is compiled but never called).
//!
//! Health parses the `calc`, `warn` and `crit` lines of an alert into expressions and evaluates them at every run
//! of the alert. Users see the source text, the text printed back from the parsed nodes (`parsed_as`), the result,
//! and the message of the last evaluation, so all of them reproduce C's bytes, quirks included: every parse failure
//! is "remaining characters after expression", reported at the last token the lexer returned; `abs(x)` prints back
//! as `abs((x)`; an expression that fills the parser's fixed stack silently loses its head.
//!
//! A leaf crate: bytes in, bytes out, as `netdata-agent-text`; an input ends at its first NUL. Nothing here
//! recurses, so no expression, however long, can exhaust a thread's stack.

#![forbid(unsafe_code)]

mod ast;
mod error;
mod evaluate;
mod hardcode;
mod lex;
mod parse;
mod print;

use netdata_agent_text::c::c_str;

use ast::{Node, NodeId};

pub use error::{
    ERROR_IF_THEN_ELSE_MISSING_ELSE, ERROR_INVALID_NUMBER_OF_OPERANDS, ERROR_INVALID_VALUE,
    ERROR_MISSING_CLOSE_SUBEXPRESSION, ERROR_MISSING_OPERAND, ERROR_MISSING_OPERATOR, ERROR_OK, ERROR_REMAINING_GARBAGE,
    ERROR_UNKNOWN_OPERAND, ERROR_UNKNOWN_VARIABLE, ERROR_VALUE_IS_INFINITE, ERROR_VALUE_IS_NAN, ParseError, strerror,
};
pub use evaluate::{NoVariables, Resolver};
pub use lex::MAX_VARIABLE_NAME_LENGTH;

/// `EVAL_EXPRESSION`: a parsed expression and the state of its last evaluation.
///
/// Not `Clone`: C never copies an expression, it parses the source again for every alert that runs it, and what a
/// second parse makes of a source rewritten by [`Expression::hardcode_variable`] is observable.
#[derive(Debug)]
pub struct Expression {
    source: Vec<u8>,
    parsed_as: Vec<u8>,
    nodes: Vec<Node>,
    root: NodeId,
    result: f64,
    error: i32,
    error_msg: Vec<u8>,
}

impl Expression {
    /// `expression_parse()`.
    pub fn parse(input: &[u8]) -> Result<Expression, ParseError> {
        let input = c_str(input);
        if input.is_empty() {
            return Err(ParseError::Empty);
        }
        let (nodes, root) = parse::parse(input).map_err(|failed_at| ParseError::Syntax { failed_at })?;
        Ok(Expression {
            source: input.to_vec(),
            parsed_as: print::parsed_as(&nodes, root),
            nodes,
            root,
            result: 0.0,
            error: ERROR_OK,
            error_msg: Vec::new(),
        })
    }

    /// `expression_evaluate()`: true when the result is a number. The result, the error and the message of the
    /// previous evaluation are replaced.
    pub fn evaluate(&mut self, vars: &mut dyn Resolver) -> bool {
        let outcome = evaluate::evaluate(&self.nodes, self.root, vars, &mut self.error_msg);
        self.result = outcome.result;
        self.error = outcome.error;
        outcome.ok
    }

    /// `expression_source()`: the text given to [`Expression::parse`], or what
    /// [`Expression::hardcode_variable`] rewrote it to.
    pub fn source(&self) -> &[u8] {
        &self.source
    }

    /// `expression_parsed_as()`: the expression printed back from its nodes when it was parsed.
    pub fn parsed_as(&self) -> &[u8] {
        &self.parsed_as
    }

    /// `expression_result()`: 0 before the first evaluation, NaN after a failed one.
    pub fn result(&self) -> f64 {
        self.result
    }

    /// The error code of the last evaluation (`expression->error`).
    pub fn error(&self) -> i32 {
        self.error
    }

    /// `expression_error_msg()`: the lookups of the last evaluation, each as `[ ${name} = value ] ` or
    /// `[ undefined variable 'name' ] `, then the reason when it failed.
    pub fn error_msg(&self) -> &[u8] {
        &self.error_msg
    }

    /// `expression_hardcode_variable()`: every variable named `name` becomes the constant `value`, and the source
    /// text follows as far as C's textual replacement takes it.
    pub fn hardcode_variable(&mut self, name: &[u8], value: f64) {
        let name = c_str(name);
        let matches = hardcode::hardcode_nodes(&mut self.nodes, name, value);
        if matches == 0 {
            return;
        }
        if let Some(source) = hardcode::hardcode_source(&self.source, name, value, matches) {
            self.source = source;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_input_ends_at_its_first_nul() {
        let expression = Expression::parse(b"1 + 2\0 garbage").expect("parses");
        assert_eq!(expression.source(), b"1 + 2");
        assert_eq!(expression.parsed_as(), b"(1 + 2)");
        assert_eq!(Expression::parse(&[0, b'1']).unwrap_err(), ParseError::Empty);
        assert_eq!(Expression::parse(b"").unwrap_err(), ParseError::Empty);
    }

    #[test]
    fn a_parsed_expression_starts_with_c_zeroed_state() {
        let expression = Expression::parse(b"$a / 0").expect("parses");
        assert_eq!(expression.result().to_bits(), 0f64.to_bits());
        assert_eq!(expression.error(), ERROR_OK);
        assert_eq!(expression.error_msg(), b"");
    }

    #[test]
    fn a_failed_parse_reports_code_5_and_its_log_line() {
        let input = b"1 + @ 2";
        let error = Expression::parse(input).unwrap_err();
        // the last token the lexer returned is `+`, not the byte it choked on
        assert_eq!(error, ParseError::Syntax { failed_at: 2 });
        assert_eq!(error.code(), Some(ERROR_REMAINING_GARBAGE));
        assert_eq!(
            error.log_text(input).as_deref(),
            Some(&b"failed to parse expression '1 + @ 2': remaining characters after expression at character 3 (i.e.: '+ @ 2')."[..])
        );
        assert_eq!(ParseError::Empty.code(), None);
        assert_eq!(ParseError::Empty.log_text(b""), None);
    }

    #[test]
    fn error_texts_are_c() {
        let texts = [
            (ERROR_OK, "success"),
            (ERROR_MISSING_CLOSE_SUBEXPRESSION, "missing closing parenthesis"),
            (ERROR_UNKNOWN_OPERAND, "unknown operand"),
            (ERROR_MISSING_OPERAND, "expected operand"),
            (ERROR_MISSING_OPERATOR, "expected operator"),
            (ERROR_REMAINING_GARBAGE, "remaining characters after expression"),
            (ERROR_IF_THEN_ELSE_MISSING_ELSE, "missing second sub-expression of inline conditional"),
            (ERROR_INVALID_VALUE, "invalid value structure - internal error"),
            (ERROR_INVALID_NUMBER_OF_OPERANDS, "wrong number of operands for operation - internal error"),
            (ERROR_VALUE_IS_NAN, "value is unset"),
            (ERROR_VALUE_IS_INFINITE, "computed value is infinite"),
            (ERROR_UNKNOWN_VARIABLE, "undefined variable"),
            (7, "unknown error"),
            (-1, "unknown error"),
        ];
        for (code, text) in texts {
            assert_eq!(strerror(code), text);
        }
    }
}
