//! Wire types for the `otel-logs` function.
//!
//! Request shape mirrors the legacy `JournalRequest` (so the agent's
//! existing wiring works unchanged); response is one of two shapes —
//! `Info` for capability discovery, `Logs` for actual queries —
//! serialized untagged so the JSON payload looks like a hand-rolled
//! response of either shape.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::wire::LogsResponse;

/// Request param names accepted by this function, advertised to the UI
/// in [`InfoResponse::accepted_params`] and echoed in the non-info
/// [`LogsResponse`]'s same field. The UI uses this list to gate which
/// params it's allowed to send. Values mirror the legacy
/// systemd-journal function so the cloud-frontend wiring works without
/// changes.
pub(super) const ACCEPTED_PARAMS: &[&str] = &[
    "info",
    "after",
    "before",
    "anchor",
    "direction",
    "last",
    "query",
    "facets",
    "histogram",
    "if_modified_since",
    "data_only",
    "delta",
    "tail",
    "sampling",
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
pub(crate) struct OtelLogsRequest {
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
    #[serde(default)]
    pub anchor: Option<u64>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Direction {
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
pub(crate) enum OtelLogsResponse {
    Info(InfoResponse),
    Logs(LogsResponse),
}

#[derive(Debug, Serialize)]
pub(crate) struct InfoResponse {
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
