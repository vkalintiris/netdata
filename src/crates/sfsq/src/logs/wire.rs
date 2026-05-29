//! Netdata UI response envelope for the `otel-logs` function.
//!
//! Mirrors the field shape of the legacy `JournalResponse` so the existing
//! cloud-frontend wiring (`/api/v3/function?function=otel-logs`) works
//! without any changes on the UI side. The cloud-frontend transform reads:
//!
//! - `facets` → sidebar filter options (when `data_only:false`).
//! - `histogram` → main time-series chart.
//! - `data` + `columns` → log-row table.
//! - `items` → pagination footer counts.
//! - `accepted_params` → which request params the UI may send.
//!
//! Step 1 of the otel-logs rewrite: types only. The SFST → wire
//! translation and real handler plumbing land in later steps; for now
//! [`LogsResult::empty_stub`] produces a valid envelope with no data
//! so the build stays green.

use serde::Serialize;

// ── Top-level envelope ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct LogsResult {
    pub progress: u32,
    #[serde(rename = "v")]
    pub version: Version,
    pub accepted_params: Vec<&'static str>,
    pub required_params: Vec<RequiredParam>,
    pub facets: Vec<Facet>,
    pub available_histograms: Vec<AvailableHistogram>,
    pub histogram: Histogram,
    pub columns: serde_json::Value,
    pub data: serde_json::Value,
    pub default_charts: Vec<u32>,
    pub items: Items,
    pub show_ids: bool,
    pub has_history: bool,
    pub status: u32,
    #[serde(rename = "type")]
    pub response_type: String,
    pub help: String,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize)]
pub struct Version(u32);

impl Default for Version {
    fn default() -> Self {
        Self(3)
    }
}

// ── Facets ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Facet {
    pub id: String,
    pub name: String,
    pub order: usize,
    pub options: Vec<FacetOption>,
}

#[derive(Debug, Serialize)]
pub struct FacetOption {
    pub id: String,
    pub name: String,
    pub order: usize,
    pub count: usize,
}

// ── Histogram ───────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AvailableHistogram {
    pub id: String,
    pub name: String,
    pub order: usize,
}

#[derive(Debug, Serialize)]
pub struct Histogram {
    pub id: String,
    pub name: String,
    pub chart: Chart,
}

#[derive(Debug, Serialize)]
pub struct Chart {
    pub view: ChartView,
    pub result: ChartResult,
}

#[derive(Debug, Serialize)]
pub struct ChartView {
    pub title: String,
    pub after: u32,
    pub before: u32,
    pub update_every: u32,
    pub units: String,
    pub chart_type: String,
    pub dimensions: ChartDimensions,
}

#[derive(Debug, Serialize)]
pub struct ChartDimensions {
    pub ids: Vec<String>,
    pub names: Vec<String>,
    pub units: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ChartResult {
    pub labels: Vec<String>,
    pub point: ChartPoint,
    pub data: Vec<DataPoint>,
}

#[derive(Debug, Serialize)]
pub struct ChartPoint {
    pub value: u64,
    pub arp: u64,
    pub pa: u64,
}

/// A single histogram bucket. Serializes as a flat array
/// `[timestamp_ms, [v, arp, pa], [v, arp, pa], …]` — the format the
/// cloud-frontend chart renderer expects, where the first element is the
/// bucket timestamp followed by one `[value, arp, pa]` triple per
/// dimension.
#[derive(Debug)]
pub struct DataPoint {
    pub timestamp_ms: u64,
    pub items: Vec<[usize; 3]>,
}

impl Serialize for DataPoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(1 + self.items.len()))?;
        seq.serialize_element(&self.timestamp_ms)?;
        for item in &self.items {
            seq.serialize_element(item)?;
        }
        seq.end()
    }
}

impl<'de> serde::Deserialize<'de> for DataPoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{SeqAccess, Visitor};

        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = DataPoint;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an array: timestamp_ms followed by [v, arp, pa] triples")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let timestamp_ms = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(DataPoint {
                    timestamp_ms,
                    items,
                })
            }
        }
        deserializer.deserialize_seq(V)
    }
}

// ── Items / pagination ──────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Items {
    pub evaluated: usize,
    pub unsampled: usize,
    pub estimated: usize,
    pub matched: usize,
    pub before: usize,
    pub after: usize,
    pub returned: usize,
    pub max_to_return: usize,
}

#[derive(Debug, Serialize)]
pub struct Pagination {
    pub enabled: bool,
    pub key: &'static str,
    pub column: &'static str,
    pub units: &'static str,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            enabled: true,
            key: "anchor",
            // The hidden opaque-cursor column (see `cursor.rs`). The UI
            // echoes this row's value back as the `anchor` param.
            column: "cursor",
            units: "",
        }
    }
}

// ── Required params (always empty for the otel-logs function) ──────

/// `required_params` is `Vec::new()` in every response the otel-logs
/// function emits — the legacy systemd-journal plugin established this
/// and the UI tolerates it. The enum exists for wire-shape fidelity in
/// case a future request mode needs to surface a required selector.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum RequiredParam {
    // Type shape kept for wire-format parity; no response currently
    // populates it (every emission uses `Vec::new()`).
    #[allow(dead_code)]
    MultiSelection(MultiSelection),
}

#[derive(Debug, Serialize)]
pub struct MultiSelection {
    pub id: &'static str,
    pub name: String,
    pub help: String,
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub options: Vec<MultiSelectionOption>,
}

#[derive(Debug, Serialize)]
pub struct MultiSelectionOption {
    pub id: String,
    pub name: String,
    pub pill: String,
    pub info: String,
}

// ── Empty-stub constructor (step 1 only) ────────────────────────────

impl LogsResult {
    /// Empty envelope used while SFST query plumbing is being wired up
    /// in steps 2–4. The shape is what the cloud-frontend renders as
    /// "no data": valid types throughout, zero rows, zero items.
    pub fn empty_stub(after: u32, before: u32, last: usize) -> Self {
        Self {
            progress: 100,
            version: Version::default(),
            accepted_params: super::types::ACCEPTED_PARAMS.to_vec(),
            required_params: Vec::new(),
            facets: Vec::new(),
            available_histograms: Vec::new(),
            histogram: Histogram {
                id: String::new(),
                name: String::new(),
                chart: Chart {
                    view: ChartView {
                        title: String::new(),
                        after,
                        before,
                        update_every: 0,
                        units: String::from("events"),
                        chart_type: String::from("stackedBar"),
                        dimensions: ChartDimensions {
                            ids: Vec::new(),
                            names: Vec::new(),
                            units: Vec::new(),
                        },
                    },
                    result: ChartResult {
                        labels: Vec::new(),
                        point: ChartPoint {
                            value: 0,
                            arp: 1,
                            pa: 2,
                        },
                        data: Vec::new(),
                    },
                },
            },
            columns: serde_json::json!({}),
            data: serde_json::json!([]),
            default_charts: Vec::new(),
            items: Items {
                evaluated: 0,
                unsampled: 0,
                estimated: 0,
                matched: 0,
                before: 0,
                after: 0,
                returned: 0,
                max_to_return: last,
            },
            show_ids: false,
            has_history: true,
            status: 200,
            response_type: String::from("table"),
            help: String::from("Query and visualize OpenTelemetry logs."),
            pagination: Pagination::default(),
        }
    }
}

#[cfg(test)]
mod tests;
