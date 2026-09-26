//! Netdata function wire types for `otel-traces`.
//!
//! The transport layer between the netdata function protocol and the
//! wire-neutral [`sfsq::traces`] engine. Four peer modes, each selected by
//! exactly one top-level sub-object — `info`, `explore`, `values`, `trace` —
//! every mode's params self-contained in its object. A request naming no
//! selector, or more than one, is a client error; unknown keys anywhere are
//! client errors.
//!
//! The trace answer's nanosecond values (`*_ns`, `time_unix_nano`) go on
//! the wire as JSON numbers and exceed 2^53: JavaScript consumers read them
//! with ~256 ns granularity — fine for display, NOT for arithmetic requiring
//! ns exactness. The explorer sends the values a client computes with or
//! echoes back (row starts, self-time sums, row cursors) as decimal STRINGS
//! for exactly this reason.

use serde::{Deserialize, Serialize};

pub use super::super::status::{PartialReasonWire, StatusWire};

// ── Request ─────────────────────────────────────────────────────────

/// Request param names accepted by this function, advertised to the UI
/// in [`InfoResponse::accepted_params`]: exactly the TOP-LEVEL keys, the
/// mode selectors and `tenant`. Per-mode body fields live inside their mode
/// objects and are documented on the param structs.
pub const ACCEPTED_PARAMS: &[&str] = &["info", "explore", "values", "trace", "tenant"];

/// The mode selectors, in the order the error messages name them.
const MODES: &[&str] = &["info", "explore", "values", "trace"];

/// The raw top-level shape: the mode selectors captured presence-preserving
/// (see [`present`]), plus `tenant`. Deserialized only through
/// [`OtelTracesRequest`]'s manual `Deserialize` (which enforces a top-level
/// JSON object) and immediately converted by [`TryFrom`] into the typed
/// request — nothing outside this module sees the raw form.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOtelTracesRequest {
    /// The capability-discovery selector. `{"info": {}}` — the strict
    /// empty object, validated by [`InfoParams`]; the traces GET shim
    /// synthesizes exactly this for the `info` URL token.
    #[serde(default, deserialize_with = "present")]
    info: Option<serde_json::Value>,
    /// The traces explorer: histogram and totals over span rows.
    #[serde(default, deserialize_with = "present")]
    explore: Option<serde_json::Value>,
    /// Value suggestions for one field.
    #[serde(default, deserialize_with = "present")]
    values: Option<serde_json::Value>,
    /// The trace mode: one trace's spans by trace id, with their tree.
    #[serde(default, deserialize_with = "present")]
    trace: Option<serde_json::Value>,
    /// Tenant whose data the query reads — a scoping selector supplied
    /// by the caller, not a security boundary; omitted/invalid falls
    /// back to the default tenant
    /// ([`file_registry::TenantId::resolve_query`]), never an implicit
    /// all-tenant union. Top-level because it scopes the CALL the same
    /// way in every data mode (`info` ignores it — capability
    /// discovery reads no data).
    #[serde(default)]
    tenant: Option<String>,
}

/// Request payload: exactly one mode, its params self-contained, plus
/// the optional call-scoping `tenant`. Implements `Deserialize`
/// manually — top level must be a JSON object (arrays and scalars are
/// client errors; the object streams through [`RawOtelTracesRequest`]'s
/// derived visitor, preserving TOP-LEVEL duplicate-key and unknown-key
/// rejection — inside a mode object, duplicates collapse last-wins at
/// the `Value` capture, a known serde_json DOM property) — and does
/// NOT implement `Serialize` (tests build JSON bodies directly).
#[derive(Debug, Clone)]
pub struct OtelTracesRequest {
    pub tenant: Option<String>,
    pub mode: TracesMode,
}

/// The typed mode: selector identity plus its parsed params.
#[derive(Debug, Clone)]
pub enum TracesMode {
    Info,
    Explore(Box<ExploreParams>),
    Values(ValuesParams),
    Trace(TraceParams),
}

