//! Real answers of every `otel-traces` mode, asked through the Functions
//! bridge, validate against the published Function response schema
//! (`src/plugins.d/FUNCTION_UI_SCHEMA.json`): the explorer's requests of a
//! tier-2 plan (every section, selections with a comparison and a delta, row
//! pages, fields, value suggestions), trace-by-id whole and capped, `info`,
//! and an empty window.

use std::collections::BTreeMap;

use super::*;
use crate::ledger::rpc::traces::fixtures::{
    call_through_bridge, install_sealed, install_wal_at_span_starts, make_registries_at,
};
use bridge::function::HandlerAdapter;
use otel_oracle::corpus::{self, MeshParams};
use otel_oracle::tier2;
use serde_json::{Value, json};

const T0_S: u64 = 1_700_000_000;
const CHUNK_ENTRIES: u64 = 100;
const SCHEMA: &str = "file:///FUNCTION_UI_SCHEMA.json";

fn published_schema() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins.d/FUNCTION_UI_SCHEMA.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The schema compiled whole, and at its `traces_status` definition.
fn compiled() -> (boon::Schemas, boon::SchemaIndex, boon::SchemaIndex) {
    let mut schemas = boon::Schemas::new();
    let mut compiler = boon::Compiler::new();
    compiler.add_resource(SCHEMA, published_schema()).unwrap();
    let response = compiler.compile(SCHEMA, &mut schemas).unwrap();
    let status = compiler
        .compile(
            &format!("{SCHEMA}#/definitions/traces_status"),
            &mut schemas,
        )
        .unwrap();
    (schemas, response, status)
}

/// Every answer, by request id, from a mesh stored half sealed and half in a
/// live WAL.
async fn answers() -> BTreeMap<String, Value> {
    let generated = corpus::generate(&MeshParams {
        traces: 60,
        start_ns: T0_S * 1_000_000_000,
        trace_spacing_ns: 900_000_000,
        seed: 71,
    });
    let requests = corpus::build_requests(&generated, 40);
    let half = requests.len() / 2;
    let mut rows = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        rows.extend(otel_oracle::model::spans_of_request(
            request,
            usize::from(index >= half),
        ));
    }
    let last_s = rows.iter().map(|row| row.start_ns).max().unwrap() / 1_000_000_000;

    let store = tempfile::tempdir().unwrap();
    let registries = make_registries_at(store.path());
    install_sealed(&registries, "default", 1, requests[..half].to_vec()).await;
    install_wal_at_span_starts(&registries, "default", 2, requests[half..].to_vec()).await;
    let adapter = HandlerAdapter::new(OtelTracesHandler::new(
        registries,
        Arc::new(ChunkCache::new(64 * 1024 * 1024)),
        CHUNK_ENTRIES,
        None,
    ));

    let after = T0_S as u32;
    let before = u32::try_from(last_s).unwrap() + 1;
    let mut plan = tier2::plan(after, before, &rows, 2);
    tier2::add_traces(&mut plan, &rows, &|unit| unit == 1);
    let mut asked: Vec<(String, Value)> = vec![
        ("info".into(), json!({"info": {}})),
        (
            "an empty window".into(),
            json!({"explore": {"after": 1_000, "before": 2_000, "sections": {
                "histogram": {"stack": "status_code", "percentiles": true},
                "facets": {}, "groups": {}, "fields": {},
                "rows": {"order": "newest", "limit": 10}}}}),
        ),
    ];
    let mut answers = BTreeMap::new();
    for _ in 0..3 {
        for request in &plan.requests {
            asked.push((request.id.clone(), request.body.clone()));
        }
        for (id, body) in asked.drain(..) {
            if answers.contains_key(&id) {
                continue;
            }
            let bytes = serde_json::to_vec(&body).unwrap();
            let (status, payload) = call_through_bridge(&adapter, Some(&bytes)).await;
            assert_eq!(status, 200, "{id}: {}", String::from_utf8_lossy(&payload));
            answers.insert(id, serde_json::from_slice(&payload).unwrap());
        }
        tier2::add_pages(&mut plan, &answers);
    }
    answers
}

#[tokio::test]
async fn responses_validate_against_function_ui_schema() {
    let answers = answers().await;
    let (schemas, response, _) = compiled();

    let mut modes = BTreeMap::new();
    for (id, answer) in &answers {
        let mode = answer["mode"]
            .as_str()
            .or(answer["data"]["mode"].as_str())
            .unwrap_or("?");
        *modes.entry(mode.to_string()).or_insert(0) += 1;
        if let Err(error) = schemas.validate(answer, response) {
            panic!("{id} does not validate: {error:#}");
        }
    }
    assert_eq!(
        modes.keys().collect::<Vec<_>>(),
        ["explore", "info", "trace", "values"]
    );
    let sections = ["histogram", "facets", "groups", "rows", "fields"];
    for section in sections {
        assert!(
            answers.values().any(|a| a["data"][section].is_object()),
            "no answer with {section}"
        );
    }
    let delta = answers
        .values()
        .any(|a| a["data"]["groups"]["delta"].is_object());
    let comparison = answers
        .values()
        .any(|a| a["data"]["facets"]["comparison"].is_object());
    let capped = answers
        .values()
        .any(|a| a["status"]["partial"][0]["reason"] == "size_cap");
    let origin = answers.values().any(|a| {
        a["spans"]
            .as_array()
            .is_some_and(|spans| spans.iter().any(|s| s["error_origin"] == true))
    });
    let events = answers.values().any(|a| {
        a["spans"].as_array().is_some_and(|spans| {
            spans.iter().any(|s| {
                !s["events"].as_array().unwrap().is_empty() && s["self_duration_ns"].is_i64()
            })
        })
    });
    assert!(delta && comparison && capped && origin && events);
}

#[tokio::test]
async fn the_schema_rejects_what_the_wire_never_sends() {
    let answers = answers().await;
    let (schemas, response, status) = compiled();
    let trace = answers
        .values()
        .find(|a| a["mode"] == "trace" && a["spans"][0].is_object())
        .unwrap()
        .clone();

    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut edit = |name, change: &dyn Fn(&mut Value)| {
        let mut answer = trace.clone();
        change(&mut answer);
        cases.push((name, answer));
    };
    edit("an origin that is not a flag", &|a| {
        a["spans"][0]["error_origin"] = json!("yes")
    });
    edit("a negative self time", &|a| {
        a["spans"][0]["self_duration_ns"] = json!(-1)
    });
    edit("an unknown partial reason", &|a| {
        a["status"] = json!({"partial": [{"reason": "tired", "count": 1}]})
    });
    edit("a status false", &|a| {
        a["status"] = json!({"complete": false})
    });
    edit("another mode", &|a| a["mode"] = json!("overview"));
    edit("a span without self time", &|a| {
        a["spans"][0]
            .as_object_mut()
            .unwrap()
            .remove("self_duration_ns");
    });
    let mut search = answers
        .values()
        .find(|a| a["data"]["mode"] == "explore")
        .unwrap()
        .clone();
    search["data"]["mode"] = json!("search");
    cases.push(("a retired search answer", search));
    for (name, answer) in cases {
        assert!(schemas.validate(&answer, response).is_err(), "{name}");
    }

    let partial = json!({"partial": [
        {"reason": "source_failure", "count": 1, "of": 4, "detail": ["a file"]},
        {"reason": "size_cap", "count": 1}
    ]});
    assert!(schemas.validate(&partial, status).is_ok());
    assert!(
        schemas
            .validate(
                &json!({"partial": [{"reason": "size_cap", "count": 0}]}),
                status
            )
            .is_err()
    );
}
