//! Handlers for supervisor requests (function calls, shutdown).
//!
//! The "otel-logs" function is the harness entry point for the metadata-tier
//! query path. It accepts a `JournalRequest`-shaped payload (the same wire
//! format the legacy `otel-signal-viewer-plugin` accepts) and returns a
//! `CandidatePlanResponse` listing the SFST / WAL / remote-only files the
//! planner would consult to satisfy the query.
//!
//! Within-file scan, fetch, decode, and merge are out of scope for this
//! milestone — they will be layered on top once the candidate-list
//! verification harness is in place.

use std::collections::HashMap;

use bridge::{LedgerRequest, LedgerResponse};
use serde::{Deserialize, Serialize};

use super::Ledger;
use crate::query::CandidateSource;

impl Ledger {
    /// Handle a supervisor request. Returns `true` if the loop should exit.
    pub(super) async fn handle_supervisor_req(
        &mut self,
        req: LedgerRequest,
    ) -> Result<bool, ferryboat::Error> {
        match req {
            LedgerRequest::Call {
                transaction,
                name,
                args,
                payload,
                ..
            } => {
                tracing::info!("function call: name={name} args={args:?}");
                let result = self.handle_function_call(&name, &args, payload.as_deref());
                let resp = LedgerResponse::Result(netdata_plugin_types::FunctionResult {
                    transaction,
                    ..result
                });
                self.supervisor.send(resp).await?;
                Ok(false)
            }
            LedgerRequest::Cancel { .. } => Ok(false),
            LedgerRequest::Shutdown => {
                tracing::info!("received Shutdown from supervisor");
                Ok(true)
            }
            LedgerRequest::Configure(_) => {
                tracing::warn!("unexpected late Configure message");
                Ok(false)
            }
        }
    }

    fn handle_function_call(
        &self,
        name: &str,
        args: &[String],
        payload: Option<&[u8]>,
    ) -> netdata_plugin_types::FunctionResult {
        match name {
            "otel-logs" => handle_otel_logs(&self.registries, args, payload),
            _ => text_result(404, format!("unknown function: {name}")),
        }
    }
}

