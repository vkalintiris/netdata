//! Mapping between the `otel-traces` wire types and the wire-neutral
//! [`sfsq::traces`] engine — the traces analogue of the logs `adapter`.
//! Wire shapes live in [`super::wire`]; this module owns the
//! request-side parsing (hex ids, the selection-key grammar, the
//! pagination cursor, window canonicalization) and the response-side
//! conversion (engine data → wire, ids rendered as W3C lowercase hex).

use std::collections::HashMap;

use sfsq::traces::{
    AttributeOwner, BuiltinField, CompareOp, Condition, DURATION_BIN_LABELS, DurationPercentiles,
    FieldKinds, OverviewData, Predicate, PredicateTarget, PredicateValue, SearchData, TraceData,
};

use super::wire::{
    AnchorWire, CoverageWire, EventWire, FacetListWire, FacetValueWire, FieldKindsWire, LinkWire,
    OverviewGridWire, OverviewPercentilesWire, OverviewResult, OverviewSection, OverviewTotals,
    SearchItems, SearchResult, SpanWire, StatusWire, TraceItems, TraceResult, TraceSummaryWire,
};

/// Parse a W3C text-form trace id: exactly 32 hex chars (16 bytes),
/// case-insensitive. The all-zero (unset) id parses here — the engine
/// rejects it with its own precise message.
pub(crate) fn parse_trace_id(s: &str) -> Result<sfst::TraceId, String> {
    let s = s.trim();
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "trace id must be 32 hex characters (16 bytes), got {s:?}"
        ));
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
        // Infallible: both bytes were checked hex above.
        bytes[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
    }
    Ok(sfst::TraceId::from(bytes))
}

/// Validate the `trace` sub-object's optional assembly bounds into a
/// capture range. Both-or-neither; `after < before` — violations are
/// client errors (the structural-error precedent of the envelope
/// window). Any width is accepted: the agent is relaxed on time ranges
/// and the response's `coverage` declares the range used. `None` =
/// full retention.
pub(crate) fn validate_trace_bounds(
    after: Option<u32>,
    before: Option<u32>,
) -> Result<Option<std::ops::Range<u32>>, String> {
    let (after, before) = match (after, before) {
        (None, None) => return Ok(None),
        (Some(a), Some(b)) => (a, b),
        _ => {
            return Err(
                "trace bounds require both 'after' and 'before' (or neither for full retention)"
                    .to_string(),
            );
        }
    };
    if after >= before {
        return Err(format!(
            "invalid trace bounds: after {after} >= before {before}"
        ));
    }
    Ok(Some(after..before))
}

/// Search completion slack, per side, seconds: the completion capture
/// is the match window widened by `clamp(window_width, 1h, 24h)` on
/// each side, so a hit whose trace straddles the window edge still
/// assembles its out-of-window spans. Saturating at the u32 second
/// boundaries. The window width is the scale hint: a narrow view gets
/// at least an hour of slack, a wide one at most a day per side.
pub(crate) const SEARCH_COMPLETION_SLACK_MIN_S: u32 = 3_600;
pub(crate) const SEARCH_COMPLETION_SLACK_MAX_S: u32 = 86_400;

/// The completion capture range for a match window: window widened by
/// the clamped per-side slack. Deterministic in the window alone, so a
/// pagination cursor that freezes the ORIGINAL window re-derives the
/// same completion range on every page.
pub(crate) fn completion_capture_range(window: &std::ops::Range<u32>) -> std::ops::Range<u32> {
    let slack = (window.end - window.start)
        .clamp(SEARCH_COMPLETION_SLACK_MIN_S, SEARCH_COMPLETION_SLACK_MAX_S);
    window.start.saturating_sub(slack)..window.end.saturating_add(slack)
}

/// Shape one assembled trace into the wire result. `trace_id` is echoed
/// back in canonical (lowercase) hex regardless of the request's casing.
/// `coverage` is the capture range assembly actually used.
pub(crate) fn to_trace_result(
    trace_id: &sfst::TraceId,
    data: TraceData,
    coverage: CoverageWire,
) -> TraceResult {
    let t = data.trace;
    let summary_root = t.summary_root();
    let family = data.family;
    let returned = t.spans.len();
    let mut spans = Vec::with_capacity(returned);
    for (index, span) in t.spans.into_iter().enumerate() {
        let mut wire = span_wire(span);
        wire.self_duration_ns = Some(wire.duration_ns - family.child_ns[index]);
        wire.error_origin = Some(family.error_origin[index]);
        spans.push(wire);
    }
    TraceResult {
        mode: "trace",
        version: 1,
        trace_id: trace_id.to_string(),
        coverage,
        status: StatusWire::from(&data.status),
        items: TraceItems { returned },
        summary_root,
        roots: t.roots,
        children: t.children,
        spans,
        field_kinds: field_kinds_wire(data.field_kinds),
    }
}

