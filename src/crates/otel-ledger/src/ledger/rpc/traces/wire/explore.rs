//! The explorer request (`{"explore": {...}}`) and its response.
//!
//! Every shape and value check runs while the request deserializes, so a bad
//! request is an HTTP 400 before any data is read. Sections this build cannot
//! answer yet are refused by name rather than ignored.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::StatusWire;

use sfsq::traces::explore::TRACE_IDS_MAX;

/// Length of the default window, seconds.
pub const DEFAULT_WINDOW_S: i64 = 900;

/// Rows per page (or K) when the rows section names no limit.
pub const ROWS_DEFAULT_LIMIT: usize = 100;

/// The explorer's parameters, validated.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawExploreParams")]
pub struct ExploreParams {
    pub window: RequestWindow,
    /// Scope chips in storage field names: OR within a field, AND across
    /// fields. The engine applies no default scope; the UI sends it.
    pub filter: BTreeMap<String, Vec<String>>,
    /// A literal searched in every value.
    pub text: Option<String>,
    /// Keep only these traces' spans (1 to [`TRACE_IDS_MAX`]).
    pub trace_ids: Vec<sfst::TraceId>,
    /// A part of the scope to compare with the rest; rows follow it.
    pub selection: Option<SelectionRequest>,
    pub histogram: Option<HistogramRequest>,
    pub facets: Option<FacetsRequest>,
    pub rows: Option<sfsq::traces::explore::RowsSpec>,
    /// Whether to group every span of the scope's traces by service and
    /// operation.
    pub groups: bool,
    /// Whether to list the window's fields.
    pub fields: bool,
}

/// A window in whole seconds: both bounds relative to now (`≤ 0`) or both
/// absolute unix seconds (`> 0`), `after < before`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestWindow {
    pub after: i64,
    pub before: i64,
}

impl RequestWindow {
    /// The window of a request's `after` and `before`: `before` defaults to
    /// now (0), `after` to [`DEFAULT_WINDOW_S`] before `before`.
    pub fn parse(after: Option<i64>, before: Option<i64>) -> Result<Self, String> {
        let before = before.unwrap_or(0);
        let after = after.unwrap_or(before - DEFAULT_WINDOW_S);
        if (after <= 0) != (before <= 0) {
            return Err(
                "`after` and `before` must both be relative (≤ 0) or both absolute (> 0)".into(),
            );
        }
        if after >= before {
            return Err(format!(
                "`after` ({after}) must be before `before` ({before})"
            ));
        }
        Ok(RequestWindow { after, before })
    }

    /// Absolute unix seconds for a request received at `now_s`.
    pub fn resolve(&self, now_s: u32) -> (u32, u32) {
        let absolute = |s: i64| {
            let s = if self.before <= 0 {
                i64::from(now_s) + s
            } else {
                s
            };
            u32::try_from(s.max(0)).unwrap_or(u32::MAX)
        };
        (absolute(self.after), absolute(self.before))
    }
}

/// The facets section: values with their scope-row counts per field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetsRequest {
    /// `None`: every field low or mid cardinality in every file.
    pub fields: Option<Vec<String>>,
}

