//! Reading evicted files back from remote storage, end to end through the
//! Function: the answers equal the same files served locally, and every way
//! remote data can be missing is reported, never silent.

use super::*;
use crate::ledger::rpc::traces::fixtures::{
    Evicted, TestRemote, install_sealed, make_registries, otlp_req_svc,
};
use bridge::function::ProgressState;
use file_registry::test_identity;
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Corpus base, unix seconds.
const T_S: u32 = 1_700_000_000;
const MIB: u64 = 1024 * 1024;

fn base_ns(offset_s: u64) -> u64 {
    (u64::from(T_S) + offset_s) * 1_000_000_000
}

/// File 1: traces A (1 span) and B (2 spans) in the window's first half.
fn file_one() -> Vec<opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest> {
    vec![
        otlp_req_svc(0x0A, 1, base_ns(10), "svc-a"),
        otlp_req_svc(0x0B, 2, base_ns(20), "svc-b"),
    ]
}

/// File 2: traces C (3 spans) and E (1 span) in its second half.
fn file_two() -> Vec<opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest> {
    vec![
        otlp_req_svc(0x0C, 3, base_ns(30), "svc-a"),
        otlp_req_svc(0x0E, 1, base_ns(40), "svc-a"),
    ]
}

fn trace_id(byte: u8) -> String {
    format!("{byte:02x}").repeat(16)
}

fn handler(registries: Arc<RwLock<TenantRegistries>>, remote: &TestRemote) -> OtelTracesHandler {
    OtelTracesHandler::new(
        registries,
        Arc::new(ChunkCache::new(64 * 1024 * 1024)),
        4,
        Some(remote.read()),
    )
}

async fn call(
    h: &OtelTracesHandler,
    body: serde_json::Value,
) -> netdata_plugin_error::Result<serde_json::Value> {
    call_with(h, body, &ProgressState::new()).await
}

async fn call_with(
    h: &OtelTracesHandler,
    body: serde_json::Value,
    progress: &ProgressState,
) -> netdata_plugin_error::Result<serde_json::Value> {
    let ctx = FunctionCallContext::new(
        "tx-remote".to_string(),
        progress.clone(),
        CancellationToken::new(),
    );
    let req: OtelTracesRequest = serde_json::from_value(body).unwrap();
    h.on_call(ctx, req)
        .await
        .map(|resp| serde_json::to_value(resp).unwrap())
}

/// The explorer over the corpus window, every section it answers by
/// default (histogram, facets, groups, rows).
fn explore_body() -> serde_json::Value {
    json!({"explore": {"after": T_S, "before": T_S + 100}})
}

fn trace_body(byte: u8) -> serde_json::Value {
    json!({"trace": {"id": trace_id(byte)}})
}

/// Both files local; remote storage on but nothing cataloged.
async fn local_setup() -> (OtelTracesHandler, TestRemote) {
    let registries = make_registries();
    install_sealed(&registries, "default", 1, file_one()).await;
    install_sealed(&registries, "default", 2, file_two()).await;
    let remote = TestRemote::new(64 * MIB);
    (handler(registries, &remote), remote)
}

/// Both files evicted: remote objects and catalog entries only.
async fn evicted_setup(capacity: u64) -> (OtelTracesHandler, TestRemote, [Evicted; 2]) {
    let registries = make_registries();
    let remote = TestRemote::new(capacity);
    let one = remote
        .evicted(&registries, "default", test_identity(), 1, file_one())
        .await;
    let two = remote
        .evicted(&registries, "default", test_identity(), 2, file_two())
        .await;
    (handler(registries, &remote), remote, [one, two])
}

/// The distinct traces the explorer's rows name, newest first.
fn traces_of(v: &serde_json::Value) -> Vec<String> {
    let mut traces: Vec<String> = Vec::new();
    for row in v["data"]["rows"]["items"].as_array().unwrap() {
        let id = row["trace_id"].as_str().unwrap()[..2].to_string();
        if !traces.contains(&id) {
            traces.push(id);
        }
    }
    traces
}

#[tokio::test]
async fn evicted_files_answer_exactly_as_local_files() {
    let (local, _) = local_setup().await;
    let (evicted, remote, _) = evicted_setup(64 * MIB).await;

    for body in [trace_body(0x0B), trace_body(0x0C), explore_body()] {
        let from_local = call(&local, body.clone()).await.unwrap();
        let from_remote = call(&evicted, body.clone()).await.unwrap();
        assert_eq!(from_remote, from_local, "{body}");
    }
    // The local side is Complete with remote storage on: nothing remote
    // was needed, nothing reported missing.
    let local_explore = call(&local, explore_body()).await.unwrap();
    assert_eq!(local_explore["data"]["status"], json!({"complete": true}));
    assert_eq!(traces_of(&local_explore), ["0e", "0c", "0b", "0a"]);
    // The answers came from downloaded files.
    assert_eq!(remote.cache.file_count(), 2);
}

