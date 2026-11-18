//! Captured GenAI telemetry against independently computed totals.
//!
//! `tests/fixtures/<run>.traces.jsonl` and `<run>.logs.jsonl` hold the OTLP export requests of one captured run, one
//! request per line, with only the attributes the rules read. `expected.json` holds the chart totals an independent
//! script computed from the raw captures with the rules as specified.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as Av;
use otel_genai::{Call, Candidate, Normalizer, Pairs};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

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

/// The export requests in `path`; none when the run has no records of that signal.
fn exports<T: serde::de::DeserializeOwned>(path: &Path) -> Vec<T> {
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        })
        .collect()
}

/// The calls of one export request that count, after pair detection.
fn calls(
    normalizer: &Normalizer,
    pairs: &Pairs<String>,
    export: &ExportTraceServiceRequest,
) -> Vec<Call> {
    let mut candidates = Vec::new();
    for resource_spans in &export.resource_spans {
        let attributes = resource_spans
            .resource
            .as_ref()
            .map_or(&[][..], |resource| &resource.attributes[..]);
        let group: String = [
            "service.name",
            "service.namespace",
            "deployment.environment.name",
        ]
        .iter()
        .map(|key| {
            attributes
                .iter()
                .find(|attribute| attribute.key == *key)
                .and_then(
                    |attribute| match attribute.value.as_ref()?.value.as_ref()? {
                        Av::StringValue(value) => Some(value.as_str()),
                        _ => None,
                    },
                )
                .unwrap_or("")
        })
        .collect::<Vec<_>>()
        .join("\0");
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                if let Some(call) = normalizer.span(scope_spans.scope.as_ref(), span).unwrap() {
                    candidates.push(Candidate {
                        call,
                        trace_id: span.trace_id.clone(),
                        span_id: span.span_id.clone(),
                        parent_span_id: span.parent_span_id.clone(),
                        group: group.clone(),
                        data: (),
                    });
                }
            }
        }
    }
    pairs
        .retain(0, candidates)
        .into_iter()
        .map(|candidate| candidate.call)
        .collect()
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
        // One run is one producer session: its own pair memory.
        let pairs = Pairs::new();
        let mut got = Totals::default();
        for export in exports(&fixtures.join(format!("{run}.traces.jsonl"))) {
            for call in calls(&normalizer, &pairs, &export) {
                got.add(&call);
            }
        }
        for export in
            exports::<ExportLogsServiceRequest>(&fixtures.join(format!("{run}.logs.jsonl")))
        {
            for resource_logs in &export.resource_logs {
                for scope_logs in &resource_logs.scope_logs {
                    for record in &scope_logs.log_records {
                        if let Some(call) =
                            normalizer.log(scope_logs.scope.as_ref(), record).unwrap()
                        {
                            got.add(&call);
                        }
                    }
                }
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
    assert_eq!(checked, expected_runs.len());
}

/// The gates admit every record a rule matches, and keep the rest away from VRL.
#[test]
fn gates_admit_every_call_and_skip_the_rest() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let normalizer = Normalizer::new().unwrap();
    let (mut admitted, mut skipped) = ([0; 2], [0; 2]);
    let mut count = |signal: usize, admits: bool, ungated_is_none: bool, what: String| {
        if admits {
            admitted[signal] += 1;
        } else {
            skipped[signal] += 1;
            assert!(ungated_is_none, "{what} matched without its gate");
        }
    };
    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".traces.jsonl") {
            for export in exports::<ExportTraceServiceRequest>(&path) {
                for resource_spans in &export.resource_spans {
                    for scope_spans in &resource_spans.scope_spans {
                        let scope = scope_spans.scope.as_ref();
                        for span in &scope_spans.spans {
                            let ungated = normalizer.span_ungated(scope, span).unwrap();
                            assert_eq!(normalizer.span(scope, span).unwrap(), ungated);
                            let admits = normalizer.is_candidate(scope, span);
                            count(
                                0,
                                admits,
                                ungated.is_none(),
                                format!("{name}: {}", span.name),
                            );
                        }
                    }
                }
            }
        } else if name.ends_with(".logs.jsonl") {
            for export in exports::<ExportLogsServiceRequest>(&path) {
                for resource_logs in &export.resource_logs {
                    for scope_logs in &resource_logs.scope_logs {
                        let scope = scope_logs.scope.as_ref();
                        for record in &scope_logs.log_records {
                            let ungated = normalizer.log_ungated(scope, record).unwrap();
                            assert_eq!(normalizer.log(scope, record).unwrap(), ungated);
                            let admits = normalizer.is_log_candidate(scope, record);
                            count(
                                1,
                                admits,
                                ungated.is_none(),
                                format!("{name}: a log record"),
                            );
                        }
                    }
                }
            }
        }
    }
    // The fixtures hold both kinds for both signals: HTTP, agent and tool spans next to the calls, and other events
    // next to the counted ones.
    for signal in 0..2 {
        assert!(
            admitted[signal] > 0 && skipped[signal] > 0,
            "admitted {admitted:?}, skipped {skipped:?}"
        );
    }
}
