//! Trace-by-id at tier 2: which traces to ask for in a window, and how an
//! answer compares with the calculator's assembly of the same trace from the
//! rows of the units the request reads ([`crate::matching::trace_units`]).
//!
//! The traces are picked from the rows alone: the slowest entry span's trace,
//! then, each the first in trace id byte order that fits and is not asked
//! yet, an error origin's, one stored in two units, one in a sealed and a
//! live unit, one with a span stored twice; and the largest, also asked with
//! a cap one span short.
//!
//! A judged answer gives ORC-TRACE findings: its echo, its span count, the
//! first span that is no stored copy allowed at its place (at the first part
//! that differs), then the derived values and the tree. No finding carries a
//! served span's text, only aliased values and ids.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{Ask, Judge, Plan, Request, hex, in_window, name, parse_hex, reasons};
use crate::assembly::{self, SpanContent, TraceView, Unsettled};
use crate::calc::{self, Grid, Scope};
use crate::model::{ERR_ORIGIN_FIELD, OracleSpan, SpanEvent, SpanLink};
use crate::report::{CheckCount, Finding, Locator, Subject};
use crate::wire;

/// The span cap asked by default: the documented largest cap.
pub const TRACE_SPAN_CAP: usize = 65_536;

/// What the picks need to know of one trace's rows.
#[derive(Debug, Default)]
struct TraceRows {
    rows: usize,
    in_window: bool,
    units: BTreeSet<usize>,
    live: bool,
    sealed: bool,
    spans: BTreeSet<([u8; 8], i32)>,
    resent: bool,
    origin: bool,
}

