//! The lexer (`re2c_lemon/lexer.re`, as re2c compiled it into `lexer.c`).
//!
//! re2c takes the longest match and, between rules of equal length, the earlier one. There are no word boundaries:
//! `1and2` is `1 && 2`, `nano` is NaN followed by an error at `o`.

use netdata_agent_text::c::at;
use netdata_agent_text::parse::str2ndd;

use crate::ast::BinaryOp;

/// `EVAL_MAX_VARIABLE_NAME_LENGTH`: a name of this many bytes or more is cut to one byte less.
pub const MAX_VARIABLE_NAME_LENGTH: usize = 300;

#[derive(Debug)]
pub(crate) enum Token {
    Number(f64),
    /// The name, without `$` or braces, cut to its limit.
    Variable(Box<[u8]>),
    LParen,
    RParen,
    /// `+`: binary after an expression, a sign otherwise.
    Plus,
    /// `-`: binary after an expression, a sign otherwise.
    Minus,
    /// Every other binary operator.
    Binary(BinaryOp),
    Not,
    QMark,
    Colon,
    Abs,
}

impl Token {
    /// The binary operator this token is after an expression.
    pub(crate) fn binary(&self) -> Option<BinaryOp> {
        match self {
            Token::Plus => Some(BinaryOp::Plus),
            Token::Minus => Some(BinaryOp::Minus),
            Token::Binary(op) => Some(*op),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Lexed {
    /// A token and the offset of its first byte.
    Token(Token, usize),
    /// The NUL that ends the input.
    End,
    /// What no rule but the last matches: `${}`, an unclosed `${`, a `$` before a byte no name can hold, a lone `&`,
    /// `|` or `.`, the start of a keyword that is not completed (`nul`, `an`, `i`), or any other byte that starts no
    /// token.
    Error,
}

pub(crate) struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    /// `input` is a C string's bytes, without its terminator.
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Lexer { input, pos: 0 }
    }

    /// `scan()`.
    pub(crate) fn next(&mut self) -> Lexed {
        let s = self.input;

        // "[ \t\r\n]+ { continue; }": vertical tab and form feed are not whitespace here
        while matches!(at(s, self.pos), b' ' | b'\t' | b'\r' | b'\n') {
            self.pos += 1;
        }

        let start = self.pos;
        let (token, len) = match at(s, start) {
            0 => return Lexed::End,
            b'0'..=b'9' | b'.' => match self.number(start) {
                Some(found) => found,
                None => return Lexed::Error,
            },
            b'$' => match self.variable(start) {
                Some(found) => found,
                None => return Lexed::Error,
            },
            b'+' => (Token::Plus, 1),
            b'-' => (Token::Minus, 1),
            b'*' => (Token::Binary(BinaryOp::Multiply), 1),
            b'/' => (Token::Binary(BinaryOp::Divide), 1),
            b'%' => (Token::Binary(BinaryOp::Modulo), 1),
            b'&' if at(s, start + 1) == b'&' => (Token::Binary(BinaryOp::And), 2),
            b'|' if at(s, start + 1) == b'|' => (Token::Binary(BinaryOp::Or), 2),
            b'!' if at(s, start + 1) == b'=' => (Token::Binary(BinaryOp::NotEqual), 2),
            b'!' => (Token::Not, 1),
            b'=' if at(s, start + 1) == b'=' => (Token::Binary(BinaryOp::Equal), 2),
            b'=' => (Token::Binary(BinaryOp::Equal), 1),
            b'<' if at(s, start + 1) == b'=' => (Token::Binary(BinaryOp::LessThanOrEqual), 2),
            b'<' if at(s, start + 1) == b'>' => (Token::Binary(BinaryOp::NotEqual), 2),
            b'<' => (Token::Binary(BinaryOp::Less), 1),
            b'>' if at(s, start + 1) == b'=' => (Token::Binary(BinaryOp::GreaterThanOrEqual), 2),
            b'>' => (Token::Binary(BinaryOp::Greater), 1),
            b'?' => (Token::QMark, 1),
            b':' => (Token::Colon, 1),
            b'(' => (Token::LParen, 1),
            b')' => (Token::RParen, 1),
            b'a' | b'A' if self.word(start, b"and") => (Token::Binary(BinaryOp::And), 3),
            b'a' | b'A' if self.word(start, b"abs") => (Token::Abs, 3),
            b'o' | b'O' if self.word(start, b"or") => (Token::Binary(BinaryOp::Or), 2),
            b'n' | b'N' if self.word(start, b"not") => (Token::Not, 3),
            // "matching str2ndd behavior": nan and null are the number NaN
            b'n' | b'N' if self.word(start, b"nan") => (Token::Number(f64::NAN), 3),
            b'n' | b'N' if self.word(start, b"null") => (Token::Number(f64::NAN), 4),
            b'i' | b'I' if self.word(start, b"infinity") => (Token::Number(f64::INFINITY), 8),
            b'i' | b'I' if self.word(start, b"inf") => (Token::Number(f64::INFINITY), 3),
            _ => return Lexed::Error,
        };

        self.pos = start + len;
        Lexed::Token(token, start)
    }

    /// Whether the input at `start` spells `word` in any letter case.
    fn word(&self, start: usize, word: &[u8]) -> bool {
        self.input
            .get(start..start + word.len())
            .is_some_and(|found| found.eq_ignore_ascii_case(word))
    }

    /// The number rules: digits, digits `.` digits (none needed after the dot), `.` digits, each with an optional
    /// exponent that is taken only when it has a digit. A number has no sign.
    fn number(&self, start: usize) -> Option<(Token, usize)> {
        let s = self.input;
        let digits = |mut i: usize| {
            while at(s, i).is_ascii_digit() {
                i += 1;
            }
            i
        };

        let mut end = digits(start);
        if at(s, end) == b'.' {
            let fraction = digits(end + 1);
            if end == start && fraction == end + 1 {
                // a dot with no digit on either side
                return None;
            }
            end = fraction;
        }

        if matches!(at(s, end), b'e' | b'E') {
            let mut exponent = end + 1;
            if matches!(at(s, exponent), b'+' | b'-') {
                exponent += 1;
            }
            if at(s, exponent).is_ascii_digit() {
                end = digits(exponent);
            }
        }

        // C converts from the token's first byte through the rest of the string, not the token alone. The two differ
        // only when digits follow whitespace after the dot (`1. 5`), and a number after a number never parses.
        Some((Token::Number(str2ndd(&s[start..]).0), end - start))
    }

    /// The variable rules: `$` and one or more name bytes, or `${`, one or more bytes other than `}`, and `}`.
    fn variable(&self, start: usize) -> Option<(Token, usize)> {
        let s = self.input;
        let (name, len) = if at(s, start + 1) == b'{' {
            let name = start + 2;
            let close = name + s[name..].iter().position(|&c| c == b'}')?;
            if close == name {
                // "${}": the empty-variable rule comes first
                return None;
            }
            (&s[name..close], close + 1 - start)
        } else {
            let name = start + 1;
            let end = name + s[name..].iter().position(|&c| !is_name_byte(c)).unwrap_or(s.len() - name);
            if end == name {
                return None;
            }
            (&s[name..end], end - start)
        };

        // the whole token is consumed, but the name is cut
        let name = &name[..name.len().min(MAX_VARIABLE_NAME_LENGTH - 1)];
        Some((Token::Variable(name.into()), len))
    }
}

/// A byte of an unbraced name: anything but whitespace, the operators' first bytes, parentheses and braces
/// (`[^\000 \t\r\n&|!><=%+\-*/?()}{]`). `.`, `:`, `$` and every byte from 0x80 up are name bytes.
fn is_name_byte(c: u8) -> bool {
    !matches!(
        c,
        b' ' | b'\t'
            | b'\r'
            | b'\n'
            | b'&'
            | b'|'
            | b'!'
            | b'>'
            | b'<'
            | b'='
            | b'%'
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'?'
            | b'('
            | b')'
            | b'}'
            | b'{'
    )
}