fn span_wire(s: sfst::TraceSpan) -> SpanWire {
    SpanWire {
        span_id: s.span_id.to_string(),
        // OTel semantics: an unset parent means "root", rendered as an
        // absent field rather than the all-zero sentinel string.
        parent_span_id: (!s.parent_span_id.is_unset()).then(|| s.parent_span_id.to_string()),
        start_ns: s.start_ns,
        duration_ns: s.duration_ns,
        self_duration_ns: None,
        error_origin: None,
        kind: s.kind,
        flags: s.flags,
        dropped_attributes_count: s.dropped_attributes_count,
        dropped_events_count: s.dropped_events_count,
        dropped_links_count: s.dropped_links_count,
        fields: s.fields,
        events: s.events.into_iter().map(event_wire).collect(),
        links: s.links.into_iter().map(link_wire).collect(),
    }
}

fn event_wire(e: sfst::TraceEvent) -> EventWire {
    EventWire {
        time_unix_nano: e.time_unix_nano,
        name: e.name,
        dropped_attributes_count: e.dropped_attributes_count,
        attributes: e.attributes,
    }
}

fn link_wire(l: sfst::TraceLink) -> LinkWire {
    LinkWire {
        trace_id: l.trace_id.to_string(),
        span_id: l.span_id.to_string(),
        trace_state: l.trace_state,
        flags: l.flags,
        dropped_attributes_count: l.dropped_attributes_count,
        attributes: l.attributes,
    }
}

fn field_kinds_wire(k: FieldKinds) -> FieldKindsWire {
    let section = |v: Vec<(String, sfst::ValueKind)>| -> Vec<(String, &'static str)> {
        v.into_iter().map(|(n, k)| (n, kind_word(k))).collect()
    };
    FieldKindsWire {
        fields: section(k.fields),
        event_attributes: section(k.event_attributes),
        link_attributes: section(k.link_attributes),
    }
}

// ── Search: selection-key grammar ───────────────────────────────────

/// The wire word for each builtin field (snake_case). Also the key
/// grammar step 1.4's enumeration emits, so filters and facet keys stay
/// one vocabulary. The lockstep test walks `BuiltinField::ALL`, so a new
/// engine builtin breaks the suite until it gets a wire word.
const BUILTIN_WORDS: [(&str, BuiltinField); 17] = [
    ("name", BuiltinField::Name),
    ("kind", BuiltinField::Kind),
    ("status", BuiltinField::Status),
    ("status_message", BuiltinField::StatusMessage),
    ("instrumentation_name", BuiltinField::InstrumentationName),
    ("instrumentation_version", BuiltinField::InstrumentationVersion),
    ("event_name", BuiltinField::EventName),
    ("duration", BuiltinField::Duration),
    ("span_id", BuiltinField::SpanId),
    ("parent_span_id", BuiltinField::ParentSpanId),
    ("trace_id", BuiltinField::TraceId),
    ("link_span_id", BuiltinField::LinkSpanId),
    ("link_trace_id", BuiltinField::LinkTraceId),
    ("event_time_since_start", BuiltinField::EventTimeSinceStart),
    ("root_name", BuiltinField::RootName),
    ("root_service_name", BuiltinField::RootServiceName),
    ("trace_duration", BuiltinField::TraceDuration),
];

/// The attribute owners a selection key may name, as `<owner>.<key>`.
/// `Any` is deliberately absent from the wire (enumeration — step 1.4 —
/// emits owner-qualified keys, so selections are always qualified) and
/// `Builtin` is spelled as the bare words above. The exhaustive match in
/// `owner_word_for` keeps this table in lockstep with the engine enum.
const OWNER_WORDS: [(&str, AttributeOwner); 5] = [
    ("resource", AttributeOwner::Resource),
    ("span", AttributeOwner::Span),
    ("instrumentation", AttributeOwner::Instrumentation),
    ("event", AttributeOwner::Event),
    ("link", AttributeOwner::Link),
];

