//! `OtelLogsHandler` — typed `FunctionHandler` implementation.
//!
//! Holds a shared, read-only handle to the tenant registries; the
//! run-loop's mutators acquire write locks for brief periods, this
//! handler acquires read locks for the candidate walk.
//!
//! Within-file scan, fetch, decode, and merge are out of scope here —
//! they will be layered onto `on_call` in a later phase.

use std::sync::Arc;

use async_trait::async_trait;
use bridge::function::{FunctionCallContext, FunctionHandler};
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_types::HttpAccess;
use tokio::sync::RwLock;

use super::types::{
    Candidate, CandidatePlanResponse, InfoResponse, OtelLogsRequest, OtelLogsResponse,
};
use crate::registry::TenantRegistries;

pub(crate) struct OtelLogsHandler {
    registries: Arc<RwLock<TenantRegistries>>,
}

impl OtelLogsHandler {
    pub(crate) fn new(registries: Arc<RwLock<TenantRegistries>>) -> Self {
        Self { registries }
    }

    /// Canonical function declaration. Used both by `FunctionHandler::declaration`
    /// and by the worker entry point in `lib.rs` to advertise the function
    /// to the supervisor before the full ledger is initialized.
    pub(crate) fn function_declaration() -> FunctionDeclaration {
        let mut d = FunctionDeclaration::new("otel-logs", "Query OpenTelemetry logs");
        d.global = true;
        d.tags = Some("logs".to_string());
        d.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        d
    }
}

#[async_trait]
impl FunctionHandler for OtelLogsHandler {
    type Request = OtelLogsRequest;
    type Response = OtelLogsResponse;

    async fn on_call(
        &self,
        _ctx: FunctionCallContext,
        req: Self::Request,
    ) -> netdata_plugin_error::Result<Self::Response> {
        if req.info {
            return Ok(OtelLogsResponse::Info(InfoResponse::default()));
        }

        let q = req.to_query();
        let candidates = {
            let guard = self.registries.read().await;
            plan_to_candidates(&guard, &q)
        };

        Ok(OtelLogsResponse::CandidatePlan(CandidatePlanResponse {
            version: 1,
            status: 200,
            query: req,
            candidates,
        }))
    }

    fn declaration(&self) -> FunctionDeclaration {
        Self::function_declaration()
    }
}

/// Replicate the rt-level GET shim (`netdata-plugin/rt/src/lib.rs:962-984`):
/// when args carry `after:N` / `before:N` tokens, synthesize a JSON object
/// payload with `info: true` plus the parsed window. Returns `None` when
/// no synthesis happened, in which case the caller falls back to the
/// original payload.
pub(super) fn patch_args_into_payload(args: &[String], payload: Option<&[u8]>) -> Option<Vec<u8>> {
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

fn plan_to_candidates(registries: &TenantRegistries, q: &file_registry::Query) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();

    for (tenant_id, registry) in registries.tenants.iter() {
        let tenant = tenant_id.as_str().to_string();
        for cs in registry.plan_candidates(q) {
            out.push(Candidate::from_source(&tenant, cs));
        }
    }

    // Sort by seq for determinism. The per-tenant planner already sorts
    // internally; this gives a stable global order across the fan-out.
    out.sort_by_key(Candidate::seq);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use file_registry::{ByteSize, FileId, StreamEntry, TenantId, TimestampNs};
    use serde_json::Value;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;
    use wal::FileEvent;

    use crate::registry::Registry;

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
            sfst::Summary {
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

        let mut catalog =
            otel_catalog::Catalog::new(TenantId::from("tenant1"), date, machine(), boot());
        catalog.add(entry);

        let path = reg
            .catalog_files
            .file_path(date, machine(), boot(), seq, min_s, max_s);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, catalog.to_json().unwrap()).unwrap();
        let size = ByteSize(std::fs::metadata(&path).unwrap().len());
        reg.catalog_files.track(
            otel_catalog::File::new(date, machine(), boot(), seq, min_s, max_s, size),
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

    fn make_handler(tr: TenantRegistries) -> OtelLogsHandler {
        OtelLogsHandler::new(Arc::new(RwLock::new(tr)))
    }

    fn make_ctx(transaction: &str) -> FunctionCallContext {
        FunctionCallContext::new(
            transaction.to_string(),
            bridge::function::ProgressState::new(),
            CancellationToken::new(),
        )
    }

    #[tokio::test]
    async fn info_request_returns_capability_descriptor() {
        let h = make_handler(make_tenant_registries());
        let req: OtelLogsRequest = serde_json::from_slice(br#"{"info": true}"#).unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], 200);
        assert!(
            v["accepted_params"]
                .as_array()
                .unwrap()
                .contains(&Value::String("after".into()))
        );
        assert!(v.get("candidates").is_none());
    }

    #[tokio::test]
    async fn empty_payload_defaults_to_info_true() {
        // `serde_json::from_slice(b"{}")` is what the engine does for
        // a None payload — verify the `info` default is `true`.
        let req: OtelLogsRequest = serde_json::from_slice(b"{}").unwrap();
        assert!(req.info);
        let h = make_handler(make_tenant_registries());
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
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

    #[tokio::test]
    async fn returns_sfst_wal_remote_candidates_for_full_window() {
        let mut tr = make_tenant_registries();

        let mut a = make_registry();
        track_sfst(&mut a, 1, 7, 100, 200);
        track_wal(&mut a, 2, 7, 300, 400);
        install_tenant(&mut tr, "tenant-a", a);

        let mut b = make_registry();
        track_remote(&mut b, 3, 500, 600);
        install_tenant(&mut tr, "tenant-b", b);

        let h = make_handler(tr);
        let req: OtelLogsRequest =
            serde_json::from_slice(br#"{"info": false, "after": 0, "before": 4294967295}"#)
                .unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
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

    #[tokio::test]
    async fn window_excludes_files_outside_range() {
        let mut tr = make_tenant_registries();
        let mut r = make_registry();
        track_sfst(&mut r, 1, 7, 100, 200);
        track_sfst(&mut r, 2, 7, 1000, 2000);
        install_tenant(&mut tr, "t", r);

        let h = make_handler(tr);
        let req: OtelLogsRequest =
            serde_json::from_slice(br#"{"info": false, "after": 0, "before": 500}"#).unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        let candidates = v["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["seq"], 1);
    }

    #[test]
    fn declaration_carries_legacy_flags() {
        let h = make_handler(make_tenant_registries());
        let d = h.declaration();
        assert_eq!(d.name, "otel-logs");
        assert!(d.global);
        assert_eq!(d.tags.as_deref(), Some("logs"));
        let access = d.access.unwrap();
        assert!(access.contains(HttpAccess::SIGNED_ID));
        assert!(access.contains(HttpAccess::SAME_SPACE));
        assert!(access.contains(HttpAccess::SENSITIVE_DATA));
    }
}
