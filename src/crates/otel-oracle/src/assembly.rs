//! One trace as trace-by-id returns it, assembled by brute force from every
//! stored copy of its spans, by the documented rules (the combiner's contract
//! and the trace docs of the design record, not the plugin's code):
//!
//! - a span is one per `(span id, raw kind)`; of its stored copies, the one
//!   with the earliest start is served, and copies with that same start are
//!   equally valid (the store breaks the tie by a hash of its own encoding);
//!   spans without a span id are each their own span;
//! - spans are ordered by `(start, span id, kind)`; spans without an id and
//!   with equal start and kind may come in any order;
//! - a cap keeps the first spans in that order, and the answer is truncated
//!   only when a further unique span exists;
//! - the tree: a span is a root when its parent id is unset, is its own id, or
//!   names no span of the trace; otherwise its parent is the first SERVER span
//!   carrying that id, else the first span carrying it; children are listed in
//!   order; roots are the natural roots in order, then, while a span is
//!   unreachable, the first unreachable one; the summary root is the first span
//!   whose parent id is unset, else the first root;
//! - self time and error origin come from [`crate::calc::derived_in`] over the
//!   served spans, the seal's rule, which counts a child under every span
//!   carrying its parent id and makes a span without an id nobody's child.
//!
//! Which files are read is decided per file, so the caller passes the units a
//! request reads, not a span-time window.

use std::collections::BTreeSet;

use crate::calc::derived_in;
use crate::model::{OracleSpan, SpanDetail};

const SERVER_KIND: i32 = 2;

/// A span as a comparison sees it: everything trace-by-id serves, with the row
/// fields as a set, minus the event and link fields (served in `detail`) and
/// the stored error-origin token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanContent {
    pub span_id: Option<[u8; 8]>,
    pub parent_span_id: Option<[u8; 8]>,
    pub start_ns: i64,
    pub duration_ns: i64,
    pub detail: SpanDetail,
    pub fields: BTreeSet<(String, String)>,
}

impl SpanContent {
    pub fn of(span: &OracleSpan) -> SpanContent {
        let mut fields = BTreeSet::new();
        for (name, values) in span.fields.iter() {
            let served = !name.starts_with("events.")
                && !name.starts_with("links.")
                && name != crate::model::ERR_ORIGIN_FIELD;
            if !served {
                continue;
            }
            for value in values {
                fields.insert((name.to_string(), value.to_string()));
            }
        }
        SpanContent {
            span_id: span.span_id,
            parent_span_id: span.parent_span_id,
            start_ns: span.start_ns,
            duration_ns: span.duration_ns,
            detail: span.detail.clone(),
            fields,
        }
    }
}

/// A trace-by-id answer, as the comparison sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceView {
    pub spans: Vec<SpanContent>,
    /// Parallel to `spans`.
    pub self_ns: Vec<Option<i64>>,
    pub error_origin: Vec<Option<bool>>,
    pub roots: Vec<usize>,
    pub children: Vec<Vec<usize>>,
    pub summary_root: Option<usize>,
    pub truncated: bool,
}

/// One span of the trace: the copies it may be served as, all with its
/// earliest start.
#[derive(Debug, Clone)]
pub struct Item {
    pub copies: Vec<OracleSpan>,
}

impl Item {
    fn key(&self) -> (i64, [u8; 8], i32) {
        let span = &self.copies[0];
        (
            span.start_ns,
            span.span_id.unwrap_or([0; 8]),
            span.detail.kind,
        )
    }
}

/// Why an answer's spans are not the ones the rules allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsettled {
    /// The answer serves `got` spans where the rules serve `expected`.
    Count { got: usize, expected: usize },
    /// The first span that is no stored copy allowed at its place.
    At(usize),
}

/// The trace's spans in order, before the cap, and whether the cap cuts it.
#[derive(Debug, Clone)]
pub struct Assembly {
    pub items: Vec<Item>,
    pub cap: usize,
}

impl Assembly {
    pub fn truncated(&self) -> bool {
        self.items.len() > self.cap
    }

    /// The copies served when every free choice takes its first option.
    pub fn first_choice(&self) -> Vec<OracleSpan> {
        let mut out = Vec::new();
        for item in self.items.iter().take(self.cap) {
            out.push(item.copies[0].clone());
        }
        out
    }

    /// The copies an answer served, when each is one the rules allow at its
    /// place: a copy of the span there, or, inside a run of spans without an
    /// id that tie on start and kind, a copy of an unused span of that run.
    pub fn settle(&self, got: &TraceView) -> Result<Vec<OracleSpan>, Unsettled> {
        let served = self.items.len().min(self.cap);
        if got.spans.len() != served {
            return Err(Unsettled::Count {
                got: got.spans.len(),
                expected: served,
            });
        }
        let mut kept = Vec::with_capacity(served);
        let mut start = 0;
        while start < served {
            let mut end = start + 1;
            while end < self.items.len() && self.items[end].key() == self.items[start].key() {
                end += 1;
            }
            let mut used = vec![false; end - start];
            for at in start..end.min(served) {
                let mut found = None;
                for (offset, item) in self.items[start..end].iter().enumerate() {
                    if used[offset] {
                        continue;
                    }
                    for copy in &item.copies {
                        if SpanContent::of(copy) == got.spans[at] {
                            found = Some((offset, copy));
                            break;
                        }
                    }
                    if found.is_some() {
                        break;
                    }
                }
                let Some((offset, copy)) = found else {
                    return Err(Unsettled::At(at));
                };
                used[offset] = true;
                kept.push(copy.clone());
            }
            start = end;
        }
        Ok(kept)
    }

