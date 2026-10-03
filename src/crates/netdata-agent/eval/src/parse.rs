//! The parser: Lemon's LALR(1) automaton for `re2c_lemon/parser.y`, written by hand as a shift-reduce precedence
//! parser, and the driver around it (`lexer.re` `parse_expression_with_re2c_lemon()`).
//!
//! The stack holds the symbols Lemon's holds, so it has Lemon's depth at every shift. That depth is observable:
//! Lemon's stack is fixed (`YYSTACKDEPTH` 100, entry 0 being the base), and a shift onto a full stack empties it,
//! drops the token and reports nothing (`parser.c` `yy_shift()`, `yyStackOverflow()`), so the tokens that follow
//! start a new expression.

use crate::ast::{BinaryOp, Node, NodeId, TERNARY_LEVEL, UNARY_LEVEL, UnaryOp};
use crate::lex::{Lexed, Lexer, Token};

/// The symbols Lemon's stack can hold: `YYSTACKDEPTH - 1`.
const STACK_SYMBOLS: usize = 99;

#[derive(Debug, Clone, Copy)]
enum Symbol {
    Expr(NodeId),
    LParen,
    Abs,
    Unary(UnaryOp),
    Binary(BinaryOp),
    QMark,
    Colon,
}

/// A rule whose right-hand side is complete on top of the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handle {
    /// `+e`, `-e`, `!e`.
    Unary,
    /// `e OP e`.
    Binary(BinaryOp),
    /// `e ? e : e`.
    Ternary,
}

/// The token that follows a complete handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lookahead {
    Binary(BinaryOp),
    QMark,
    Colon,
    RParen,
    End,
}

/// Lemon's choice between reducing `handle` and shifting `lookahead`: reduce when the handle's rule has the higher
/// precedence, or the same one and the lookahead is left-associative. `:`, `)` and the end cannot be shifted onto a
/// complete handle, so they always reduce.
pub(crate) fn reduces(handle: Handle, lookahead: Lookahead) -> bool {
    let handle_level = match handle {
        Handle::Unary => UNARY_LEVEL,
        Handle::Binary(op) => op.level(),
        Handle::Ternary => TERNARY_LEVEL,
    };
    match lookahead {
        Lookahead::Colon | Lookahead::RParen | Lookahead::End => true,
        // right-associative
        Lookahead::QMark => handle_level > TERNARY_LEVEL,
        // left-associative
        Lookahead::Binary(op) => handle_level >= op.level(),
    }
}

/// What a token does once the reductions it causes are done.
enum Shift {
    Symbol(Symbol),
    Leaf(Node),
    /// `)`: folds `( e )` or `abs ( e )`.
    Close,
}

struct Parser {
    nodes: Vec<Node>,
    stack: Vec<Symbol>,
}

/// The nodes and the root of `input` (a C string's bytes), or the offset the failure is reported at.
pub(crate) fn parse(input: &[u8]) -> Result<(Vec<Node>, NodeId), usize> {
    let mut parser = Parser { nodes: Vec::new(), stack: Vec::new() };
    let mut lexer = Lexer::new(input);

    // "error_pos": the start of the last token the lexer returned, or of the string
    let mut failed_at = 0;
    loop {
        match lexer.next() {
            Lexed::Token(token, start) => {
                failed_at = start;
                if !parser.feed(token) {
                    return Err(failed_at);
                }
            }
            Lexed::End => return parser.finish().map(|root| (parser.nodes, root)).ok_or(failed_at),
            // C still feeds the end to the parser, then discards whatever it accepted
            Lexed::Error => return Err(failed_at),
        }
    }
}

impl Parser {
    fn node(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        (self.nodes.len() - 1) as NodeId
    }

    fn handle(&self) -> Option<Handle> {
        match self.stack[..] {
            [.., Symbol::Unary(_), Symbol::Expr(_)] => Some(Handle::Unary),
            [.., Symbol::Binary(op), Symbol::Expr(_)] => Some(Handle::Binary(op)),
            [.., Symbol::Colon, Symbol::Expr(_)] => Some(Handle::Ternary),
            _ => None,
        }
    }

