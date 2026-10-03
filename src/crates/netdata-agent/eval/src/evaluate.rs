//! Evaluation (`eval-evaluate.c`).
//!
//! One error slot and one message serve a whole evaluation. Operands are evaluated left to right, each variable
//! lookup appends to the message as it happens, and `/` and `%` give up as soon as the slot holds any error, even
//! one an unrelated earlier operand left there.

use crate::ast::{BinaryOp, Node, NodeId, UnaryOp};
use crate::error::{ERROR_OK, ERROR_UNKNOWN_VARIABLE, ERROR_VALUE_IS_INFINITE, ERROR_VALUE_IS_NAN, strerror};
use crate::print::print_constant;

/// What gives variables their values: C's lookup callback.
pub trait Resolver {
    /// The value of `name` (no `$`, no braces), or `None` when there is no such variable.
    fn lookup(&mut self, name: &[u8]) -> Option<f64>;
}

/// An expression with no lookup callback: every variable is undefined.
pub struct NoVariables;

impl Resolver for NoVariables {
    fn lookup(&mut self, _name: &[u8]) -> Option<f64> {
        None
    }
}

/// `considered_equal_ndd()`'s epsilon.
const EPSILON: f64 = 0.0000001;

/// What is left to do. C recurses; a chain of operators is as deep as it is long, so this keeps its own stacks.
enum Step {
    Enter(NodeId),
    Unary(UnaryOp),
    Abs,
    /// Both operands are on the value stack.
    Binary(BinaryOp),
    /// The left operand of `&&`, `||`, `/` or `%` is on the value stack: the right one may not be evaluated.
    AfterLeft(BinaryOp, NodeId),
    /// The right operand of `&&` or `||` is on the value stack.
    Truth,
    /// The condition is on the value stack: only the chosen branch is evaluated.
    Choose(NodeId, NodeId),
}

/// `is_true()`: not NaN, and either infinite or not zero.
fn is_true(n: f64) -> bool {
    !n.is_nan() && n != 0.0
}