/// The selection, validated: chips, an inclusive duration range and a
/// start-time range (unix nanoseconds, `[after, before)`), all ANDed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionRequest {
    pub filter: BTreeMap<String, Vec<String>>,
    pub duration: Option<sfst::DurationRange>,
    pub time_ns: Option<std::ops::Range<i64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSelection {
    #[serde(default)]
    filter: BTreeMap<String, Vec<String>>,
    #[serde(default, deserialize_with = "super::present")]
    duration: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "super::present")]
    time: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDuration {
    #[serde(default)]
    min_ns: Option<i64>,
    #[serde(default)]
    max_ns: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTime {
    after_ns: String,
    before_ns: String,
}

fn check_filter(filter: &BTreeMap<String, Vec<String>>, what: &str) -> Result<(), String> {
    for (field, values) in filter {
        if field.is_empty() {
            return Err(format!("a {what} field name is empty"));
        }
        if values.is_empty() {
            return Err(format!("the {what} on `{field}` lists no values"));
        }
    }
    Ok(())
}

fn parse_selection(value: serde_json::Value) -> Result<SelectionRequest, String> {
    if value.is_null() {
        return Err("`selection` is null; omit it instead".into());
    }
    let raw: RawSelection =
        serde_json::from_value(value).map_err(|e| format!("`selection`: {e}"))?;
    check_filter(&raw.filter, "selection filter")?;
    let duration = match raw.duration {
        None => None,
        Some(serde_json::Value::Null) => {
            return Err("the selection's `duration` is null; omit it instead".into());
        }
        Some(value) => {
            let raw: RawDuration = serde_json::from_value(value)
                .map_err(|e| format!("the selection's `duration`: {e}"))?;
            if raw.min_ns.is_none() && raw.max_ns.is_none() {
                return Err("the selection's `duration` needs `min_ns` or `max_ns`".into());
            }
            if raw.min_ns.is_some_and(|min| min < 0) || raw.max_ns.is_some_and(|max| max < 0) {
                return Err("the selection's `duration` bounds cannot be negative".into());
            }
            if let (Some(min), Some(max)) = (raw.min_ns, raw.max_ns)
                && min > max
            {
                return Err("the selection's `duration` has `min_ns` above `max_ns`".into());
            }
            Some(sfst::DurationRange {
                min_ns: raw.min_ns,
                max_ns: raw.max_ns,
            })
        }
    };
    let time_ns = match raw.time {
        None => None,
        Some(serde_json::Value::Null) => {
            return Err("the selection's `time` is null; omit it instead".into());
        }
        Some(value) => {
            let raw: RawTime = serde_json::from_value(value)
                .map_err(|e| format!("the selection's `time`: {e}"))?;
            let parse = |text: &str, name: &str| {
                text.parse::<i64>().map_err(|_| {
                    format!("the selection's `time.{name}` is not a decimal nanosecond timestamp")
                })
            };
            let after = parse(&raw.after_ns, "after_ns")?;
            let before = parse(&raw.before_ns, "before_ns")?;
            if after >= before {
                return Err("the selection's `time` must have `after_ns` below `before_ns`".into());
            }
            Some(after..before)
        }
    };
    if raw.filter.is_empty() && duration.is_none() && time_ns.is_none() {
        return Err("`selection` needs a filter, a duration or a time".into());
    }
    Ok(SelectionRequest {
        filter: raw.filter,
        duration,
        time_ns,
    })
}

/// The histogram section: rows per bucket stacked by one field, with the
/// duration percentiles per bucket and for the window unless turned off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistogramRequest {
    pub stack: String,
    pub percentiles: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExploreParams {
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    filter: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    trace_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::present")]
    selection: Option<serde_json::Value>,
    #[serde(default)]
    sections: Option<RawSections>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSections {
    #[serde(default, deserialize_with = "super::present")]
    histogram: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "super::present")]
    facets: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "super::present")]
    groups: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "super::present")]
    rows: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "super::present")]
    fields: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFacets {
    #[serde(default)]
    fields: Option<Vec<String>>,
}

/// A section's parameters: absent, or an object of the section's shape;
/// `null` and other JSON values are refused.
fn section<T: serde::de::DeserializeOwned>(
    name: &str,
    value: Option<serde_json::Value>,
) -> Result<Option<T>, String> {
    match value {
        None => Ok(None),
        Some(serde_json::Value::Null) => Err(format!("section `{name}` is null; omit it instead")),
        Some(value) if !value.is_object() => Err(format!("section `{name}` must be an object")),
        Some(value) => T::deserialize(&value)
            .map(Some)
            .map_err(|e| format!("section `{name}`: {e}")),
    }
}

/// The fields section takes no parameters.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFields {}

