//! The tier-2 comparison: the requests to send the live agent for one judged
//! window, and how its answers compare with the calculator's over the rows of
//! the units that window overlaps.
//!
//! Each difference is one finding at the leaf that differs (a count, a flag,
//! a value); a list that differs gives one finding at its first difference, so
//! one wrong number never shows up as several.
//!
//! Row pages past the first depend on the first page's answer: the runner
//! adds them with [`add_pages`] once it has the answers, asks them, and
//! judges them with the rest.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::calc::{self, Grid, Scope, Selection, Tier, fixed_histogram};
use crate::model::{DURATION_BAND_FIELD, OracleSpan, ROLE_FIELD, SERVICE_FIELD, STATUS_FIELD};
use crate::report::{CheckCount, Finding, Locator, Subject};
use crate::wire;

mod traces;

pub use traces::{TRACE_SPAN_CAP, add_traces, judge_traces, trace_view};

/// Rows asked in a newest page and in each slowest list.
pub const ROWS_LIMIT: usize = 100;
/// The larger slowest list, asked for every span only.
pub const SLOWEST_WIDE: usize = 1_000;
/// Values asked per suggestion request.
pub const VALUES_LIMIT: usize = 100;
/// Trace ids in the trace-id scope.
const SCOPE_TRACES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    /// Readable: no stored value appears in it.
    pub name: String,
    pub scope: Scope,
    /// A W3 selection: facets are compared under it and rows follow it.
    pub selection: Option<Selection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowsAsk {
    /// The first newest page.
    Newest(usize),
    /// A newest page past `anchor`, the key of a row the agent returned.
    Page {
        limit: usize,
        anchor: calc::RowKey,
        walk: calc::Walk,
    },
    Slowest(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Explore {
        scenario: usize,
        stack: Option<String>,
        facets: bool,
        groups: bool,
        rows: Option<RowsAsk>,
        fields: bool,
    },
    Values {
        field: String,
        prefix: String,
    },
    /// Trace-by-id over the window's grid, judged by [`judge_traces`].
    Trace {
        trace_id: [u8; 16],
        after_s: u32,
        before_s: u32,
        span_cap: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    pub ask: Ask,
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub after_s: u32,
    pub before_s: u32,
    /// Sources the agent reads for the window (its partial counts are out of
    /// these).
    pub candidates: u64,
    pub scenarios: Vec<Scenario>,
    pub requests: Vec<Request>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn first_value<'a>(span: &'a OracleSpan, field: &str) -> Option<&'a str> {
    span.fields
        .get(field)
        .and_then(|values| values.iter().next())
}

/// The value of `field` most frequent among `rows` (ties to the first in byte
/// order).
fn most_frequent<'a>(rows: &[&'a OracleSpan], field: &str) -> Option<&'a str> {
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for span in rows {
        if let Some(value) = first_value(span, field) {
            *counts.entry(value).or_default() += 1;
        }
    }
    let mut best: Option<(&str, u64)> = None;
    for (value, count) in counts {
        if best.is_none_or(|(_, top)| count > top) {
            best = Some((value, count));
        }
    }
    best.map(|(value, _)| value)
}

fn in_window(span: &OracleSpan, grid: &Grid) -> bool {
    grid.bucket_of(span.start_ns).is_some()
}

/// Up to three traces for the trace-id scope: the slowest entry span's, an
/// error's, and one stored in two units.
fn scope_traces(spans: &[OracleSpan], grid: &Grid) -> Vec<[u8; 16]> {
    let mut picked: Vec<[u8; 16]> = Vec::new();
    let mut add = |id: Option<[u8; 16]>| {
        if let Some(id) = id
            && !picked.contains(&id)
            && picked.len() < SCOPE_TRACES
        {
            picked.push(id);
        }
    };
    add(calc::slowest(spans, grid, &Scope::entry_spans(), 1)
        .first()
        .and_then(|span| span.trace_id));
    add(spans
        .iter()
        .filter(|span| in_window(span, grid) && span.is_error())
        .find_map(|span| span.trace_id));
    let mut units: BTreeMap<[u8; 16], BTreeSet<usize>> = BTreeMap::new();
    for span in spans.iter().filter(|span| in_window(span, grid)) {
        if let Some(id) = span.trace_id {
            units.entry(id).or_default().insert(span.unit);
        }
    }
    add(units
        .into_iter()
        .find(|(_, units)| units.len() > 1)
        .map(|(id, _)| id));
    picked
}

fn scenarios(spans: &[OracleSpan], grid: &Grid) -> Vec<Scenario> {
    let entry: Vec<&OracleSpan> = spans
        .iter()
        .filter(|span| in_window(span, grid) && Scope::entry_spans().matches(span))
        .collect();
    let mut out = vec![
        Scenario {
            name: "F0 every span".to_string(),
            scope: Scope::default(),
            selection: None,
        },
        Scenario {
            name: "F1 entry spans".to_string(),
            scope: Scope::entry_spans(),
            selection: None,
        },
    ];
    if let Some(service) = most_frequent(&entry, SERVICE_FIELD) {
        out.push(Scenario {
            name: "F2 entry spans of the busiest service".to_string(),
            scope: Scope::entry_spans().with(SERVICE_FIELD, &[service]),
            selection: None,
        });
    }
    if let Some(name) = most_frequent(&entry, "name") {
        out.push(Scenario {
            name: "F4 the busiest operation's name as text".to_string(),
            scope: Scope::default().with_text(name),
            selection: None,
        });
    }
    let entry_scope = Scope::entry_spans();
    for (label, selection) in selections(spans, grid, &entry_scope) {
        out.push(Scenario {
            name: format!("F1 entry spans × {label}"),
            scope: entry_scope.clone(),
            selection: Some(selection),
        });
    }
    let traces = scope_traces(spans, grid);
    if !traces.is_empty() {
        out.push(Scenario {
            name: format!("F5 {} trace ids", traces.len()),
            scope: Scope::default().with_trace_ids(&traces),
            selection: None,
        });
    }
    out.push(Scenario {
        name: "F6 entry spans with an unset status".to_string(),
        scope: Scope::entry_spans().with(STATUS_FIELD, &["unset"]),
        selection: None,
    });
    out
}

/// The W3 selections judged: errors, the slow bands, the middle third of the
/// window, durations at least the scope's p95, and errors or an unset status.
fn selections(spans: &[OracleSpan], grid: &Grid, scope: &Scope) -> Vec<(&'static str, Selection)> {
    let mut out = vec![
        (
            "E1 errors",
            Selection {
                terms: Scope::default().with(STATUS_FIELD, &["error"]),
                ..Selection::default()
            },
        ),
        (
            "E2 slow bands",
            Selection {
                terms: Scope::default().with(DURATION_BAND_FIELD, &["100ms-1s", "1-10s"]),
                ..Selection::default()
            },
        ),
    ];
    let start = i64::from(grid.after_s) * 1_000_000_000;
    let third = (i64::from(grid.before_s) - i64::from(grid.after_s)) * 1_000_000_000 / 3;
    out.push((
        "E3 middle third",
        Selection {
            time_ns: Some((start + third, start + 2 * third)),
            ..Selection::default()
        },
    ));
    let mut durations: Vec<i64> = Vec::new();
    for span in spans {
        if in_window(span, grid) && scope.matches(span) {
            durations.push(span.duration_ns);
        }
    }
    durations.sort_unstable();
    if !durations.is_empty() {
        let p95 = durations[(durations.len() * 95).div_ceil(100) - 1];
        out.push((
            "E4 at least the p95",
            Selection {
                duration: Some((Some(p95), None)),
                ..Selection::default()
            },
        ));
    }
    out.push((
        "E5 errors or unset",
        Selection {
            terms: Scope::default().with(STATUS_FIELD, &["error", "unset"]),
            ..Selection::default()
        },
    ));
    out
}

/// A scope's chips as the wire's filter: `null` for the rows without a field.
fn chips_json(scope: &Scope) -> Value {
    let mut filter = serde_json::Map::new();
    for (field, wanted) in &scope.terms {
        let mut values: Vec<Value> = wanted.values.iter().map(|v| json!(v)).collect();
        if wanted.absent {
            values.push(Value::Null);
        }
        filter.insert(field.clone(), Value::Array(values));
    }
    Value::Object(filter)
}

fn selection_json(selection: &Selection) -> Value {
    let mut out = json!({});
    if !selection.terms.terms.is_empty() {
        out["filter"] = chips_json(&selection.terms);
    }
    if let Some((min, max)) = selection.duration {
        let mut duration = json!({});
        if let Some(min) = min {
            duration["min_ns"] = json!(min);
        }
        if let Some(max) = max {
            duration["max_ns"] = json!(max);
        }
        out["duration"] = duration;
    }
    if let Some((after, before)) = selection.time_ns {
        out["time"] = json!({"after_ns": after.to_string(), "before_ns": before.to_string()});
    }
    out
}

fn explore_body(
    after_s: u32,
    before_s: u32,
    scope: &Scope,
    selection: Option<&Selection>,
    sections: Value,
) -> Value {
    let mut explore = json!({"after": after_s, "before": before_s, "sections": sections});
    if let Some(selection) = selection {
        explore["selection"] = selection_json(selection);
    }
    if !scope.terms.is_empty() {
        explore["filter"] = chips_json(scope);
    }
    if let Some(text) = &scope.text {
        explore["text"] = json!(text);
    }
    if !scope.trace_ids.is_empty() {
        let ids: Vec<String> = scope.trace_ids.iter().map(|id| hex(id)).collect();
        explore["trace_ids"] = json!(ids);
    }
    json!({ "explore": explore })
}

