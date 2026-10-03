//! The parsed expression: nodes in one arena, and the operator table (`eval-internal.h`; `eval-evaluate.c`
//! `operators[]`; the precedence declarations of `re2c_lemon/parser.y`).

/// Index of a node in its expression's arena.
pub(crate) type NodeId = usize;

/// C wraps every leaf in a one-operand `EVAL_OPERATOR_NOP` node that prints and evaluates as its operand
/// (`parser.y`'s `expr ::= NUMBER` and `expr ::= VARIABLE`); the leaves are stored directly here.
#[derive(Debug)]
pub(crate) enum Node {
    Number(f64),
    /// The name, without `$` or braces.
    Variable(Box<[u8]>),
    /// `( e )`: `EVAL_OPERATOR_EXPRESSION_OPEN`.
    Paren(NodeId),
    Unary(UnaryOp, NodeId),
    /// `abs ( e )`: the operand is the inner expression itself, with no node for the parentheses.
    Abs(NodeId),
    Binary(BinaryOp, NodeId, NodeId),
    /// Condition, then, else.
    Ternary(NodeId, NodeId, NodeId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Not,
    SignPlus,
    SignMinus,
}

impl UnaryOp {
    /// `operators[].print_as`.
    pub(crate) fn print_as(self) -> &'static [u8] {
        match self {
            UnaryOp::Not => b"!",
            UnaryOp::SignPlus => b"+",
            UnaryOp::SignMinus => b"-",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    And,
    Or,
    GreaterThanOrEqual,
    LessThanOrEqual,
    NotEqual,
    Equal,
    Less,
    Greater,
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
}

impl BinaryOp {
    /// `operators[].print_as`.
    pub(crate) fn print_as(self) -> &'static [u8] {
        match self {
            BinaryOp::And => b"&&",
            BinaryOp::Or => b"||",
            BinaryOp::GreaterThanOrEqual => b">=",
            BinaryOp::LessThanOrEqual => b"<=",
            BinaryOp::NotEqual => b"!=",
            BinaryOp::Equal => b"==",
            BinaryOp::Less => b"<",
            BinaryOp::Greater => b">",
            BinaryOp::Plus => b"+",
            BinaryOp::Minus => b"-",
            BinaryOp::Multiply => b"*",
            BinaryOp::Divide => b"/",
            BinaryOp::Modulo => b"%",
        }
    }

    /// The operator's precedence level as Lemon computed it from `parser.y`'s declarations, lowest first
    /// (`parser.out`, "precedence=N"). Every binary level is left-associative.
    pub(crate) fn level(self) -> u8 {
        match self {
            BinaryOp::Or | BinaryOp::And => 2, // %left OR AND.
            BinaryOp::Equal | BinaryOp::NotEqual => 3, // %left EQ NE.
            BinaryOp::Less | BinaryOp::LessThanOrEqual | BinaryOp::Greater | BinaryOp::GreaterThanOrEqual => 4, // %left LT LE GT GE.
            BinaryOp::Plus | BinaryOp::Minus => 5, // %left PLUS MINUS.
            BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 6, // %left MULTIPLY DIVIDE MODULO.
        }
    }
}

/// `%right COLON QMARK.`: the ternary's level, the lowest.
pub(crate) const TERNARY_LEVEL: u8 = 1;
/// `%right UMINUS UPLUS NOT.`: the unary operators' level, the highest.
pub(crate) const UNARY_LEVEL: u8 = 7;