#[tokio::test]
async fn a_failed_download_is_reported_on_every_answer() {
    let (h, remote, [_, two]) = evicted_setup(64 * MIB).await;
    remote.lose(&two);

    // B lives in the file that downloads: served, but the lookup cannot
    // know whether the lost file held more of it.
    let v = call(&h, trace_body(0x0B)).await.unwrap();
    assert_eq!(
        v["status"],
        json!({"partial": [{"reason": "remote_unavailable", "count": 1}]})
    );
    assert_eq!(v["items"]["returned"], 2);

    // The explorer: every section says so, and still answers from the file
    // that downloaded.
    let v = call(&h, explore_body()).await.unwrap();
    let partial = json!({"partial": [{"reason": "remote_unavailable", "count": 1, "of": 2}]});
    for section in ["histogram", "facets", "groups", "rows"] {
        assert_eq!(v["data"][section]["status"], partial, "{section}");
    }
    assert_eq!(traces_of(&v), ["0b", "0a"]);
}

#[tokio::test]
async fn a_storage_error_costs_only_its_own_file() {
    // File 1 downloads first and fails with a storage error, not as a missing
    // object; file 2 is still downloaded and served.
    let registries = make_registries();
    let remote = TestRemote::new(64 * MIB);
    remote
        .refused(&registries, "default", test_identity(), 1, file_one())
        .await;
    remote
        .evicted(&registries, "default", test_identity(), 2, file_two())
        .await;
    let h = handler(registries, &remote);

    let v = call(&h, explore_body()).await.unwrap();
    assert_eq!(
        v["data"]["status"],
        json!({"partial": [{"reason": "remote_unavailable", "count": 1, "of": 2}]})
    );
    assert_eq!(traces_of(&v), ["0e", "0c"]);
    assert_eq!(remote.cache.file_count(), 1, "file 2 was downloaded");
}

#[tokio::test]
async fn an_unreadable_catalog_is_reported() {
    let (h, _remote, [one, _]) = evicted_setup(64 * MIB).await;
    std::fs::write(&one.catalog, b"not a catalog").unwrap();
    let partial = json!({"partial": [{"reason": "remote_unavailable", "count": 1}]});

    let v = call(&h, trace_body(0x0A)).await.unwrap();
    assert_eq!(v["status"], partial);
    assert_eq!(v["items"]["returned"], 0);

    let v = call(&h, explore_body()).await.unwrap();
    assert_eq!(
        v["data"]["status"],
        json!({"partial": [{"reason": "remote_unavailable", "count": 1, "of": 2}]})
    );
    assert_eq!(traces_of(&v), ["0e", "0c"]);
}

#[tokio::test]
async fn a_query_too_large_for_the_download_cache_is_a_hard_error() {
    let (h, remote, _) = evicted_setup(100).await;

    for body in [trace_body(0x0B), explore_body()] {
        let err = call(&h, body.clone()).await.unwrap_err().to_string();
        assert!(
            err.contains("more than 100 B of remote trace data")
                && err.contains(
                    "narrow the time range or raise `remote_storage.read_cache_max_size`"
                ),
            "{body}: {err}"
        );
    }
    assert_eq!(remote.cache.file_count(), 0, "nothing was downloaded");
}

#[tokio::test]
async fn an_unwritable_cache_directory_degrades_each_download() {
    let (h, remote, _) = evicted_setup(64 * MIB).await;
    // The cache path stops being a directory: every write into it fails,
    // for root too (unlike a permission change).
    std::fs::remove_dir(&remote.cache_dir).unwrap();
    std::fs::write(&remote.cache_dir, b"not a directory").unwrap();

    let v = call(&h, trace_body(0x0B)).await.unwrap();

    // Both files fail to download, and the count says so.
    assert_eq!(
        v["status"],
        json!({"partial": [{"reason": "remote_unavailable", "count": 2}]})
    );
    assert_eq!(v["items"]["returned"], 0);
}

