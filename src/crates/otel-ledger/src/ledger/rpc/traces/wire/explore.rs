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

/// The histogram section: rows per bucket stacked by one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistogramRequest {
    pub stack: String,
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

        let histogram = match raw.sections {
            None => Some(HistogramRequest {
                stack: sfsq::traces::explore::DEFAULT_STACK_FIELD.to_string(),
            }),
            Some(sections) => {
                for (name, value) in [
                    ("facets", &sections.facets),
                    ("groups", &sections.groups),
                    ("rows", &sections.rows),
                    ("fields", &sections.fields),
                ] {
                    if value.is_some() {
                        return Err(format!("section `{name}` is not available yet"));
                    }
                }
                match sections.histogram {
                    None => None,
                    Some(serde_json::Value::Null) => {
                        return Err("section `histogram` is null; omit it instead".into());
                    }
                    Some(value) => {
                        if !value.is_object() {
                            return Err("section `histogram` must be an object".into());
                        }
                        let spec = RawHistogram::deserialize(&value)
                            .map_err(|e| format!("section `histogram`: {e}"))?;
                        if spec.percentiles == Some(true) {
                            return Err("histogram percentiles are not available yet".into());
                        }
                        let stack = spec.stack.unwrap_or_else(|| {
                            sfsq::traces::explore::DEFAULT_STACK_FIELD.to_string()
                        });
                        if stack.trim().is_empty() {
                            return Err("the histogram's `stack` field is empty".into());
                        }
                        Some(HistogramRequest { stack })
                    }
                }
            }
        };

        Ok(ExploreParams {
            window: RequestWindow { after, before },
            filter: raw.filter,
            histogram,
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
}

/// `counts` per value, `unset` for rows without the stack field, `other`
/// for rows of files where the stack field is high-cardinality.
#[derive(Debug, Serialize)]
pub struct BucketWire {
    pub counts: Vec<u64>,
    pub unset: u64,
    pub other: u64,
}

#[derive(Debug, Serialize)]
pub struct TotalsWire {
    pub count: u64,
    pub errors: u64,
}
