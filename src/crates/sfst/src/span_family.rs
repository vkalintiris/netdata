//! The span relations the traces seal derives within one file (and the
//! query's live pass and trace-by-id derive the same way): which ERROR rows
//! originate an error, and how much of each row's time its children cover.
//!
//! Linking rules:
//! - a row is a **child** iff its trace id, span id and parent id are set and
//!   its parent id differs from its own span id, so a self-parent row is
//!   nobody's child, not even of its own resent copy;
//! - a row is a **parent** iff its trace id and span id are set; its children
//!   are every child row with the same trace id whose parent id is its span
//!   id, so every stored copy of a resent parent gets the same children and
//!   duplicate children change nothing;
//! - only direct children count, so cycles need no special case.
//!
//! Values:
//! - error origin: the row is ERROR and none of its children is;
//! - child time: the union of the children's `[start, start + duration)`
//!   intervals clipped to the row's own, so `0 ≤ child ≤ duration` and the
//!   row's self time is `duration − child`.
//!
//! O(n log n) over the rows, and independent of row order by construction.

use crate::{Error, ParentSpanIds, SpanId, SpanIds, TraceId, TraceIds};

/// Name of the seal-derived error-origin token (`_err_origin=true`).
pub const ERR_ORIGIN_FIELD: &str = "_err_origin";

/// Index-parallel per-row inputs, in any row order. Rows are stored copies: a
/// resent span is two rows.
pub struct SpanRows<'a> {
    pub trace_ids: &'a TraceIds,
    pub span_ids: &'a SpanIds,
    pub parent_span_ids: &'a ParentSpanIds,
    pub start_ns: &'a [i64],
    pub duration_ns: &'a [i64],
    /// Whether the row's status is ERROR.
    pub is_error: &'a [bool],
}

/// Index-parallel outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanFamily {
    /// ERROR, and no child is ERROR.
    pub error_origin: Vec<bool>,
    /// Time covered by the children, clipped to the row: `0 ≤ child ≤ duration`.
    pub child_ns: Vec<i64>,
}

fn check_len(column: &'static str, got: usize, expected: usize) -> Result<(), Error> {
    if got == expected {
        Ok(())
    } else {
        Err(Error::ColumnLengthMismatch {
            column,
            got,
            expected,
        })
    }
}

/// Derives every row's error origin and child time. Fails with
/// [`Error::ColumnLengthMismatch`] when the inputs differ in length.
pub fn derive_span_family(rows: &SpanRows<'_>) -> Result<SpanFamily, Error> {
    let n = rows.trace_ids.len();
    check_len(SpanIds::NAME, rows.span_ids.len(), n)?;
    check_len(ParentSpanIds::NAME, rows.parent_span_ids.len(), n)?;
    check_len("start_ns", rows.start_ns.len(), n)?;
    check_len("duration_ns", rows.duration_ns.len(), n)?;
    check_len("is_error", rows.is_error.len(), n)?;

    let mut children: Vec<(TraceId, SpanId, usize)> = Vec::new();
    for row in 0..n {
        let trace = rows.trace_ids.get(row);
        let span = rows.span_ids.get(row);
        let parent = rows.parent_span_ids.get(row);
        if !trace.is_unset() && !span.is_unset() && !parent.is_unset() && parent != span {
            children.push((trace, parent, row));
        }
    }
    children.sort_unstable();

    let mut error_origin = vec![false; n];
    let mut child_ns = vec![0; n];
    let mut intervals: Vec<(i64, i64)> = Vec::new();
    for row in 0..n {
        let trace = rows.trace_ids.get(row);
        let span = rows.span_ids.get(row);
        let own_start = rows.start_ns[row];
        let own_end = own_start.saturating_add(rows.duration_ns[row]);
        let mut child_error = false;
        intervals.clear();
        if !trace.is_unset() && !span.is_unset() {
            let from = children.partition_point(|&(t, p, _)| (t, p) < (trace, span));
            for &(t, p, child) in &children[from..] {
                if (t, p) != (trace, span) {
                    break;
                }
                child_error |= rows.is_error[child];
                let start = rows.start_ns[child];
                let end = start.saturating_add(rows.duration_ns[child]);
                let (start, end) = (start.max(own_start), end.min(own_end));
                if start < end {
                    intervals.push((start, end));
                }
            }
        }
        error_origin[row] = rows.is_error[row] && !child_error;
        child_ns[row] = covered(&mut intervals);
    }
    Ok(SpanFamily {
        error_origin,
        child_ns,
    })
}

