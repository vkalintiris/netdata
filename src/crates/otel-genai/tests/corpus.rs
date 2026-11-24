//! Captured GenAI telemetry against its expected chart totals and against what the provider's API returned.
//!
//! `tests/fixtures/<run>.traces.jsonl` holds the OTLP span export requests of one captured run, one request per line,
//! with only the attributes the rules read.
//!
//! - `expected.json` pins the chart totals of every run: a script that implements the rules as specified computed
//!   them from the raw captures. It tells when a change alters what is charted, not whether that is right. A run
//!   with tool calls carries them under `tools`, per tool name; a run without the key charts none.
//! - `wire.json` holds, for the runs that made real model calls, what the API returned: a recording proxy between
//!   the client and the provider wrote down each request's outcome and usage. A run that is known to differ from it
//!   names the fields that may differ and why.

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as Av;
use otel_genai::{Call, Candidate, Normalizer, Pairs, ToolCall};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
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
    first_chunk_count: u64,
    first_chunk_micros: u64,
    labels: BTreeSet<[String; 4]>,
    /// Per tool name (`-` for none): the tool calls that succeeded and that failed.
    tools: BTreeMap<String, [u64; 2]>,
    tool_duration_micros: u64,
}

impl Totals {
    fn add_tool(&mut self, tool: &ToolCall) {
        let name = tool.name.clone().unwrap_or_else(|| "-".to_string());
        self.tools.entry(name).or_default()[usize::from(tool.error)] += 1;
        self.tool_duration_micros += (tool.duration_seconds * 1e6).round() as u64;
    }

    fn tool_calls(&self) -> u64 {
        self.tools.values().flatten().sum()
    }

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
        self.duration_count += 1;
        self.duration_micros += (call.duration_seconds * 1e6).round() as u64;
        if let Some(seconds) = call.time_to_first_chunk_seconds {
            self.first_chunk_count += 1;
            self.first_chunk_micros += (seconds * 1e6).round() as u64;
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
        first_chunk_count: number("first_chunk_count"),
        first_chunk_micros: (run["first_chunk_sum"].as_f64().unwrap() * 1e6).round() as u64,
        labels,
        tools: run["tools"]["names"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, counts)| {
                let count = |key: &str| counts[key].as_u64().unwrap_or_else(|| panic!("{key}"));
                (name.clone(), [count("success"), count("error")])
            })
            .collect(),
        tool_duration_micros: (run["tools"]["duration_sum"].as_f64().unwrap_or(0.0) * 1e6).round()
            as u64,
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
                        scope: scope_spans
                            .scope
                            .as_ref()
                            .map(|scope| scope.name.clone())
                            .unwrap_or_default(),
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

/// The tool calls of one export request: a span that is a model call is not tried as a tool call.
fn tool_calls(normalizer: &Normalizer, export: &ExportTraceServiceRequest) -> Vec<ToolCall> {
    let mut tools = Vec::new();
    for resource_spans in &export.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            let scope = scope_spans.scope.as_ref();
            for span in &scope_spans.spans {
                if normalizer.span(scope, span).unwrap().is_some() {
                    continue;
                }
                tools.extend(normalizer.tool_span(scope, span).unwrap());
            }
        }
    }
    tools
}

/// The chart totals of one captured run.
fn totals(normalizer: &Normalizer, fixtures: &Path, run: &str) -> Totals {
    // One run is one producer session: its own pair memory.
    let pairs = Pairs::new();
    let mut got = Totals::default();
    for export in exports(&fixtures.join(format!("{run}.traces.jsonl"))) {
        for call in calls(normalizer, &pairs, &export) {
            got.add(&call);
        }
        for tool in tool_calls(normalizer, &export) {
            got.add_tool(&tool);
        }
    }
    got
}

fn fixture_file(fixtures: &Path, name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures.join(name)).unwrap()).unwrap()
}