/// The requests for the window `[after_s, before_s)` over the rows of the
/// units that the window overlaps once aligned to its grid (the agent answers
/// explore requests over the aligned window, and echoes it; value suggestions
/// keep the window as sent).
pub fn plan(after_s: u32, before_s: u32, spans: &[OracleSpan], candidates: u64) -> Plan {
    let grid = Grid::for_window(after_s, before_s);
    let scenarios = scenarios(spans, &grid);
    let mut requests = Vec::new();
    for (index, scenario) in scenarios.iter().enumerate() {
        let everything = index == 0;
        let mut first = json!({
            "histogram": {"stack": STATUS_FIELD, "percentiles": true},
            "facets": {},
            "groups": {},
            "rows": {"order": "newest", "limit": ROWS_LIMIT},
        });
        if everything {
            first["fields"] = json!({});
        }
        let mut asks = vec![(
            "status",
            Ask::Explore {
                scenario: index,
                stack: Some(STATUS_FIELD.to_string()),
                facets: true,
                groups: true,
                rows: Some(RowsAsk::Newest(ROWS_LIMIT)),
                fields: everything,
            },
            first,
        )];
        for (label, stack) in [("band", DURATION_BAND_FIELD), ("service", SERVICE_FIELD)] {
            asks.push((
                label,
                Ask::Explore {
                    scenario: index,
                    stack: Some(stack.to_string()),
                    facets: false,
                    groups: false,
                    rows: None,
                    fields: false,
                },
                json!({"histogram": {"stack": stack, "percentiles": true}}),
            ));
        }
        let mut slowest = vec![ROWS_LIMIT];
        if everything {
            slowest.push(SLOWEST_WIDE);
        }
        for k in slowest {
            asks.push((
                if k == ROWS_LIMIT {
                    "slowest"
                } else {
                    "slowest-wide"
                },
                Ask::Explore {
                    scenario: index,
                    stack: None,
                    facets: false,
                    groups: false,
                    rows: Some(RowsAsk::Slowest(k)),
                    fields: false,
                },
                json!({"rows": {"order": "slowest", "limit": k}}),
            ));
        }
        for (label, ask, sections) in asks {
            requests.push(Request {
                id: format!("{}:{label}", scenario.name),
                body: explore_body(
                    after_s,
                    before_s,
                    &scenario.scope,
                    scenario.selection.as_ref(),
                    sections,
                ),
                ask,
            });
        }
    }

    let tiers = calc::field_list(spans);
    let pick = |tier: Tier| {
        tiers
            .iter()
            .find(|(field, t)| **t == tier && field.starts_with("attributes."))
            .map(|(field, _)| field.clone())
    };
    let mut values = vec![
        ("name".to_string(), String::new()),
        (SERVICE_FIELD.to_string(), String::new()),
    ];
    for tier in [Tier::Mid, Tier::High] {
        if let Some(field) = pick(tier) {
            values.push((field, String::new()));
        }
    }
    let window_rows: Vec<&OracleSpan> = spans.iter().filter(|s| in_window(s, &grid)).collect();
    if let Some(name) = most_frequent(&window_rows, "name")
        && let Some(first) = name.chars().next()
    {
        values.push(("name".to_string(), first.to_string()));
    }
    for (index, (field, prefix)) in values.into_iter().enumerate() {
        requests.push(Request {
            id: format!("values {index} {field}"),
            body: json!({"values": {
                "after": after_s,
                "before": before_s,
                "field": field,
                "prefix": prefix,
                "limit": VALUES_LIMIT,
            }}),
            ask: Ask::Values { field, prefix },
        });
    }

    Plan {
        after_s,
        before_s,
        candidates,
        scenarios,
        requests,
    }
}

/// Findings and per-check counts, filled leaf by leaf.
#[derive(Debug, Default)]
struct Judge {
    findings: Vec<Finding>,
    checks: BTreeMap<String, CheckCount>,
}

impl Judge {
    fn compare(
        &mut self,
        check: &'static str,
        scenario: &str,
        at: Vec<Locator>,
        want: Subject,
        got: Subject,
    ) {
        let count = self.checks.entry(check.to_string()).or_default();
        count.compared += 1;
        if want != got {
            count.differing += 1;
            self.findings.push(Finding {
                check,
                scenario: scenario.to_string(),
                at,
                want,
                got,
            });
        }
    }

    /// Compares two lists; a difference is one finding at its first index.
    fn list(
        &mut self,
        check: &'static str,
        scenario: &str,
        at: Vec<Locator>,
        want: &[Subject],
        got: &[Subject],
    ) {
        let first = (0..want.len().max(got.len())).find(|&i| want.get(i) != got.get(i));
        let mut at = at;
        match first {
            None => self.compare(
                check,
                scenario,
                at,
                Subject::Flag(true),
                Subject::Flag(true),
            ),
            Some(i) => {
                at.push(Locator::Index(i));
                let pick = |list: &[Subject]| list.get(i).cloned().unwrap_or(Subject::Missing);
                self.compare(check, scenario, at, pick(want), pick(got));
            }
        }
    }
}

fn name(text: &str) -> Locator {
    Locator::Name(text.to_string())
}

fn reasons(judge: &mut Judge, scenario: &str, at: &str, want: &[calc::Reason], got: &wire::Status) {
    let got = got.reasons();
    let wanted: BTreeMap<&str, &calc::Reason> =
        want.iter().map(|reason| (reason.reason, reason)).collect();
    let names: BTreeSet<&str> = wanted
        .keys()
        .copied()
        .chain(got.keys().map(String::as_str))
        .collect();
    if names.is_empty() {
        judge.compare(
            "ORC-STATUS",
            scenario,
            vec![name(at)],
            Subject::Flag(true),
            Subject::Flag(true),
        );
    }
    for reason in names {
        let at = vec![name(at), name(reason)];
        let want = wanted.get(reason);
        let got = got.get(reason);
        let count = |c: Option<u64>| c.map_or(Subject::Missing, Subject::Count);
        judge.compare(
            "ORC-STATUS",
            scenario,
            at.clone(),
            count(want.map(|r| r.count)),
            count(got.map(|r| r.count)),
        );
        judge.compare(
            "ORC-STATUS",
            scenario,
            at.clone(),
            count(want.and_then(|r| r.of)),
            count(got.and_then(|r| r.of)),
        );
        let detail =
            |fields: Vec<String>| fields.into_iter().map(Subject::Name).collect::<Vec<_>>();
        judge.list(
            "ORC-STATUS",
            scenario,
            at,
            &detail(
                want.map(|r| r.detail.iter().cloned().collect())
                    .unwrap_or_default(),
            ),
            &detail(got.map(|r| r.detail.clone()).unwrap_or_default()),
        );
    }
}

fn percentiles(values: Option<[i64; 3]>) -> Vec<Subject> {
    values.map_or_else(
        || vec![Subject::Missing],
        |v| v.iter().map(|n| Subject::Ns(*n)).collect(),
    )
}

fn judge_histogram(
    judge: &mut Judge,
    scenario: &Scenario,
    spans: &[OracleSpan],
    grid: &Grid,
    candidates: u64,
    stack: &str,
    got: &wire::Histogram,
) {
    let at = |parts: &[Locator]| {
        let mut out = vec![name("histogram"), name(stack)];
        out.extend_from_slice(parts);
        out
    };
    let want = calc::histogram(spans, grid, &scenario.scope, stack);
    let durations = calc::bucket_durations(spans, grid, &scenario.scope);
    judge.compare(
        "ORC-HIST",
        &scenario.name,
        at(&[name("buckets")]),
        Subject::Count(want.len() as u64),
        Subject::Count(got.buckets.len() as u64),
    );
    for (index, (want, got_bucket)) in want.iter().zip(&got.buckets).enumerate() {
        let mut got_counts: BTreeMap<&str, u64> = BTreeMap::new();
        for (value, count) in got.dimensions.iter().zip(&got_bucket.counts) {
            if *count > 0 {
                got_counts.insert(value.as_str(), *count);
            }
        }
        let values: BTreeSet<&str> = want
            .counts
            .keys()
            .map(String::as_str)
            .chain(got_counts.keys().copied())
            .collect();
        for value in values {
            judge.compare(
                "ORC-HIST",
                &scenario.name,
                at(&[
                    Locator::Index(index),
                    Locator::Value {
                        field: stack.to_string(),
                        value: value.to_string(),
                    },
                ]),
                Subject::Count(want.counts.get(value).copied().unwrap_or(0)),
                Subject::Count(got_counts.get(value).copied().unwrap_or(0)),
            );
        }
        for (label, want, got) in [
            ("unset", want.unset, got_bucket.unset),
            ("other", want.other, got_bucket.other),
        ] {
            judge.compare(
                "ORC-HIST",
                &scenario.name,
                at(&[Locator::Index(index), name(label)]),
                Subject::Count(want),
                Subject::Count(got),
            );
        }
        judge.list(
            "ORC-PCT",
            &scenario.name,
            at(&[Locator::Index(index)]),
            &percentiles(fixed_histogram::percentiles(&durations[index])),
            &percentiles(got_bucket.percentiles.map(|p| p.as_array())),
        );
    }

    let totals = calc::totals(spans, grid, &scenario.scope);
    for (label, want, got) in [
        ("count", totals.spans, got.totals.count),
        ("errors", totals.errors, got.totals.errors),
    ] {
        judge.compare(
            "ORC-TOTALS",
            &scenario.name,
            at(&[name("totals"), name(label)]),
            Subject::Count(want),
            Subject::Count(got),
        );
    }
    let window: Vec<i64> = durations.concat();
    judge.list(
        "ORC-PCT",
        &scenario.name,
        at(&[name("totals")]),
        &percentiles(fixed_histogram::percentiles(&window)),
        &percentiles(got.totals.percentiles.map(|p| p.as_array())),
    );
    reasons(
        judge,
        &scenario.name,
        "histogram",
        &calc::histogram_reasons(spans, stack, candidates),
        &got.status,
    );
}