/// Parse one selection key: `<owner>.<key>` (the key part verbatim, dots
/// included — `resource.service.name` → the `service.name` resource
/// attribute) or a bare builtin word.
pub(crate) fn parse_selection_key(key: &str) -> Result<PredicateTarget, String> {
    for (word, owner) in OWNER_WORDS {
        if let Some(rest) = key.strip_prefix(word).and_then(|r| r.strip_prefix('.')) {
            if rest.is_empty() {
                return Err(format!("selection key {key:?} names no attribute"));
            }
            return Ok(PredicateTarget::Attribute(owner, rest.to_string()));
        }
    }
    BUILTIN_WORDS
        .iter()
        .find(|(w, _)| *w == key)
        .map(|(_, f)| PredicateTarget::Builtin(*f))
        .ok_or_else(|| {
            format!(
                "unknown selection key {key:?}: use resource.<key> / span.<key> / \
                 instrumentation.<key> / event.<key> / link.<key>, or a builtin \
                 field word"
            )
        })
}

/// Build the engine predicate from the wire's filters: each selection
/// key is one Eq condition (values OR within the key, keys AND — the
/// engine's multi-value semantics); the duration bounds become builtin
/// Duration comparisons. Keys iterate sorted so requests are
/// deterministic. An empty value list is no constraint.
pub(crate) fn build_predicate(
    selections: &HashMap<String, Vec<String>>,
    min_duration_ns: Option<i64>,
    max_duration_ns: Option<i64>,
    min_trace_duration_ns: Option<i64>,
    max_trace_duration_ns: Option<i64>,
) -> Result<Predicate, String> {
    if let (Some(min), Some(max)) = (min_duration_ns, max_duration_ns)
        && min > max
    {
        return Err(format!(
            "min_duration_ns {min} exceeds max_duration_ns {max}: nothing can match"
        ));
    }
    if let (Some(min), Some(max)) = (min_trace_duration_ns, max_trace_duration_ns)
        && min > max
    {
        return Err(format!(
            "min_trace_duration_ns {min} exceeds max_trace_duration_ns {max}: nothing can match"
        ));
    }
    let mut conditions = Vec::new();
    let mut keys: Vec<&String> = selections.keys().collect();
    keys.sort();
    for key in keys {
        // Validate the key even when its value list is empty — a typo'd
        // key must not hide behind an empty selection.
        let target = parse_selection_key(key)?;
        let values = &selections[key];
        if values.is_empty() {
            continue;
        }
        conditions.push(Condition {
            target,
            op: CompareOp::Eq,
            values: values
                .iter()
                .map(|v| PredicateValue::Text(v.clone()))
                .collect(),
        });
    }
    if let Some(min) = min_duration_ns {
        conditions.push(Condition {
            target: PredicateTarget::Builtin(BuiltinField::Duration),
            op: CompareOp::Gte,
            values: vec![PredicateValue::Integer(min)],
        });
    }
    if let Some(max) = max_duration_ns {
        conditions.push(Condition {
            target: PredicateTarget::Builtin(BuiltinField::Duration),
            op: CompareOp::Lte,
            values: vec![PredicateValue::Integer(max)],
        });
    }
    if let Some(min) = min_trace_duration_ns {
        conditions.push(Condition {
            target: PredicateTarget::Builtin(BuiltinField::TraceDuration),
            op: CompareOp::Gte,
            values: vec![PredicateValue::Integer(min)],
        });
    }
    if let Some(max) = max_trace_duration_ns {
        conditions.push(Condition {
            target: PredicateTarget::Builtin(BuiltinField::TraceDuration),
            op: CompareOp::Lte,
            values: vec![PredicateValue::Integer(max)],
        });
    }
    Ok(Predicate { conditions })
}

/// The selection-grammar word of a builtin field — for error messages
/// that must speak the wire's language, not the engine's.
pub(crate) fn builtin_word(field: BuiltinField) -> &'static str {
    BUILTIN_WORDS
        .iter()
        .find(|(_, f)| *f == field)
        .map(|(w, _)| *w)
        .expect("the lockstep test pins a wire word for every builtin")
}

// ── Overview: engine data → wire ────────────────────────────────────

/// Everything the standalone overview mode and the search response's
/// embedded section share. Shaped once so the two surfaces cannot
/// report the same window differently.
struct OverviewParts {
    status: StatusWire,
    grid: OverviewGridWire,
    totals: OverviewTotals,
    top_root_services: Option<FacetListWire>,
    top_root_operations: Option<FacetListWire>,
}

