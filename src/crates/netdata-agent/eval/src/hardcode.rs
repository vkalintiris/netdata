//! Replacing a variable with a constant (`eval-utils.c` `expression_hardcode_variable()`).
//!
//! The nodes are replaced by name; the source is then rewritten as text, by plain substring search, into a buffer
//! whose size C computes from the number of replaced nodes. The two disagree when the name is also the start of a
//! longer one: the text may change in other variables, or stay unchanged while the nodes already hold the constant.
//! `parsed_as` is never rebuilt.

use crate::ast::Node;
use crate::print::print_constant;

/// `expression_hardcode_node_variable()`: the leaves named `name` become `value`; how many did.
pub(crate) fn hardcode_nodes(nodes: &mut [Node], name: &[u8], value: f64) -> usize {
    let mut matches = 0;
    for node in nodes {
        if matches!(node, Node::Variable(found) if **found == *name) {
            *node = Node::Number(value);
            matches += 1;
        }
    }
    matches
}

/// The source rewrite, after `matches` leaves were replaced: the new source, or `None` when C leaves it unchanged.
pub(crate) fn hardcode_source(source: &[u8], name: &[u8], value: f64, mut matches: usize) -> Option<Vec<u8>> {
    let mut replace = Vec::new();
    print_constant(&mut replace, value);

    let plain = [&b"$"[..], name].concat();
    let braced = [&b"${"[..], name, &b"}"[..]].concat();

    // "source length + (max replacement length - min variable length) * matches + null terminator"
    let buffer_size = source.len() + 1 + matches * replace.len().saturating_sub(plain.len());

    let mut current: Option<Vec<u8>> = None;
    while matches != 0 {
        let matches_before = matches;
        for find in [&plain, &braced] {
            if matches == 0 {
                break;
            }
            let from = current.as_deref().unwrap_or(source);
            if let Some((rewritten, matched)) = str_replace_cpy(buffer_size, from, find, &replace) {
                current = Some(rewritten);
                matches -= matches.min(matched);
            }
        }
        if matches == matches_before {
            return None;
        }
    }
    current
}

/// `str_replace_cpy()` into a buffer of `dst_size` bytes: `src` with every `variable` replaced by `value` and how
/// many were, or `None` (C's 0) when there is none or the result with its terminator does not fit.
fn str_replace_cpy(dst_size: usize, src: &[u8], variable: &[u8], value: &[u8]) -> Option<(Vec<u8>, usize)> {
    if dst_size == 0 || variable.is_empty() || value.is_empty() {
        return None;
    }

    let mut dst = Vec::new();
    let mut matches = 0;
    let mut at = 0;
    let mut next = find(src, variable, 0)?;
    while at < src.len() {
        if at == next {
            if dst.len() + value.len() >= dst_size {
                return None;
            }
            matches += 1;
            dst.extend_from_slice(value);
            at += variable.len();
            // past the end when there is no further match
            next = find(src, variable, at).unwrap_or(src.len());
        } else {
            if dst.len() + 1 >= dst_size {
                return None;
            }
            dst.push(src[at]);
            at += 1;
        }
    }

    if dst.len() >= dst_size {
        return None;
    }
    Some((dst, matches))
}

/// `strstr()` from `from`.
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack[from..].windows(needle.len()).position(|window| window == needle).map(|at| from + at)
}