#[tokio::test]
async fn a_cache_that_cannot_evict_is_a_hard_error() {
    let (_, mut remote, [one, two]) = evicted_setup(64 * MIB).await;
    // Room for either file, not both: the second lookup must evict the first.
    let (a, b) = (one.entry.size.as_u64(), two.entry.size.as_u64());
    let capacity = a.max(b);
    assert!(a + b > capacity);
    remote.cache = file_cache::FileCache::open(&remote.cache_dir, capacity).unwrap();
    let registries = make_registries();
    {
        let mut guard = registries.write().await;
        crate::test_helpers::track_catalog_entry(&mut guard, "default", one.entry.clone());
        crate::test_helpers::track_catalog_entry(&mut guard, "default", two.entry.clone());
    }
    let h = handler(registries, &remote);
    // Bounded lookups: each plans only its own file.
    let first = json!({"trace": {"id": trace_id(0x0A), "after": T_S + 5, "before": T_S + 25}});
    let second = json!({"trace": {"id": trace_id(0x0C), "after": T_S + 28, "before": T_S + 45}});
    assert_eq!(
        call(&h, first).await.unwrap()["status"],
        json!({"complete": true})
    );

    // The cached first file becomes a directory the cache cannot unlink,
    // for root too (unlike a permission change).
    let cached = remote.cache_dir.join(one.entry.id.to_filename("sfst"));
    std::fs::remove_file(&cached).unwrap();
    std::fs::create_dir(&cached).unwrap();
    let err = call(&h, second).await.unwrap_err().to_string();
    assert!(
        err.contains("download cache directory is unwritable"),
        "{err}"
    );
}

#[tokio::test]
async fn a_local_copy_is_served_and_not_downloaded() {
    // Uploaded, cataloged and still local: the catalog entry is masked, so
    // the lost object is never needed.
    let registries = make_registries();
    let remote = TestRemote::new(64 * MIB);
    let one = remote
        .evicted(&registries, "default", test_identity(), 1, file_one())
        .await;
    remote.lose(&one);
    install_sealed(&registries, "default", 1, file_one()).await;
    let h = handler(registries, &remote);

    let v = call(&h, trace_body(0x0B)).await.unwrap();
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 2);
    assert_eq!(remote.cache.file_count(), 0);
}

#[tokio::test]
async fn another_identitys_file_at_the_same_seq_is_downloaded() {
    // A prior process instance used the same seq: its remote file is a
    // different file, and the local one does not mask it.
    let registries = make_registries();
    let remote = TestRemote::new(64 * MIB);
    install_sealed(&registries, "default", 1, file_one()).await;
    let foreign = file_registry::Identity::new(
        file_registry::MachineId::new(uuid::Uuid::from_u128(0x77)).unwrap(),
        file_registry::InstanceId::new(uuid::Uuid::from_u128(0x78)).unwrap(),
    );
    remote
        .evicted(&registries, "default", foreign, 1, file_two())
        .await;
    let h = handler(registries, &remote);

    let v = call(&h, explore_body()).await.unwrap();
    assert_eq!(v["data"]["status"], json!({"complete": true}));
    assert_eq!(traces_of(&v), ["0e", "0c", "0b", "0a"]);
    assert_eq!(remote.cache.file_count(), 1);
}

/// File 1 local, file 2 evicted.
async fn half_evicted_setup() -> (OtelTracesHandler, TestRemote) {
    let registries = make_registries();
    let remote = TestRemote::new(64 * MIB);
    install_sealed(&registries, "default", 1, file_one()).await;
    remote
        .evicted(&registries, "default", test_identity(), 2, file_two())
        .await;
    (handler(registries, &remote), remote)
}

#[tokio::test]
async fn progress_counts_the_downloads_beside_the_sources() {
    // Every walked source ticks, and so does each planned download: two
    // sources plus one download; the explorer's Groups section walks the
    // two sources a second time.
    for (body, expected) in [(trace_body(0x0C), (3, 3)), (explore_body(), (5, 5))] {
        let (h, _remote) = half_evicted_setup().await;
        let progress = ProgressState::new();
        call_with(&h, body.clone(), &progress).await.unwrap();
        assert_eq!(progress.load(), expected, "{body}");
    }

    // A repeat finds the file cached: nothing downloads, so the total is an
    // upper bound the answer arrives under.
    let (h, _remote) = half_evicted_setup().await;
    call(&h, trace_body(0x0C)).await.unwrap();
    let progress = ProgressState::new();
    call_with(&h, trace_body(0x0C), &progress).await.unwrap();
    assert_eq!(progress.load(), (2, 3));
}

/// The value suggestions over the corpus window.
fn values_body() -> serde_json::Value {
    json!({"values": {
        "after": T_S, "before": T_S + 100, "field": "resource.attributes.service.name"
    }})
}