/// Shape the trace-density grid's shared body. The wire's
/// second-granular geometry derives from the SAME `grid` the engine
/// binned on — one source of truth; the shared derivation guarantees
/// whole seconds (debug-asserted).
fn overview_parts(data: OverviewData, grid: sfst::Grid) -> OverviewParts {
    const NS_PER_S: i64 = 1_000_000_000;
    debug_assert_eq!(grid.bucket_start_ns % NS_PER_S, 0);
    debug_assert_eq!(grid.bucket_width_ns % NS_PER_S, 0);
    let to_list = |l: sfsq::traces::FacetList| FacetListWire {
        top: l
            .top
            .into_iter()
            .map(|(value, traces)| FacetValueWire { value, traces })
            .collect(),
        other: l.other,
        unattributed: l.unattributed,
    };
    let (top_root_services, top_root_operations) = match data.root_facets {
        Some(f) => (Some(to_list(f.services)), Some(to_list(f.operations))),
        None => (None, None),
    };
    OverviewParts {
        status: StatusWire::from(&data.status),
        grid: OverviewGridWire {
            bucket_start_s: (grid.bucket_start_ns / NS_PER_S) as u32,
            bucket_width_s: (grid.bucket_width_ns / NS_PER_S) as u32,
            duration_bins: DURATION_BIN_LABELS.to_vec(),
            cells: data.cells.iter().map(|row| row.to_vec()).collect(),
            error_cells: data.error_cells.iter().map(|row| row.to_vec()).collect(),
            duration_percentiles_ns: to_percentiles(&data.bucket_percentiles),
        },
        totals: OverviewTotals {
            traces: data.total_traces,
            spans: data.total_spans,
            errors: data.total_errors,
        },
        top_root_services,
        top_root_operations,
    }
}

/// Transpose the engine's per-bucket percentile triples into one array
/// per rank — the shape a per-rank series is drawn from.
fn to_percentiles(buckets: &[Option<DurationPercentiles>]) -> OverviewPercentilesWire {
    OverviewPercentilesWire {
        p50: buckets.iter().map(|b| b.map(|p| p.p50)).collect(),
        p95: buckets.iter().map(|b| b.map(|p| p.p95)).collect(),
        p99: buckets.iter().map(|b| b.map(|p| p.p99)).collect(),
    }
}

/// The heatmap's predicate from a request's `selections` alone — the
/// page's grammar and builder, minus every duration bound (the grid
/// never applies those; see the wire's `scope` doc). `None` when the
/// selections constrain nothing, so the caller runs the plain grid.
pub(crate) fn heatmap_predicate(
    selections: &HashMap<String, Vec<String>>,
) -> Result<Option<Predicate>, String> {
    let predicate = build_predicate(selections, None, None, None, None)?;
    Ok((!predicate.is_all()).then_some(predicate))
}

/// Shape the standalone `overview` mode's response; `scope` is what
/// actually ran (the handler's decision, from the predicate it passed).
pub(crate) fn to_overview_result(
    data: OverviewData,
    grid: sfst::Grid,
    scope: &'static str,
) -> OverviewResult {
    let parts = overview_parts(data, grid);
    OverviewResult {
        mode: "overview",
        version: 1,
        unit: "traces",
        status: parts.status,
        scope,
        grid: parts.grid,
        totals: parts.totals,
        top_root_services: parts.top_root_services,
        top_root_operations: parts.top_root_operations,
    }
}

/// Shape the same aggregate as the SEARCH response's embedded section:
/// the legacy body minus `mode`, plus `coverage` — the GRID's aligned
/// window, which the caller derived together with `grid` — and `scope`.
///
/// `scope` is the section's honesty: the handler passes the value for
/// the predicate the engine pass actually carried, never what a caller
/// wanted.
pub(crate) fn to_overview_section(
    data: OverviewData,
    grid: sfst::Grid,
    coverage: CoverageWire,
    scope: &'static str,
) -> OverviewSection {
    let parts = overview_parts(data, grid);
    OverviewSection {
        version: 1,
        unit: "traces",
        status: parts.status,
        coverage,
        scope,
        grid: parts.grid,
        totals: parts.totals,
        top_root_services: parts.top_root_services,
        top_root_operations: parts.top_root_operations,
    }
}

// ── Search: pagination cursor ───────────────────────────────────────

