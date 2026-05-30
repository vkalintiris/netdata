//! Request and response wire types for the log query.
//!
//! The request shape follows the wire contract the consumer speaks, so
//! its existing clients work unchanged. The response is one of two
//! shapes — [`InfoResponse`] for capability discovery, [`LogsResult`]
//! for actual queries — serialized untagged so the JSON payload is just
//! one shape or the other, with no enclosing tag.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::wire::LogsResult;

/// Request param names accepted by this function, advertised to the UI
/// in [`InfoResponse::accepted_params`] and echoed in the non-info
/// [`LogsResult`]'s same field. The UI gates which params it sends on
/// this list.
///
/// We advertise only what we actually honor. Notably `data_only` is
/// **omitted**: the UI computes its `dataOnly` flag as
/// `data_only && accepted_params.includes("data_only")`, so leaving it
/// out forces `dataOnly=false`. That makes the UI refresh columns /
/// pagination / facets from each full response (which we recompute
/// every call) instead of preserving stale prior state; infinite scroll
/// still works off `merge` + the row anchors. `if_modified_since`,
/// `delta`, `tail`, and `sampling` are likewise omitted — they drive
/// incremental / live-tail / sampling modes we don't implement.
pub const ACCEPTED_PARAMS: &[&str] = &[
    "info",
    "after",
    "before",
    "anchor",
    "direction",
    "last",
    "query",
    "facets",
    "histogram",
    "slice",
];

/// Request payload. The field set follows the wire contract the
/// consumer speaks, so its existing clients work unchanged.
///
/// Only `info` selects between the two response modes; every other
/// field is optional and falls back to its `#[serde(default)]` value
/// when the consumer omits it. Together those defaults reproduce the
/// default query — the newest page over the default time window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsRequest {
    /// `info: true` requests a capability descriptor; `info: false`
    /// (the default) requests a data query. Consumers omit this field on
    /// every data request, so the default must be `false` for those
    /// requests to reach the query path. Capability discovery arrives
    /// either as an explicit `{"info": true}` body or as a request whose
    /// transport carries a literal `info` token.
    #[serde(default)]
    pub info: bool,
    #[serde(default)]
    pub after: u32,
    #[serde(default)]
    pub before: u32,
    /// Pagination anchor, in one of two forms (see `AnchorParam`):
    /// the opaque row cursor string echoed from a boundary row's hidden
    /// cursor column, or a bare microsecond timestamp the UI sends when
    /// the user clicks a histogram bar ("jump to this time").
    #[serde(default)]
    pub anchor: Option<AnchorParam>,
    /// Maximum number of log entries to return.
    #[serde(default = "default_last")]
    pub last: usize,
    #[serde(default)]
    pub facets: Vec<String>,
    #[serde(default)]
    pub histogram: String,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default)]
    pub slice: Option<bool>,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub selections: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub timeout: Option<u32>,
}

fn default_last() -> usize {
    200
}

/// The two anchor forms the UI sends. A JSON string is our opaque row
/// [`Cursor`](super::cursor::Cursor); a JSON number is a microsecond
/// timestamp from a histogram-bar click. Untagged so the JSON type
/// alone selects the variant — our cursor strings always contain `:`,
/// so they never collide with a bare integer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnchorParam {
    Cursor(String),
    TimestampUs(u64),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Forward,
    #[default]
    Backward,
}

/// Two response shapes — [`Info`](Self::Info) for capability discovery,
/// [`Logs`](Self::Logs) for actual queries. Untagged: the JSON payload
/// is just one shape or the other, so the consumer doesn't have to
/// learn a new envelope.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum LogsResponse {
    Info(InfoResponse),
    Logs(LogsResult),
}

#[derive(Debug, Serialize)]
pub struct InfoResponse {
    version: u32,
    status: u32,
    accepted_params: Vec<&'static str>,
    required_params: Vec<&'static str>,
    help: &'static str,
}

impl Default for InfoResponse {
    fn default() -> Self {
        Self {
            version: 1,
            status: 200,
            accepted_params: ACCEPTED_PARAMS.to_vec(),
            required_params: vec![],
            help: "Query and visualize OpenTelemetry logs.",
        }
    }
}
