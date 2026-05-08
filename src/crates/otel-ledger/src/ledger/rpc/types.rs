//! Wire types for the `otel-logs` function.
//!
//! Request shape mirrors the legacy `JournalRequest` (so the agent's
//! existing wiring works unchanged); response is one of two shapes —
//! `Info` for capability discovery, `CandidatePlan` for actual
//! queries — serialized untagged so the JSON payload looks like a
//! hand-rolled response of either shape.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::query::CandidateSource;

/// Request payload — mirrors the legacy `JournalRequest` field set
/// (`journal-function/src/netdata/types.rs`) so the agent's wire format
/// continues to work unchanged. Only `info`, `after`, and `before`
/// influence the response today; the other fields are echoed back in
/// the response's `query` field but otherwise ignored — they're
/// reserved for the within-file scan layer that comes in a later phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OtelLogsRequest {
    #[serde(default = "default_true")]
    pub info: bool,
    #[serde(default)]
    pub after: u32,
    #[serde(default)]
    pub before: u32,
    #[serde(default)]
    pub anchor: Option<u64>,
    #[serde(default)]
    pub last: Option<usize>,
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

fn default_true() -> bool {
    true
}

impl OtelLogsRequest {
    pub(super) fn to_query(&self) -> file_registry::Query {
        file_registry::Query {
            time_range: self.after..self.before,
            stream: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Direction {
    Forward,
    #[default]
    Backward,
}

/// Two response shapes — `Info` for capability discovery,
/// `CandidatePlan` for actual queries. Untagged: the JSON payload is
/// indistinguishable on the wire from a hand-rolled response of either
/// shape, so the agent / UI doesn't have to learn a new envelope.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum OtelLogsResponse {
    Info(InfoResponse),
    CandidatePlan(CandidatePlanResponse),
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
            accepted_params: vec!["info", "after", "before"],
            required_params: vec![],
            help: "Query OpenTelemetry logs (candidate-file plan)",
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct CandidatePlanResponse {
    pub(super) version: u32,
    pub(super) status: u32,
    pub(super) query: OtelLogsRequest,
    pub(super) candidates: Vec<Candidate>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub(super) enum Candidate {
    Sfst {
        tenant_id: String,
        seq: u64,
        machine_id: String,
        boot_id: String,
        ns_hash: u64,
        min_timestamp_s: u32,
        max_timestamp_s: u32,
        total_logs: u32,
        stream: StreamRef,
        size_bytes: u64,
    },
    Wal {
        tenant_id: String,
        seq: u64,
        machine_id: String,
        boot_id: String,
        ns_hash: u64,
        status: WalStatus,
        created_at_ns: u64,
        min_timestamp_ns: u64,
        max_timestamp_ns: u64,
        size_bytes: u64,
    },
    Remote {
        tenant_id: String,
        seq: u64,
        machine_id: String,
        boot_id: String,
        ns_hash: u64,
        remote_key: String,
        min_timestamp_s: u32,
        max_timestamp_s: u32,
        total_logs: u32,
        stream: StreamRef,
        size_bytes: u64,
        uploaded_at_ns: u64,
    },
}

impl Candidate {
    pub(super) fn seq(&self) -> u64 {
        match self {
            Candidate::Sfst { seq, .. }
            | Candidate::Wal { seq, .. }
            | Candidate::Remote { seq, .. } => *seq,
        }
    }

    pub(super) fn from_source(tenant_id: &str, cs: CandidateSource<'_>) -> Self {
        match cs {
            CandidateSource::Sfst(f) => Self::Sfst {
                tenant_id: tenant_id.to_string(),
                seq: f.id.seq,
                machine_id: f.id.machine_id.as_simple().to_string(),
                boot_id: f.id.boot_id.as_simple().to_string(),
                ns_hash: f.id.ns_hash,
                min_timestamp_s: f.summary.min_timestamp_s,
                max_timestamp_s: f.summary.max_timestamp_s,
                total_logs: f.summary.total_logs,
                stream: (&f.summary.stream).into(),
                size_bytes: f.size.0,
            },
            CandidateSource::Wal(f) => Self::Wal {
                tenant_id: tenant_id.to_string(),
                seq: f.id.seq,
                machine_id: f.id.machine_id.as_simple().to_string(),
                boot_id: f.id.boot_id.as_simple().to_string(),
                ns_hash: f.id.ns_hash,
                status: f.status.into(),
                created_at_ns: f.created_at_ns.0,
                min_timestamp_ns: f.min_timestamp_ns.0,
                max_timestamp_ns: f.max_timestamp_ns.0,
                size_bytes: f.size.0,
            },
            CandidateSource::Remote(e) => Self::Remote {
                tenant_id: tenant_id.to_string(),
                seq: e.id.seq,
                machine_id: e.id.machine_id.as_simple().to_string(),
                boot_id: e.id.boot_id.as_simple().to_string(),
                ns_hash: e.id.ns_hash,
                remote_key: e.remote_key,
                min_timestamp_s: e.min_timestamp_s,
                max_timestamp_s: e.max_timestamp_s,
                total_logs: e.total_logs,
                stream: (&e.stream).into(),
                size_bytes: e.size.0,
                uploaded_at_ns: e.uploaded_at_ns.0,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct StreamRef {
    namespace: String,
    name: String,
}

impl From<&file_registry::StreamEntry> for StreamRef {
    fn from(s: &file_registry::StreamEntry) -> Self {
        Self {
            namespace: s.namespace.clone(),
            name: s.name.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum WalStatus {
    Active,
    Archived,
}

impl From<wal::registry::FileStatus> for WalStatus {
    fn from(s: wal::registry::FileStatus) -> Self {
        match s {
            wal::registry::FileStatus::Active => Self::Active,
            wal::registry::FileStatus::Archived => Self::Archived,
        }
    }
}
