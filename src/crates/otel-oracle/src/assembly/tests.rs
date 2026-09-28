use super::*;
use crate::model::{Fields, STATUS_FIELD};

const TRACE: [u8; 16] = [7; 16];
const CLIENT: i32 = 3;
const INTERNAL: i32 = 1;

/// A stored row of `TRACE`: ids as repeated bytes (0 = unset), times in ns.
fn row(unit: usize, (id, parent): (u8, u8), kind: i32, start: i64, duration: i64) -> OracleSpan {
    OracleSpan {
        trace_id: Some(TRACE),
        span_id: (id != 0).then_some([id; 8]),
        parent_span_id: (parent != 0).then_some([parent; 8]),
        start_ns: start,
        duration_ns: duration,
        fields: Fields::default(),
        unit,
        self_ns: None,
        detail: SpanDetail {
            kind,
            ..SpanDetail::default()
        },
    }
}

fn failed(mut span: OracleSpan) -> OracleSpan {
    span.fields.insert(STATUS_FIELD, "error");
    span
}

fn named(mut span: OracleSpan, name: &str) -> OracleSpan {
    span.fields.insert("name", name);
    span
}

/// A served span as (id byte, start, duration, self time, error origin).
type Served = (u8, i64, i64, Option<i64>, Option<bool>);

fn served(view: &TraceView) -> Vec<Served> {
    let mut out = Vec::new();
    for (at, span) in view.spans.iter().enumerate() {
        out.push((
            span.span_id.map_or(0, |id| id[0]),
            span.start_ns,
            span.duration_ns,
            view.self_ns[at],
            view.error_origin[at],
        ));
    }
    out
}

fn first_view(rows: &[OracleSpan], cap: usize) -> TraceView {
    let assembly = assemble_trace(rows, TRACE, cap, &|_| true);
    assembly.view(&assembly.first_choice())
}

#[test]
fn the_earliest_copy_is_served_and_identical_copies_collapse() {
    let rows = vec![
        row(1, (1, 0), 2, 20, 900),
        row(0, (1, 0), 2, 10, 100),
        row(0, (2, 1), CLIENT, 30, 40),
        row(1, (2, 1), CLIENT, 30, 40),
        failed(row(2, (3, 1), CLIENT, 50, 10)),
    ];
    let assembly = assemble_trace(&rows, TRACE, 100, &|unit| unit < 2);
    assert_eq!(assembly.items.len(), 2);
    let view = assembly.view(&assembly.first_choice());
    assert_eq!(
        served(&view),
        vec![
            (1, 10, 100, Some(60), Some(false)),
            (2, 30, 40, Some(40), Some(false)),
        ]
    );
    assert_eq!(view.roots, vec![0]);
    assert_eq!(view.children, vec![vec![1], vec![]]);
    assert_eq!(view.summary_root, Some(0));
    assert!(!view.truncated);
}

#[test]
fn a_shared_id_hangs_its_child_under_the_server_but_counts_it_under_both() {
    let rows = vec![
        row(0, (1, 0), 2, 0, 1_000),
        row(0, (5, 1), CLIENT, 100, 500),
        row(0, (5, 1), 2, 120, 400),
        failed(row(0, (6, 5), CLIENT, 200, 100)),
    ];
    let view = first_view(&rows, 100);
    assert_eq!(
        served(&view),
        vec![
            (1, 0, 1_000, Some(500), Some(false)),
            (5, 100, 500, Some(400), Some(false)),
            (5, 120, 400, Some(300), Some(false)),
            (6, 200, 100, Some(100), Some(true)),
        ]
    );
    assert_eq!(view.children, vec![vec![1, 2], vec![], vec![3], vec![]]);

    let without_server = vec![rows[0].clone(), rows[1].clone(), rows[3].clone()];
    let view = first_view(&without_server, 100);
    assert_eq!(view.children, vec![vec![1], vec![2], vec![]]);
}

#[test]
fn kinds_are_part_of_a_span_s_identity() {
    let rows = vec![
        row(0, (4, 0), 42, 10, 5),
        row(0, (4, 0), 0, 10, 5),
        row(0, (4, 0), 0, 11, 5),
    ];
    let view = first_view(&rows, 100);
    let kinds: Vec<i32> = view.spans.iter().map(|span| span.detail.kind).collect();
    assert_eq!(kinds, vec![0, 42]);
    assert_eq!(view.roots, vec![0, 1]);
}

#[test]
fn spans_without_an_id_are_each_served_in_any_tie_order() {
    let rows = vec![
        named(row(0, (0, 1), INTERNAL, 50, 10), "a"),
        named(row(0, (0, 1), INTERNAL, 50, 10), "b"),
        row(0, (1, 0), 2, 0, 100),
    ];
    let assembly = assemble_trace(&rows, TRACE, 100, &|_| true);
    assert_eq!(assembly.items.len(), 3);
    let view = assembly.view(&assembly.first_choice());
    assert_eq!(view.children, vec![vec![1, 2], vec![], vec![]]);
    assert_eq!(
        view.self_ns[0],
        Some(100),
        "a span without an id is nobody's child"
    );

    let mut swapped = view.clone();
    swapped.spans.swap(1, 2);
    assert_eq!(trace_diff(&assembly, &swapped), None);

    let mut doubled = view.clone();
    doubled.spans[2] = doubled.spans[1].clone();
    assert!(trace_diff(&assembly, &doubled).is_some());
}

