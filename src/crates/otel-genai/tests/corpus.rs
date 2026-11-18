//! Captured GenAI telemetry against independently computed totals.
//!
//! `tests/fixtures/<run>.jsonl` holds the OTLP trace export requests of one captured run (content and identity
//! attributes removed), one request per line. `expected.json` holds the chart totals an independent script computed
//! from the raw captures with the rules as specified.

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use otel_genai::{Call, Normalizer};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// Runs whose expected totals need call-span pair detection, which these tests do not apply yet.
const PAIRED: &[&str] = &[
    "otel-genai-anthropic-chat-r3",
    "otel-genai-anthropic-error",
    "otel-genai-anthropic-stream",
    "otel-genai-anthropic-tool",
    "pydantic-ai-anthropic-chat",
    "pydantic-ai-anthropic-error",
    "pydantic-ai-anthropic-stream",
    "pydantic-ai-anthropic-tool",
    "openllmetry-anthropic-chat",
    "openllmetry-anthropic-stream",
    "openllmetry-anthropic-tool",
];

#[derive(Debug, Default, PartialEq)]
struct Totals {
    success: u64,
    error: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    duration_count: u64,
    duration_micros: u64,
    labels: BTreeSet<[String; 4]>,
}

impl Totals {
    fn add(&mut self, call: &Call) {
        if call.error {
            self.error += 1;
        } else {
            self.success += 1;
        }
        let tokens = call.tokens;
        self.input += tokens.input.unwrap_or(0);
        self.output += tokens.output.unwrap_or(0);
        self.cache_read += tokens.cache_read.unwrap_or(0);
        self.cache_write += tokens.cache_write.unwrap_or(0);
        self.reasoning += tokens.reasoning.unwrap_or(0);
        if let Some(seconds) = call.duration_seconds {
            self.duration_count += 1;
            self.duration_micros += (seconds * 1e6).round() as u64;
        }
        let label = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_string());
        self.labels.insert([
            label(&call.provider),
            call.operation.clone(),
            label(&call.request_model),
            label(&call.response_model),
        ]);
    }
}

fn expected(run: &Value) -> Totals {
    let number = |key: &str| run[key].as_u64().unwrap_or_else(|| panic!("{key}"));
    let labels = run["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| {
            let parts: Vec<String> = label
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            [
                parts[0].clone(),
                parts[1].clone(),
                parts[2].clone(),
                parts[3].clone(),
            ]
        })
        .collect();
    Totals {
        success: number("success"),
        error: number("error"),
        input: number("input"),
        output: number("output"),
        cache_read: number("cache_read"),
        cache_write: number("cache_write"),
        reasoning: number("reasoning"),
        duration_count: number("duration_count"),
        duration_micros: (run["duration_sum"].as_f64().unwrap() * 1e6).round() as u64,
        labels,
    }
}

fn exports(path: &Path) -> Vec<ExportTraceServiceRequest> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        })
        .collect()
}

fn calls(normalizer: &Normalizer, export: &ExportTraceServiceRequest) -> Vec<Call> {
    let mut calls = Vec::new();
    for resource_spans in &export.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                if let Some(call) = normalizer.span(scope_spans.scope.as_ref(), span).unwrap() {
                    calls.push(call);
                }
            }
        }
    }
    calls
}

#[test]
fn corpus_runs_match_the_oracle() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let expected_runs: serde_json::Map<String, Value> =
        serde_json::from_str(&std::fs::read_to_string(fixtures.join("expected.json")).unwrap())
            .unwrap();
    let normalizer = Normalizer::new().unwrap();
    let mut checked = 0;
    for (run, want) in &expected_runs {
        if PAIRED.contains(&run.as_str()) {
            continue;
        }
        let mut got = Totals::default();
        for export in exports(&fixtures.join(format!("{run}.jsonl"))) {
            for call in calls(&normalizer, &export) {
                got.add(&call);
            }
        }
        // Durations are summed in microseconds on both sides; allow one microsecond of rounding per call.
        let want = expected(want);
        let slack = got.duration_count;
        assert!(
            got.duration_micros.abs_diff(want.duration_micros) <= slack,
            "{run}: duration {} vs {}",
            got.duration_micros,
            want.duration_micros
        );
        got.duration_micros = want.duration_micros;
        assert_eq!(got, want, "{run}");
        checked += 1;
    }
    assert_eq!(checked, expected_runs.len() - PAIRED.len());
}