impl<'de> Deserialize<'de> for OtelTracesRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TopLevel;
        impl<'de> serde::de::Visitor<'de> for TopLevel {
            type Value = OtelTracesRequest;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an otel-traces request object")
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let raw = RawOtelTracesRequest::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                OtelTracesRequest::try_from(raw).map_err(serde::de::Error::custom)
            }
        }
        deserializer.deserialize_map(TopLevel)
    }
}

impl TryFrom<RawOtelTracesRequest> for OtelTracesRequest {
    type Error = String;

    /// Mode resolution then the typed parse, in that order: ALL present
    /// selectors are counted BEFORE any selector value is decoded, so a
    /// conflicting body reports the conflict even when one selector is
    /// also malformed (`{"trace": null, "explore": {}}` is a conflict,
    /// not an invalid trace selector).
    fn try_from(raw: RawOtelTracesRequest) -> Result<Self, String> {
        let present: Vec<&'static str> = MODES
            .iter()
            .copied()
            .zip([
                raw.info.is_some(),
                raw.explore.is_some(),
                raw.values.is_some(),
                raw.trace.is_some(),
            ])
            .filter_map(|(name, set)| set.then_some(name))
            .collect();

        match present.as_slice() {
            [] => {
                return Err(format!(
                    "no mode selector: name exactly one of {}",
                    MODES.join(", ")
                ));
            }
            [_] => {}
            names => return Err(format!("conflicting mode selectors: {}", names.join(", "))),
        }

        /// The typed parse behind an explicit object gate: serde's
        /// derived struct visitors also accept JSON sequences
        /// (positional arrays), so `is_object` must be checked before
        /// delegating or `{"trace": ["id", 7]}` would silently parse.
        fn typed<T: serde::de::DeserializeOwned>(
            name: &str,
            v: &serde_json::Value,
        ) -> Result<T, String> {
            if !v.is_object() {
                return Err(format!("invalid {name} selector: expected an object"));
            }
            T::deserialize(v).map_err(|e| format!("invalid {name} selector: {e}"))
        }

        let mode = if let Some(v) = &raw.info {
            typed::<InfoParams>("info", v)?;
            TracesMode::Info
        } else if let Some(v) = &raw.explore {
            TracesMode::Explore(Box::new(typed("explore", v)?))
        } else if let Some(v) = &raw.values {
            TracesMode::Values(typed("values", v)?)
        } else if let Some(v) = &raw.trace {
            TracesMode::Trace(typed("trace", v)?)
        } else {
            unreachable!("exactly one selector was verified present");
        };

        Ok(Self {
            tenant: raw.tenant,
            mode,
        })
    }
}

/// The `info` selector's params: the strict empty object. Anything but
/// `{}` — bools (both of the old wire's forms), numbers, null, arrays,
/// or an object with any field — is a client error; a malformed
/// selector must not silently select.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InfoParams {}

/// Presence-preserving selector deserializer: serde's stock
/// `Option<Value>` maps a present-but-`null` key to `None`, which would
/// silently fall through to another mode — here it becomes
/// `Some(Value::Null)` so the mode is selected and the typed parse
/// rejects the null with a clean error. Absent keys never reach this
/// function (`#[serde(default)]` covers them).
fn present<'de, D>(d: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(d).map(Some)
}

/// The `trace` mode's typed parameters. Unknown fields are rejected —
/// a misspelled parameter on a small object is a client error, not a
/// silent ignore.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceParams {
    /// The trace id: 32 hex chars (16 bytes), case-insensitive — the
    /// W3C trace-context text form.
    pub id: String,
    /// Span cap override — may only TIGHTEN the engine default
    /// (65,536): zero and values beyond the default are client errors.
    #[serde(default)]
    pub span_cap: Option<usize>,
    /// Optional assembly bounds, unix seconds: only files whose
    /// summary range overlaps `[after, before)` are probed for the
    /// trace's spans. Both-or-neither; `after < before` — violations
    /// are client errors. Any width is accepted. Absent = full
    /// retention, local and remote. The response's `coverage` declares
    /// the range actually used either way.
    #[serde(default)]
    pub after: Option<u32>,
    #[serde(default)]
    pub before: Option<u32>,
}