fn flag(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

pub(crate) struct Outcome {
    pub(crate) ok: bool,
    pub(crate) result: f64,
    pub(crate) error: i32,
}

/// `expression_evaluate()`: `error_msg` is rewritten with the lookups' trace and, on a failure, its reason.
pub(crate) fn evaluate(nodes: &[Node], root: NodeId, vars: &mut dyn Resolver, error_msg: &mut Vec<u8>) -> Outcome {
    let mut error = ERROR_OK;
    error_msg.clear();

    let mut steps = vec![Step::Enter(root)];
    let mut values: Vec<f64> = Vec::new();
    let pop = |values: &mut Vec<f64>| values.pop().expect("every step finds its operands");

    while let Some(step) = steps.pop() {
        match step {
            Step::Enter(id) => match &nodes[id as usize] {
                Node::Number(n) => values.push(*n),
                Node::Variable(name) => values.push(variable(name, vars, &mut error, error_msg)),
                // eval_nop()
                Node::Paren(inner) => steps.push(Step::Enter(*inner)),
                Node::Unary(op, operand) => {
                    steps.push(Step::Unary(*op));
                    steps.push(Step::Enter(*operand));
                }
                Node::Abs(operand) => {
                    steps.push(Step::Abs);
                    steps.push(Step::Enter(*operand));
                }
                Node::Binary(op, left, right) => {
                    if matches!(op, BinaryOp::And | BinaryOp::Or | BinaryOp::Divide | BinaryOp::Modulo) {
                        steps.push(Step::AfterLeft(*op, *right));
                    } else {
                        steps.push(Step::Binary(*op));
                        steps.push(Step::Enter(*right));
                    }
                    steps.push(Step::Enter(*left));
                }
                Node::Ternary(condition, then, otherwise) => {
                    steps.push(Step::Choose(*then, *otherwise));
                    steps.push(Step::Enter(*condition));
                }
            },

            Step::Unary(op) => {
                let n = pop(&mut values);
                values.push(match op {
                    // eval_sign_plus()
                    UnaryOp::SignPlus => n,
                    // eval_sign_minus(): either infinity gives the positive one
                    UnaryOp::SignMinus if n.is_nan() => f64::NAN,
                    UnaryOp::SignMinus if n.is_infinite() => f64::INFINITY,
                    UnaryOp::SignMinus => -n,
                    // eval_not()
                    UnaryOp::Not => flag(!is_true(n)),
                });
            }

            // eval_abs(): the ABS() macro, which leaves a negative zero as it is
            Step::Abs => {
                let n = pop(&mut values);
                values.push(if n.is_nan() {
                    f64::NAN
                } else if n.is_infinite() {
                    f64::INFINITY
                } else if n < 0.0 {
                    -n
                } else {
                    n
                });
            }

            Step::AfterLeft(op, right) => {
                let left = pop(&mut values);
                match op {
                    // eval_and(), eval_or(): the right side only when the left one does not decide
                    BinaryOp::And if !is_true(left) => values.push(0.0),
                    BinaryOp::Or if is_true(left) => values.push(1.0),
                    BinaryOp::And | BinaryOp::Or => {
                        steps.push(Step::Truth);
                        steps.push(Step::Enter(right));
                    }
                    // eval_divide(), eval_modulo(): "propagate previous errors", whoever set them
                    _ if error != ERROR_OK => values.push(f64::NAN),
                    _ => {
                        values.push(left);
                        steps.push(Step::Binary(op));
                        steps.push(Step::Enter(right));
                    }
                }
            }

            Step::Truth => {
                let n = pop(&mut values);
                values.push(flag(is_true(n)));
            }

            Step::Choose(then, otherwise) => {
                let condition = pop(&mut values);
                steps.push(Step::Enter(if is_true(condition) { then } else { otherwise }));
            }

            Step::Binary(op) => {
                let right = pop(&mut values);
                let left = pop(&mut values);
                values.push(binary(op, left, right, &mut error));
            }
        }
    }

    let mut result = pop(&mut values);
    if result.is_nan() {
        if error == ERROR_OK {
            error = ERROR_VALUE_IS_NAN;
        }
    } else if result.is_infinite() {
        if error == ERROR_OK {
            error = ERROR_VALUE_IS_INFINITE;
        }
    } else if error == ERROR_UNKNOWN_VARIABLE {
        // "although there is an unknown variable the expression was evaluated successfully"
        error = ERROR_OK;
    }

    if error != ERROR_OK {
        result = f64::NAN;
        if !error_msg.is_empty() {
            error_msg.extend_from_slice(b"; ");
        }
        error_msg.extend_from_slice(
            format!("failed to evaluate expression with error {error} ({})", strerror(error)).as_bytes(),
        );
    }

    Outcome { ok: error == ERROR_OK, result, error }
}

/// `eval_variable()`.
fn variable(name: &[u8], vars: &mut dyn Resolver, error: &mut i32, error_msg: &mut Vec<u8>) -> f64 {
    match vars.lookup(name) {
        Some(n) => {
            error_msg.extend_from_slice(b"[ ${");
            error_msg.extend_from_slice(name);
            error_msg.extend_from_slice(b"} = ");
            print_constant(error_msg, n);
            error_msg.extend_from_slice(b" ] ");
            n
        }
        None => {
            *error = ERROR_UNKNOWN_VARIABLE;
            error_msg.extend_from_slice(b"[ undefined variable '");
            error_msg.extend_from_slice(name);
            error_msg.extend_from_slice(b"' ] ");
            f64::NAN
        }
    }
}

/// The two-operand operators, both operands evaluated.
fn binary(op: BinaryOp, left: f64, right: f64, error: &mut i32) -> f64 {
    let either_nan = left.is_nan() || right.is_nan();
    let either_infinite = left.is_infinite() || right.is_infinite();
    match op {
        // C's quiet comparison macros: false when either side is NaN
        BinaryOp::GreaterThanOrEqual => flag(left >= right),
        BinaryOp::LessThanOrEqual => flag(left <= right),
        BinaryOp::Less => flag(left < right),
        BinaryOp::Greater => flag(left > right),
        BinaryOp::Equal => flag(equal(left, right)),
        BinaryOp::NotEqual => flag(!equal(left, right)),

        // eval_plus(), eval_minus(), eval_multiply(): an infinity on either side gives the positive one
        BinaryOp::Plus | BinaryOp::Minus | BinaryOp::Multiply if either_nan => f64::NAN,
        BinaryOp::Plus | BinaryOp::Minus | BinaryOp::Multiply if either_infinite => f64::INFINITY,
        BinaryOp::Plus => left + right,
        BinaryOp::Minus => left - right,
        BinaryOp::Multiply => left * right,

        // eval_divide(), eval_modulo()
        BinaryOp::Divide | BinaryOp::Modulo => {
            if *error != ERROR_OK {
                f64::NAN
            } else if either_nan {
                *error = ERROR_VALUE_IS_NAN;
                f64::NAN
            } else if either_infinite {
                *error = ERROR_VALUE_IS_INFINITE;
                f64::INFINITY
            } else if right == 0.0 {
                // "we treat all division by zero as INFINITE error"
                *error = ERROR_VALUE_IS_INFINITE;
                match op {
                    BinaryOp::Divide if left >= 0.0 => f64::INFINITY,
                    BinaryOp::Divide => f64::NEG_INFINITY,
                    _ => f64::NAN,
                }
            } else if op == BinaryOp::Divide {
                left / right
            } else {
                // fmod()
                left % right
            }
        }

        BinaryOp::And | BinaryOp::Or => unreachable!("decided after their left operand"),
    }
}

/// `eval_equal()`: two NaNs are equal, and so are two infinities of any signs.
fn equal(left: f64, right: f64) -> bool {
    if left.is_nan() && right.is_nan() {
        return true;
    }
    if left.is_infinite() && right.is_infinite() {
        return true;
    }
    if left.is_nan() || right.is_nan() || left.is_infinite() || right.is_infinite() {
        return false;
    }
    (left - right).abs() < EPSILON
}
