//! `parsed_as`: the expression printed back from its nodes (`eval-utils.c` `print_parsed_as_node()`).

use netdata_agent_text::print::print_netdata_double;

use crate::ast::{Node, NodeId};

/// `print_parsed_as_constant()`: `nan`, `inf` for either infinity, else `print_netdata_double()`. Also the value in
/// a lookup's trace and the text a hardcoded variable is replaced with.
pub(crate) fn print_constant(dst: &mut Vec<u8>, n: f64) {
    if n.is_nan() {
        dst.extend_from_slice(b"nan");
    } else if n.is_infinite() {
        dst.extend_from_slice(b"inf");
    } else {
        print_netdata_double(dst, n);
    }
}

enum Item {
    Node(NodeId),
    Text(&'static [u8]),
}

/// `print_parsed_as_node()` from the root. C recurses; a chain of operators is as deep as it is long, so this walks
/// with a stack of its own.
pub(crate) fn parsed_as(nodes: &[Node], root: NodeId) -> Vec<u8> {
    let mut out = Vec::new();
    let mut work = vec![Item::Node(root)];
    while let Some(item) = work.pop() {
        let id = match item {
            Item::Text(text) => {
                out.extend_from_slice(text);
                continue;
            }
            Item::Node(id) => id,
        };

        // each node's pieces are pushed in reverse
        match &nodes[id] {
            Node::Number(n) => print_constant(&mut out, *n),
            Node::Variable(name) => {
                out.extend_from_slice(b"${");
                out.extend_from_slice(name);
                out.push(b'}');
            }
            // its operator has no text of its own
            Node::Paren(inner) => {
                out.push(b'(');
                work.push(Item::Text(b")"));
                work.push(Item::Node(*inner));
            }
            Node::Unary(op, operand) => {
                out.extend_from_slice(op.print_as());
                out.push(b'(');
                work.push(Item::Text(b")"));
                work.push(Item::Node(*operand));
            }
            // the function's text is "abs(" and the function branch adds a parenthesis of its own
            Node::Abs(operand) => {
                out.extend_from_slice(b"abs((");
                work.push(Item::Text(b")"));
                work.push(Item::Node(*operand));
            }
            Node::Binary(op, left, right) => {
                out.push(b'(');
                work.push(Item::Text(b")"));
                work.push(Item::Node(*right));
                work.push(Item::Text(b" "));
                work.push(Item::Text(op.print_as()));
                work.push(Item::Text(b" "));
                work.push(Item::Node(*left));
            }
            Node::Ternary(condition, then, otherwise) => {
                work.push(Item::Node(*otherwise));
                work.push(Item::Text(b" : "));
                work.push(Item::Node(*then));
                work.push(Item::Text(b" ? "));
                work.push(Item::Node(*condition));
            }
        }
    }
    out
}