// ── Response ────────────────────────────────────────────────────────

/// Response shapes. The enum is untagged AND serialize-only (no serde
/// routing exists); every shape self-describes through its leading
/// `mode` field instead.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum OtelTracesResponse {
    Info(Box<InfoResponse>),
    Explore(Box<ExploreResponse>),
    Values(ValuesResponse),
    Trace(Box<TraceResult>),
}

/// A declared coverage range, unix seconds, always present — full
/// retention is the literal `{after: 0, before: 4294967295}`, never a
/// null a consumer must default-interpret.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CoverageWire {
    pub after: u32,
    pub before: u32,
}

// ── Trace mode response ─────────────────────────────────────────────

/// One assembled trace: its spans plus the parent/child graph and the
/// typed-field map — everything the drawer's waterfall is drawn from.
///
/// The graph is node-index adjacency over `spans`: walk from `roots`
/// via `children`; `children` can carry cycle edges (pathological
/// input), so a walker MUST guard against revisiting a node.
/// `summary_root` is the DISPLAY root (the OTLP root convention), a
/// separate derivation from the reachability roots.
#[derive(Debug, Serialize)]
pub struct TraceResult {
    /// The response's self-description: always `"trace"`.
    pub mode: &'static str,
    pub version: u32,
    /// The queried id, echoed in canonical lowercase hex.
    pub trace_id: String,
    /// The assembly bounds actually used, unix seconds: only files
    /// overlapping this range were probed for spans. Absent request
    /// bounds declare the literal full range `{after: 0, before:
    /// 4294967295}`. Spans beyond the declared range are UNKNOWN, not
    /// absent — the honesty is the declaration (file granularity can
    /// only extend real coverage past the declared range, never
    /// shrink it).
    pub coverage: CoverageWire,
    /// Query-level completeness. An absent id yields a COMPLETE empty
    /// trace (zero spans) — "nothing stored" is an answer, not an error.
    pub status: StatusWire,
    pub items: TraceItems,
    pub summary_root: Option<usize>,
    pub roots: Vec<usize>,
    pub children: Vec<Vec<usize>>,
    pub spans: Vec<SpanWire>,
    pub field_kinds: FieldKindsWire,
    /// The `otel-logs` stream keys of the trace's services (the ids its
    /// `__streams` selector uses), sorted: where the trace's logs live.
    pub log_streams: Vec<String>,
}

/// Result accounting. `returned` counts the spans in this response —
/// under a `size_cap` partial, more unique spans exist than returned.
#[derive(Debug, Serialize)]
pub struct TraceItems {
    pub returned: usize,
}

/// One span, ids in W3C lowercase hex. `fields` are the engine's
/// resolved row facets as `[name, value]` string pairs (order
/// preserved; names are storage names — `field_kinds.fields` carries
/// their schema kinds). `kind` is the raw OTLP span-kind int; the
/// readable label is in `fields` under `kind`.
#[derive(Debug, Serialize)]
pub struct SpanWire {
    pub span_id: String,
    /// Absent for a root span (the OTLP unset-parent convention).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub start_ns: i64,
    pub duration_ns: i64,
    /// The duration less the time its direct children cover, over the
    /// assembled trace (all its spans, across files). Children are found by
    /// parent id with the seal's rule, which can differ from the drawn tree:
    /// a child counts under every span carrying its parent id (a CLIENT and
    /// a SERVER sharing an id), and a span without its own id is nobody's
    /// child.
    pub self_duration_ns: i64,
    /// ERROR, and none of its direct children in the trace (the same
    /// children as `self_duration_ns`) is ERROR.
    pub error_origin: bool,
    pub kind: i32,
    pub flags: u32,
    pub dropped_attributes_count: u32,
    pub dropped_events_count: u32,
    pub dropped_links_count: u32,
    pub fields: Vec<(String, String)>,
    pub events: Vec<EventWire>,
    pub links: Vec<LinkWire>,
}