/// ORC-GROUPS: every group's key and numbers in order, `other`, the self-time
/// total, and the section's own reasons.
fn judge_groups(
    judge: &mut Judge,
    scenario: &Scenario,
    spans: &[OracleSpan],
    grid: &Grid,
    got: &wire::Groups,
) {
    let want = calc::groups(spans, grid, &scenario.scope, scenario.selection.as_ref());
    let key = group_key;
    let numbers = |spans: u64, errors: u64, origins: u64, p95: Option<i64>, self_ns: String| {
        [
            Subject::Count(spans),
            Subject::Count(errors),
            Subject::Count(origins),
            p95.map_or(Subject::Missing, Subject::Ns),
            Subject::Name(self_ns),
        ]
    };
    let mut want_rows = Vec::new();
    for (k, n) in &want.rows {
        want_rows.extend(key(&k.service, &k.operation));
        want_rows.extend(numbers(
            n.spans,
            n.errors,
            n.errors_originated,
            n.p95_ns,
            n.self_ns.to_string(),
        ));
    }
    let mut got_rows = Vec::new();
    for row in &got.rows {
        let n = &row.numbers;
        got_rows.extend(key(&row.service, &row.operation));
        got_rows.extend(numbers(
            n.spans,
            n.errors,
            n.errors_originated,
            n.p95_ns,
            n.self_ns.clone(),
        ));
    }
    judge.list(
        "ORC-GROUPS",
        &scenario.name,
        vec![name("groups"), name("rows")],
        &want_rows,
        &got_rows,
    );
    let other = |folded: Option<(u64, [Subject; 5])>| match folded {
        Some((groups, numbers)) => {
            let mut out = vec![Subject::Count(groups)];
            out.extend(numbers);
            out
        }
        None => vec![Subject::Missing],
    };
    let want_other = want.other.as_ref().map(|(groups, n)| {
        let numbers = numbers(
            n.spans,
            n.errors,
            n.errors_originated,
            n.p95_ns,
            n.self_ns.to_string(),
        );
        (*groups, numbers)
    });
    let got_other = got.other.as_ref().map(|o| {
        let n = &o.numbers;
        let numbers = numbers(
            n.spans,
            n.errors,
            n.errors_originated,
            n.p95_ns,
            n.self_ns.clone(),
        );
        (o.groups, numbers)
    });
    judge.list(
        "ORC-GROUPS",
        &scenario.name,
        vec![name("groups"), name("other")],
        &other(want_other),
        &other(got_other),
    );
    judge.compare(
        "ORC-GROUPS",
        &scenario.name,
        vec![name("groups"), name("self total")],
        Subject::Name(want.self_ns_total.to_string()),
        Subject::Name(got.self_ns_total.clone()),
    );
    reasons(
        judge,
        &scenario.name,
        "groups",
        &calc::groups_reasons(&want),
        &got.status,
    );
    judge_delta(judge, scenario, &want, got);
}

/// A facet value as a subject: `None` is the field's unset value.
fn value_subject(field: &str, value: Option<&str>) -> Subject {
    match value {
        Some(value) => Subject::Value {
            field: field.to_string(),
            value: value.to_string(),
        },
        None => Subject::Unset {
            field: field.to_string(),
        },
    }
}

/// A group's key as subjects.
fn group_key(service: &Option<String>, operation: &Option<String>) -> [Subject; 2] {
    let part = |field: &str, value: &Option<String>| {
        value
            .as_ref()
            .map_or(Subject::Missing, |value| Subject::Value {
                field: field.to_string(),
                value: value.clone(),
            })
    };
    [part(SERVICE_FIELD, service), part("name", operation)]
}

/// ORC-DELTA (QRY-20): under a selection, the trace counts and self time per
/// side, and each listed group's and `other`'s sides, exactly.
fn judge_delta(judge: &mut Judge, scenario: &Scenario, want: &calc::Groups, got: &wire::Groups) {
    if want.delta.is_none() && got.delta.is_none() {
        return;
    }
    let side = |spans: u64, origins: u64, self_ns: String| {
        [
            Subject::Count(spans),
            Subject::Count(origins),
            Subject::Name(self_ns),
        ]
    };
    let calc_side = |s: &calc::Side| side(s.spans, s.errors_originated, s.self_ns.to_string());
    let wire_side = |s: &wire::DeltaSide| side(s.spans, s.errors_originated, s.self_ns.clone());

    let mut want_totals = vec![Subject::Missing];
    let mut want_rows = Vec::new();
    let mut want_other = vec![Subject::Missing];
    if let Some(delta) = &want.delta {
        want_totals = vec![
            Subject::Count(delta.selection_traces),
            Subject::Count(delta.baseline_traces),
            Subject::Name(delta.selection_self_ns_total.to_string()),
            Subject::Name(delta.baseline_self_ns_total.to_string()),
        ];
        for ((key, _), (selection, baseline)) in want.rows.iter().zip(&delta.rows) {
            want_rows.extend(group_key(&key.service, &key.operation));
            want_rows.extend(calc_side(selection));
            want_rows.extend(calc_side(baseline));
        }
        if let (Some((groups, _)), Some((selection, baseline))) = (&want.other, &delta.other) {
            want_other = vec![Subject::Count(*groups)];
            want_other.extend(calc_side(selection));
            want_other.extend(calc_side(baseline));
        }
    }
    let mut got_totals = vec![Subject::Missing];
    let mut got_rows = Vec::new();
    let mut got_other = vec![Subject::Missing];
    if let Some(delta) = &got.delta {
        got_totals = vec![
            Subject::Count(delta.selection_traces),
            Subject::Count(delta.baseline_traces),
            Subject::Name(delta.selection_self_ns_total.clone()),
            Subject::Name(delta.baseline_self_ns_total.clone()),
        ];
        for row in &delta.rows {
            got_rows.extend(group_key(&row.service, &row.operation));
            got_rows.extend(wire_side(&row.selection));
            got_rows.extend(wire_side(&row.baseline));
        }
        if let Some(other) = &delta.other {
            got_other = vec![Subject::Count(other.groups)];
            got_other.extend(wire_side(&other.selection));
            got_other.extend(wire_side(&other.baseline));
        }
    }
    let at = |part: &str| vec![name("groups"), name("delta"), name(part)];
    judge.list(
        "ORC-DELTA",
        &scenario.name,
        at("totals"),
        &want_totals,
        &got_totals,
    );
    judge.list(
        "ORC-DELTA",
        &scenario.name,
        at("rows"),
        &want_rows,
        &got_rows,
    );
    judge.list(
        "ORC-DELTA",
        &scenario.name,
        at("other"),
        &want_other,
        &got_other,
    );
}

/// The spans a scenario's selection keeps, when it has one: what its rows
/// list.
fn selected_spans(scenario: &Scenario, spans: &[OracleSpan]) -> Option<Vec<OracleSpan>> {
    let selection = scenario.selection.as_ref()?;
    let mut out = Vec::new();
    for span in spans {
        if selection.matches(span) {
            out.push(span.clone());
        }
    }
    Some(out)
}

fn fraction_subject(fraction: Option<calc::Fraction>) -> Subject {
    fraction.map_or(Subject::Missing, |(num, den)| {
        Subject::Name(format!("{:?}", num as f64 / den as f64))
    })
}

fn float_subject(value: Option<f64>) -> Subject {
    value.map_or(Subject::Missing, |value| {
        Subject::Name(format!("{value:?}"))
    })
}

/// Takes the wanted difference where the agent's is one unit in the last
/// place away: serde_json without `float_roundtrip` can parse the agent's
/// shortest decimal one ULP off, which is the reader's error, not the agent's.
fn snap_differences(want: &[Subject], got: &mut [Subject]) {
    for (want, got) in want.iter().zip(got.iter_mut()) {
        if let (Subject::Name(w), Subject::Name(g)) = (want, &*got)
            && let (Ok(w), Ok(g)) = (w.parse::<f64>(), g.parse::<f64>())
            && w.signum() == g.signum()
            && w.to_bits().abs_diff(g.to_bits()) <= 1
        {
            *got = want.clone();
        }
    }
}

fn rank_subject(rank: Option<u32>) -> Subject {
    rank.map_or(Subject::Missing, |rank| Subject::Count(u64::from(rank)))
}