/// A trace to ask for: (label, trace id, span cap).
type Pick = (&'static str, [u8; 16], usize);

/// A trace to ask for when one fits.
type Criterion = (&'static str, fn(&TraceRows) -> bool);

fn picks(rows: &[OracleSpan], grid: &Grid, live: &dyn Fn(usize) -> bool) -> Vec<Pick> {
    let mut traces: BTreeMap<[u8; 16], TraceRows> = BTreeMap::new();
    for row in rows {
        let Some(id) = row.trace_id else {
            continue;
        };
        let trace = traces.entry(id).or_default();
        trace.rows += 1;
        trace.in_window |= in_window(row, grid);
        trace.units.insert(row.unit);
        if live(row.unit) {
            trace.live = true;
        } else {
            trace.sealed = true;
        }
        if let Some(span_id) = row.span_id
            && !trace.spans.insert((span_id, row.detail.kind))
        {
            trace.resent = true;
        }
        trace.origin |= row.has(ERR_ORIGIN_FIELD, "true");
    }
    traces.retain(|_, trace| trace.in_window);

    let mut out: Vec<Pick> = Vec::new();
    let asked = |out: &[Pick], id: [u8; 16]| out.iter().any(|&(_, at, _)| at == id);
    let slowest = calc::slowest(rows, grid, &Scope::entry_spans(), 1)
        .first()
        .and_then(|span| span.trace_id);
    if let Some(id) = slowest {
        out.push(("slowest entry", id, TRACE_SPAN_CAP));
    }
    let criteria: [Criterion; 4] = [
        ("error origin", |t| t.origin),
        ("two units", |t| t.units.len() > 1),
        ("sealed and live", |t| t.sealed && t.live),
        ("resent", |t| t.resent),
    ];
    for (label, fits) in criteria {
        for (id, trace) in &traces {
            if fits(trace) && !asked(&out, *id) {
                out.push((label, *id, TRACE_SPAN_CAP));
                break;
            }
        }
    }
    let mut largest: Option<([u8; 16], usize)> = None;
    for (id, trace) in &traces {
        if largest.is_none_or(|(_, rows)| trace.rows > rows) {
            largest = Some((*id, trace.rows));
        }
    }
    if let Some((id, _)) = largest {
        if !asked(&out, id) {
            out.push(("largest", id, TRACE_SPAN_CAP));
        }
        let spans = assembly::assemble_trace(rows, id, usize::MAX, &|_| true)
            .items
            .len();
        if spans > 1 {
            out.push(("largest capped", id, spans - 1));
        }
    }
    out
}

/// Adds the window's trace asks, bounded by its grid, to `plan`; `rows` are
/// those of the units such a request reads, and `live` says which units are
/// a live WAL's. Returns how many were added.
pub fn add_traces(plan: &mut Plan, rows: &[OracleSpan], live: &dyn Fn(usize) -> bool) -> usize {
    let grid = Grid::for_window(plan.after_s, plan.before_s);
    let mut added = 0;
    for (label, trace_id, span_cap) in picks(rows, &grid, live) {
        plan.requests.push(Request {
            id: format!("trace {label}"),
            ask: Ask::Trace {
                trace_id,
                after_s: grid.after_s,
                before_s: grid.before_s,
                span_cap,
            },
            body: json!({"trace": {
                "id": hex(&trace_id),
                "after": grid.after_s,
                "before": grid.before_s,
                "span_cap": span_cap,
            }}),
        });
        added += 1;
    }
    added
}

fn span_id(text: &str) -> Option<Option<[u8; 8]>> {
    let id = parse_hex::<8>(text)?;
    Some((id != [0; 8]).then_some(id))
}

/// The answer as the comparison sees it, the way the calculator describes a
/// span: unset ids as `None`, attributes sorted, fields as a set; `None` when
/// an id is not hex of its length.
pub fn trace_view(answer: &wire::TraceAnswer) -> Option<TraceView> {
    let mut view = TraceView {
        spans: Vec::new(),
        self_ns: Vec::new(),
        error_origin: Vec::new(),
        roots: answer.roots.clone(),
        children: answer.children.clone(),
        summary_root: answer.summary_root,
        truncated: answer.status.reasons().contains_key("size_cap"),
    };
    for span in &answer.spans {
        let parent_span_id = match &span.parent_span_id {
            Some(text) => span_id(text)?,
            None => None,
        };
        let mut events = Vec::new();
        for event in &span.events {
            let mut attributes = event.attributes.clone();
            attributes.sort();
            events.push(SpanEvent {
                time_unix_nano: event.time_unix_nano,
                name: event.name.clone(),
                dropped_attributes_count: event.dropped_attributes_count,
                attributes,
            });
        }
        let mut links = Vec::new();
        for link in &span.links {
            let mut attributes = link.attributes.clone();
            attributes.sort();
            links.push(SpanLink {
                trace_id: parse_hex::<16>(&link.trace_id)?,
                span_id: parse_hex::<8>(&link.span_id)?,
                trace_state: link.trace_state.clone(),
                flags: link.flags,
                dropped_attributes_count: link.dropped_attributes_count,
                attributes,
            });
        }
        view.spans.push(SpanContent {
            span_id: span_id(&span.span_id)?,
            parent_span_id,
            start_ns: span.start_ns,
            duration_ns: span.duration_ns,
            detail: crate::model::SpanDetail {
                kind: span.kind,
                flags: span.flags,
                dropped_attributes_count: span.dropped_attributes_count,
                dropped_events_count: span.dropped_events_count,
                dropped_links_count: span.dropped_links_count,
                events,
                links,
            },
            fields: span.fields.iter().cloned().collect(),
        });
        view.self_ns.push(span.self_duration_ns);
        view.error_origin.push(span.error_origin);
    }
    Some(view)
}

fn id_subject(id: Option<[u8; 8]>) -> Subject {
    id.map_or(Subject::Missing, Subject::Span)
}

/// The first part of a served span that differs from the stored copy
/// expected at its place: where, and what each side holds there.
fn first_difference(want: &SpanContent, got: &SpanContent) -> (Locator, Subject, Subject) {
    let part = |text: &str| Locator::Name(text.to_string());
    let count = |n: u32| Subject::Count(u64::from(n));
    let (a, b) = (&want.detail, &got.detail);
    let scalars = [
        ("span id", id_subject(want.span_id), id_subject(got.span_id)),
        (
            "parent span id",
            id_subject(want.parent_span_id),
            id_subject(got.parent_span_id),
        ),
        (
            "start",
            Subject::Ns(want.start_ns),
            Subject::Ns(got.start_ns),
        ),
        (
            "duration",
            Subject::Ns(want.duration_ns),
            Subject::Ns(got.duration_ns),
        ),
        (
            "kind",
            Subject::Name(a.kind.to_string()),
            Subject::Name(b.kind.to_string()),
        ),
        ("flags", count(a.flags), count(b.flags)),
        (
            "dropped attributes",
            count(a.dropped_attributes_count),
            count(b.dropped_attributes_count),
        ),
        (
            "dropped events",
            count(a.dropped_events_count),
            count(b.dropped_events_count),
        ),
        (
            "dropped links",
            count(a.dropped_links_count),
            count(b.dropped_links_count),
        ),
    ];
    for (what, want, got) in scalars {
        if want != got {
            return (part(what), want, got);
        }
    }
    if a.events != b.events {
        return (part("events"), Subject::Flag(true), Subject::Flag(false));
    }
    if a.links != b.links {
        return (part("links"), Subject::Flag(true), Subject::Flag(false));
    }
    let value = |(field, value): &(String, String)| Subject::Value {
        field: field.clone(),
        value: value.clone(),
    };
    let mut only_want = want.fields.difference(&got.fields);
    let mut only_got = got.fields.difference(&want.fields);
    match (only_want.next(), only_got.next()) {
        (Some(w), Some(g)) if g < w => (part(&g.0), Subject::Missing, value(g)),
        (Some(w), _) => (part(&w.0), value(w), Subject::Missing),
        (None, Some(g)) => (part(&g.0), Subject::Missing, value(g)),
        (None, None) => (part("span"), Subject::Flag(true), Subject::Flag(true)),
    }
}

fn indexes(list: &[usize]) -> Vec<Subject> {
    list.iter().map(|&i| Subject::Count(i as u64)).collect()
}

fn judge_trace(
    judge: &mut Judge,
    scenario: &str,
    ask: (&[u8; 16], u32, u32),
    want: &assembly::Assembly,
    got: &wire::TraceAnswer,
) {
    let (trace_id, after_s, before_s) = ask;
    let echo = [
        Subject::Name("trace".into()),
        Subject::Trace(*trace_id),
        Subject::Count(u64::from(after_s)),
        Subject::Count(u64::from(before_s)),
        Subject::Count(got.spans.len() as u64),
    ];
    let got_echo = [
        Subject::Name(got.mode.clone()),
        parse_hex::<16>(&got.trace_id).map_or(Subject::Missing, Subject::Trace),
        Subject::Count(u64::from(got.coverage.after)),
        Subject::Count(u64::from(got.coverage.before)),
        Subject::Count(got.items.returned as u64),
    ];
    judge.list("ORC-TRACE", scenario, vec![name("echo")], &echo, &got_echo);

    let mut size_cap = Vec::new();
    if want.truncated() {
        size_cap.push(calc::Reason {
            reason: "size_cap",
            count: 1,
            of: None,
            detail: BTreeSet::new(),
        });
    }
    reasons(judge, scenario, "status", &size_cap, &got.status);

    let Some(view) = trace_view(got) else {
        judge.compare(
            "ORC-ANSWER",
            scenario,
            vec![name("spans")],
            Subject::Name("hex ids".into()),
            Subject::Missing,
        );
        return;
    };
    let served = want.items.len().min(want.cap);
    judge.compare(
        "ORC-TRACE",
        scenario,
        vec![name("spans")],
        Subject::Count(served as u64),
        Subject::Count(view.spans.len() as u64),
    );
    let kept = match want.settle(&view) {
        Ok(kept) => kept,
        Err(Unsettled::Count { .. }) => return,
        Err(Unsettled::At(at)) => {
            let expected = SpanContent::of(&want.items[at].copies[0]);
            let (part, want, got) = first_difference(&expected, &view.spans[at]);
            let at = vec![name("spans"), Locator::Index(at), part];
            judge.compare("ORC-TRACE", scenario, at, want, got);
            return;
        }
    };
    let expected = want.view(&kept);
    let ns = |list: &[Option<i64>]| -> Vec<Subject> {
        list.iter()
            .map(|n| n.map_or(Subject::Missing, Subject::Ns))
            .collect()
    };
    let flags = |list: &[Option<bool>]| -> Vec<Subject> {
        list.iter()
            .map(|b| b.map_or(Subject::Missing, Subject::Flag))
            .collect()
    };
    let lists = |children: &[Vec<usize>]| -> Vec<Subject> {
        children
            .iter()
            .map(|list| Subject::Name(format!("{list:?}")))
            .collect()
    };
    let root = |at: Option<usize>| at.map_or(Subject::Missing, |i| Subject::Count(i as u64));
    let checks = [
        ("self time", ns(&expected.self_ns), ns(&view.self_ns)),
        (
            "error origin",
            flags(&expected.error_origin),
            flags(&view.error_origin),
        ),
        ("roots", indexes(&expected.roots), indexes(&view.roots)),
        ("children", lists(&expected.children), lists(&view.children)),
        (
            "summary root",
            vec![root(expected.summary_root)],
            vec![root(view.summary_root)],
        ),
    ];
    for (what, want, got) in checks {
        judge.list("ORC-TRACE", scenario, vec![name(what)], &want, &got);
    }
}

/// Judges `plan`'s trace asks against the calculator's assembly over `rows`,
/// those of the units the requests read.
pub fn judge_traces(
    plan: &Plan,
    rows: &[OracleSpan],
    answers: &BTreeMap<String, Value>,
) -> (Vec<Finding>, BTreeMap<String, CheckCount>) {
    let mut judge = Judge::default();
    for request in &plan.requests {
        let Ask::Trace {
            trace_id,
            after_s,
            before_s,
            span_cap,
        } = &request.ask
        else {
            continue;
        };
        let got = answers
            .get(&request.id)
            .and_then(|answer| serde_json::from_value::<wire::TraceAnswer>(answer.clone()).ok());
        let Some(got) = got else {
            judge.compare(
                "ORC-ANSWER",
                &request.id,
                vec![name(&request.id)],
                Subject::Name("a trace".into()),
                Subject::Missing,
            );
            continue;
        };
        let want = assembly::assemble_trace(rows, *trace_id, *span_cap, &|_| true);
        judge_trace(
            &mut judge,
            &request.id,
            (trace_id, *after_s, *before_s),
            &want,
            &got,
        );
    }
    (judge.findings, judge.checks)
}

#[cfg(test)]
mod tests;