#[tokio::test]
async fn evicted_files_answer_value_suggestions_as_local_files() {
    let (local, _) = local_setup().await;
    let (evicted, _remote, _) = evicted_setup(64 * MIB).await;

    let from_local = call(&local, values_body()).await.unwrap();
    let from_remote = call(&evicted, values_body()).await.unwrap();
    assert_eq!(from_local["status"], json!({"complete": true}));
    assert_eq!(from_remote, from_local);
}

#[tokio::test]
async fn value_suggestions_report_a_failed_download() {
    let (h, remote, [_, two]) = evicted_setup(64 * MIB).await;
    remote.lose(&two);

    let v = call(&h, values_body()).await.unwrap();
    let reason = &v["status"]["partial"][0];
    assert_eq!(
        (&reason["reason"], &reason["count"]),
        (&json!("remote_unavailable"), &json!(1)),
        "{v}"
    );
    // What the downloaded file holds is still served.
    assert!(!v["values"].as_array().unwrap().is_empty(), "{v}");
}

#[tokio::test]
async fn explore_counts_a_lost_remote_file() {
    let (h, remote, [_, two]) = evicted_setup(64 * MIB).await;
    remote.lose(&two);
    let body = json!({"explore": {
        "after": T_S, "before": T_S + 100,
        "sections": {"histogram": {}, "rows": {"limit": 100}}
    }});
    let v = call(&h, body).await.unwrap();
    let partial = json!({"partial": [{"reason": "remote_unavailable", "count": 1, "of": 2}]});
    assert_eq!(v["data"]["status"], partial);
    assert_eq!(v["data"]["rows"]["status"], partial);
    let rows = v["data"]["rows"]["items"].as_array().unwrap();
    assert!(!rows.is_empty(), "the downloaded file still answers");
    assert_eq!(
        v["data"]["rows"]["matched"],
        v["data"]["histogram"]["totals"]["count"]
    );
}

#[tokio::test]
async fn a_lost_file_outside_the_window_does_not_make_an_answer_partial() {
    // File 2 (seconds 30-40) is lost; a window over file 1 alone does not
    // need it.
    let (h, remote, [_, two]) = evicted_setup(64 * MIB).await;
    remote.lose(&two);

    for body in [
        json!({"explore": {"after": T_S + 5, "before": T_S + 25}}),
        json!({"values": {"after": T_S + 5, "before": T_S + 25, "field": "name"}}),
    ] {
        let v = call(&h, body.clone()).await.unwrap();
        let status = v.get("data").map_or(&v["status"], |data| &data["status"]);
        assert_eq!(status, &json!({"complete": true}), "{body}");
    }
}

/// Traces read catalog files only after releasing the registry read lock. A
/// catalog that blocks its reader (a FIFO not yet written) must not keep the
/// write lock — which the ledger loop takes on every WAL event — from being
/// granted.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalogs_are_read_off_the_registry_lock() {
    use crate::test_helpers::{FifoUnblocker, mkfifo, open_fifo_writer};
    use std::io::Write;
    let registries = make_registries();
    let remote = TestRemote::new(64 * MIB);
    let one = remote
        .evicted(&registries, "default", test_identity(), 1, file_one())
        .await;
    let catalog_bytes = std::fs::read(&one.catalog).unwrap();
    std::fs::remove_file(&one.catalog).unwrap();
    mkfifo(&one.catalog);
    let h = Arc::new(handler(Arc::clone(&registries), &remote));

    let query = tokio::spawn({
        let h = Arc::clone(&h);
        async move { call(&h, trace_body(0x0A)).await }
    });
    // Whatever happens below, a reader still blocked on the FIFO gets EOF
    // when the test ends, so a failure cannot hang the runtime's shutdown.
    let _unblock = FifoUnblocker(one.catalog.clone());

    // The query is now blocked reading the catalog.
    let deadline = std::time::Duration::from_secs(10);
    let mut writer = open_fifo_writer(&one.catalog, deadline).await;
    let write_lock = tokio::time::timeout(deadline, registries.write())
        .await
        .map(drop);
    // Feed the catalog either way, so a failing run ends instead of hanging.
    writer.write_all(&catalog_bytes).unwrap();
    drop(writer);
    let v = tokio::time::timeout(deadline, query)
        .await
        .expect("the query finishes once the catalog is fed")
        .unwrap()
        .unwrap();

    assert!(
        write_lock.is_ok(),
        "the query held the registry lock while reading a catalog"
    );
    // The catalog was read after all: the evicted trace came back.
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 1);
}