/// The total length of the union of `intervals` (sorted in place).
fn covered(intervals: &mut [(i64, i64)]) -> i64 {
    intervals.sort_unstable();
    let mut total: i64 = 0;
    let mut current: Option<(i64, i64)> = None;
    for &(start, end) in intervals.iter() {
        current = match current {
            Some((open, close)) if start <= close => Some((open, close.max(end))),
            Some((open, close)) => {
                total = total.saturating_add(close.saturating_sub(open));
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    if let Some((open, close)) = current {
        total = total.saturating_add(close.saturating_sub(open));
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row: ids as single repeated bytes (0 = unset).
    #[derive(Clone, Copy)]
    struct Row {
        trace: u8,
        span: u8,
        parent: u8,
        start: i64,
        duration: i64,
        error: bool,
    }

    fn row(trace: u8, span: u8, parent: u8, start: i64, duration: i64, error: bool) -> Row {
        Row {
            trace,
            span,
            parent,
            start,
            duration,
            error,
        }
    }

    fn derive(rows: &[Row]) -> Result<SpanFamily, Error> {
        let mut trace_ids = TraceIds::with_capacity(rows.len());
        let mut span_ids = SpanIds::with_capacity(rows.len());
        let mut parent_span_ids = ParentSpanIds::with_capacity(rows.len());
        let mut start_ns = Vec::new();
        let mut duration_ns = Vec::new();
        let mut is_error = Vec::new();
        for r in rows {
            trace_ids.push(TraceId::from([r.trace; 16]));
            span_ids.push(SpanId::from([r.span; 8]));
            parent_span_ids.push(SpanId::from([r.parent; 8]));
            start_ns.push(r.start);
            duration_ns.push(r.duration);
            is_error.push(r.error);
        }
        derive_span_family(&SpanRows {
            trace_ids: &trace_ids,
            span_ids: &span_ids,
            parent_span_ids: &parent_span_ids,
            start_ns: &start_ns,
            duration_ns: &duration_ns,
            is_error: &is_error,
        })
    }

    const E: bool = true;
    const OK: bool = false;

    /// A case name, its rows, and the expected origins and child times.
    type Case = (&'static str, Vec<Row>, Vec<bool>, Vec<i64>);

    #[test]
    fn derivation_cases() {
        let near_max = i64::MAX - 10;
        let cases: Vec<Case> = vec![
            (
                "an ERROR chain marks only its leaf",
                vec![
                    row(1, 1, 0, 0, 100, E),
                    row(1, 2, 1, 10, 50, E),
                    row(1, 3, 2, 20, 10, E),
                ],
                vec![false, false, true],
                vec![50, 10, 0],
            ),
            (
                "an ERROR parent with OK children is an origin",
                vec![row(1, 1, 0, 0, 100, E), row(1, 2, 1, 10, 20, OK)],
                vec![true, false],
                vec![20, 0],
            ),
            (
                "ERROR, OK, ERROR gives two origins",
                vec![
                    row(1, 1, 0, 0, 100, E),
                    row(1, 2, 1, 10, 50, OK),
                    row(1, 3, 2, 20, 10, E),
                ],
                vec![true, false, true],
                vec![50, 10, 0],
            ),
            (
                "a lone ERROR row is an origin; OK rows never are",
                vec![row(1, 1, 0, 0, 10, E), row(2, 1, 0, 0, 10, OK)],
                vec![true, false],
                vec![0, 0],
            ),
            (
                "unset trace or span ids link nothing",
                vec![
                    row(0, 1, 0, 0, 100, E),
                    row(0, 2, 1, 10, 10, E),
                    row(1, 0, 0, 0, 100, E),
                    row(1, 3, 0, 0, 100, OK),
                    row(1, 4, 3, 10, 10, OK),
                ],
                vec![true, true, true, false, false],
                vec![0, 0, 0, 10, 0],
            ),
            (
                "a self-parent row and its resent copy are not each other's child",
                vec![row(1, 1, 1, 0, 100, E), row(1, 1, 1, 0, 100, E)],
                vec![true, true],
                vec![0, 0],
            ),
            (
                "every copy of a resent parent gets the same children",
                vec![
                    row(1, 1, 0, 0, 100, E),
                    row(1, 1, 0, 0, 100, E),
                    row(1, 2, 1, 10, 30, E),
                ],
                vec![false, false, true],
                vec![30, 30, 0],
            ),
            (
                "a duplicate child changes nothing",
                vec![
                    row(1, 1, 0, 0, 100, OK),
                    row(1, 2, 1, 10, 30, OK),
                    row(1, 2, 1, 10, 30, OK),
                ],
                vec![false, false, false],
                vec![30, 0, 0],
            ),
            (
                "overlapping children count once",
                vec![
                    row(1, 1, 0, 0, 100, OK),
                    row(1, 2, 1, 10, 30, OK),
                    row(1, 3, 1, 20, 30, OK),
                    row(1, 4, 1, 70, 10, OK),
                ],
                vec![false; 4],
                vec![50, 0, 0, 0],
            ),
            (
                "children outside the parent are clipped",
                vec![
                    row(1, 1, 0, 100, 100, OK),
                    row(1, 2, 1, 50, 100, OK),
                    row(1, 3, 1, 180, 100, OK),
                ],
                vec![false; 3],
                vec![70, 0, 0],
            ),
            (
                "a zero-length child and a zero-length parent cover nothing",
                vec![
                    row(1, 1, 0, 0, 100, OK),
                    row(1, 2, 1, 10, 0, OK),
                    row(2, 1, 0, 0, 0, OK),
                    row(2, 2, 1, 0, 10, OK),
                ],
                vec![false; 4],
                vec![0, 0, 0, 0],
            ),
            (
                "an end past i64::MAX saturates",
                vec![
                    row(1, 1, 0, near_max, 100, OK),
                    row(1, 2, 1, near_max + 5, 100, OK),
                ],
                vec![false, false],
                vec![5, 0],
            ),
            (
                "a cycle counts direct children only",
                vec![row(1, 1, 2, 0, 100, E), row(1, 2, 1, 10, 50, E)],
                vec![false, false],
                vec![50, 50],
            ),
            (
                "a child in another trace is not a child",
                vec![row(1, 1, 0, 0, 100, E), row(2, 2, 1, 10, 50, E)],
                vec![true, true],
                vec![0, 0],
            ),
        ];
        for (name, rows, origins, children) in cases {
            let family = derive(&rows).unwrap();
            assert_eq!(
                family,
                SpanFamily {
                    error_origin: origins,
                    child_ns: children
                },
                "{name}"
            );
            for (i, r) in rows.iter().enumerate() {
                assert!(
                    (0..=r.duration.max(0)).contains(&family.child_ns[i]),
                    "{name}: row {i}"
                );
            }
        }
    }

    #[test]
    fn derivation_is_order_independent() {
        let rows = vec![
            row(1, 1, 0, 0, 100, E),
            row(1, 2, 1, 10, 50, OK),
            row(1, 3, 2, 20, 10, E),
            row(1, 4, 1, 40, 30, E),
            row(2, 1, 0, 0, 100, E),
            row(2, 1, 0, 0, 100, E),
            row(2, 2, 1, 5, 5, OK),
            row(0, 7, 0, 0, 10, E),
        ];
        let original = derive(&rows).unwrap();
        let order = [5, 2, 7, 0, 3, 6, 1, 4];
        let shuffled: Vec<Row> = order.iter().map(|&i| rows[i]).collect();
        let family = derive(&shuffled).unwrap();
        for (at, &i) in order.iter().enumerate() {
            assert_eq!(family.error_origin[at], original.error_origin[i], "row {i}");
            assert_eq!(family.child_ns[at], original.child_ns[i], "row {i}");
        }
    }

    #[test]
    fn unequal_inputs_are_refused() {
        let trace_ids = TraceIds::with_capacity(0);
        let mut span_ids = SpanIds::with_capacity(1);
        span_ids.push(SpanId::from([1; 8]));
        let parent_span_ids = ParentSpanIds::with_capacity(0);
        let error = derive_span_family(&SpanRows {
            trace_ids: &trace_ids,
            span_ids: &span_ids,
            parent_span_ids: &parent_span_ids,
            start_ns: &[],
            duration_ns: &[],
            is_error: &[],
        })
        .unwrap_err();
        assert!(matches!(
            error,
            Error::ColumnLengthMismatch {
                got: 1,
                expected: 0,
                ..
            }
        ));
    }
}