/// ORC-CMP: under a selection, the totals, then every field in the answer's
/// order with its in-selection flag, totals, rank and best difference, and
/// every value with its scope, selection and baseline rows, eligibility, rank
/// and difference (differences as the nearest double of the exact fraction).
/// A field the selection is made of has its plain counts and nothing else.
fn judge_comparison(
    judge: &mut Judge,
    scenario: &Scenario,
    selection: &Selection,
    spans: &[OracleSpan],
    grid: &Grid,
    got: &wire::Facets,
) {
    let facets = calc::facets(spans, grid, &scenario.scope, None);
    let requested: Vec<String> = facets.fields.iter().map(|f| f.field.clone()).collect();
    let want = calc::comparison(spans, grid, &scenario.scope, selection, &requested);
    let totals = |scope: u64, selection: u64, min: u64| {
        vec![
            Subject::Count(scope),
            Subject::Count(selection),
            Subject::Count(min),
        ]
    };
    let got_totals = got.comparison.as_ref().map_or(vec![Subject::Missing], |c| {
        totals(c.scope, c.selection, c.min_support)
    });
    judge.list(
        "ORC-CMP",
        &scenario.name,
        vec![name("facets"), name("comparison")],
        &totals(want.scope, want.selection, calc::MIN_SELECTION_ROWS),
        &got_totals,
    );

    let mut want_lines = Vec::new();
    for field in &want.fields {
        match field {
            calc::ComparedField::Compared(field) => {
                want_lines.extend([
                    Subject::Name(field.field.clone()),
                    Subject::Flag(false),
                    Subject::Count(field.scope),
                    Subject::Count(field.selection),
                    rank_subject(field.rank),
                    fraction_subject(field.best),
                ]);
                for value in &field.values {
                    want_lines.extend([
                        value_subject(&field.field, value.value.as_deref()),
                        Subject::Count(value.count),
                        Subject::Count(value.selection),
                        Subject::Count(value.baseline),
                        Subject::Flag(value.eligible),
                        rank_subject(value.rank),
                        fraction_subject(value.diff),
                    ]);
                }
            }
            calc::ComparedField::InSelection(facet) => {
                want_lines.extend([
                    Subject::Name(facet.field.clone()),
                    Subject::Flag(true),
                    Subject::Missing,
                    Subject::Missing,
                    Subject::Missing,
                    Subject::Missing,
                ]);
                for (value, count) in &facet.values {
                    want_lines.extend([
                        value_subject(&facet.field, value.as_deref()),
                        Subject::Count(*count),
                        Subject::Missing,
                        Subject::Missing,
                        Subject::Missing,
                        Subject::Missing,
                        Subject::Missing,
                    ]);
                }
            }
        }
    }
    let mut got_lines = Vec::new();
    for field in &got.fields {
        let (scope, selection) = field
            .totals
            .as_ref()
            .map_or((Subject::Missing, Subject::Missing), |t| {
                (Subject::Count(t.scope), Subject::Count(t.selection))
            });
        got_lines.extend([
            Subject::Name(field.field.clone()),
            Subject::Flag(field.in_selection),
            scope,
            selection,
            rank_subject(field.rank),
            float_subject(field.best_diff),
        ]);
        for value in &field.values {
            got_lines.extend([
                value_subject(&field.field, value.value.as_deref()),
                Subject::Count(value.count),
                value.selection.map_or(Subject::Missing, Subject::Count),
                value.baseline.map_or(Subject::Missing, Subject::Count),
                value.eligible.map_or(Subject::Missing, Subject::Flag),
                rank_subject(value.rank),
                float_subject(value.diff),
            ]);
        }
    }
    snap_differences(&want_lines, &mut got_lines);
    judge.list(
        "ORC-CMP",
        &scenario.name,
        vec![name("facets"), name("fields")],
        &want_lines,
        &got_lines,
    );
    let names = |list: Vec<&String>| {
        list.into_iter()
            .map(|f| Subject::Name(f.clone()))
            .collect::<Vec<_>>()
    };
    judge.list(
        "ORC-CMP",
        &scenario.name,
        vec![name("facets"), name("unavailable")],
        &names(facets.unavailable.iter().collect()),
        &names(got.unavailable.iter().map(|u| &u.field).collect()),
    );
    reasons(
        judge,
        &scenario.name,
        "facets",
        &calc::facet_reasons(&facets),
        &got.status,
    );
}

fn judge_facets(
    judge: &mut Judge,
    scenario: &Scenario,
    spans: &[OracleSpan],
    grid: &Grid,
    got: &wire::Facets,
) {
    let want = calc::facets(spans, grid, &scenario.scope, None);
    let fields = |names: Vec<&String>| {
        names
            .into_iter()
            .map(|f| Subject::Name(f.clone()))
            .collect::<Vec<_>>()
    };
    judge.list(
        "ORC-FACET",
        &scenario.name,
        vec![name("facets"), name("fields")],
        &fields(want.fields.iter().map(|f| &f.field).collect()),
        &fields(got.fields.iter().map(|f| &f.field).collect()),
    );
    judge.list(
        "ORC-FACET",
        &scenario.name,
        vec![name("facets"), name("unavailable")],
        &fields(want.unavailable.iter().collect()),
        &fields(got.unavailable.iter().map(|u| &u.field).collect()),
    );
    for (want, got) in want.fields.iter().zip(&got.fields) {
        if want.field != got.field {
            continue;
        }
        let at = vec![name("facets"), name(&want.field)];
        let values = |pairs: Vec<(Option<&str>, u64)>| {
            let mut out = Vec::new();
            for (value, count) in pairs {
                out.push(value_subject(&want.field, value));
                out.push(Subject::Count(count));
            }
            out
        };
        judge.list(
            "ORC-FACET",
            &scenario.name,
            at.clone(),
            &values(
                want.values
                    .iter()
                    .map(|(v, c)| (v.as_deref(), *c))
                    .collect(),
            ),
            &values(
                got.values
                    .iter()
                    .map(|v| (v.value.as_deref(), v.count))
                    .collect(),
            ),
        );
        for (label, want_n, got_n) in [
            ("omitted values", want.omitted_values, got.omitted_values),
            ("omitted rows", want.omitted_rows, got.omitted_rows),
        ] {
            let mut at = at.clone();
            at.push(name(label));
            judge.compare(
                "ORC-FACET",
                &scenario.name,
                at,
                Subject::Count(want_n),
                Subject::Count(got_n),
            );
        }
    }
    reasons(
        judge,
        &scenario.name,
        "facets",
        &calc::facet_reasons(&want),
        &got.status,
    );
}

fn row_subjects(span: &OracleSpan) -> Vec<Subject> {
    let shown = |field: &str| {
        first_value(span, field).map_or(Subject::Missing, |value| Subject::Value {
            field: field.to_string(),
            value: value.to_string(),
        })
    };
    vec![
        Subject::Ns(span.start_ns),
        Subject::Trace(span.trace_id.unwrap_or_default()),
        Subject::Span(span.span_id.unwrap_or_default()),
        Subject::Ns(span.duration_ns),
        shown(SERVICE_FIELD),
        shown("name"),
        shown(ROLE_FIELD),
        shown(STATUS_FIELD),
    ]
}

fn parse_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != 2 * N {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn wire_row_subjects(row: &wire::Row) -> Vec<Subject> {
    let shown = |field: &str, value: &Option<String>| {
        value
            .as_ref()
            .map_or(Subject::Missing, |value| Subject::Value {
                field: field.to_string(),
                value: value.clone(),
            })
    };
    vec![
        row.start_ns().map_or(Subject::Missing, Subject::Ns),
        parse_hex::<16>(&row.trace_id).map_or(Subject::Missing, Subject::Trace),
        parse_hex::<8>(&row.span_id).map_or(Subject::Missing, Subject::Span),
        Subject::Ns(row.duration_ns),
        shown(SERVICE_FIELD, &row.service),
        shown("name", &row.name),
        shown(ROLE_FIELD, &row.role),
        shown(STATUS_FIELD, &row.status),
    ]
}

fn judge_rows(
    judge: &mut Judge,
    scenario: &Scenario,
    spans: &[OracleSpan],
    grid: &Grid,
    ask: &RowsAsk,
    got: &wire::Rows,
) {
    let newest = |limit: usize, anchor: Option<calc::RowKey>, walk: calc::Walk| {
        let page = calc::newest_page(spans, grid, &scenario.scope, limit, anchor, walk);
        (page.rows, Some((page.has_older, page.has_newer)))
    };
    let (label, check, (want_rows, flags)) = match ask {
        RowsAsk::Newest(limit) => (
            "newest",
            "ORC-ROWS",
            newest(*limit, None, calc::Walk::Older),
        ),
        RowsAsk::Page {
            limit,
            anchor,
            walk,
        } => (
            match walk {
                calc::Walk::Older => "newest older",
                calc::Walk::Newer => "newest newer",
            },
            "ORC-ROWS",
            newest(*limit, Some(*anchor), *walk),
        ),
        RowsAsk::Slowest(k) => (
            "slowest",
            "ORC-TOPK",
            (calc::slowest(spans, grid, &scenario.scope, *k), None),
        ),
    };
    let at = vec![name("rows"), name(label)];
    let matched = calc::totals(spans, grid, &scenario.scope).spans;
    let mut at_matched = at.clone();
    at_matched.push(name("matched"));
    judge.compare(
        check,
        &scenario.name,
        at_matched,
        Subject::Count(matched),
        Subject::Count(got.matched),
    );
    if let Some((has_older, has_newer)) = flags {
        for (flag, want, got) in [
            ("has older", has_older, got.has_older),
            ("has newer", has_newer, got.has_newer),
        ] {
            let mut at_flag = at.clone();
            at_flag.push(name(flag));
            judge.compare(
                check,
                &scenario.name,
                at_flag,
                Subject::Flag(want),
                Subject::Flag(got.unwrap_or(false)),
            );
        }
    }
    let want: Vec<Subject> = want_rows
        .iter()
        .flat_map(|span| row_subjects(span))
        .collect();
    let got_subjects: Vec<Subject> = got.items.iter().flat_map(wire_row_subjects).collect();
    judge.list(check, &scenario.name, at.clone(), &want, &got_subjects);

    // Copies of a resent row share its key but not always its self time
    // (their children may sit in other files), and neither side orders them:
    // self times compare per key, a row whose key does not parse as its own.
    type Key = Option<calc::RowKey>;
    let mut want_self: BTreeMap<Key, Vec<Option<i64>>> = BTreeMap::new();
    for span in &want_rows {
        want_self
            .entry(Some(calc::row_key(span)))
            .or_default()
            .push(span.self_ns);
    }
    let mut got_self: BTreeMap<Key, Vec<Option<i64>>> = BTreeMap::new();
    for row in &got.items {
        let key = match (
            row.start_ns(),
            parse_hex::<16>(&row.trace_id),
            parse_hex::<8>(&row.span_id),
        ) {
            (Some(start_ns), Some(trace), Some(span)) => Some((start_ns, trace, span)),
            _ => None,
        };
        got_self.entry(key).or_default().push(row.self_duration_ns);
    }
    let flatten = |by_key: BTreeMap<Key, Vec<Option<i64>>>| {
        let mut out = Vec::new();
        for (_, mut copies) in by_key {
            copies.sort_unstable();
            for copy in copies {
                out.push(copy.map_or(Subject::Missing, Subject::Ns));
            }
        }
        out
    };
    let mut at_self = at;
    at_self.push(name("self time by key"));
    judge.list(
        check,
        &scenario.name,
        at_self,
        &flatten(want_self),
        &flatten(got_self),
    );
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Low => "low",
        Tier::Mid => "mid",
        Tier::High => "high",
    }
}

