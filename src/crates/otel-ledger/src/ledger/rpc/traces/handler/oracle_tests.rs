//! The explorer's Function answer compared with the reference calculator:
//! a seeded corpus sealed into two files plus an active WAL, the handler
//! called with an `explore` body, the JSON histogram and totals compared with
//! `otel_oracle::calc`. The engine's own comparison proves the numbers; this
//! proves the wire carries them unchanged.

use std::collections::BTreeMap;

use super::*;
use crate::ledger::rpc::traces::fixtures::{install_sealed, install_wal, make_registries};
use bridge::function::ProgressState;
use otel_oracle::calc::{self, Grid, Scope, fixed_histogram};
use otel_oracle::corpus::{self, MeshParams};
use otel_oracle::model;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const T0_S: u64 = 1_700_000_000;

#[tokio::test]
async fn explore_histogram_and_totals_match_the_calculator() {
    let spans = corpus::generate(&MeshParams {
        traces: 300,
        start_ns: T0_S * 1_000_000_000,
        trace_spacing_ns: 900_000_000,
        seed: 61,
    });
    let requests = corpus::build_requests(&spans, 40);
    let third = requests.len() / 3;
    let mut oracle = Vec::new();
    for request in &requests {
        oracle.extend(model::spans_of_request(request, 0));
    }

    let registries = make_registries();
    install_sealed(&registries, "default", 1, requests[..third].to_vec()).await;
    install_sealed(
        &registries,
        "default",
        2,
        requests[third..2 * third].to_vec(),
    )
    .await;
    install_wal(&registries, "default", 3, requests[2 * third..].to_vec()).await;
    let h = OtelTracesHandler::new(
        registries,
        Arc::new(ChunkCache::new(64 * 1024 * 1024)),
        200,
        None,
    );

    let last_s = oracle.iter().map(|s| s.start_ns).max().unwrap() / 1_000_000_000;
    let (after, before) = (T0_S as u32, last_s as u32 + 1);
    let grid = Grid::for_window(after, before);
    for (filter, scope) in [
        (json!({}), Scope::default()),
        (json!({"_role": ["root", "inbound"]}), Scope::entry_spans()),
    ] {
        for stack in [model::STATUS_FIELD, model::SERVICE_FIELD] {
            let body = json!({"explore": {
                "after": after, "before": before, "filter": filter,
                "sections": {"histogram": {"stack": stack}}
            }});
            let ctx = FunctionCallContext::new(
                "tx-oracle".to_string(),
                ProgressState::new(),
                CancellationToken::new(),
            );
            let req: OtelTracesRequest = serde_json::from_value(body.clone()).unwrap();
            let v = serde_json::to_value(h.on_call(ctx, req).await.unwrap()).unwrap();
            let data = &v["data"];
            assert_eq!(data["status"], json!({"complete": true}), "{body}");
            assert_eq!(
                data["window"]["grid"]["bucket_ns"],
                i64::from(grid.width_s) * 1_000_000_000,
                "{body}"
            );

            let histogram = &data["histogram"];
            let dimensions: Vec<String> = histogram["dimensions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d.as_str().unwrap().to_string())
                .collect();
            let mut got = Vec::new();
            for bucket in histogram["buckets"].as_array().unwrap() {
                let mut counts = BTreeMap::new();
                for (value, count) in dimensions.iter().zip(bucket["counts"].as_array().unwrap()) {
                    let count = count.as_u64().unwrap();
                    if count > 0 {
                        counts.insert(value.clone(), count);
                    }
                }
                got.push(calc::Bucket {
                    counts,
                    unset: bucket["unset"].as_u64().unwrap(),
                });
            }
            assert_eq!(
                got,
                calc::histogram(&oracle, &grid, &scope, stack),
                "{body}"
            );

            let pct = |values: Option<[i64; 3]>| match values {
                Some([p50, p95, p99]) => json!({"p50_ns": p50, "p95_ns": p95, "p99_ns": p99}),
                None => json!({}),
            };
            let per_bucket = calc::bucket_durations(&oracle, &grid, &scope);
            let mut window_durations = Vec::new();
            for (bucket, durations) in histogram["buckets"]
                .as_array()
                .unwrap()
                .iter()
                .zip(&per_bucket)
            {
                let mut got = json!({});
                for field in ["p50_ns", "p95_ns", "p99_ns"] {
                    if let Some(value) = bucket.get(field) {
                        got[field] = value.clone();
                    }
                }
                assert_eq!(got, pct(fixed_histogram::percentiles(durations)), "{body}");
                window_durations.extend_from_slice(durations);
            }

            let totals = calc::totals(&oracle, &grid, &scope);
            let mut want = json!({"count": totals.spans, "errors": totals.errors});
            if let Some([p50, p95, p99]) = fixed_histogram::percentiles(&window_durations) {
                want["p50_ns"] = json!(p50);
                want["p95_ns"] = json!(p95);
                want["p99_ns"] = json!(p99);
            }
            assert_eq!(histogram["totals"], want, "{body}");
        }
    }
}
