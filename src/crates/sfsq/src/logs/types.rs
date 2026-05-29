//! Wire types for the `otel-logs` function.
//!
//! Request shape mirrors the legacy `JournalRequest` (so the agent's
//! existing wiring works unchanged); response is one of two shapes —
//! `Info` for capability discovery, `Logs` for actual queries —
//! serialized untagged so the JSON payload looks like a hand-rolled
//! response of either shape.

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

/// Request payload — mirrors the legacy `JournalRequest` field set
/// (`journal-function/src/netdata/types.rs`) so the agent's wire format
/// continues to work unchanged.
///
/// Only `info`, `after`, `before`, and `last` are consumed by the
/// current handler; the remaining fields are accepted (so deserializing
/// doesn't fail) and will be consumed once the SFST query plumbing
/// lands in steps 2–4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsRequest {
    /// `info: true` requests a capability descriptor; `info: false`
    /// (the default — matches the legacy `JournalRequest` semantic)
    /// requests a data query. The UI's POST bodies omit this field on
    /// every data request, so the default must be `false` for them to
    /// reach the query path. Info discovery is sent either as an
    /// explicit POST `{"info": true}` or as a GET with the literal
    /// `info` token in the URL args (translated by the rt-level shim).
    #[serde(default)]
    pub info: bool,
    #[serde(default)]
    pub after: u32,
    #[serde(default)]
    pub before: u32,
    /// Pagination anchor, in one of two forms (see [`AnchorParam`]):
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

/// Two response shapes — `Info` for capability discovery, `Logs` for
/// actual queries. Untagged: the JSON payload is indistinguishable on
/// the wire from a hand-rolled response of either shape, so the agent /
/// UI doesn't have to learn a new envelope.
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