fn judge_fields(judge: &mut Judge, scenario: &Scenario, spans: &[OracleSpan], got: &wire::Fields) {
    let want = calc::field_list(spans);
    let got_by_name: BTreeMap<&str, &wire::Field> = got
        .items
        .iter()
        .map(|field| (field.name.as_str(), field))
        .collect();
    let names: BTreeSet<&str> = want
        .keys()
        .map(String::as_str)
        .chain(got_by_name.keys().copied())
        .collect();
    for field in names {
        let at = vec![name("fields"), name(field)];
        let want = want
            .get(field)
            .map(|tier| (tier_name(*tier), calc::field_flags(field, *tier)));
        let got = got_by_name.get(field);
        let described = |tier: &str, flags: [bool; 5]| {
            let mut out = vec![Subject::Name(tier.to_string())];
            out.extend(flags.map(Subject::Flag));
            out
        };
        let want_list = want.map_or_else(Vec::new, |(tier, f)| {
            described(tier, [f.chip, f.facet, f.stack, f.text, f.column])
        });
        let got_list = got.map_or_else(Vec::new, |f| {
            described(&f.tier, [f.chip, f.facet, f.stack, f.text, f.column])
        });
        judge.list("ORC-FIELDS", &scenario.name, at, &want_list, &got_list);
    }
}

/// ORC-VALUES: sorted, within the limit, never a value outside the window's
/// units, and every value of the window's rows (below the last one returned
/// when truncated).
fn judge_values(
    judge: &mut Judge,
    plan: &Plan,
    spans: &[OracleSpan],
    field: &str,
    prefix: &str,
    got: &wire::ValuesAnswer,
) {
    let scenario = format!("values {field}");
    let after_ns = i64::from(plan.after_s) * 1_000_000_000;
    let before_ns = i64::from(plan.before_s) * 1_000_000_000;
    let window: Vec<OracleSpan> = spans
        .iter()
        .filter(|span| span.start_ns >= after_ns && span.start_ns < before_ns)
        .cloned()
        .collect();
    let lower = calc::unit_values(&window, field, prefix);
    let upper = calc::unit_values(spans, field, prefix);
    let flag = |judge: &mut Judge, label: &str, holds: bool| {
        judge.compare(
            "ORC-VALUES",
            &scenario,
            vec![name(label)],
            Subject::Flag(true),
            Subject::Flag(holds),
        );
    };
    let sorted = got.values.windows(2).all(|pair| pair[0] < pair[1]);
    flag(judge, "sorted", sorted);
    flag(judge, "within the limit", got.values.len() <= VALUES_LIMIT);
    let outside = got.values.iter().find(|value| !upper.contains(*value));
    judge.compare(
        "ORC-VALUES",
        &scenario,
        vec![name("a value outside the window's units")],
        Subject::Missing,
        outside.map_or(Subject::Missing, |value| Subject::Value {
            field: field.to_string(),
            value: value.clone(),
        }),
    );
    let returned: BTreeSet<&String> = got.values.iter().collect();
    let last = got.values.last();
    let missing = lower.iter().find(|value| {
        let expected = !got.truncated || last.is_some_and(|last| *value < last);
        expected && !returned.contains(value)
    });
    judge.compare(
        "ORC-VALUES",
        &scenario,
        vec![name("a window value left out")],
        Subject::Missing,
        missing.map_or(Subject::Missing, |value| Subject::Value {
            field: field.to_string(),
            value: value.clone(),
        }),
    );
}

/// Compares every answer with the calculator; `answers` maps request ids to
/// the agent's JSON. Returns the findings and what each check compared.
/// Adds to `plan` the row pages that follow the answers so far, as the
/// explorer walks them: older than a first page's last row, then newer than
/// that older page's first row. The agent's cursor goes back as it came; the
/// calculator anchors on the row's own key. Returns how many were added: the
/// runner asks them and calls again until none are.
pub fn add_pages(plan: &mut Plan, answers: &BTreeMap<String, Value>) -> usize {
    let mut added = Vec::new();
    for request in &plan.requests {
        let Ask::Explore {
            scenario,
            rows: Some(rows),
            ..
        } = &request.ask
        else {
            continue;
        };
        let (limit, walk) = match rows {
            RowsAsk::Newest(limit) => (*limit, calc::Walk::Older),
            RowsAsk::Page {
                limit,
                walk: calc::Walk::Older,
                ..
            } => (*limit, calc::Walk::Newer),
            _ => continue,
        };
        let direction = match walk {
            calc::Walk::Older => "older",
            calc::Walk::Newer => "newer",
        };
        let id = format!("{} {direction}", request.id);
        if plan.requests.iter().any(|r| r.id == id) {
            continue;
        }
        let Some(answer) = answers.get(&request.id) else {
            continue;
        };
        let Ok(got) = serde_json::from_value::<wire::ExploreAnswer>(answer.clone()) else {
            continue;
        };
        let Some(got) = got.data.rows else {
            continue;
        };
        let row = match walk {
            calc::Walk::Older => got.items.last(),
            calc::Walk::Newer => got.items.first(),
        };
        let Some(row) = row else {
            continue;
        };
        let (Some(start_ns), Some(trace_id), Some(span_id)) = (
            row.start_ns(),
            parse_hex::<16>(&row.trace_id),
            parse_hex::<8>(&row.span_id),
        ) else {
            continue;
        };
        let rows = json!({
            "order": "newest",
            "limit": limit,
            "anchor": row.cursor,
            "direction": direction,
        });
        added.push(Request {
            id,
            ask: Ask::Explore {
                scenario: *scenario,
                stack: None,
                facets: false,
                groups: false,
                rows: Some(RowsAsk::Page {
                    limit,
                    anchor: (start_ns, trace_id, span_id),
                    walk,
                }),
                fields: false,
            },
            body: explore_body(
                plan.after_s,
                plan.before_s,
                &plan.scenarios[*scenario].scope,
                plan.scenarios[*scenario].selection.as_ref(),
                json!({ "rows": rows }),
            ),
        });
    }
    let count = added.len();
    plan.requests.extend(added);
    count
}