/// The wire pagination state. The engine has no anchor concept, and its
/// rank — (newest matched-span start DESC, trace_id ASC) — is
/// WINDOW-DEPENDENT: narrowing the window re-ranks any trace whose
/// matched spans straddle the boundary through an older span,
/// duplicating it on a later page (a review-caught flaw of the first
/// design). So the cursor NEVER narrows the window. Instead it:
///
/// - FREEZES the canonicalized window of page 1, so every page ranks
///   the same corpus the same way (and a default `now`-derived window
///   can't drift between pages);
/// - carries the AFTER-KEY — the last served trace's (rank, trace_id) —
///   and the served count. The next page re-runs the SAME query with
///   `limit = served + limit` and drops everything ranked at-or-above
///   the key: at a fixed corpus the engine's deterministic total order
///   makes the pages an exact partition, tie runs and straddling
///   traces included.
///
/// Live-data caveats (inherent to most-recent-first pagination; walk a
/// window that ends in the past to avoid them):
///
/// - Late arrivals ranking ABOVE the key are never shown by this walk
///   AND consume the over-fetch allowance — a burst larger than `limit`
///   can shorten a page below `limit` and end the walk early (silently:
///   indistinguishable from exhaustion). Late arrivals ranking BELOW
///   the key appear on later pages normally.
/// - The cursor freezes the window but NOT the filters: echoing it with
///   different selections re-partitions a different result set. The
///   client must keep the filters fixed for the walk's lifetime.
///
/// Cost note: each page reruns the query with `limit = served + limit`,
/// so a full walk of N traces does O(N²/limit) engine ranking work —
/// acceptable at human paging depths and hard-capped by
/// [`CURSOR_SERVED_CAP`]; the engine's own ceilings bound any single
/// request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchCursor {
    /// The frozen page-1 window, unix seconds.
    pub after_s: u32,
    pub before_s: u32,
    /// The after-key: the last served trace's rank (newest matched-span
    /// start) and id. A trace is already served iff it ranks at-or-above
    /// this key.
    pub rank_ns: i64,
    pub trace_id: sfst::TraceId,
    /// Total traces served so far — the engine over-fetch allowance.
    pub served: usize,
}

/// Walks deeper than this stop paging (the engine limit grows with the
/// served count); the cursor is rejected with advice to narrow.
const CURSOR_SERVED_CAP: usize = 10_000;

pub(crate) fn encode_cursor(c: &SearchCursor) -> String {
    format!(
        "t2:{}:{}:{}:{}:{}",
        c.after_s, c.before_s, c.rank_ns, c.trace_id, c.served
    )
}

pub(crate) fn parse_cursor(s: &str) -> Result<SearchCursor, String> {
    let malformed = || format!("malformed anchor {s:?}");
    let mut parts = s.split(':');
    let (Some("t2"), Some(after), Some(before), Some(rank), Some(id), Some(served), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(malformed());
    };
    let cursor = SearchCursor {
        after_s: after.parse().map_err(|_| malformed())?,
        before_s: before.parse().map_err(|_| malformed())?,
        rank_ns: rank.parse().map_err(|_| malformed())?,
        trace_id: parse_trace_id(id).map_err(|_| malformed())?,
        served: served.parse().map_err(|_| malformed())?,
    };
    if cursor.served == 0 || cursor.after_s >= cursor.before_s {
        return Err(malformed());
    }
    if cursor.served > CURSOR_SERVED_CAP {
        return Err(format!(
            "anchor walk exceeds {CURSOR_SERVED_CAP} traces; narrow the filters or window"
        ));
    }
    Ok(cursor)
}

/// Whether `t` was already served by the cursor's walk: it ranks
/// at-or-above the after-key in the engine's total order — a higher
/// rank, or the same rank with a trace id at-or-before the key's
/// (equal ranks order by ascending id).
fn is_served(c: &SearchCursor, t: &sfsq::traces::TraceSummary) -> bool {
    t.newest_matched_start_ns > c.rank_ns
        || (t.newest_matched_start_ns == c.rank_ns && t.trace_id <= c.trace_id)
}

// ── Search: window canonicalization ─────────────────────────────────

/// The canonicalized search window: the engine bounds (ns) plus the
/// registry capture range (seconds — a safe superset for file pruning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedWindow {
    pub start_ns: i64,
    pub end_ns: i64,
    pub capture: std::ops::Range<u32>,
}