/// One span event; attribute keys are prefix-stripped (`foo`, not
/// `events.attributes.foo`).
#[derive(Debug, Serialize)]
pub struct EventWire {
    pub time_unix_nano: u64,
    pub name: String,
    pub dropped_attributes_count: u32,
    pub attributes: Vec<(String, String)>,
}

/// One span link, ids in W3C lowercase hex.
#[derive(Debug, Serialize)]
pub struct LinkWire {
    pub trace_id: String,
    pub span_id: String,
    pub trace_state: String,
    pub flags: u32,
    pub dropped_attributes_count: u32,
    pub attributes: Vec<(String, String)>,
}

/// The sectioned name→kind map (span fields / event attributes / link
/// attributes — sectioned because an event attr and a link attr may
/// share a name with different kinds). Kind words are pinned by the
/// adapter's exhaustive mapping.
#[derive(Debug, Serialize)]
pub struct FieldKindsWire {
    pub fields: Vec<(String, &'static str)>,
    pub event_attributes: Vec<(String, &'static str)>,
    pub link_attributes: Vec<(String, &'static str)>,
}

#[derive(Debug, Serialize)]
pub struct InfoResponse {
    /// The response's self-description: always `"info"`.
    pub mode: &'static str,
    version: u32,
    status: u32,
    #[serde(rename = "type")]
    response_type: &'static str,
    has_history: bool,
    #[serde(rename = "v")]
    protocol_version: u32,
    accepted_params: Vec<&'static str>,
    required_params: Vec<&'static str>,
    help: &'static str,
    /// The explorer sections an `explore` request may ask for.
    sections: Vec<&'static str>,
    /// What the explorer opens with.
    defaults: InfoDefaults,
    /// The largest values an `explore` request may ask for.
    limits: InfoLimits,
    /// How duration percentiles are computed.
    percentiles: InfoPercentiles,
    /// Numbers the explorer derives per stored file, not per whole trace.
    approximations: InfoApproximations,
    /// The fixed duration bands, fastest first, with inclusive bounds.
    duration_bands: Vec<DurationBandWire>,
    /// The partial reasons an `explore` answer may carry.
    partial_reasons: Vec<PartialReasonWire>,
}

#[derive(Debug, Serialize)]
pub struct InfoDefaults {
    /// The default scope chips (entry spans).
    filter: std::collections::BTreeMap<&'static str, Vec<&'static str>>,
    stack: &'static str,
    window_s: i64,
    rows_limit: usize,
    values_limit: usize,
}

#[derive(Debug, Serialize)]
pub struct InfoLimits {
    rows_page_max: usize,
    top_k_max: usize,
    row_columns_max: usize,
    values_max: usize,
    trace_ids_max: usize,
    /// Values one facet lists; the rest are counted as omitted.
    facet_values_max: usize,
    /// Groups listed; the rest fold into `other`.
    groups_max: usize,
    /// Selection rows a facet value needs to be ranked.
    min_support: u64,
}

/// Error origins and self time are derived within each stored file (a live
/// WAL counts as one): a parent whose child landed in another file can be
/// counted as an origin, or its self time overstated. They are shown with
/// `label`; the trace drawer derives them over the whole trace, exactly.
#[derive(Debug, Serialize)]
pub struct InfoApproximations {
    origin_and_self_time: &'static str,
    label: &'static str,
}

/// Percentiles come from a fixed log-linear histogram: each value is within
/// `max_relative_error` of the exact one, and is shown with `label`.
#[derive(Debug, Serialize)]
pub struct InfoPercentiles {
    approximate: bool,
    max_relative_error: f64,
    label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DurationBandWire {
    label: &'static str,
    min_ns: i64,
    /// `None` for the last, open-ended band.
    max_ns: Option<i64>,
}

fn duration_bands() -> Vec<DurationBandWire> {
    let mut bands = Vec::with_capacity(sfsq::traces::explore::DURATION_BAND_COUNT);
    let mut min_ns = 0;
    for (index, label) in sfsq::traces::explore::DURATION_BAND_LABELS.into_iter().enumerate() {
        let next = sfsq::traces::explore::DURATION_BAND_EDGES_NS.get(index).copied();
        bands.push(DurationBandWire {
            label,
            min_ns,
            max_ns: next.map(|edge| edge - 1),
        });
        if let Some(edge) = next {
            min_ns = edge;
        }
    }
    bands
}

impl Default for InfoResponse {
    fn default() -> Self {
        let (role_field, roles) = sfsq::traces::explore::DEFAULT_POPULATION;
        Self {
            mode: "info",
            version: 1,
            status: 200,
            response_type: "traces",
            has_history: true,
            protocol_version: 3,
            accepted_params: ACCEPTED_PARAMS.to_vec(),
            required_params: vec![],
            help: "Query and visualize OpenTelemetry traces.",
            sections: vec!["histogram", "facets", "groups", "rows", "fields"],
            defaults: InfoDefaults {
                filter: std::collections::BTreeMap::from([(role_field, roles.to_vec())]),
                stack: sfsq::traces::explore::DEFAULT_STACK_FIELD,
                window_s: explore::DEFAULT_WINDOW_S,
                rows_limit: explore::ROWS_DEFAULT_LIMIT,
                values_limit: VALUES_DEFAULT_LIMIT,
            },
            limits: InfoLimits {
                rows_page_max: sfsq::traces::explore::ROWS_PAGE_MAX,
                top_k_max: sfsq::traces::explore::TOP_K_MAX,
                row_columns_max: sfsq::traces::explore::ROW_COLUMNS_MAX,
                values_max: sfsq::traces::explore::VALUES_LIMIT_MAX,
                trace_ids_max: sfsq::traces::explore::TRACE_IDS_MAX,
                facet_values_max: sfsq::traces::explore::MAX_FACET_VALUES,
                groups_max: sfsq::traces::explore::GROUPS_CAP,
                min_support: sfsq::traces::explore::MIN_SUPPORT,
            },
            percentiles: InfoPercentiles {
                approximate: true,
                max_relative_error: sfsq::traces::duration_hist::MAX_RELATIVE_ERROR,
                label: "≈",
            },
            approximations: InfoApproximations {
                origin_and_self_time: "per stored file",
                label: "≈",
            },
            duration_bands: duration_bands(),
            partial_reasons: vec![
                PartialReasonWire::SourceFailure,
                PartialReasonWire::RemoteUnavailable,
                PartialReasonWire::Cancelled,
                PartialReasonWire::LegacyFile,
                PartialReasonWire::StackFieldHighCard,
                PartialReasonWire::FacetHighCard,
                PartialReasonWire::FacetValueCap,
                PartialReasonWire::GroupsCap,
                PartialReasonWire::LivePassFailed,
            ],
        }
    }
}

mod explore;
mod values;
pub use explore::{
    BucketWire, Chips, ComparisonTotalsWire, ComparisonWire, DeltaOtherWire, DeltaRowWire,
    DeltaSideWire, ExploreDataWire, ExploreFacetValueWire, ExploreFacetWire, ExploreFacetsWire,
    ExploreParams, ExploreResponse, FieldWire, FieldsWire, GridWire, GroupNumbersWire, GroupWire,
    GroupsDeltaWire, GroupsWire, HistogramWire, OtherGroupsWire, PercentileMethodWire,
    PercentilesWire, RowWire, RowsWire, TotalsWire, UnavailableFacetWire, WindowWire,
};
pub use values::{VALUES_DEFAULT_LIMIT, ValuesParams, ValuesResponse};

#[cfg(test)]
mod tests;