fn handle_otel_logs(
    registries: &crate::registry::TenantRegistries,
    args: &[String],
    payload: Option<&[u8]>,
) -> netdata_plugin_types::FunctionResult {
    let synthesized = patch_args_into_payload(args, payload);
    let bytes = synthesized.as_deref().or(payload).unwrap_or(b"{}");

    let req: OtelLogsRequest = match serde_json::from_slice(bytes) {
        Ok(r) => r,
        Err(e) => return text_result(400, format!("invalid request: {e}")),
    };

    if req.info {
        return json_result(200, &InfoResponse::default());
    }

    let q = build_query(&req);
    let candidates = plan_to_candidates(registries, &q);
    json_result(
        200,
        &CandidatePlanResponse {
            version: 1,
            status: 200,
            query: req,
            candidates,
        },
    )
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Request payload — mirrors the legacy `JournalRequest` field set
/// (`journal-function/src/netdata/types.rs`) so the agent's wire format and
/// the rt-level GET shim continue to work unchanged. Only `info`, `after`,
/// and `before` influence the response today; the other fields are echoed
/// back in the response's `query` field but otherwise ignored — they're
/// reserved for the within-file scan layer that comes in a later phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OtelLogsRequest {
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(super) enum Direction {
    Forward,
    #[default]
    Backward,
}

#[derive(Debug, Serialize)]
struct InfoResponse {
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
struct CandidatePlanResponse {
    version: u32,
    status: u32,
    query: OtelLogsRequest,
    candidates: Vec<Candidate>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "source", rename_all = "lowercase")]
enum Candidate {
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

#[derive(Debug, Serialize)]
struct StreamRef {
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
enum WalStatus {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Replicate the rt-level GET shim (`netdata-plugin/rt/src/lib.rs:962-984`):
/// when args carry `after:N` / `before:N` tokens, synthesize a JSON object
/// payload with `info: true` plus the parsed window. Returns `None` when
/// no synthesis happened, in which case the caller falls back to the
/// original payload.
fn patch_args_into_payload(args: &[String], payload: Option<&[u8]>) -> Option<Vec<u8>> {
    if args.is_empty() || payload.is_some() {
        return None;
    }

    let mut map = serde_json::Map::new();
    map.insert("info".into(), serde_json::json!(true));

    for arg in args {
        if let Some(rest) = arg.strip_prefix("after:") {
            if let Ok(v) = rest.parse::<u64>() {
                map.insert("after".into(), serde_json::json!(v));
            }
        } else if let Some(rest) = arg.strip_prefix("before:") {
            if let Ok(v) = rest.parse::<u64>() {
                map.insert("before".into(), serde_json::json!(v));
            }
        }
    }

    serde_json::to_vec(&serde_json::Value::Object(map)).ok()
}

fn build_query(req: &OtelLogsRequest) -> file_registry::Query {
    file_registry::Query {
        time_range: req.after..req.before,
        stream: None,
    }
}

fn plan_to_candidates(
    registries: &crate::registry::TenantRegistries,
    q: &file_registry::Query,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();

    for (tenant_id, registry) in registries.tenants.iter() {
        let tenant = tenant_id.as_str().to_string();
        for cs in registry.plan_candidates(q) {
            out.push(candidate_from_source(&tenant, cs));
        }
    }

    // Sort by seq for determinism. The per-tenant planner already sorts
    // internally; this gives a stable global order across the fan-out.
    out.sort_by_key(seq_of);
    out
}

fn seq_of(c: &Candidate) -> u64 {
    match c {
        Candidate::Sfst { seq, .. } | Candidate::Wal { seq, .. } | Candidate::Remote { seq, .. } => {
            *seq
        }
    }
}

fn candidate_from_source(tenant_id: &str, cs: CandidateSource<'_>) -> Candidate {
    match cs {
        CandidateSource::Sfst(f) => Candidate::Sfst {
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
        CandidateSource::Wal(f) => Candidate::Wal {
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
        CandidateSource::Remote(e) => Candidate::Remote {
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

fn json_result<T: Serialize>(status: u32, value: &T) -> netdata_plugin_types::FunctionResult {
    match serde_json::to_vec(value) {
        Ok(payload) => netdata_plugin_types::FunctionResult {
            transaction: String::new(),
            status,
            format: "application/json".to_string(),
            expires: 0,
            payload,
        },
        Err(e) => text_result(500, format!("serialization error: {e}")),
    }
}

fn text_result(status: u32, body: String) -> netdata_plugin_types::FunctionResult {
    netdata_plugin_types::FunctionResult {
        transaction: String::new(),
        status,
        format: "text/plain".to_string(),
        expires: 0,
        payload: body.into_bytes(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use file_registry::{ByteSize, FileId, StreamEntry, TenantId, TimestampNs};
    use serde_json::Value;
    use uuid::Uuid;
    use wal::FileEvent;

    use crate::registry::{Registry, TenantRegistries};

    fn machine() -> Uuid {
        Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
    }
    fn boot() -> Uuid {
        Uuid::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111)
    }
    fn fid(seq: u64, ns_hash: u64) -> FileId {
        FileId::new(machine(), boot(), seq, ns_hash)
    }

    fn make_registry() -> Registry {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let catalog_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::Registry::new(sfst_dir.path());
        let catalog_files =
            otel_catalog::Registry::new(catalog_dir.path(), TenantId::from("tenant1"));
        std::mem::forget((wal_dir, sfst_dir, catalog_dir));
        Registry::new(wal, sfst, catalog_files)
    }

    fn track_wal(reg: &mut Registry, seq: u64, ns_hash: u64, min_s: u32, max_s: u32) {
        const NS: u64 = 1_000_000_000;
        let id = fid(seq, ns_hash);
        reg.wal
            .apply_event(&FileEvent::Created {
                file_id: id,
                created_at_ns: TimestampNs(0),
            })
            .unwrap();
        reg.wal
            .apply_event(&FileEvent::Closed {
                file_id: id,
                frame_count: 0,
                min_timestamp_ns: TimestampNs(min_s as u64 * NS),
                max_timestamp_ns: TimestampNs(max_s as u64 * NS),
                size: ByteSize(0),
            })
            .unwrap();
    }

    fn track_sfst(reg: &mut Registry, seq: u64, ns_hash: u64, min_s: u32, max_s: u32) {
        let id = fid(seq, ns_hash);
        reg.sfst.track(
            id,
            ByteSize(1),
            sfst::FileSummary {
                min_timestamp_s: min_s,
                max_timestamp_s: max_s,
                total_logs: 1,
                stream: StreamEntry::new("ns", "a"),
            },
        );
    }

    fn track_remote(reg: &mut Registry, seq: u64, min_s: u32, max_s: u32) {
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let entry = otel_catalog::CatalogEntry {
            id: fid(seq, 0),
            remote_key: format!("k{seq}"),
            min_timestamp_s: min_s,
            max_timestamp_s: max_s,
            total_logs: 1,
            stream: StreamEntry::new("ns", "a"),
            size: ByteSize(1),
            uploaded_at_ns: TimestampNs(0),
        };

        let mut catalog = otel_catalog::Catalog::new(
            TenantId::from("tenant1"),
            date,
            machine(),
            boot(),
            TimestampNs(0),
        );
        catalog.add(entry, TimestampNs(0));

        let path = reg.catalog_files.file_path(date, machine(), boot(), seq);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, catalog.to_json().unwrap()).unwrap();
        let size = ByteSize(std::fs::metadata(&path).unwrap().len());
        reg.catalog_files.track(
            otel_catalog::File::new(date, machine(), boot(), seq, TimestampNs(0), size),
            path,
        );
    }

    fn make_tenant_registries() -> TenantRegistries {
        TenantRegistries::new(
            tempfile::tempdir().unwrap().keep(),
            tempfile::tempdir().unwrap().keep(),
            tempfile::tempdir().unwrap().keep(),
        )
    }

    fn install_tenant(tr: &mut TenantRegistries, tenant: &str, registry: Registry) {
        tr.tenants.insert(TenantId::from(tenant), registry);
    }

    #[test]
    fn info_request_returns_capability_descriptor() {
        let result = handle_otel_logs(&make_tenant_registries(), &[], Some(br#"{"info": true}"#));
        assert_eq!(result.status, 200);
        assert_eq!(result.format, "application/json");
        let v: Value = serde_json::from_slice(&result.payload).unwrap();
        assert_eq!(v["status"], 200);
        assert!(v["accepted_params"].as_array().unwrap().contains(&Value::String("after".into())));
        assert!(v.get("candidates").is_none());
    }

    #[test]
    fn empty_payload_defaults_to_info_true() {
        let result = handle_otel_logs(&make_tenant_registries(), &[], None);
        assert_eq!(result.status, 200);
        let v: Value = serde_json::from_slice(&result.payload).unwrap();
        assert!(v.get("candidates").is_none());
        assert!(v.get("accepted_params").is_some());
    }

    #[test]
    fn patches_after_before_args_into_payload() {
        let args = vec!["after:100".to_string(), "before:200".to_string()];
        let bytes = patch_args_into_payload(&args, None).unwrap();
        let req: OtelLogsRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(req.after, 100);
        assert_eq!(req.before, 200);
        assert!(req.info);
    }

    #[test]
    fn returns_sfst_wal_remote_candidates_for_full_window() {
        let mut tr = make_tenant_registries();

        let mut a = make_registry();
        track_sfst(&mut a, 1, 7, 100, 200);
        track_wal(&mut a, 2, 7, 300, 400);
        install_tenant(&mut tr, "tenant-a", a);

        let mut b = make_registry();
        track_remote(&mut b, 3, 500, 600);
        install_tenant(&mut tr, "tenant-b", b);

        let body = br#"{"info": false, "after": 0, "before": 4294967295}"#;
        let result = handle_otel_logs(&tr, &[], Some(body));
        assert_eq!(result.status, 200);

        let v: Value = serde_json::from_slice(&result.payload).unwrap();
        let candidates = v["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 3);

        // Sorted by seq.
        assert_eq!(candidates[0]["source"], "sfst");
        assert_eq!(candidates[0]["seq"], 1);
        assert_eq!(candidates[0]["tenant_id"], "tenant-a");

        assert_eq!(candidates[1]["source"], "wal");
        assert_eq!(candidates[1]["seq"], 2);
        assert_eq!(candidates[1]["tenant_id"], "tenant-a");

        assert_eq!(candidates[2]["source"], "remote");
        assert_eq!(candidates[2]["seq"], 3);
        assert_eq!(candidates[2]["tenant_id"], "tenant-b");
    }

    #[test]
    fn window_excludes_files_outside_range() {
        let mut tr = make_tenant_registries();
        let mut r = make_registry();
        track_sfst(&mut r, 1, 7, 100, 200);
        track_sfst(&mut r, 2, 7, 1000, 2000);
        install_tenant(&mut tr, "t", r);

        let body = br#"{"info": false, "after": 0, "before": 500}"#;
        let result = handle_otel_logs(&tr, &[], Some(body));
        let v: Value = serde_json::from_slice(&result.payload).unwrap();
        let candidates = v["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["seq"], 1);
    }

    #[test]
    fn bad_json_returns_400_text_plain() {
        let result = handle_otel_logs(&make_tenant_registries(), &[], Some(b"{not json"));
        assert_eq!(result.status, 400);
        assert_eq!(result.format, "text/plain");
    }

}