    /// Reduces the handles on top of the stack for as long as Lemon reduces them before `lookahead`.
    fn reduce(&mut self, lookahead: Lookahead) {
        while let Some(handle) = self.handle() {
            if !reduces(handle, lookahead) {
                break;
            }
            let (node, symbols) = match self.stack[..] {
                [.., Symbol::Unary(op), Symbol::Expr(operand)] => (Node::Unary(op, operand), 2),
                [.., Symbol::Expr(left), Symbol::Binary(op), Symbol::Expr(right)] => (Node::Binary(op, left, right), 3),
                [.., Symbol::Expr(condition), Symbol::QMark, Symbol::Expr(then), Symbol::Colon, Symbol::Expr(otherwise)] => {
                    (Node::Ternary(condition, then, otherwise), 5)
                }
                _ => unreachable!("a binary operator and a colon are only shifted onto their left sides"),
            };
            self.stack.truncate(self.stack.len() - symbols);
            let id = self.node(node);
            self.stack.push(Symbol::Expr(id));
        }
    }

    /// One token: the reductions it causes, then its shift. False is Lemon's syntax error.
    fn feed(&mut self, token: Token) -> bool {
        let shift = if matches!(self.stack.last(), Some(Symbol::Expr(_))) {
            if let Some(op) = token.binary() {
                self.reduce(Lookahead::Binary(op));
                return self.shift(Shift::Symbol(Symbol::Binary(op)));
            }
            match token {
                Token::QMark => {
                    self.reduce(Lookahead::QMark);
                    Shift::Symbol(Symbol::QMark)
                }
                Token::Colon => {
                    self.reduce(Lookahead::Colon);
                    if !matches!(self.stack[..], [.., Symbol::QMark, Symbol::Expr(_)]) {
                        return false;
                    }
                    Shift::Symbol(Symbol::Colon)
                }
                Token::RParen => {
                    self.reduce(Lookahead::RParen);
                    if !matches!(self.stack[..], [.., Symbol::LParen, Symbol::Expr(_)]) {
                        return false;
                    }
                    Shift::Close
                }
                _ => return false,
            }
        } else {
            // `abs` must be followed by `(`
            if matches!(self.stack.last(), Some(Symbol::Abs)) && !matches!(token, Token::LParen) {
                return false;
            }
            match token {
                Token::Number(number) => Shift::Leaf(Node::Number(number)),
                Token::Variable(name) => Shift::Leaf(Node::Variable(name)),
                Token::LParen => Shift::Symbol(Symbol::LParen),
                Token::Plus => Shift::Symbol(Symbol::Unary(UnaryOp::SignPlus)),
                Token::Minus => Shift::Symbol(Symbol::Unary(UnaryOp::SignMinus)),
                Token::Not => Shift::Symbol(Symbol::Unary(UnaryOp::Not)),
                Token::Abs => Shift::Symbol(Symbol::Abs),
                _ => return false,
            }
        };

        self.shift(shift)
    }

    /// Lemon's shift, which the fixed stack can refuse.
    fn shift(&mut self, shift: Shift) -> bool {
        if self.stack.len() == STACK_SYMBOLS {
            // the stack overflow: every symbol and its nodes are freed, the token is dropped, no error is set
            self.stack.clear();
            self.nodes.clear();
            return true;
        }

        match shift {
            Shift::Symbol(symbol) => self.stack.push(symbol),
            Shift::Leaf(node) => {
                let id = self.node(node);
                self.stack.push(Symbol::Expr(id));
            }
            Shift::Close => {
                // Lemon holds `( e )` until the next token arrives; no shift can happen in between
                let Some(Symbol::Expr(inner)) = self.stack.pop() else {
                    unreachable!("checked before the shift");
                };
                self.stack.pop();
                let node = if matches!(self.stack.last(), Some(Symbol::Abs)) {
                    self.stack.pop();
                    Node::Abs(inner)
                } else {
                    Node::Paren(inner)
                };
                let id = self.node(node);
                self.stack.push(Symbol::Expr(id));
            }
        }
        true
    }