#[test]
fn copies_tied_on_start_are_equally_valid() {
    let rows = vec![
        row(0, (1, 0), 2, 0, 100),
        named(row(0, (1, 0), 2, 0, 100), "later write"),
        row(1, (1, 0), 2, 5, 100),
    ];
    let assembly = assemble_trace(&rows, TRACE, 100, &|_| true);
    assert_eq!(assembly.items.len(), 1);
    assert_eq!(assembly.items[0].copies.len(), 2);

    for copy in &assembly.items[0].copies {
        let view = assembly.view(std::slice::from_ref(copy));
        assert_eq!(trace_diff(&assembly, &view), None);
    }
    let late = assembly.view(&[rows[2].clone()]);
    assert!(trace_diff(&assembly, &late).is_some());
}

#[test]
fn the_cap_keeps_the_first_spans_and_derives_over_them() {
    let rows = vec![
        row(0, (1, 0), 2, 0, 100),
        row(0, (2, 1), CLIENT, 10, 30),
        failed(row(0, (3, 1), CLIENT, 50, 20)),
        row(1, (3, 1), CLIENT, 60, 20),
    ];
    let whole = first_view(&rows, 3);
    assert!(!whole.truncated, "a resend past the cap truncates nothing");
    assert_eq!(whole.spans.len(), 3);

    let capped = first_view(&rows, 2);
    assert!(capped.truncated);
    assert_eq!(
        served(&capped),
        vec![
            (1, 0, 100, Some(70), Some(false)),
            (2, 10, 30, Some(30), Some(false)),
        ]
    );
}

#[test]
fn roots_cover_orphans_self_parents_and_cycles() {
    let rows = vec![
        row(0, (1, 1), INTERNAL, 0, 10),
        row(0, (2, 9), INTERNAL, 1, 10),
        row(0, (3, 4), INTERNAL, 2, 10),
        row(0, (4, 3), INTERNAL, 3, 10),
        row(0, (5, 0), INTERNAL, 4, 10),
    ];
    let view = first_view(&rows, 100);
    assert_eq!(view.roots, vec![0, 1, 4, 2]);
    assert_eq!(
        view.children,
        vec![vec![], vec![], vec![3], vec![2], vec![]]
    );
    assert_eq!(
        view.summary_root,
        Some(4),
        "the first span with an unset parent"
    );

    let no_unset_parent = &rows[..4];
    assert_eq!(first_view(no_unset_parent, 100).summary_root, Some(0));
}

#[test]
fn the_diff_names_each_difference() {
    let rows = vec![
        row(0, (1, 0), 2, 0, 100),
        failed(named(row(0, (2, 1), CLIENT, 10, 30), "charge")),
    ];
    let assembly = assemble_trace(&rows, TRACE, 100, &|_| true);
    let good = assembly.view(&assembly.first_choice());
    assert_eq!(trace_diff(&assembly, &good), None);

    let mut wrong: Vec<TraceView> = Vec::new();
    let mut edit = |change: &dyn Fn(&mut TraceView)| {
        let mut view = good.clone();
        change(&mut view);
        wrong.push(view);
    };
    edit(&|view| view.spans[1].duration_ns += 1);
    edit(&|view| view.spans[1].start_ns += 1);
    edit(&|view| view.spans[1].parent_span_id = None);
    edit(&|view| view.spans[1].detail.flags = 1);
    edit(&|view| {
        view.spans[1]
            .fields
            .insert(("name".into(), "refund".into()));
    });
    edit(&|view| view.self_ns[0] = Some(100));
    edit(&|view| view.error_origin[1] = Some(false));
    edit(&|view| view.roots = vec![0, 1]);
    edit(&|view| view.children = vec![vec![], vec![]]);
    edit(&|view| view.summary_root = Some(1));
    edit(&|view| view.truncated = true);
    edit(&|view| {
        view.spans.pop();
    });
    for (at, view) in wrong.iter().enumerate() {
        assert!(
            trace_diff(&assembly, view).is_some(),
            "change {at} went unnoticed"
        );
    }
}

#[test]
fn settle_names_the_first_span_it_cannot_place() {
    let rows = vec![
        row(0, (1, 0), 2, 0, 100),
        named(row(0, (0, 1), INTERNAL, 50, 10), "a"),
        named(row(0, (0, 1), INTERNAL, 50, 10), "b"),
        row(0, (2, 1), CLIENT, 60, 20),
    ];
    let assembly = assemble_trace(&rows, TRACE, 100, &|_| true);
    let good = assembly.view(&assembly.first_choice());
    assert!(assembly.settle(&good).is_ok());

    let mut late = good.clone();
    late.spans[3].start_ns += 1;
    assert_eq!(assembly.settle(&late).unwrap_err(), Unsettled::At(3));

    let mut doubled = good.clone();
    doubled.spans[2] = doubled.spans[1].clone();
    assert_eq!(
        assembly.settle(&doubled).unwrap_err(),
        Unsettled::At(2),
        "a tie run serves each of its spans once"
    );

    let mut short = good.clone();
    short.spans.pop();
    assert_eq!(
        assembly.settle(&short).unwrap_err(),
        Unsettled::Count {
            got: 3,
            expected: 4
        }
    );
}