/// Canonicalize the wire window (unix seconds; the logs precedent for
/// unspecified bounds: `before` → now, `after` → before − 900 s). An
/// anchor page uses the cursor's FROZEN window verbatim — the request's
/// own window fields (and the drifting `now`) are ignored so every page
/// of one walk ranks the same corpus.
pub(crate) fn resolve_window(
    after_s: u32,
    before_s: u32,
    now_s: u32,
    anchor: Option<&SearchCursor>,
) -> Result<ResolvedWindow, String> {
    let (after, before) = match anchor {
        Some(c) => (c.after_s, c.before_s), // parse_cursor validated after < before
        None => {
            let before = if before_s == 0 { now_s } else { before_s };
            let after = if after_s == 0 {
                before.saturating_sub(900)
            } else {
                after_s
            };
            if after >= before {
                return Err(format!("invalid window: after {after} >= before {before}"));
            }
            (after, before)
        }
    };
    Ok(ResolvedWindow {
        start_ns: i64::from(after) * 1_000_000_000,
        end_ns: i64::from(before) * 1_000_000_000,
        // Second-granular equivalent for registry file pruning; the ns
        // bounds do the exact filtering inside the engine.
        capture: after..before,
    })
}

// ── Search: engine data → wire ──────────────────────────────────────

/// Shape one search result page. Drops everything the cursor's walk
/// already served (the engine re-emits the served prefix first — same
/// query, same deterministic total order), trims to `limit`, and emits
/// the next cursor only for a FULL page with a COMPLETE status: a short
/// page means the walk is exhausted, and a PARTIAL page's continuation
/// would not be a stable prefix (the engine's work ceilings scale with
/// the limit), so the walk ends there with the status saying why.
/// `frozen` is the window this page actually ran (carried into the next
/// cursor).
pub(crate) fn to_search_result(
    data: SearchData,
    limit: usize,
    incoming: Option<&SearchCursor>,
    frozen: (u32, u32),
    completion_coverage: CoverageWire,
) -> SearchResult {
    let mut traces = data.traces;
    if let Some(c) = incoming {
        traces.retain(|t| !is_served(c, t));
    }
    traces.truncate(limit);

    let complete = data.status.is_complete();
    let served_before = incoming.map_or(0, |c| c.served);
    let anchor = (traces.len() == limit && limit > 0 && complete)
        .then(|| {
            let tail = traces.last().expect("page is full, limit > 0");
            let served = served_before + traces.len();
            // A walk past the cap can't over-fetch any further — end it
            // here (the client sees a full page with no continuation).
            (served <= CURSOR_SERVED_CAP).then(|| AnchorWire {
                next: encode_cursor(&SearchCursor {
                    after_s: frozen.0,
                    before_s: frozen.1,
                    rank_ns: tail.newest_matched_start_ns,
                    trace_id: tail.trace_id,
                    served,
                }),
            })
        })
        .flatten();

    SearchResult {
        mode: "search",
        version: 1,
        status: StatusWire::from(&data.status),
        completion_coverage,
        items: SearchItems {
            returned: traces.len(),
            max_to_return: limit,
        },
        traces: traces.into_iter().map(summary_wire).collect(),
        field_kinds: field_kinds_wire(data.field_kinds),
        anchor,
        // Composed by the caller when it ran the aggregate pass: only
        // the Functions view does, and only it holds the second engine
        // call's result.
        overview: None,
    }
}

fn summary_wire(t: sfsq::traces::TraceSummary) -> TraceSummaryWire {
    TraceSummaryWire {
        trace_id: t.trace_id.to_string(),
        root_service: t.root_service,
        root_name: t.root_name,
        start_ns: t.start_ns,
        newest_matched_start_ns: t.newest_matched_start_ns,
        duration_ns: t.duration_ns,
        span_count: t.span_count,
        error_count: t.error_count,
        matched_count: t.matched_count,
        exact: t.exact,
        matched_spans: t.matched_spans.into_iter().map(span_wire).collect(),
    }
}

/// The wire word for each schema value kind. Exhaustive on purpose: a
/// new engine kind fails compilation here until its wire name is decided.
fn kind_word(k: sfst::ValueKind) -> &'static str {
    match k {
        sfst::ValueKind::Null => "null",
        sfst::ValueKind::Bool => "bool",
        sfst::ValueKind::Int => "int",
        sfst::ValueKind::Double => "double",
        sfst::ValueKind::Str => "str",
        sfst::ValueKind::Bytes => "bytes",
        sfst::ValueKind::EmptyKvlist => "empty_kvlist",
        sfst::ValueKind::EmptyArray => "empty_array",
        sfst::ValueKind::Kvlist => "kvlist",
        sfst::ValueKind::Array => "array",
    }
}