/// Judges `plan`'s explore and values asks; its trace asks are [`judge_traces`]'s.
pub fn judge(
    plan: &Plan,
    spans: &[OracleSpan],
    answers: &BTreeMap<String, Value>,
) -> (Vec<Finding>, BTreeMap<String, CheckCount>) {
    let grid = Grid::for_window(plan.after_s, plan.before_s);
    let mut judge = Judge::default();
    for request in &plan.requests {
        if let Ask::Trace { .. } = request.ask {
            continue;
        }
        let at = vec![name(&request.id)];
        let Some(answer) = answers.get(&request.id) else {
            judge.compare(
                "ORC-ANSWER",
                &request.id,
                at,
                Subject::Name("an answer".into()),
                Subject::Missing,
            );
            continue;
        };
        match &request.ask {
            Ask::Trace { .. } => {}
            Ask::Values { field, prefix } => {
                match serde_json::from_value::<wire::ValuesAnswer>(answer.clone()) {
                    Ok(got) => judge_values(&mut judge, plan, spans, field, prefix, &got),
                    Err(_) => judge.compare(
                        "ORC-ANSWER",
                        &request.id,
                        at,
                        Subject::Name("values".into()),
                        Subject::Missing,
                    ),
                }
            }
            Ask::Explore {
                scenario,
                stack,
                facets,
                groups,
                rows,
                fields,
            } => {
                let Ok(got) = serde_json::from_value::<wire::ExploreAnswer>(answer.clone()) else {
                    judge.compare(
                        "ORC-ANSWER",
                        &request.id,
                        at,
                        Subject::Name("explore".into()),
                        Subject::Missing,
                    );
                    continue;
                };
                let scenario = &plan.scenarios[*scenario];
                let data = &got.data;
                let window = [
                    Subject::Count(u64::from(grid.after_s)),
                    Subject::Count(u64::from(grid.before_s)),
                    Subject::Ns(i64::from(grid.after_s) * 1_000_000_000),
                    Subject::Ns(i64::from(grid.width_s) * 1_000_000_000),
                    Subject::Count(grid.buckets() as u64),
                ];
                let got_window = [
                    Subject::Count(u64::from(data.window.after)),
                    Subject::Count(u64::from(data.window.before)),
                    wire::ns(&data.window.grid.start_ns).map_or(Subject::Missing, Subject::Ns),
                    Subject::Ns(data.window.grid.bucket_ns),
                    Subject::Count(data.window.grid.buckets as u64),
                ];
                judge.list("ORC-WINDOW", &scenario.name, at, &window, &got_window);
                let present = |judge: &mut Judge, section: &str, asked: bool, there: bool| {
                    judge.compare(
                        "ORC-ANSWER",
                        &scenario.name,
                        vec![name(section)],
                        Subject::Flag(asked),
                        Subject::Flag(there),
                    );
                };
                present(
                    &mut judge,
                    "histogram",
                    stack.is_some(),
                    data.histogram.is_some(),
                );
                present(&mut judge, "facets", *facets, data.facets.is_some());
                present(&mut judge, "groups", *groups, data.groups.is_some());
                present(&mut judge, "rows", rows.is_some(), data.rows.is_some());
                present(&mut judge, "fields", *fields, data.fields.is_some());
                if let (Some(stack), Some(got)) = (stack, &data.histogram) {
                    judge_histogram(
                        &mut judge,
                        scenario,
                        spans,
                        &grid,
                        plan.candidates,
                        stack,
                        got,
                    );
                }
                if let Some(got) = data.facets.as_ref().filter(|_| *facets) {
                    match &scenario.selection {
                        Some(selection) => {
                            judge_comparison(&mut judge, scenario, selection, spans, &grid, got)
                        }
                        None => judge_facets(&mut judge, scenario, spans, &grid, got),
                    }
                }
                if let Some(got) = data.groups.as_ref().filter(|_| *groups) {
                    judge_groups(&mut judge, scenario, spans, &grid, got);
                }
                if let (Some(ask), Some(got)) = (rows, &data.rows) {
                    let selected = selected_spans(scenario, spans);
                    let rows_of = selected.as_deref().unwrap_or(spans);
                    judge_rows(&mut judge, scenario, rows_of, &grid, ask, got);
                }
                if let Some(got) = data.fields.as_ref().filter(|_| *fields) {
                    judge_fields(&mut judge, scenario, spans, got);
                }
            }
        }
    }
    (judge.findings, judge.checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{self, MeshParams};
    use crate::model::spans_of_request;

    const T0_S: u32 = 1_700_000_000;

    /// A corpus split into two units, and the window holding all of it.
    fn corpus_spans() -> (Vec<OracleSpan>, u32, u32) {
        let generated = corpus::generate(&MeshParams {
            traces: 60,
            start_ns: u64::from(T0_S) * 1_000_000_000,
            trace_spacing_ns: 700_000_000,
            seed: 7,
        });
        let requests = corpus::build_requests(&generated, 50);
        let cut = requests.len() * 2 / 3;
        let mut spans = Vec::new();
        for (index, request) in requests.iter().enumerate() {
            spans.extend(spans_of_request(request, usize::from(index >= cut)));
        }
        let last_s = spans.iter().map(|span| span.start_ns).max().unwrap() / 1_000_000_000;
        (spans, T0_S, u32::try_from(last_s).unwrap() + 1)
    }

    fn status(reasons: &[calc::Reason]) -> Value {
        if reasons.is_empty() {
            return json!({"complete": true});
        }
        let partial: Vec<Value> = reasons
            .iter()
            .map(|r| json!({"reason": r.reason, "count": r.count, "of": r.of, "detail": r.detail}))
            .collect();
        json!({ "partial": partial })
    }

    fn pct(values: Option<[i64; 3]>, into: &mut Value) {
        if let Some([p50, p95, p99]) = values {
            into["p50_ns"] = json!(p50);
            into["p95_ns"] = json!(p95);
            into["p99_ns"] = json!(p99);
        }
    }

    fn row(span: &OracleSpan) -> Value {
        json!({
            "cursor": format!("cursor {}", span.start_ns),
            "start_ns": span.start_ns.to_string(),
            "duration_ns": span.duration_ns,
            "self_duration_ns": span.self_ns,
            "trace_id": hex(&span.trace_id.unwrap_or_default()),
            "span_id": hex(&span.span_id.unwrap_or_default()),
            "service": first_value(span, SERVICE_FIELD),
            "name": first_value(span, "name"),
            "role": first_value(span, ROLE_FIELD),
            "status": first_value(span, STATUS_FIELD),
            "columns": {},
        })
    }

    /// What the agent would answer if it agreed with the calculator.
    fn answer(plan: &Plan, spans: &[OracleSpan], request: &Request) -> Value {
        let grid = Grid::for_window(plan.after_s, plan.before_s);
        let (scenario, stack, facets, groups, rows, fields) = match &request.ask {
            Ask::Values { field, prefix } => {
                let after_ns = i64::from(plan.after_s) * 1_000_000_000;
                let before_ns = i64::from(plan.before_s) * 1_000_000_000;
                let window: Vec<OracleSpan> = spans
                    .iter()
                    .filter(|s| s.start_ns >= after_ns && s.start_ns < before_ns)
                    .cloned()
                    .collect();
                let values: Vec<String> = calc::unit_values(&window, field, prefix)
                    .into_iter()
                    .collect();
                let truncated = values.len() > VALUES_LIMIT;
                let values: Vec<String> = values.into_iter().take(VALUES_LIMIT).collect();
                return json!({"mode": "values", "version": 1, "field": field, "values": values,
                    "truncated": truncated, "status": {"complete": true}});
            }
            Ask::Trace { .. } => return Value::Null,
            Ask::Explore {
                scenario,
                stack,
                facets,
                groups,
                rows,
                fields,
            } => (
                &plan.scenarios[*scenario],
                stack,
                facets,
                groups,
                rows,
                fields,
            ),
        };
        let scope = &scenario.scope;
        let mut all_reasons = Vec::new();
        let mut data = json!({
            "mode": "explore",
            "version": 1,
            "window": {"after": grid.after_s, "before": grid.before_s, "grid": {
                "start_ns": (i64::from(grid.after_s) * 1_000_000_000).to_string(),
                "bucket_ns": i64::from(grid.width_s) * 1_000_000_000,
                "buckets": grid.buckets(),
            }},
        });
        if let Some(stack) = stack {
            let buckets = calc::histogram(spans, &grid, scope, stack);
            let durations = calc::bucket_durations(spans, &grid, scope);
            let dimensions: Vec<String> = buckets
                .iter()
                .flat_map(|b| b.counts.keys().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let mut wire_buckets = Vec::new();
            for (bucket, durations) in buckets.iter().zip(&durations) {
                let counts: Vec<u64> = dimensions
                    .iter()
                    .map(|d| bucket.counts.get(d).copied().unwrap_or(0))
                    .collect();
                let mut b = json!({"counts": counts, "unset": bucket.unset, "other": bucket.other});
                pct(fixed_histogram::percentiles(durations), &mut b);
                wire_buckets.push(b);
            }
            let totals = calc::totals(spans, &grid, scope);
            let mut t = json!({"count": totals.spans, "errors": totals.errors});
            pct(fixed_histogram::percentiles(&durations.concat()), &mut t);
            let reasons = calc::histogram_reasons(spans, stack, plan.candidates);
            data["histogram"] = json!({"status": status(&reasons), "stack": stack, "dimensions": dimensions,
                "buckets": wire_buckets, "totals": t,
                "percentiles": {"approximate": true, "max_relative_error": fixed_histogram::MAX_RELATIVE_ERROR}});
            all_reasons.extend(reasons);
        }
        if let (true, Some(selection)) = (*facets, &scenario.selection) {
            let plain = calc::facets(spans, &grid, scope, None);
            let requested: Vec<String> = plain.fields.iter().map(|f| f.field.clone()).collect();
            let want = calc::comparison(spans, &grid, scope, selection, &requested);
            let as_f64 = |fraction: Option<calc::Fraction>| {
                fraction.map(|(num, den)| num as f64 / den as f64)
            };
            let fields: Vec<Value> = want
                .fields
                .iter()
                .map(|f| match f {
                    calc::ComparedField::InSelection(facet) => json!({"field": facet.field,
                        "values": facet.values.iter()
                            .map(|(v, c)| json!({"value": v, "count": c}))
                            .collect::<Vec<_>>(),
                        "omitted_values": facet.omitted_values, "omitted_rows": facet.omitted_rows,
                        "in_selection": true}),
                    calc::ComparedField::Compared(f) => {
                        let values: Vec<Value> = f
                            .values
                            .iter()
                            .map(|v| {
                                let mut value = json!({"value": v.value, "count": v.count,
                                    "selection": v.selection, "baseline": v.baseline,
                                    "eligible": v.eligible});
                                if let Some(rank) = v.rank {
                                    value["rank"] = json!(rank);
                                }
                                if let Some(diff) = as_f64(v.diff) {
                                    value["diff"] = json!(diff);
                                }
                                value
                            })
                            .collect();
                        let mut field = json!({"field": f.field, "values": values,
                            "omitted_values": f.omitted_values, "omitted_rows": f.omitted_rows,
                            "totals": {"scope": f.scope, "selection": f.selection}});
                        if let Some(rank) = f.rank {
                            field["rank"] = json!(rank);
                        }
                        if let Some(best) = as_f64(f.best) {
                            field["best_diff"] = json!(best);
                        }
                        field
                    }
                })
                .collect();
            let reasons = calc::facet_reasons(&plain);
            data["facets"] = json!({"status": status(&reasons), "fields": fields, "unavailable": [],
                "comparison": {"scope": want.scope, "selection": want.selection,
                    "min_support": calc::MIN_SELECTION_ROWS}});
            all_reasons.extend(reasons);
        } else if *facets {
            let want = calc::facets(spans, &grid, scope, None);
            let fields: Vec<Value> = want
                .fields
                .iter()
                .map(|f| json!({"field": f.field,
                    "values": f.values.iter().map(|(v, c)| json!({"value": v, "count": c})).collect::<Vec<_>>(),
                    "omitted_values": f.omitted_values, "omitted_rows": f.omitted_rows}))
                .collect();
            let unavailable: Vec<Value> = want
                .unavailable
                .iter()
                .map(|f| json!({"field": f, "reason": "facet_high_card"}))
                .collect();
            let reasons = calc::facet_reasons(&want);
            data["facets"] =
                json!({"status": status(&reasons), "fields": fields, "unavailable": unavailable});
            all_reasons.extend(reasons);
        }
        if *groups {
            let want = calc::groups(spans, &grid, scope, scenario.selection.as_ref());
            let numbers = |n: &calc::GroupNumbers| {
                json!({"spans": n.spans, "errors": n.errors,
                    "errors_originated": n.errors_originated, "p95_ns": n.p95_ns,
                    "self_ns": n.self_ns.to_string()})
            };
            let mut rows = Vec::new();
            for (key, n) in &want.rows {
                let mut row = numbers(n);
                row["service"] = json!(key.service);
                row["operation"] = json!(key.operation);
                rows.push(row);
            }
            let other = want.other.as_ref().map(|(groups, n)| {
                let mut other = numbers(n);
                other["groups"] = json!(groups);
                other
            });
            let reasons = calc::groups_reasons(&want);
            data["groups"] = json!({"status": status(&reasons),
                "window_s": u64::from(grid.before_s - grid.after_s),
                "self_ns_total": want.self_ns_total.to_string(), "rows": rows, "other": other});
            if let Some(delta) = &want.delta {
                let side = |s: &calc::Side| {
                    json!({"spans": s.spans, "errors_originated": s.errors_originated,
                        "self_ns": s.self_ns.to_string()})
                };
                let mut delta_rows = Vec::new();
                for ((key, _), (selection, baseline)) in want.rows.iter().zip(&delta.rows) {
                    delta_rows.push(json!({"service": key.service, "operation": key.operation,
                        "selection": side(selection), "baseline": side(baseline)}));
                }
                let other = want.other.as_ref().zip(delta.other.as_ref()).map(
                    |((groups, _), (selection, baseline))| {
                        json!({"groups": groups, "selection": side(selection),
                            "baseline": side(baseline)})
                    },
                );
                data["groups"]["delta"] = json!({
                    "selection_traces": delta.selection_traces,
                    "baseline_traces": delta.baseline_traces,
                    "selection_self_ns_total": delta.selection_self_ns_total.to_string(),
                    "baseline_self_ns_total": delta.baseline_self_ns_total.to_string(),
                    "rows": delta_rows, "other": other});
            }
            all_reasons.extend(reasons);
        }
        if let Some(ask) = rows {
            let selected = selected_spans(scenario, spans);
            let spans = selected.as_deref().unwrap_or(spans);
            let matched = calc::totals(spans, &grid, scope).spans;
            let newest = |limit: usize, anchor: Option<calc::RowKey>, walk: calc::Walk| {
                let page = calc::newest_page(spans, &grid, scope, limit, anchor, walk);
                ("newest", page.rows, Some((page.has_older, page.has_newer)))
            };
            let (order, items, flags) = match ask {
                RowsAsk::Newest(limit) => newest(*limit, None, calc::Walk::Older),
                RowsAsk::Page {
                    limit,
                    anchor,
                    walk,
                } => newest(*limit, Some(*anchor), *walk),
                RowsAsk::Slowest(k) => ("slowest", calc::slowest(spans, &grid, scope, *k), None),
            };
            let mut r = json!({"status": {"complete": true}, "order": order, "matched": matched,
                "items": items.iter().map(|s| row(s)).collect::<Vec<_>>()});
            if let Some((has_older, has_newer)) = flags {
                r["has_older"] = json!(has_older);
                r["has_newer"] = json!(has_newer);
            }
            data["rows"] = r;
        }
        if *fields {
            let items: Vec<Value> = calc::field_list(spans)
                .into_iter()
                .map(|(name, tier)| {
                    let f = calc::field_flags(&name, tier);
                    json!({"name": name, "tier": tier_name(tier), "chip": f.chip, "facet": f.facet,
                        "stack": f.stack, "text": f.text, "column": f.column})
                })
                .collect();
            data["fields"] = json!({"status": {"complete": true}, "items": items,
                "columns": ["duration", "self_duration", "trace_id", "span_id"]});
        }
        data["status"] = status(&all_reasons);
        json!({"status": 200, "type": "traces", "data": data})
    }

    fn answers(plan: &Plan, spans: &[OracleSpan]) -> BTreeMap<String, Value> {
        plan.requests
            .iter()
            .map(|request| (request.id.clone(), answer(plan, spans, request)))
            .collect()
    }

    /// The plan's answers, then its pages' (older, then newer).
    fn walk(plan: &mut Plan, spans: &[OracleSpan]) -> BTreeMap<String, Value> {
        let mut asked = answers(plan, spans);
        for _ in 0..2 {
            add_pages(plan, &asked);
            asked = answers(plan, spans);
        }
        asked
    }

    fn key_of(row: &Value) -> calc::RowKey {
        (
            row["start_ns"].as_str().unwrap().parse().unwrap(),
            parse_hex(row["trace_id"].as_str().unwrap()).unwrap(),
            parse_hex(row["span_id"].as_str().unwrap()).unwrap(),
        )
    }

    fn page_of<'a>(plan: &'a Plan, id: &str) -> (&'a Request, calc::RowKey, calc::Walk) {
        let request = plan.requests.iter().find(|r| r.id == id).unwrap();
        let Ask::Explore {
            rows: Some(RowsAsk::Page { anchor, walk, .. }),
            ..
        } = &request.ask
        else {
            panic!("{id} is not a page");
        };
        (request, *anchor, *walk)
    }

    #[test]
    fn pages_follow_the_first_by_the_agents_cursor_and_are_judged() {
        let (spans, after, before) = corpus_spans();
        let mut plan = plan(after, before, &spans, 2);
        let first_id = "F0 every span:status";
        let first = answers(&plan, &spans)[first_id]["data"]["rows"]["items"].clone();
        let first = first.as_array().unwrap();
        assert_eq!(first.len(), ROWS_LIMIT);

        let asked = walk(&mut plan, &spans);

        let older_id = format!("{first_id} older");
        let (older, anchor, walk) = page_of(&plan, &older_id);
        let last = &first[ROWS_LIMIT - 1];
        assert_eq!((anchor, walk), (key_of(last), calc::Walk::Older));
        assert_eq!(
            older.body["explore"]["sections"],
            json!({"rows": {"order": "newest", "limit": ROWS_LIMIT, "anchor": last["cursor"],
                "direction": "older"}})
        );
        let older_first = &asked[&older_id]["data"]["rows"]["items"][0];
        let newer_id = format!("{older_id} newer");
        let (newer, anchor, walk) = page_of(&plan, &newer_id);
        assert_eq!((anchor, walk), (key_of(older_first), calc::Walk::Newer));
        assert_eq!(
            newer.body["explore"]["sections"]["rows"]["anchor"],
            older_first["cursor"]
        );
        assert_eq!(asked[&newer_id]["data"]["rows"]["items"], json!(first));
        assert_eq!(add_pages(&mut plan, &asked), 0);

        assert_eq!(judge(&plan, &spans, &asked).0, vec![]);

        let mut wrong = asked.clone();
        let items = wrong.get_mut(&older_id).unwrap()["data"]["rows"]["items"]
            .as_array_mut()
            .unwrap();
        items.remove(1);
        let (findings, _) = judge(&plan, &spans, &wrong);
        assert_eq!(findings.len(), 1, "{findings:#?}");
        assert_eq!(
            (findings[0].check, &findings[0].at[..2]),
            ("ORC-ROWS", &[name("rows"), name("newest older")][..])
        );

        let mut wrong = asked.clone();
        let rows = &mut wrong.get_mut(&older_id).unwrap()["data"]["rows"];
        assert_eq!(rows["has_newer"], json!(true));
        rows["has_newer"] = json!(false);
        let (findings, _) = judge(&plan, &spans, &wrong);
        assert_eq!(findings.len(), 1, "{findings:#?}");
        assert_eq!(findings[0].at.last(), Some(&name("has newer")));
    }

    #[test]
    fn plans_every_scenario_and_section() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);

        let names: Vec<&str> = plan.scenarios.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names[..4],
            [
                "F0 every span",
                "F1 entry spans",
                "F2 entry spans of the busiest service",
                "F4 the busiest operation's name as text",
            ]
        );
        assert_eq!(
            names[4..9],
            [
                "F1 entry spans × E1 errors",
                "F1 entry spans × E2 slow bands",
                "F1 entry spans × E3 middle third",
                "F1 entry spans × E4 at least the p95",
                "F1 entry spans × E5 errors or unset",
            ]
        );
        assert!(names[9].starts_with("F5 "), "{names:?}");
        assert_eq!(names[10..], ["F6 entry spans with an unset status"]);
        let selections: Vec<&Value> = plan
            .requests
            .iter()
            .filter_map(|r| r.body["explore"].get("selection"))
            .collect();
        assert!(
            selections
                .iter()
                .any(|s| s["filter"]["status_code"] == json!(["error"]))
        );
        assert!(
            selections
                .iter()
                .any(|s| s["filter"]["status_code"] == json!(["error", "unset"]))
        );
        assert!(
            plan.requests
                .iter()
                .any(|r| r.body["explore"]["filter"]["status_code"] == json!(["unset"]))
        );
        assert!(selections.iter().any(|s| s["time"]["after_ns"].is_string()));
        assert!(selections.iter().any(|s| s["duration"]["min_ns"].is_i64()));
        assert!(
            plan.requests
                .iter()
                .any(|r| r.body["explore"]["sections"]["fields"].is_object())
        );
        assert!(
            plan.requests
                .iter()
                .any(|r| r.body["explore"]["trace_ids"].is_array())
        );
        assert!(
            plan.requests
                .iter()
                .any(|r| r.body["explore"]["text"].is_string())
        );
        assert!(plan.requests.iter().any(|r| {
            r.body["values"]["prefix"]
                .as_str()
                .is_some_and(|p| !p.is_empty())
        }));
    }

    #[test]
    fn an_agent_agreeing_with_the_calculator_gives_no_finding() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);

        let (findings, checks) = judge(&plan, &spans, &answers(&plan, &spans));

        assert_eq!(findings, vec![]);
        for check in [
            "ORC-WINDOW",
            "ORC-HIST",
            "ORC-PCT",
            "ORC-TOTALS",
            "ORC-FACET",
            "ORC-CMP",
            "ORC-GROUPS",
            "ORC-DELTA",
            "ORC-ROWS",
            "ORC-TOPK",
            "ORC-FIELDS",
            "ORC-VALUES",
            "ORC-STATUS",
        ] {
            assert!(
                checks.get(check).is_some_and(|c| c.compared > 0),
                "{check}: {checks:?}"
            );
        }
    }

    #[test]
    fn one_wrong_bucket_count_is_exactly_one_finding() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);
        let mut answers = answers(&plan, &spans);
        let id = plan.requests[0].id.clone();
        let buckets = answers.get_mut(&id).unwrap()["data"]["histogram"]["buckets"]
            .as_array_mut()
            .unwrap();
        let bucket = buckets
            .iter_mut()
            .find(|b| {
                b["counts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c.as_u64() > Some(0))
            })
            .unwrap();
        let counts = bucket["counts"].as_array_mut().unwrap();
        let slot = counts.iter().position(|c| c.as_u64() > Some(0)).unwrap();
        counts[slot] = json!(counts[slot].as_u64().unwrap() + 1);

        let (findings, _) = judge(&plan, &spans, &answers);

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(
            (findings[0].check, findings[0].scenario.as_str()),
            ("ORC-HIST", "F0 every span")
        );
    }

    #[test]
    fn row_self_times_compare_per_key() {
        let (mut spans, after, before) = corpus_spans();
        // The slowest span with children, copied alone into a third unit: two
        // rows with one key and different self times.
        let values = calc::derived(&spans);
        let mut parent: Option<&OracleSpan> = None;
        for (span, value) in spans.iter().zip(&values) {
            if value.child_ns > 0 && parent.is_none_or(|p| span.duration_ns > p.duration_ns) {
                parent = Some(span);
            }
        }
        let mut copy = parent.unwrap().clone();
        copy.unit = 2;
        spans.push(copy);
        calc::add_derived(&mut spans, &|unit| Some(unit));
        let plan = plan(after, before, &spans, 3);
        let answers = answers(&plan, &spans);
        assert_eq!(judge(&plan, &spans, &answers).0, vec![]);

        fn items_of(answer: &mut Value) -> Option<&mut Vec<Value>> {
            answer
                .get_mut("data")?
                .get_mut("rows")?
                .get_mut("items")?
                .as_array_mut()
        }
        let same_row = |a: &Value, b: &Value| {
            ["start_ns", "trace_id", "span_id"]
                .iter()
                .all(|key| a[key] == b[key])
        };
        let mut swapped = answers.clone();
        let mut pairs = 0;
        for answer in swapped.values_mut() {
            let Some(items) = items_of(answer) else {
                continue;
            };
            for i in 1..items.len() {
                let copies = same_row(&items[i - 1], &items[i])
                    && items[i - 1]["self_duration_ns"] != items[i]["self_duration_ns"];
                if copies {
                    items.swap(i - 1, i);
                    pairs += 1;
                }
            }
        }
        assert!(pairs > 0, "some page holds both copies");
        assert_eq!(judge(&plan, &spans, &swapped).0, vec![]);

        let mut wrong = answers;
        let row = wrong
            .values_mut()
            .find_map(|answer| {
                let items = items_of(answer)?;
                items
                    .iter_mut()
                    .find(|item| item["self_duration_ns"].is_i64())
            })
            .unwrap();
        row["self_duration_ns"] = json!(row["self_duration_ns"].as_i64().unwrap() + 1);
        let (findings, _) = judge(&plan, &spans, &wrong);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].at.contains(&name("self time by key")));
    }

    #[test]
    fn one_wrong_group_number_is_exactly_one_finding() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);
        let mut answers = answers(&plan, &spans);
        let id = plan.requests[0].id.clone();
        let rows = answers.get_mut(&id).unwrap()["data"]["groups"]["rows"]
            .as_array_mut()
            .unwrap();
        assert!(!rows.is_empty());
        rows[0]["errors_originated"] = json!(rows[0]["errors_originated"].as_u64().unwrap() + 1);

        let (findings, _) = judge(&plan, &spans, &answers);

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check, "ORC-GROUPS");
    }

    #[test]
    fn one_wrong_delta_side_is_exactly_one_finding() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);
        let mut answers = answers(&plan, &spans);
        let id = plan
            .requests
            .iter()
            .find(|r| answers[&r.id]["data"]["groups"]["delta"].is_object())
            .expect("a delta under some selection")
            .id
            .clone();
        let row = &mut answers.get_mut(&id).unwrap()["data"]["groups"]["delta"]["rows"][0];
        row["baseline"]["spans"] = json!(row["baseline"]["spans"].as_u64().unwrap() + 1);

        let (findings, _) = judge(&plan, &spans, &answers);

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check, "ORC-DELTA");
    }

    #[test]
    fn one_wrong_comparison_rank_is_exactly_one_finding() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);
        let mut answers = answers(&plan, &spans);
        let (id, index) = plan
            .requests
            .iter()
            .filter(|r| r.body["explore"].get("selection").is_some())
            .find_map(|r| {
                let fields = answers[&r.id]["data"]["facets"]["fields"].as_array()?;
                let index = fields.iter().position(|f| f["rank"].is_u64())?;
                Some((r.id.clone(), index))
            })
            .expect("a ranked field under some selection");
        let field = &mut answers.get_mut(&id).unwrap()["data"]["facets"]["fields"][index];
        field["rank"] = json!(field["rank"].as_u64().unwrap() + 7);

        let (findings, _) = judge(&plan, &spans, &answers);

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check, "ORC-CMP");
    }

    #[test]
    fn a_selection_field_answered_as_compared_is_exactly_one_finding() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);
        let mut answers = answers(&plan, &spans);
        let (id, index) = plan
            .requests
            .iter()
            .filter(|r| r.body["explore"].get("selection").is_some())
            .find_map(|r| {
                let fields = answers[&r.id]["data"]["facets"]["fields"].as_array()?;
                let index = fields.iter().position(|f| f["in_selection"] == true)?;
                Some((r.id.clone(), index))
            })
            .expect("a field some selection is made of");
        let field = &mut answers.get_mut(&id).unwrap()["data"]["facets"]["fields"][index];
        field.as_object_mut().unwrap().remove("in_selection");
        field["rank"] = json!(1);

        let (findings, _) = judge(&plan, &spans, &answers);

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check, "ORC-CMP");
    }

    #[test]
    fn a_difference_read_one_ulp_off_is_not_a_finding() {
        let want = -0.0235f64;
        let name = |x: f64| Subject::Name(format!("{x:?}"));
        let mut got = [
            name(f64::from_bits(want.to_bits() + 1)),
            name(f64::from_bits(want.to_bits() + 2)),
            name(-want),
        ];
        snap_differences(&[name(want), name(want), name(want)], &mut got);
        assert_eq!(
            got,
            [
                name(want),
                name(f64::from_bits(want.to_bits() + 2)),
                name(-want)
            ]
        );
    }

    #[test]
    fn a_missing_answer_is_a_finding() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);
        let mut answers = answers(&plan, &spans);
        answers.remove(&plan.requests[1].id);

        let (findings, _) = judge(&plan, &spans, &answers);

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check, "ORC-ANSWER");
    }

    #[test]
    fn a_truncated_values_answer_owes_only_the_values_before_its_last() {
        let (spans, after, before) = corpus_spans();
        let plan = plan(after, before, &spans, 2);
        let mut answers = answers(&plan, &spans);
        let id = plan
            .requests
            .iter()
            .find(|r| matches!(&r.ask, Ask::Values { field, prefix } if field == "name" && prefix.is_empty()))
            .unwrap()
            .id
            .clone();
        let values = answers[&id]["values"].as_array().unwrap().clone();
        assert!(values.len() >= 4);
        let answer = answers.get_mut(&id).unwrap();
        answer["values"] = json!(values[..3]);
        answer["truncated"] = json!(true);

        assert_eq!(judge(&plan, &spans, &answers).0, vec![]);

        answers.get_mut(&id).unwrap()["values"] = json!([values[0], values[2]]);
        let (findings, _) = judge(&plan, &spans, &answers);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].check, "ORC-VALUES");
    }

    #[test]
    fn an_explore_window_comes_back_aligned_to_its_grid() {
        let (spans, after, _) = corpus_spans();
        let (unaligned, before) = (after + 1, after + 900);
        let grid = Grid::for_window(unaligned, before);
        assert_ne!(grid.after_s, unaligned);
        let plan = plan(unaligned, before, &spans, 2);

        let (findings, _) = judge(&plan, &spans, &answers(&plan, &spans));

        assert_eq!(findings, vec![]);
    }
}
