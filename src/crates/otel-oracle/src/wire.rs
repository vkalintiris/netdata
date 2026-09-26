//! The explorer's JSON answers, as the calculator reads them: its own copies
//! of the shapes the `otel-traces` Function returns (never the plugin's
//! types), tolerant of fields it does not know. Reasons stay strings, so a new
//! reason parses; nanosecond values that arrive as strings are read as exact
//! integers, never through floating point.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A section's or an answer's status: complete, or partial with reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Status {
    Complete { complete: bool },
    Partial { partial: Vec<Reason> },
}

impl Status {
    pub fn is_complete(&self) -> bool {
        matches!(self, Status::Complete { complete: true })
    }

    /// The reasons by name (the wire lists them in the plugin's own order).
    pub fn reasons(&self) -> BTreeMap<String, Reason> {
        let mut out = BTreeMap::new();
        if let Status::Partial { partial } = self {
            for reason in partial {
                out.insert(reason.reason.clone(), reason.clone());
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub reason: String,
    pub count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<String>,
}

/// An explore answer: `{status, type, data}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExploreAnswer {
    pub status: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Explore,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Explore {
    pub mode: String,
    pub version: u32,
    pub window: Window,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram: Option<Histogram>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<Facets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Groups>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Rows>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Fields>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Groups {
    pub status: Status,
    pub window_s: u64,
    /// Nanoseconds, as a decimal string.
    pub self_ns_total: String,
    pub rows: Vec<Group>,
    pub other: Option<OtherGroups>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<GroupsDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupsDelta {
    pub selection_traces: u64,
    pub baseline_traces: u64,
    /// Nanoseconds, as decimal strings.
    pub selection_self_ns_total: String,
    pub baseline_self_ns_total: String,
    pub rows: Vec<DeltaRow>,
    pub other: Option<DeltaOther>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaRow {
    pub service: Option<String>,
    pub operation: Option<String>,
    pub selection: DeltaSide,
    pub baseline: DeltaSide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaOther {
    pub groups: u64,
    pub selection: DeltaSide,
    pub baseline: DeltaSide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaSide {
    pub spans: u64,
    pub errors_originated: u64,
    /// Nanoseconds, as a decimal string.
    pub self_ns: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub service: Option<String>,
    pub operation: Option<String>,
    #[serde(flatten)]
    pub numbers: GroupNumbers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtherGroups {
    pub groups: u64,
    #[serde(flatten)]
    pub numbers: GroupNumbers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupNumbers {
    pub spans: u64,
    pub errors: u64,
    pub errors_originated: u64,
    pub p95_ns: Option<i64>,
    /// Nanoseconds, as a decimal string.
    pub self_ns: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub after: u32,
    pub before: u32,
    pub grid: Grid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub start_ns: String,
    pub bucket_ns: i64,
    pub buckets: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Histogram {
    pub status: Status,
    pub stack: String,
    pub dimensions: Vec<String>,
    pub buckets: Vec<Bucket>,
    pub totals: Totals,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<PercentileMethod>,
}

/// Percentiles are flattened into their bucket or totals object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Percentiles {
    pub p50_ns: i64,
    pub p95_ns: i64,
    pub p99_ns: i64,
}

impl Percentiles {
    pub fn as_array(&self) -> [i64; 3] {
        [self.p50_ns, self.p95_ns, self.p99_ns]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    /// Parallel to the histogram's `dimensions`.
    pub counts: Vec<u64>,
    pub unset: u64,
    pub other: u64,
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<Percentiles>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub count: u64,
    pub errors: u64,
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<Percentiles>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PercentileMethod {
    pub approximate: bool,
    pub max_relative_error: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Facets {
    pub status: Status,
    pub fields: Vec<Facet>,
    pub unavailable: Vec<Unavailable>,
    /// Under a selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Comparison>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub scope: u64,
    pub selection: u64,
    pub min_support: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonTotals {
    pub scope: u64,
    pub selection: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Facet {
    pub field: String,
    pub values: Vec<FacetValue>,
    pub omitted_values: u64,
    pub omitted_rows: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_diff: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totals: Option<ComparisonTotals>,
    /// Under a selection, a field the selection is made of: plain counts,
    /// no comparison. Absent (false) otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_selection: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacetValue {
    /// `null`: the rows without the field.
    pub value: Option<String>,
    pub count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eligible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unavailable {
    pub field: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rows {
    pub status: Status,
    pub order: String,
    pub matched: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_older: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_newer: Option<bool>,
    pub items: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub cursor: String,
    pub start_ns: String,
    pub duration_ns: i64,
    /// `None` when the row's live WAL could not be derived.
    #[serde(default)]
    pub self_duration_ns: Option<i64>,
    /// Lowercase hex; an unset id is all zeros.
    pub trace_id: String,
    pub span_id: String,
    pub service: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    /// The asked columns the row has; several values joined with ", ".
    pub columns: BTreeMap<String, String>,
}

impl Row {
    pub fn start_ns(&self) -> Option<i64> {
        self.start_ns.parse().ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fields {
    pub status: Status,
    pub items: Vec<Field>,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub tier: String,
    pub chip: bool,
    pub facet: bool,
    pub stack: bool,
    pub text: bool,
    pub column: bool,
}

/// A values answer (no envelope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValuesAnswer {
    pub mode: String,
    pub version: u32,
    pub field: String,
    pub values: Vec<String>,
    pub truncated: bool,
    pub status: Status,
}

/// A nanosecond count the wire sends as a string, read exactly.
pub fn ns(text: &str) -> Option<i64> {
    text.parse().ok()
}

/// A trace answer (the `trace` mode), bare: no Functions envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceAnswer {
    pub mode: String,
    pub version: u32,
    pub trace_id: String,
    pub coverage: Coverage,
    pub status: Status,
    pub items: TraceItems,
    #[serde(default)]
    pub summary_root: Option<usize>,
    pub roots: Vec<usize>,
    pub children: Vec<Vec<usize>>,
    pub spans: Vec<TraceSpanAnswer>,
}

/// The seconds the answer searched, `[after, before)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub after: u32,
    pub before: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceItems {
    pub returned: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSpanAnswer {
    pub span_id: String,
    /// Absent for a span without a parent.
    #[serde(default)]
    pub parent_span_id: Option<String>,
    pub start_ns: i64,
    pub duration_ns: i64,
    #[serde(default)]
    pub self_duration_ns: Option<i64>,
    #[serde(default)]
    pub error_origin: Option<bool>,
    pub kind: i32,
    pub flags: u32,
    pub dropped_attributes_count: u32,
    pub dropped_events_count: u32,
    pub dropped_links_count: u32,
    pub fields: Vec<(String, String)>,
    pub events: Vec<EventAnswer>,
    pub links: Vec<LinkAnswer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAnswer {
    pub time_unix_nano: u64,
    pub name: String,
    pub dropped_attributes_count: u32,
    pub attributes: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkAnswer {
    pub trace_id: String,
    pub span_id: String,
    pub trace_state: String,
    pub flags: u32,
    pub dropped_attributes_count: u32,
    pub attributes: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn explore_json() -> serde_json::Value {
        json!({
            "status": 200,
            "type": "traces",
            "data": {
                "mode": "explore",
                "version": 1,
                "a_field_this_copy_does_not_know": [1, 2],
                "window": {
                    "after": 1_790_352_000,
                    "before": 1_790_352_900,
                    "grid": {"start_ns": "1790352000000000000", "bucket_ns": 15_000_000_000_i64, "buckets": 60}
                },
                "status": {"partial": [
                    {"reason": "stack_field_high_card", "count": 1, "of": 3, "detail": ["attributes.http.route"]},
                    {"reason": "a_reason_added_later", "count": 2}
                ]},
                "histogram": {
                    "status": {"complete": true},
                    "stack": "status_code",
                    "dimensions": ["ERROR", "OK"],
                    "buckets": [
                        {"counts": [1, 4], "unset": 2, "other": 0, "p50_ns": 10, "p95_ns": 20, "p99_ns": 30},
                        {"counts": [0, 0], "unset": 0, "other": 5}
                    ],
                    "totals": {"count": 12, "errors": 1, "p50_ns": 11, "p95_ns": 21, "p99_ns": 31},
                    "percentiles": {"approximate": true, "max_relative_error": 0.0078125}
                },
                "facets": {
                    "status": {"complete": true},
                    "fields": [{"field": "_role", "values": [{"value": "root", "count": 3}], "omitted_values": 0, "omitted_rows": 0}],
                    "unavailable": [{"field": "attributes.http.route", "reason": "facet_high_card"}]
                },
                "rows": {
                    "status": {"complete": true},
                    "order": "newest",
                    "matched": 12,
                    "has_older": true,
                    "items": [{
                        "cursor": "c1",
                        "start_ns": "1790352012345678901",
                        "duration_ns": 5,
                        "self_duration_ns": 3,
                        "trace_id": "00000000000000000000000000000001",
                        "span_id": "0000000000000002",
                        "service": "api",
                        "name": null,
                        "role": "root",
                        "status": null,
                        "columns": {"attributes.http.route": "/a, /b"}
                    }]
                },
                "fields": {
                    "status": {"complete": true},
                    "items": [{"name": "_role", "tier": "low", "chip": true, "facet": true, "stack": true, "text": false, "column": true}],
                    "columns": ["duration", "self_duration", "trace_id", "span_id"]
                }
            }
        })
    }

    #[test]
    fn reads_an_explore_answer_and_writes_it_back() {
        let answer: ExploreAnswer = serde_json::from_value(explore_json()).unwrap();
        let data = &answer.data;

        let histogram = data.histogram.as_ref().unwrap();
        assert_eq!(
            histogram.buckets,
            vec![
                Bucket {
                    counts: vec![1, 4],
                    unset: 2,
                    other: 0,
                    percentiles: Some(Percentiles {
                        p50_ns: 10,
                        p95_ns: 20,
                        p99_ns: 30
                    }),
                },
                Bucket {
                    counts: vec![0, 0],
                    unset: 0,
                    other: 5,
                    percentiles: None,
                },
            ]
        );
        assert_eq!(
            histogram.totals.percentiles.unwrap().as_array(),
            [11, 21, 31]
        );
        assert_eq!(
            data.facets.as_ref().unwrap().unavailable,
            vec![Unavailable {
                field: "attributes.http.route".to_string(),
                reason: "facet_high_card".to_string(),
            }]
        );
        let row = &data.rows.as_ref().unwrap().items[0];
        assert_eq!(row.start_ns(), Some(1_790_352_012_345_678_901));
        assert_eq!(row.name, None);
        assert_eq!(row.self_duration_ns, Some(3));
        assert_eq!(
            ns(&data.window.grid.start_ns),
            Some(1_790_352_000_000_000_000)
        );

        let again: ExploreAnswer =
            serde_json::from_value(serde_json::to_value(&answer).unwrap()).unwrap();
        assert_eq!(again, answer);
    }

    #[test]
    fn reads_both_status_forms_and_keeps_unknown_reasons() {
        let answer: ExploreAnswer = serde_json::from_value(explore_json()).unwrap();
        let reasons = answer.data.status.reasons();

        assert!(!answer.data.status.is_complete());
        assert_eq!(
            reasons["stack_field_high_card"],
            Reason {
                reason: "stack_field_high_card".to_string(),
                count: 1,
                of: Some(3),
                detail: vec!["attributes.http.route".to_string()],
            }
        );
        assert_eq!(reasons["a_reason_added_later"].count, 2);
        let complete: Status = serde_json::from_value(json!({"complete": true})).unwrap();
        assert!(complete.is_complete() && complete.reasons().is_empty());
    }

    #[test]
    fn reads_a_trace_answer() {
        let v = json!({
            "mode": "trace",
            "version": 1,
            "trace_id": "00000000000000000000000000000007",
            "coverage": {"after": 100, "before": 200},
            "status": {"partial": [{"reason": "size_cap", "count": 1}]},
            "items": {"returned": 2},
            "summary_root": 0,
            "roots": [0],
            "children": [[1], []],
            "field_kinds": {"fields": []},
            "log_streams": ["00000000000000a1"],
            "spans": [
                {
                    "span_id": "0000000000000001", "start_ns": 1_790_352_012_345_678_785_i64,
                    "duration_ns": 10, "self_duration_ns": 4, "error_origin": false, "kind": 2,
                    "flags": 257, "dropped_attributes_count": 0, "dropped_events_count": 1,
                    "dropped_links_count": 0, "fields": [["name", "GET"]],
                    "events": [{"time_unix_nano": 0, "name": "", "dropped_attributes_count": 0,
                                "attributes": [["db.rows", "3"]]}],
                    "links": [{"trace_id": "00000000000000000000000000000000",
                               "span_id": "0000000000000009", "trace_state": "k=v", "flags": 1,
                               "dropped_attributes_count": 2, "attributes": []}]
                },
                {
                    "span_id": "0000000000000002", "parent_span_id": "0000000000000001",
                    "start_ns": 1_790_352_012_345_678_790_i64, "duration_ns": 6, "kind": 3, "flags": 0,
                    "dropped_attributes_count": 0, "dropped_events_count": 0,
                    "dropped_links_count": 0, "fields": [], "events": [], "links": []
                }
            ]
        });
        let answer: TraceAnswer = serde_json::from_value(v).unwrap();
        assert_eq!(
            answer.coverage,
            Coverage {
                after: 100,
                before: 200
            }
        );
        assert!(answer.status.reasons().contains_key("size_cap"));
        assert_eq!(answer.spans[0].start_ns, 1_790_352_012_345_678_785);
        assert_eq!(answer.spans[0].parent_span_id, None);
        assert_eq!(answer.spans[1].self_duration_ns, None);
        assert_eq!(
            answer.spans[0].events[0].attributes,
            vec![("db.rows".to_string(), "3".to_string())]
        );
        assert_eq!(answer.spans[0].links[0].trace_state, "k=v");
        let back: TraceAnswer =
            serde_json::from_value(serde_json::to_value(&answer).unwrap()).unwrap();
        assert_eq!(back, answer);
    }

    #[test]
    fn reads_a_values_answer() {
        let answer: ValuesAnswer = serde_json::from_value(json!({
            "mode": "values",
            "version": 1,
            "field": "name",
            "values": ["GET", "POST"],
            "truncated": false,
            "status": {"complete": true}
        }))
        .unwrap();

        assert_eq!(answer.values, vec!["GET".to_string(), "POST".to_string()]);
        assert!(!answer.truncated && answer.status.is_complete());
    }
}