/// The engine request for an explorer call received at `now_s`, plus the
/// window aligned outward to whole buckets (the range files are captured
/// for).
pub(crate) fn to_explore_query(
    params: &super::wire::ExploreParams,
    now_s: u32,
) -> (sfsq::traces::explore::ExploreQuery, u32, u32) {
    use sfsq::traces::explore::{ExploreQuery, ExploreScope, FacetSpec, HistogramSpec, Sections};

    let (after, before) = params.window.resolve(now_s);
    // A relative window reaching before the epoch clamps to 0; keep at
    // least one second so the grid holds a bucket.
    let (grid, aligned_after, aligned_before) =
        super::super::grid::grid_for_window_s(after, before.max(after.saturating_add(1)));
    let mut filter = sfst::Filter::new();
    for (field, values) in &params.filter {
        for value in values {
            filter = filter.select(field.clone(), value.clone());
        }
    }
    let query = ExploreQuery {
        grid,
        scope: ExploreScope {
            filter,
            text: params.text.as_deref().map(sfst::text::LiteralText::new),
            trace_ids: params.trace_ids.clone(),
        },
        selection: None,
        sections: Sections {
            histogram: params.histogram.as_ref().map(|h| HistogramSpec {
                stack: h.stack.clone(),
                percentiles: h.percentiles,
            }),
            facets: params.facets.as_ref().map(|f| FacetSpec {
                fields: f.fields.clone(),
            }),
            groups: params.groups,
            rows: params.rows.clone(),
            fields: params.fields,
        },
    };
    (query, aligned_after, aligned_before)
}

/// The engine request for a value-suggestion call received at `now_s`, plus the
/// window in unix seconds (the range files are captured for).
pub(crate) fn to_values_query(
    params: &super::wire::ValuesParams,
    now_s: u32,
) -> (sfsq::traces::explore::ValuesQuery, u32, u32) {
    let (after, before) = params.window.resolve(now_s);
    // A relative window reaching before the epoch clamps to 0; keep at least
    // one second.
    let before = before.max(after.saturating_add(1));
    let query = sfsq::traces::explore::ValuesQuery {
        window: i64::from(after) * 1_000_000_000..i64::from(before) * 1_000_000_000,
        field: params.field.clone(),
        prefix: params.prefix.clone(),
        limit: params.limit,
    };
    (query, after, before)
}

pub(crate) fn to_values_response(
    data: sfsq::traces::explore::ValuesData,
    field: String,
) -> super::wire::ValuesResponse {
    super::wire::ValuesResponse {
        mode: "values",
        version: 1,
        field,
        values: data.values,
        truncated: data.truncated,
        status: StatusWire::from(&data.status),
    }
}

