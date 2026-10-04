//! `expression_parse()` as health calls it. C logs a failed parse inside the parser, so at every call site, and
//! the caller's own record follows it; the evaluator crate only renders that line.

use netdata_agent_eval::{Expression, ParseError};
use netdata_agent_log::netdata_log_error_errno;

use crate::keywords::lossy;

/// Parses `input`, logging C's "failed to parse expression" record on a failure.
pub fn parse_logged(input: &[u8]) -> Result<Expression, ParseError> {
    Expression::parse(input).inspect_err(|error| {
        if let Some(line) = error.log_text(input) {
            netdata_log_error_errno!("{}", lossy(&line));
        }
    })
}

#[cfg(test)]
mod tests {
    use netdata_agent_log::{Priority, capture};

    use super::*;

    #[test]
    fn a_failed_parse_is_logged_once_and_a_good_one_is_silent() {
        let (parsed, records) = capture(|| parse_logged(b"$a > 1"));
        assert!(parsed.is_ok());
        assert!(records.is_empty());

        let (parsed, records) = capture(|| parse_logged(b"1 + @"));
        let error = parsed.expect_err("no expression");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].priority, Priority::Err);
        // the evaluator's own line, with the bytes it was given
        let line = error.log_text(b"1 + @").expect("a line");
        assert_eq!(records[0].message.as_deref(), Some(&*String::from_utf8_lossy(&line)));
        assert!(records[0].message.as_deref().unwrap().contains("1 + @"));
    }
}