    /// The answer serving `kept`: the tree, the summary root and the derived
    /// values over those spans.
    pub fn view(&self, kept: &[OracleSpan]) -> TraceView {
        let (roots, children) = tree(kept);
        let summary_root = summary_root(kept, &roots);
        let mut scoped = kept.to_vec();
        for span in &mut scoped {
            span.unit = 0;
        }
        let derived = derived_in(&scoped, &|_| Some(0));
        let mut self_ns = Vec::with_capacity(kept.len());
        let mut error_origin = Vec::with_capacity(kept.len());
        for (span, values) in kept.iter().zip(&derived) {
            self_ns.push(Some(span.duration_ns.saturating_sub(values.child_ns)));
            error_origin.push(Some(values.error_origin));
        }
        let mut spans = Vec::with_capacity(kept.len());
        for span in kept {
            spans.push(SpanContent::of(span));
        }
        TraceView {
            spans,
            self_ns,
            error_origin,
            roots,
            children,
            summary_root,
            truncated: self.truncated(),
        }
    }
}

/// The trace `trace_id` from the rows of the units `units` accepts, with
/// `cap` spans at most.
pub fn assemble_trace(
    rows: &[OracleSpan],
    trace_id: [u8; 16],
    cap: usize,
    units: &dyn Fn(usize) -> bool,
) -> Assembly {
    let mut by_key: std::collections::BTreeMap<([u8; 8], i32), Vec<&OracleSpan>> =
        std::collections::BTreeMap::new();
    let mut items = Vec::new();
    for row in rows {
        if row.trace_id != Some(trace_id) || !units(row.unit) {
            continue;
        }
        match row.span_id {
            Some(id) => by_key.entry((id, row.detail.kind)).or_default().push(row),
            None => items.push(Item {
                copies: vec![row.clone()],
            }),
        }
    }
    for copies in by_key.into_values() {
        let mut earliest = i64::MAX;
        for copy in &copies {
            earliest = earliest.min(copy.start_ns);
        }
        let mut distinct: Vec<OracleSpan> = Vec::new();
        for copy in copies {
            let same = distinct
                .iter()
                .any(|kept| SpanContent::of(kept) == SpanContent::of(copy));
            if copy.start_ns == earliest && !same {
                distinct.push(copy.clone());
            }
        }
        items.push(Item { copies: distinct });
    }
    items.sort_by_key(Item::key);
    Assembly { items, cap }
}

fn tree(spans: &[OracleSpan]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let parent_of = |index: usize| -> Option<usize> {
        let span = &spans[index];
        let parent = span.parent_span_id?;
        if span.span_id == Some(parent) {
            return None;
        }
        let mut first = None;
        for (at, candidate) in spans.iter().enumerate() {
            if candidate.span_id != Some(parent) {
                continue;
            }
            if candidate.detail.kind == SERVER_KIND {
                return Some(at);
            }
            first = first.or(Some(at));
        }
        first
    };

    let mut children = vec![Vec::new(); spans.len()];
    let mut roots = Vec::new();
    for index in 0..spans.len() {
        match parent_of(index) {
            Some(parent) => children[parent].push(index),
            None => roots.push(index),
        }
    }

    let mut reached = vec![false; spans.len()];
    let reach = |from: usize, reached: &mut Vec<bool>| {
        let mut stack = vec![from];
        while let Some(at) = stack.pop() {
            if reached[at] {
                continue;
            }
            reached[at] = true;
            stack.extend(children[at].iter().copied());
        }
    };
    for &root in &roots {
        reach(root, &mut reached);
    }
    for index in 0..spans.len() {
        if !reached[index] {
            roots.push(index);
            reach(index, &mut reached);
        }
    }
    (roots, children)
}

fn summary_root(spans: &[OracleSpan], roots: &[usize]) -> Option<usize> {
    for (index, span) in spans.iter().enumerate() {
        if span.parent_span_id.is_none() {
            return Some(index);
        }
    }
    roots.iter().copied().min()
}

/// How `got` differs from what the rules allow for `want`; `None` when it
/// does not.
pub fn trace_diff(want: &Assembly, got: &TraceView) -> Option<String> {
    if got.truncated != want.truncated() {
        return Some(format!(
            "truncated {}, expected {}",
            got.truncated,
            want.truncated()
        ));
    }
    let kept = match want.settle(got) {
        Ok(kept) => kept,
        Err(Unsettled::Count { got, expected }) => {
            return Some(format!("{got} spans, expected {expected}"));
        }
        Err(Unsettled::At(at)) => {
            return Some(format!(
                "span {at} is no stored copy allowed there: {:?}",
                got.spans[at]
            ));
        }
    };
    let expected = want.view(&kept);
    let checks: [(&str, bool); 5] = [
        ("self time", expected.self_ns == got.self_ns),
        ("error origin", expected.error_origin == got.error_origin),
        ("roots", expected.roots == got.roots),
        ("children", expected.children == got.children),
        ("summary root", expected.summary_root == got.summary_root),
    ];
    for (what, same) in checks {
        if !same {
            return Some(format!(
                "{what} differs: expected {expected:?}, got {got:?}"
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests;