/// The explorer's engine answer on the wire.
pub(crate) fn to_explore_response(
    data: sfsq::traces::explore::ExploreData,
    grid: sfst::Grid,
    after: u32,
    before: u32,
) -> super::wire::ExploreResponse {
    use super::wire::{
        BucketWire, ExploreDataWire, ExploreFacetValueWire, ExploreFacetWire, ExploreFacetsWire,
        ExploreResponse, FieldWire, FieldsWire, GridWire, GroupNumbersWire, GroupWire, GroupsWire,
        HistogramWire, OtherGroupsWire, PercentileMethodWire, PercentilesWire, RowWire, RowsWire,
        TotalsWire, UnavailableFacetWire, WindowWire,
    };
    let numbers = |n: sfsq::traces::explore::GroupNumbers| GroupNumbersWire {
        spans: n.spans,
        errors: n.errors,
        errors_originated: n.errors_originated,
        p95_ns: n.p95_ns,
        self_ns: n.self_ns.to_string(),
    };
    let percentiles = |p: Option<sfsq::traces::explore::Percentiles>| {
        p.map(|p| PercentilesWire {
            p50_ns: p.p50_ns,
            p95_ns: p.p95_ns,
            p99_ns: p.p99_ns,
        })
    };

    let status = StatusWire::from(&data.status);
    let histogram = data.histogram.map(|h| {
        let mut buckets = Vec::with_capacity(h.buckets.len());
        for b in h.buckets {
            buckets.push(BucketWire {
                counts: b.counts,
                unset: b.unset,
                other: b.other,
                percentiles: percentiles(b.percentiles),
            });
        }
        HistogramWire {
            status: StatusWire::from(&h.status),
            stack: h.stack,
            dimensions: h.dimensions,
            buckets,
            totals: TotalsWire {
                count: h.totals.count,
                errors: h.totals.errors,
                percentiles: percentiles(h.totals.percentiles),
            },
            percentiles: h.percentiles.then_some(PercentileMethodWire {
                approximate: true,
                max_relative_error: sfsq::traces::duration_hist::MAX_RELATIVE_ERROR,
            }),
        }
    });
    let facets = data.facets.map(|f| {
        let mut fields = Vec::with_capacity(f.fields.len());
        for facet in f.fields {
            let mut values = Vec::with_capacity(facet.values.len());
            for v in facet.values {
                values.push(ExploreFacetValueWire {
                    value: v.value,
                    count: v.count,
                });
            }
            fields.push(ExploreFacetWire {
                field: facet.field,
                values,
                omitted_values: facet.omitted_values,
                omitted_rows: facet.omitted_rows,
            });
        }
        let mut unavailable = Vec::with_capacity(f.unavailable.len());
        for (field, reason) in f.unavailable {
            unavailable.push(UnavailableFacetWire {
                field,
                reason: reason.into(),
            });
        }
        ExploreFacetsWire {
            status: StatusWire::from(&f.status),
            fields,
            unavailable,
        }
    });
    let rows = data.rows.map(|r| {
        let mut items = Vec::with_capacity(r.items.len());
        for row in r.items {
            let mut columns = std::collections::BTreeMap::new();
            for (name, values) in r.columns.iter().zip(row.columns) {
                if !values.is_empty() {
                    columns.insert(name.clone(), values.join(", "));
                }
            }
            items.push(RowWire {
                cursor: row.key.encode(),
                start_ns: row.key.start_ns.to_string(),
                duration_ns: row.duration_ns,
                self_duration_ns: row.self_duration_ns,
                trace_id: row.key.trace_id.to_string(),
                span_id: row.key.span_id.to_string(),
                service: row.service,
                name: row.name,
                role: row.role,
                status: row.status,
                columns,
            });
        }
        RowsWire {
            status: StatusWire::from(&r.status),
            order: match r.order {
                sfsq::traces::explore::RowOrder::Newest { .. } => "newest",
                sfsq::traces::explore::RowOrder::Slowest => "slowest",
            },
            matched: r.matched,
            has_older: r.more.map(|m| m.older),
            has_newer: r.more.map(|m| m.newer),
            items,
        }
    });
    let fields = data.fields.map(|f| {
        let mut items = Vec::with_capacity(f.items.len());
        for field in f.items {
            items.push(FieldWire {
                tier: match field.tier {
                    sfst::FieldTier::Low => "low",
                    sfst::FieldTier::Mid => "mid",
                    sfst::FieldTier::High => "high",
                },
                name: field.name,
                chip: field.chip,
                facet: field.facet,
                stack: field.stack,
                text: field.text,
                column: field.column,
            });
        }
        FieldsWire {
            status: StatusWire::from(&f.status),
            items,
            columns: f.columns,
        }
    });
    let groups = data.groups.map(|g| {
        let mut rows = Vec::with_capacity(g.rows.len());
        for row in g.rows {
            rows.push(GroupWire {
                service: row.key.service,
                operation: row.key.operation,
                numbers: numbers(row.numbers),
            });
        }
        GroupsWire {
            status: StatusWire::from(&g.status),
            window_s: g.window_s,
            self_ns_total: g.self_ns_total.to_string(),
            rows,
            other: g.other.map(|other| OtherGroupsWire {
                groups: other.groups,
                numbers: numbers(other.numbers),
            }),
        }
    });
    ExploreResponse {
        status: 200,
        response_type: "traces",
        data: ExploreDataWire {
            mode: "explore",
            version: 1,
            window: WindowWire {
                after,
                before,
                grid: GridWire {
                    start_ns: grid.bucket_start_ns.to_string(),
                    bucket_ns: grid.bucket_width_ns,
                    buckets: grid.num_buckets,
                },
            },
            status,
            histogram,
            facets,
            groups,
            rows,
            fields,
        },
    }
}

#[cfg(test)]
mod tests;
