//! The explorer request (`{"explore": {...}}`) and its response.
//!
//! Every shape and value check runs while the request deserializes, so a bad
//! request is an HTTP 400 before any data is read. Sections this build cannot
//! answer yet are refused by name rather than ignored.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::StatusWire;

/// Length of the default window, seconds.
pub const DEFAULT_WINDOW_S: i64 = 900;

/// The explorer's parameters, validated.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawExploreParams")]
pub struct ExploreParams {
    pub window: RequestWindow,
    /// Scope chips in storage field names: OR within a field, AND across
    /// fields. The engine applies no default scope; the UI sends it.
    pub filter: BTreeMap<String, Vec<String>>,
    pub histogram: Option<HistogramRequest>,
    pub facets: Option<FacetsRequest>,
}

/// A window in whole seconds: both bounds relative to now (`≤ 0`) or both
/// absolute unix seconds (`> 0`), `after < before`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestWindow {
    pub after: i64,
    pub before: i64,
}

impl RequestWindow {
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
    #[serde(default, deserialize_with = "super::present")]
    text: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "super::present")]
    trace_ids: Option<serde_json::Value>,
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
        for (name, value) in [
            ("text", &raw.text),
            ("trace_ids", &raw.trace_ids),
            ("selection", &raw.selection),
        ] {
            if value.is_some() {
                return Err(format!("`{name}` is not available yet"));
            }
        }

        let before = raw.before.unwrap_or(0);
        let after = raw.after.unwrap_or(before - DEFAULT_WINDOW_S);
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

        for (field, values) in &raw.filter {
            if field.is_empty() {
                return Err("a filter field name is empty".into());
            }
            if values.is_empty() {
                return Err(format!("the filter on `{field}` lists no values"));
            }
        }

        let (histogram, facets) = match raw.sections {
            None => (
                Some(HistogramRequest {
                    stack: sfsq::traces::explore::DEFAULT_STACK_FIELD.to_string(),
                    percentiles: true,
                }),
                Some(FacetsRequest { fields: None }),
            ),
            Some(sections) => {
                for (name, value) in [
                    ("groups", &sections.groups),
                    ("rows", &sections.rows),
                    ("fields", &sections.fields),
                ] {
                    if value.is_some() {
                        return Err(format!("section `{name}` is not available yet"));
                    }
                }
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
                (histogram, facets)
            }
        };

        Ok(ExploreParams {
            window: RequestWindow { after, before },
            filter: raw.filter,
            histogram,
            facets,
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
}

#[derive(Debug, Serialize)]
pub struct ExploreFacetWire {
    pub field: String,
    pub values: Vec<ExploreFacetValueWire>,
    /// Values beyond the per-facet cap, and the rows they held.
    pub omitted_values: u64,
    pub omitted_rows: u64,
}

#[derive(Debug, Serialize)]
pub struct ExploreFacetValueWire {
    pub value: String,
    pub count: u64,
}

#[derive(Debug, Serialize)]
pub struct UnavailableFacetWire {
    pub field: String,
    pub reason: super::PartialReasonWire,
}