/// The groups section takes no parameters.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGroups {}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRows {
    #[serde(default)]
    order: Option<RawRowOrder>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    anchor: Option<String>,
    /// `older` (the default) or `newer`; `newer` without an anchor is the
    /// oldest page.
    #[serde(default)]
    direction: Option<RawRowDirection>,
    #[serde(default)]
    columns: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawRowOrder {
    Newest,
    Slowest,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawRowDirection {
    Older,
    Newer,
}

impl TryFrom<RawRows> for sfsq::traces::explore::RowsSpec {
    type Error = String;

    fn try_from(raw: RawRows) -> Result<Self, String> {
        use sfsq::traces::explore::{
            ROW_COLUMNS_MAX, ROWS_PAGE_MAX, RowDirection, RowKey, RowOrder, TOP_K_MAX,
            is_row_column,
        };
        let limit = raw.limit.unwrap_or(ROWS_DEFAULT_LIMIT);
        let (order, max) = match raw.order.unwrap_or(RawRowOrder::Newest) {
            RawRowOrder::Newest => {
                let anchor = match raw.anchor {
                    None => None,
                    Some(cursor) => {
                        let key = RowKey::decode(&cursor);
                        Some(
                            key.ok_or_else(|| format!("rows `anchor` {cursor:?} is not a cursor"))?,
                        )
                    }
                };
                let direction = match raw.direction.unwrap_or(RawRowDirection::Older) {
                    RawRowDirection::Older => RowDirection::Older,
                    RawRowDirection::Newer => RowDirection::Newer,
                };
                (RowOrder::Newest { anchor, direction }, ROWS_PAGE_MAX)
            }
            RawRowOrder::Slowest => {
                if raw.anchor.is_some() || raw.direction.is_some() {
                    return Err(
                        "rows `anchor` and `direction` apply only to the newest order".into(),
                    );
                }
                (RowOrder::Slowest, TOP_K_MAX)
            }
        };
        if limit == 0 || limit > max {
            return Err(format!("rows `limit` must be 1 to {max}"));
        }
        if raw.columns.len() > ROW_COLUMNS_MAX {
            return Err(format!("rows take at most {ROW_COLUMNS_MAX} columns"));
        }
        if raw.columns.iter().any(|c| c.is_empty()) {
            return Err("rows name an empty column".into());
        }
        for (i, column) in raw.columns.iter().enumerate() {
            if raw.columns[..i].contains(column) {
                return Err(format!("rows name the column `{column}` twice"));
            }
        }
        if let Some(c) = raw.columns.iter().find(|c| !is_row_column(c)) {
            return Err(format!(
                "`{c}` is an events or links field, not a span column"
            ));
        }
        Ok(sfsq::traces::explore::RowsSpec {
            order,
            limit,
            columns: raw.columns,
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHistogram {
    #[serde(default)]
    stack: Option<String>,
    #[serde(default)]
    percentiles: Option<bool>,
}

impl TryFrom<RawExploreParams> for ExploreParams {
    type Error = String;

    fn try_from(raw: RawExploreParams) -> Result<Self, String> {
        let selection = raw.selection.map(parse_selection).transpose()?;
        let text = match raw.text {
            Some(text) if text.trim().is_empty() => {
                return Err("`text` is empty; omit it instead".into());
            }
            Some(text) => Some(text.trim().to_string()),
            None => None,
        };
        let mut trace_ids = Vec::new();
        if let Some(ids) = raw.trace_ids {
            if ids.is_empty() || ids.len() > TRACE_IDS_MAX {
                return Err(format!("`trace_ids` needs 1 to {TRACE_IDS_MAX} ids"));
            }
            for id in ids {
                let parsed = super::super::adapter::parse_trace_id(&id)?;
                if parsed.is_unset() {
                    return Err("the all-zero trace id is not queryable".into());
                }
                trace_ids.push(parsed);
            }
        }

        let window = RequestWindow::parse(raw.after, raw.before)?;

        check_filter(&raw.filter, "filter")?;

        let (histogram, facets, groups, rows, fields) = match raw.sections {
            None => (
                Some(HistogramRequest {
                    stack: sfsq::traces::explore::DEFAULT_STACK_FIELD.to_string(),
                    percentiles: true,
                }),
                Some(FacetsRequest { fields: None }),
                true,
                Some(RawRows::default().try_into()?),
                true,
            ),
            Some(sections) => {
                let histogram = match section::<RawHistogram>("histogram", sections.histogram)? {
                    None => None,
                    Some(spec) => {
                        let stack = spec.stack.unwrap_or_else(|| {
                            sfsq::traces::explore::DEFAULT_STACK_FIELD.to_string()
                        });
                        if stack.trim().is_empty() {
                            return Err("the histogram's `stack` field is empty".into());
                        }
                        Some(HistogramRequest {
                            stack,
                            percentiles: spec.percentiles.unwrap_or(true),
                        })
                    }
                };
                let facets = match section::<RawFacets>("facets", sections.facets)? {
                    None => None,
                    Some(RawFacets { fields }) => {
                        if let Some(fields) = &fields {
                            if fields.is_empty() {
                                return Err("section `facets` lists no fields; omit `fields` \
                                            for every field"
                                    .into());
                            }
                            if fields.iter().any(|f| f.is_empty()) {
                                return Err("section `facets` names an empty field".into());
                            }
                        }
                        Some(FacetsRequest { fields })
                    }
                };
                let rows = match section::<RawRows>("rows", sections.rows)? {
                    None => None,
                    Some(raw) => Some(raw.try_into()?),
                };
                let groups = section::<RawGroups>("groups", sections.groups)?.is_some();
                let fields = section::<RawFields>("fields", sections.fields)?.is_some();
                (histogram, facets, groups, rows, fields)
            }
        };

        Ok(ExploreParams {
            window,
            filter: raw.filter,
            text,
            trace_ids,
            selection,
            histogram,
            facets,
            rows,
            groups,
            fields,
        })
    }
}

/// The explorer's response: the Functions envelope the UI routes on, with the
/// explorer data under `data`.
#[derive(Debug, Serialize)]
pub struct ExploreResponse {
    pub status: u32,
    #[serde(rename = "type")]
    pub response_type: &'static str,
    pub data: ExploreDataWire,
}

#[derive(Debug, Serialize)]
pub struct ExploreDataWire {
    pub mode: &'static str,
    pub version: u32,
    pub window: WindowWire,
    /// Every section's reasons together.
    pub status: StatusWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub histogram: Option<HistogramWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<ExploreFacetsWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<GroupsWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<RowsWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<FieldsWire>,
}

/// Every span in the window of the traces with a span in scope, by service
/// and operation; the UI derives rates, error shares and self-time shares.
#[derive(Debug, Serialize)]
pub struct GroupsWire {
    pub status: StatusWire,
    pub window_s: u64,
    /// Self time over every group, `other` included, in nanoseconds as a
    /// decimal string.
    pub self_ns_total: String,
    /// The groups with the most spans, at most `groups_max` (info).
    pub rows: Vec<GroupWire>,
    /// The rest folded together; `null` when nothing was folded.
    pub other: Option<OtherGroupsWire>,
    /// Only under a selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<GroupsDeltaWire>,
}

/// The groups split by a selection (QRY-20): a scope trace is on the
/// selection side when any of its scope rows is a selection row, on the
/// baseline side otherwise; a scope row without a trace id is its own trace.
#[derive(Debug, Serialize)]
pub struct GroupsDeltaWire {
    pub selection_traces: u64,
    pub baseline_traces: u64,
    /// Self time over every group, `other` included, per side, in
    /// nanoseconds as decimal strings.
    pub selection_self_ns_total: String,
    pub baseline_self_ns_total: String,
    /// Parallel to `groups.rows`.
    pub rows: Vec<DeltaRowWire>,
    /// Parallel to `groups.other`.
    pub other: Option<DeltaOtherWire>,
}

#[derive(Debug, Serialize)]
pub struct DeltaRowWire {
    pub service: Option<String>,
    pub operation: Option<String>,
    pub selection: DeltaSideWire,
    pub baseline: DeltaSideWire,
}

#[derive(Debug, Serialize)]
pub struct DeltaOtherWire {
    pub groups: u64,
    pub selection: DeltaSideWire,
    pub baseline: DeltaSideWire,
}

/// A group's rows on one side; the two sides add up to the group.
#[derive(Debug, Serialize)]
pub struct DeltaSideWire {
    pub spans: u64,
    pub errors_originated: u64,
    /// In nanoseconds, as a decimal string.
    pub self_ns: String,
}

#[derive(Debug, Serialize)]
pub struct GroupWire {
    /// `null` for spans without a service or an operation.
    pub service: Option<String>,
    pub operation: Option<String>,
    #[serde(flatten)]
    pub numbers: GroupNumbersWire,
}

#[derive(Debug, Serialize)]
pub struct OtherGroupsWire {
    /// How many groups were folded.
    pub groups: u64,
    #[serde(flatten)]
    pub numbers: GroupNumbersWire,
}

#[derive(Debug, Serialize)]
pub struct GroupNumbersWire {
    pub spans: u64,
    pub errors: u64,
    pub errors_originated: u64,
    /// Approximate (the fixed duration histogram); `null` without spans.
    pub p95_ns: Option<i64>,
    /// Summed self time in nanoseconds, as a decimal string.
    pub self_ns: String,
}

/// The window actually answered: the request's, aligned outward to whole
/// buckets.
#[derive(Debug, Serialize)]
pub struct WindowWire {
    pub after: u32,
    pub before: u32,
    pub grid: GridWire,
}

#[derive(Debug, Serialize)]
pub struct GridWire {
    /// First bucket's start, unix nanoseconds, as a decimal string.
    pub start_ns: String,
    pub bucket_ns: i64,
    pub buckets: usize,
}

#[derive(Debug, Serialize)]
pub struct HistogramWire {
    pub status: StatusWire,
    pub stack: String,
    /// The stack field's values; each bucket's `counts` is parallel to it.
    pub dimensions: Vec<String>,
    pub buckets: Vec<BucketWire>,
    pub totals: TotalsWire,
    /// Present when percentiles were asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<PercentileMethodWire>,
}

/// `counts` per value, `unset` for rows without the stack field, `other`
/// for rows of files where the stack field is high-cardinality.
#[derive(Debug, Serialize)]
pub struct BucketWire {
    pub counts: Vec<u64>,
    pub unset: u64,
    pub other: u64,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<PercentilesWire>,
}

/// Duration percentiles, nanoseconds; approximate (see
/// [`HistogramWire::percentiles`]).
#[derive(Debug, Serialize)]
pub struct PercentilesWire {
    pub p50_ns: i64,
    pub p95_ns: i64,
    pub p99_ns: i64,
}

/// How the percentiles were computed: from a fixed histogram, each value
/// within `max_relative_error` of the true one.
#[derive(Debug, Serialize)]
pub struct PercentileMethodWire {
    pub approximate: bool,
    pub max_relative_error: f64,
}

#[derive(Debug, Serialize)]
pub struct TotalsWire {
    pub count: u64,
    pub errors: u64,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<PercentilesWire>,
}

#[derive(Debug, Serialize)]
pub struct ExploreFacetsWire {
    pub status: StatusWire,
    /// Requested fields in request order, or every eligible field by name.
    pub fields: Vec<ExploreFacetWire>,
    /// Requested fields that could not be faceted, and why.
    pub unavailable: Vec<UnavailableFacetWire>,
    /// With a selection: the scope and selection rows, and the fewest
    /// selection rows a value needs to be ranked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ComparisonWire>,
}

#[derive(Debug, Serialize)]
pub struct ComparisonWire {
    pub scope: u64,
    pub selection: u64,
    pub min_support: u64,
}

/// Rows a field's comparison is out of: without the field's own chips.
#[derive(Debug, Serialize)]
pub struct ComparisonTotalsWire {
    pub scope: u64,
    pub selection: u64,
}

#[derive(Debug, Serialize)]
pub struct ExploreFacetWire {
    pub field: String,
    pub values: Vec<ExploreFacetValueWire>,
    /// Values beyond the per-facet cap, and the rows they held.
    pub omitted_values: u64,
    pub omitted_rows: u64,
    /// With a selection: the field's rank by its best difference (when a
    /// value is eligible), that difference, and its totals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_diff: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totals: Option<ComparisonTotalsWire>,
}

#[derive(Debug, Serialize)]
pub struct ExploreFacetValueWire {
    pub value: String,
    /// Scope rows.
    pub count: u64,
    /// With a selection: its rows, the rest of the scope's, whether the value
    /// has enough selection rows to rank, its rank and its share difference
    /// (absent when the selection holds no row of the field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct UnavailableFacetWire {
    pub field: String,
    pub reason: super::PartialReasonWire,
}

#[derive(Debug, Serialize)]
pub struct RowsWire {
    pub status: StatusWire,
    /// `"newest"` or `"slowest"`.
    pub order: &'static str,
    /// Scope rows in the window; the same on every page.
    pub matched: u64,
    /// Newest order only: whether rows exist beyond the page on each side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_older: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_newer: Option<bool>,
    /// Newest first (or slowest first).
    pub items: Vec<RowWire>,
}

/// One span row. Absent fields are `null`; `columns` holds only the asked
/// columns the row has, multi-valued ones joined with `", "`.
#[derive(Debug, Serialize)]
pub struct RowWire {
    /// The row's content key, the `anchor` for the next page either way.
    pub cursor: String,
    /// Unix nanoseconds, as a decimal string.
    pub start_ns: String,
    pub duration_ns: i64,
    /// The duration less the time its direct children cover; `null` when
    /// the row's live WAL could not be derived (`live_pass_failed`).
    pub self_duration_ns: Option<i64>,
    pub trace_id: String,
    pub span_id: String,
    pub service: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    pub columns: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct FieldsWire {
    pub status: StatusWire,
    /// By name.
    pub items: Vec<FieldWire>,
    /// Values every row carries besides its fields.
    pub columns: Vec<&'static str>,
}

/// One field: its highest tier across the window's files and what it
/// supports.
#[derive(Debug, Serialize)]
pub struct FieldWire {
    pub name: String,
    /// `"low"`, `"mid"` or `"high"`.
    pub tier: &'static str,
    pub chip: bool,
    pub facet: bool,
    pub stack: bool,
    pub text: bool,
    pub column: bool,
}
