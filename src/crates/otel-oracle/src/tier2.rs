//! The tier-2 comparison: the requests to send the live agent for one judged
//! window, and how its answers compare with the calculator's over the rows of
//! the units that window overlaps.
//!
//! Each difference is one finding at the leaf that differs (a count, a flag,
//! a value); a list that differs gives one finding at its first difference, so
//! one wrong number never shows up as several.
//!
//! Row pages past the first (walking by the agent's cursor) are asked and
//! judged by the runner, which holds the answers in order.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::calc::{self, Grid, Scope, Tier, fixed_histogram};
use crate::model::{DURATION_BAND_FIELD, OracleSpan, ROLE_FIELD, SERVICE_FIELD, STATUS_FIELD};
use crate::report::{CheckCount, Finding, Locator, Subject};
use crate::wire;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowsAsk {
    Newest(usize),
    Slowest(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Explore {
        scenario: usize,
        stack: Option<String>,
        facets: bool,
        rows: Option<RowsAsk>,
        fields: bool,
    },
    Values {
        field: String,
        prefix: String,
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
        .map(String::as_str)
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
        },
        Scenario {
            name: "F1 entry spans".to_string(),
            scope: Scope::entry_spans(),
        },
    ];
    if let Some(service) = most_frequent(&entry, SERVICE_FIELD) {
        out.push(Scenario {
            name: "F2 entry spans of the busiest service".to_string(),
            scope: Scope::entry_spans().with(SERVICE_FIELD, &[service]),
        });
    }
    if let Some(name) = most_frequent(&entry, "name") {
        out.push(Scenario {
            name: "F4 the busiest operation's name as text".to_string(),
            scope: Scope::default().with_text(name),
        });
    }
    let traces = scope_traces(spans, grid);
    if !traces.is_empty() {
        out.push(Scenario {
            name: format!("F5 {} trace ids", traces.len()),
            scope: Scope::default().with_trace_ids(&traces),
        });
    }
    out
}

fn explore_body(after_s: u32, before_s: u32, scope: &Scope, sections: Value) -> Value {
    let mut explore = json!({"after": after_s, "before": before_s, "sections": sections});
    if !scope.terms.is_empty() {
        let filter: BTreeMap<&String, Vec<&String>> = scope
            .terms
            .iter()
            .map(|(field, values)| (field, values.iter().collect()))
            .collect();
        explore["filter"] = json!(filter);
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
/// units it overlaps.
pub fn plan(after_s: u32, before_s: u32, spans: &[OracleSpan], candidates: u64) -> Plan {
    let grid = Grid::for_window(after_s, before_s);
    let scenarios = scenarios(spans, &grid);
    let mut requests = Vec::new();
    for (index, scenario) in scenarios.iter().enumerate() {
        let everything = index == 0;
        let mut first = json!({
            "histogram": {"stack": STATUS_FIELD, "percentiles": true},
            "facets": {},
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
                    rows: Some(RowsAsk::Slowest(k)),
                    fields: false,
                },
                json!({"rows": {"order": "slowest", "limit": k}}),
            ));
        }
        for (label, ask, sections) in asks {
            requests.push(Request {
                id: format!("{}:{label}", scenario.name),
                body: explore_body(after_s, before_s, &scenario.scope, sections),
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
        let values = |pairs: Vec<(&str, u64)>| {
            let mut out = Vec::new();
            for (value, count) in pairs {
                out.push(Subject::Value {
                    field: want.field.clone(),
                    value: value.to_string(),
                });
                out.push(Subject::Count(count));
            }
            out
        };
        judge.list(
            "ORC-FACET",
            &scenario.name,
            at.clone(),
            &values(want.values.iter().map(|(v, c)| (v.as_str(), *c)).collect()),
            &values(
                got.values
                    .iter()
                    .map(|v| (v.value.as_str(), v.count))
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
    let (label, check, want_rows, has_older) = match ask {
        RowsAsk::Newest(limit) => {
            let page = calc::newest_page(
                spans,
                grid,
                &scenario.scope,
                *limit,
                None,
                calc::Walk::Older,
            );
            ("newest", "ORC-ROWS", page.rows, Some(page.has_older))
        }
        RowsAsk::Slowest(k) => (
            "slowest",
            "ORC-TOPK",
            calc::slowest(spans, grid, &scenario.scope, *k),
            None,
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
    if let Some(has_older) = has_older {
        let mut at_older = at.clone();
        at_older.push(name("has older"));
        judge.compare(
            check,
            &scenario.name,
            at_older,
            Subject::Flag(has_older),
            Subject::Flag(got.has_older.unwrap_or(false)),
        );
    }
    let want: Vec<Subject> = want_rows
        .iter()
        .flat_map(|span| row_subjects(span))
        .collect();
    let got: Vec<Subject> = got.items.iter().flat_map(wire_row_subjects).collect();
    judge.list(check, &scenario.name, at, &want, &got);
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
pub fn judge(
    plan: &Plan,
    spans: &[OracleSpan],
    answers: &BTreeMap<String, Value>,
) -> (Vec<Finding>, BTreeMap<String, CheckCount>) {
    let grid = Grid::for_window(plan.after_s, plan.before_s);
    let mut judge = Judge::default();
    for request in &plan.requests {
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
                    Subject::Count(u64::from(plan.after_s)),
                    Subject::Count(u64::from(plan.before_s)),
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
                    judge_facets(&mut judge, scenario, spans, &grid, got);
                }
                if let (Some(ask), Some(got)) = (rows, &data.rows) {
                    judge_rows(&mut judge, scenario, spans, &grid, ask, got);
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
            "cursor": "opaque",
            "start_ns": span.start_ns.to_string(),
            "duration_ns": span.duration_ns,
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
        let (scenario, stack, facets, rows, fields) = match &request.ask {
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
            Ask::Explore {
                scenario,
                stack,
                facets,
                rows,
                fields,
            } => (&plan.scenarios[*scenario], stack, facets, rows, fields),
        };
        let scope = &scenario.scope;
        let mut all_reasons = Vec::new();
        let mut data = json!({
            "mode": "explore",
            "version": 1,
            "window": {"after": plan.after_s, "before": plan.before_s, "grid": {
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
        if *facets {
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
        if let Some(ask) = rows {
            let matched = calc::totals(spans, &grid, scope).spans;
            let (order, items, has_older) = match ask {
                RowsAsk::Newest(limit) => {
                    let page =
                        calc::newest_page(spans, &grid, scope, *limit, None, calc::Walk::Older);
                    ("newest", page.rows, Some(page.has_older))
                }
                RowsAsk::Slowest(k) => ("slowest", calc::slowest(spans, &grid, scope, *k), None),
            };
            let mut r = json!({"status": {"complete": true}, "order": order, "matched": matched,
                "items": items.iter().map(|s| row(s)).collect::<Vec<_>>()});
            if let Some(has_older) = has_older {
                r["has_older"] = json!(has_older);
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
                "columns": ["duration", "trace_id", "span_id"]});
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
        assert!(names[4].starts_with("F5 "), "{names:?}");
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
}
