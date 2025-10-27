use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct EmptyRequest {}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JournalRequest {
    #[serde(default)]
    pub info: bool,

    /// Unix timestamp for the start of the time range
    pub after: i64,

    /// Unix timestamp for the end of the time range
    pub before: i64,

    /// Maximum number of results to return
    pub last: Option<u32>,

    /// List of facets to include in the response
    #[serde(default)]
    pub facets: Vec<String>,

    /// Whether to slice the results
    pub slice: Option<bool>,

    /// Query string (empty in your example)
    #[serde(default)]
    pub query: String,

    /// Selection filters
    #[serde(default)]
    pub selections: HashMap<String, Vec<String>>,

    /// Timeout in milliseconds
    pub timeout: Option<u32>,
}

impl Default for JournalRequest {
    fn default() -> Self {
        Self {
            info: true,
            after: 0,
            before: 0,
            last: Some(200),
            facets: Vec::new(),
            slice: None,
            query: String::new(),
            selections: HashMap::new(),
            timeout: None,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestParam {
    Info,
    #[serde(rename = "__logs_sources")]
    LogsSources,
    After,
    Before,
    Anchor,
    Direction,
    Last,
    Query,
    Facets,
    Histogram,
    IfModifiedSince,
    DataOnly,
    Delta,
    Tail,
    Sampling,
    Slice,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MultiSelectionOption {
    pub id: String,
    pub name: String,
    pub pill: String,
    pub info: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MultiSelection {
    pub id: RequestParam,
    pub name: String,
    pub help: String,
    #[serde(rename = "type", default = "MultiSelection::default_type")]
    pub type_: String,
    pub options: Vec<MultiSelectionOption>,
}

impl MultiSelection {
    fn default_type() -> String {
        "multiselect".to_string()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequiredParam {
    MultiSelection(MultiSelection),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Version(u32);

impl Default for Version {
    fn default() -> Self {
        Self(3)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Pagination {
    enabled: bool,
    key: RequestParam,
    column: String,
    units: String,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            enabled: true,
            key: RequestParam::Anchor,
            column: String::from("timestamp"),
            units: String::from("timestamp_usec"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Versions {
    sources: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Columns {}

#[derive(Debug, Serialize, Deserialize)]
pub struct JournalResponse {
    #[serde(rename = "v")]
    pub version: Version,

    pub accepted_params: Vec<RequestParam>,
    pub required_params: Vec<RequiredParam>,

    pub available_histograms:
        Vec<journal::index_state::ui::available_histogram::AvailableHistogram>,
    pub histogram: journal::index_state::ui::histogram::Histogram,
    // FIXME: columns do not contain u32s
    pub columns: Columns,
    pub data: Vec<u32>,
    pub default_charts: Vec<u32>,

    // Hard-coded stuff
    pub show_ids: bool,
    pub has_history: bool,
    pub status: u32,
    #[serde(rename = "type")]
    pub response_type: String,
    pub help: String,
    pub pagination: Pagination,
    // versions: Versions,
}