#[test]
fn corpus_runs_match_the_oracle() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let expected_runs = fixture_file(&fixtures, "expected.json");
    let expected_runs = expected_runs.as_object().unwrap();
    let normalizer = Normalizer::new().unwrap();
    let mut checked = 0;
    for (run, want) in expected_runs {
        let mut got = totals(&normalizer, &fixtures, run);
        // Times are summed in microseconds on both sides; allow one microsecond of rounding per call.
        let want = expected(want);
        assert!(
            got.duration_micros.abs_diff(want.duration_micros) <= got.duration_count,
            "{run}: duration {} vs {}",
            got.duration_micros,
            want.duration_micros
        );
        got.duration_micros = want.duration_micros;
        assert!(
            got.first_chunk_micros.abs_diff(want.first_chunk_micros) <= got.first_chunk_count,
            "{run}: time to first chunk {} vs {}",
            got.first_chunk_micros,
            want.first_chunk_micros
        );
        got.first_chunk_micros = want.first_chunk_micros;
        assert!(
            got.tool_duration_micros.abs_diff(want.tool_duration_micros) <= got.tool_calls(),
            "{run}: tool duration {} vs {}",
            got.tool_duration_micros,
            want.tool_duration_micros
        );
        got.tool_duration_micros = want.tool_duration_micros;
        assert_eq!(got, want, "{run}");
        checked += 1;
    }
    assert_eq!(checked, expected_runs.len());
}

/// Every run of real model calls charts what the API returned, except in the fields a known difference names. A
/// reason no run needs any more fails the test: the gap it describes is closed and must leave `wire.json`.
#[test]
fn real_call_runs_match_the_api() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let wire = fixture_file(&fixtures, "wire.json");
    let normalizer = Normalizer::new().unwrap();
    let mut needed = BTreeSet::new();
    let mut wrong = Vec::new();
    for (run, api) in wire["runs"].as_object().unwrap() {
        let got = totals(&normalizer, &fixtures, run);
        let fields = [
            ("success", got.success),
            ("error", got.error),
            ("input", got.input),
            ("output", got.output),
            ("cache_read", got.cache_read),
            ("cache_write", got.cache_write),
            ("reasoning", got.reasoning),
        ];
        // An answer without a usage block leaves only the calls to compare.
        let comparable = if api["usage"].as_bool().unwrap() {
            fields.len()
        } else {
            2
        };
        // The fields in which this run is known to differ, each with its reason.
        let excepted = api["except"].as_object();
        for (field, ours) in &fields[..comparable] {
            let theirs = api[*field].as_u64().unwrap();
            if *ours == theirs {
                continue;
            }
            match excepted.and_then(|excepted| excepted.get(*field)) {
                Some(reason) => {
                    needed.insert(reason.as_str().unwrap());
                }
                None => wrong.push(format!(
                    "{run}: {field} is {ours}, the API returned {theirs}"
                )),
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    let reasons: BTreeSet<&str> = wire["reasons"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(needed, reasons, "reasons no run needs any more");
}

/// The gates admit every span a rule matches, and keep the rest away from VRL.
#[test]
fn gates_admit_every_call_and_skip_the_rest() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let normalizer = Normalizer::new().unwrap();
    // Model call spans, tool spans.
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
                            let ungated = normalizer.tool_span_ungated(scope, span).unwrap();
                            assert_eq!(normalizer.tool_span(scope, span).unwrap(), ungated);
                            let admits = normalizer.is_tool_candidate(scope, span);
                            count(
                                1,
                                admits,
                                ungated.is_none(),
                                format!("{name}: {} as a tool", span.name),
                            );
                        }
                    }
                }
            }
        }
    }
    // The fixtures hold both kinds for each: HTTP and agent spans next to the calls and the tool calls.
    for signal in 0..2 {
        assert!(
            admitted[signal] > 0 && skipped[signal] > 0,
            "admitted {admitted:?}, skipped {skipped:?}"
        );
    }
}