    /// The end of the input: accepted when everything reduces to one expression.
    fn finish(&mut self) -> Option<NodeId> {
        self.reduce(Lookahead::End);
        match self.stack[..] {
            [Symbol::Expr(root)] => Some(root),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINARY: [BinaryOp; 13] = [
        BinaryOp::Plus,
        BinaryOp::Minus,
        BinaryOp::Multiply,
        BinaryOp::Divide,
        BinaryOp::Modulo,
        BinaryOp::And,
        BinaryOp::Or,
        BinaryOp::Equal,
        BinaryOp::NotEqual,
        BinaryOp::Less,
        BinaryOp::LessThanOrEqual,
        BinaryOp::Greater,
        BinaryOp::GreaterThanOrEqual,
    ];

    /// The name of an operator's token in `parser.y`.
    fn terminal(op: BinaryOp) -> &'static str {
        match op {
            BinaryOp::Plus => "PLUS",
            BinaryOp::Minus => "MINUS",
            BinaryOp::Multiply => "MULTIPLY",
            BinaryOp::Divide => "DIVIDE",
            BinaryOp::Modulo => "MODULO",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
            BinaryOp::Equal => "EQ",
            BinaryOp::NotEqual => "NE",
            BinaryOp::Less => "LT",
            BinaryOp::LessThanOrEqual => "LE",
            BinaryOp::Greater => "GT",
            BinaryOp::GreaterThanOrEqual => "GE",
        }
    }

    /// Lemon's resolved actions, transcribed from `re2c_lemon/parser.out` (C's checked-in listing of the automaton):
    /// for the state in which each rule's right-hand side is complete, the lookaheads it shifts. On every other
    /// lookahead that state reduces. The unary rules and `* / %` have no such state: Lemon reduces them at once.
    const SHIFTS: &[(&str, &[&str])] = &[
        (
            "QMARK",
            &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO", "AND", "OR", "EQ", "NE", "LT", "LE", "GT", "GE", "QMARK"],
        ),
        ("AND", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO", "EQ", "NE", "LT", "LE", "GT", "GE"]),
        ("OR", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO", "EQ", "NE", "LT", "LE", "GT", "GE"]),
        ("EQ", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO", "LT", "LE", "GT", "GE"]),
        ("NE", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO", "LT", "LE", "GT", "GE"]),
        ("LT", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO"]),
        ("LE", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO"]),
        ("GT", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO"]),
        ("GE", &["PLUS", "MINUS", "MULTIPLY", "DIVIDE", "MODULO"]),
        ("PLUS", &["MULTIPLY", "DIVIDE", "MODULO"]),
        ("MINUS", &["MULTIPLY", "DIVIDE", "MODULO"]),
        ("MULTIPLY", &[]),
        ("DIVIDE", &[]),
        ("MODULO", &[]),
        ("UNARY", &[]),
    ];

    #[test]
    fn reductions_are_lemons() {
        let mut lookaheads: Vec<(&str, Lookahead)> = BINARY.iter().map(|&op| (terminal(op), Lookahead::Binary(op))).collect();
        lookaheads.push(("QMARK", Lookahead::QMark));
        lookaheads.push(("COLON", Lookahead::Colon));
        lookaheads.push(("RPAREN", Lookahead::RParen));
        lookaheads.push(("$", Lookahead::End));

        let mut handles: Vec<(&str, Handle)> = BINARY.iter().map(|&op| (terminal(op), Handle::Binary(op))).collect();
        handles.push(("QMARK", Handle::Ternary));
        handles.push(("UNARY", Handle::Unary));

        let mut cells = 0;
        for &(rule, handle) in &handles {
            let shifts = SHIFTS.iter().find(|(name, _)| *name == rule).expect("every rule is listed").1;
            for &(name, lookahead) in &lookaheads {
                assert_eq!(reduces(handle, lookahead), !shifts.contains(&name), "{rule} complete, then {name}");
                cells += 1;
            }
        }
        assert_eq!(cells, 15 * 17);
    }
}
