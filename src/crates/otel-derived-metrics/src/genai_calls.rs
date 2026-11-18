//! GenAI charts from spans and log records: model calls, tokens, token details and call duration, per service,
//! provider, operation, requested model and answering model.
//!
//! `otel-genai` turns each span or log record that describes a model call into a normalized call, whatever dialect
//! its producer speaks, and drops the second span of a call that arrives twice. This set counts what is left. Each
//! producer counts from one signal, so a call from a log record has no span twin to pair with. Token details (cache
//! reads, cache writes, reasoning) are parts of the input and output tokens, never additions to them. A call counts
//! at its end time; a call from a span is weighted like a request by its sampling threshold.

use crate::service::{service_labels, service_settings};
use crate::service_requests::adjusted_count;
use crate::shared_aggregator::SharedAggregator;
use netdata_plugin_metrics::{
    Batch, ChartDescriptor, ChartType, Counter, Histogram, Labels, MetricSet, MetricSetBuilder,
    Stats, columns,
};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use otel_genai::{Call, Candidate, Normalizer, Pairs};
use std::sync::atomic::{AtomicU64, Ordering};

/// Label sets (service, provider, operation and models) over this share the overflow entity.
pub(crate) const MAX_SERIES: usize = 1000;

/// Bucket bounds in seconds: the GenAI conventions' advice for `gen_ai.client.inference.duration`.
const DURATION_BOUNDS: &[f64] = &[
    0.01, 0.02, 0.04, 0.08, 0.16, 0.32, 0.64, 1.28, 2.56, 5.12, 10.24, 20.48, 40.96, 81.92,
];

/// At most one log line per this many seconds for rules that fail.
const RULE_ERROR_LOG_SECONDS: u64 = 60;

columns! {
    /// How a call ended.
    enum Outcome {
        Success = "success",
        Error = "error",
    }
}

columns! {
    /// All tokens a call sent and received.
    enum Tokens {
        Input = "input",
        Output = "output",
    }
}

columns! {
    /// Parts of the input and output tokens.
    enum TokenDetails {
        CacheRead = "cache_read",
        CacheWrite = "cache_write",
        Reasoning = "reasoning",
    }
}

/// The calls among the spans of one export request, normalized but not yet paired or counted.
pub(crate) type Pending = Vec<Candidate<Labels, f64>>;

/// The calls among the log records of one export request, normalized but not yet counted.
pub(crate) type PendingLogs = Vec<(Labels, Call)>;

pub(crate) struct GenAiCalls {
    metric_set: MetricSet,
    aggregator: SharedAggregator,
    normalizer: Normalizer,
    pairs: Pairs<Labels>,
    calls: Counter<Outcome>,
    tokens: Counter<Tokens>,
    token_details: Counter<TokenDetails>,
    duration: Histogram,
    rule_error_logged_at: AtomicU64,
}

impl GenAiCalls {
    pub(crate) fn new(max_series: usize) -> Result<Self, netdata_plugin_metrics::Error> {
        let mut builder = MetricSetBuilder::new(service_settings(max_series))?;
        let calls = builder.counter::<Outcome>(ChartDescriptor {
            title: "GenAI model calls".into(),
            units: "calls/s".into(),
            family: "genai".into(),
            context: "otel.derived_metrics.genai.calls".into(),
            chart_type: ChartType::Stacked,
            priority: 60_100,
        })?;
        let tokens = builder.counter::<Tokens>(ChartDescriptor {
            title: "GenAI tokens".into(),
            units: "tokens/s".into(),
            family: "genai".into(),
            context: "otel.derived_metrics.genai.tokens".into(),
            chart_type: ChartType::Line,
            priority: 60_101,
        })?;
        let token_details = builder.counter::<TokenDetails>(ChartDescriptor {
            title: "GenAI token details".into(),
            units: "tokens/s".into(),
            family: "genai".into(),
            context: "otel.derived_metrics.genai.token_details".into(),
            chart_type: ChartType::Line,
            priority: 60_102,
        })?;
        // Charts `otel.derived_metrics.genai.call_duration_avg`, `_max` and `_histogram`.
        let duration = builder.histogram(
            DURATION_BOUNDS,
            ChartDescriptor {
                title: "GenAI model call duration".into(),
                units: "seconds".into(),
                family: "genai".into(),
                context: "otel.derived_metrics.genai.call_duration".into(),
                chart_type: ChartType::Line,
                priority: 60_103,
            },
            "calls/s".into(),
        )?;
        let metric_set = builder.build()?;
        let aggregator = SharedAggregator::new(&metric_set);
        // The rules are part of this binary and their tests compile them.
        let normalizer = Normalizer::new().expect("built-in GenAI rules compile");
        Ok(Self {
            metric_set,
            aggregator,
            normalizer,
            pairs: Pairs::new(),
            calls,
            tokens,
            token_details,
            duration,
            rule_error_logged_at: AtomicU64::new(0),
        })
    }

    /// Normalize the calls among the spans of `export` into `pending`.
    pub(crate) fn record(
        &self,
        pending: &mut Pending,
        tenant: &str,
        export: &ExportTraceServiceRequest,
    ) {
        for resource_spans in &export.resource_spans {
            let mut service = None;
            for scope_spans in &resource_spans.scope_spans {
                for span in &scope_spans.spans {
                    let call = match self.normalizer.span(scope_spans.scope.as_ref(), span) {
                        Ok(Some(call)) => call,
                        Ok(None) => continue,
                        Err(error) => {
                            self.log_rule_error(&error);
                            continue;
                        }
                    };
                    let service = service.get_or_insert_with(|| {
                        service_labels(tenant, resource_spans.resource.as_ref())
                    });
                    pending.push(Candidate {
                        call,
                        trace_id: span.trace_id.clone(),
                        span_id: span.span_id.clone(),
                        parent_span_id: span.parent_span_id.clone(),
                        group: service.clone(),
                        data: adjusted_count(&span.trace_state),
                    });
                }
            }
        }
    }

    /// Normalize the calls among the log records of `export` into `pending`.
    pub(crate) fn record_logs(
        &self,
        pending: &mut PendingLogs,
        tenant: &str,
        export: &ExportLogsServiceRequest,
    ) {
        for resource_logs in &export.resource_logs {
            let mut service = None;
            for scope_logs in &resource_logs.scope_logs {
                for record in &scope_logs.log_records {
                    let call = match self.normalizer.log(scope_logs.scope.as_ref(), record) {
                        Ok(Some(call)) => call,
                        Ok(None) => continue,
                        Err(error) => {
                            self.log_rule_error(&error);
                            continue;
                        }
                    };
                    let service = service.get_or_insert_with(|| {
                        service_labels(tenant, resource_logs.resource.as_ref())
                    });
                    pending.push((service.clone(), call));
                }
            }
        }
    }

    /// Count `pending` at `now` (Unix seconds): its log records are durable.
    pub(crate) fn commit_logs(&self, pending: PendingLogs, now: u64) {
        if pending.is_empty() {
            return;
        }
        let mut batch = Batch::new(&self.metric_set);
        for (service, call) in pending {
            self.count(&mut batch, service, &call, 1.0);
        }
        self.aggregator.merge(batch, now);
    }

    /// Pair and count `pending` at `now` (Unix seconds): its spans are durable.
    pub(crate) fn commit(&self, pending: Pending, now: u64) {
        if pending.is_empty() {
            return;
        }
        let mut batch = Batch::new(&self.metric_set);
        for candidate in self.pairs.retain(now, pending) {
            self.count(&mut batch, candidate.group, &candidate.call, candidate.data);
        }
        self.aggregator.merge(batch, now);
    }

    /// Add to `chunks` every window whose allowed lateness has passed by `now` (Unix seconds).
    pub(crate) fn emit(&self, now: u64, chunks: &mut Vec<Vec<u8>>) -> Stats {
        self.aggregator.emit(now, chunks)
    }

    fn count(&self, batch: &mut Batch, service: Labels, call: &Call, weight: f64) {
        let mut labels = service;
        labels.insert("gen_ai.operation.name", &call.operation);
        for (key, value) in [
            ("gen_ai.provider.name", &call.provider),
            ("gen_ai.request.model", &call.request_model),
            ("gen_ai.response.model", &call.response_model),
        ] {
            if let Some(value) = value {
                labels.insert(key, value);
            }
        }
        let outcome = if call.error {
            Outcome::Error
        } else {
            Outcome::Success
        };
        let mut entity = batch.entity(labels);
        let mut event = entity.record(call.end_time_unix_nano / 1_000_000_000);
        event.add(self.calls, outcome, weight);
        let tokens = call.tokens;
        for (column, count) in [
            (Tokens::Input, tokens.input),
            (Tokens::Output, tokens.output),
        ] {
            if let Some(count) = count {
                event.add(self.tokens, column, count as f64 * weight);
            }
        }
        for (column, count) in [
            (TokenDetails::CacheRead, tokens.cache_read),
            (TokenDetails::CacheWrite, tokens.cache_write),
            (TokenDetails::Reasoning, tokens.reasoning),
        ] {
            if let Some(count) = count {
                event.add(self.token_details, column, count as f64 * weight);
            }
        }
        if let Some(seconds) = call.duration_seconds {
            event.observe(self.duration, seconds, weight);
        }
    }

    fn log_rule_error(&self, error: &otel_genai::RuleError) {
        let now = crate::unix_now();
        let last = self.rule_error_logged_at.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= RULE_ERROR_LOG_SECONDS
            && self
                .rule_error_logged_at
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            tracing::warn!("{error}; the record is not counted (logged at most once per minute)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    fn attr(key: &str, value: Value) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        }
    }

    fn text(key: &str, value: &str) -> KeyValue {
        attr(key, Value::StringValue(value.to_string()))
    }

    fn int(key: &str, value: i64) -> KeyValue {
        attr(key, Value::IntValue(value))
    }

    /// A call span of 0.5 s ending at second 100.
    fn call(span_id: u8, parent: u8, attributes: Vec<KeyValue>) -> Span {
        Span {
            trace_id: vec![1; 16],
            span_id: vec![span_id; 8],
            parent_span_id: if parent == 0 {
                Vec::new()
            } else {
                vec![parent; 8]
            },
            start_time_unix_nano: 99_500_000_000,
            end_time_unix_nano: 100_000_000_000,
            attributes,
            ..Default::default()
        }
    }

    fn chat_attributes() -> Vec<KeyValue> {
        vec![
            text("gen_ai.operation.name", "chat"),
            text("gen_ai.provider.name", "anthropic"),
            text("gen_ai.request.model", "claude-sonnet-4-5"),
            text("gen_ai.response.model", "claude-sonnet-4-5-20250929"),
            int("gen_ai.usage.input_tokens", 49),
            int("gen_ai.usage.output_tokens", 17),
            int("gen_ai.usage.cache_read.input_tokens", 29),
            int("gen_ai.usage.cache_write.input_tokens", 7),
        ]
    }

    fn render(spans: Vec<Span>) -> String {
        let export = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![text("service.name", "assistant")],
                    ..Default::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let genai = GenAiCalls::new(10).unwrap();
        let mut pending = Pending::new();
        genai.record(&mut pending, "default", &export);
        genai.commit(pending, 100);
        let mut chunks = Vec::new();
        genai.emit(115, &mut chunks);
        let out = netdata_plugin_protocol::readable_chart_stream(&chunks.concat());
        netdata_plugin_protocol::validate_chart_stream(out.as_bytes()).unwrap();
        out
    }

    fn first(out: &str, dimension: &str) -> String {
        let prefix = format!("SET {dimension} =");
        out.lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .map(|value| value.trim().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn charts_a_call_once_with_its_tokens_and_labels() {
        // The wrapper's span and the SDK's span inside it describe the same call.
        let out = render(vec![
            call(1, 0, chat_attributes()),
            call(2, 1, chat_attributes()),
            call(3, 0, vec![text("http.request.method", "POST")]),
        ]);
        for context in [
            "otel.derived_metrics.genai.calls",
            "otel.derived_metrics.genai.tokens",
            "otel.derived_metrics.genai.token_details",
            "otel.derived_metrics.genai.call_duration_avg",
        ] {
            assert!(out.contains(context), "{context} missing:\n{out}");
        }
        assert_eq!(first(&out, "success"), "1");
        assert_eq!(first(&out, "error"), "0");
        assert_eq!(first(&out, "input"), "49");
        assert_eq!(first(&out, "output"), "17");
        assert_eq!(first(&out, "cache_read"), "29");
        assert_eq!(first(&out, "cache_write"), "7");
        assert_eq!(first(&out, "reasoning"), "0");
        assert_eq!(first(&out, "avg"), "0.5");
        for label in [
            "CLABEL 'gen_ai.provider.name' 'anthropic' 1",
            "CLABEL 'gen_ai.operation.name' 'chat' 1",
            "CLABEL 'gen_ai.request.model' 'claude-sonnet-4-5' 1",
            "CLABEL 'gen_ai.response.model' 'claude-sonnet-4-5-20250929' 1",
            "CLABEL 'resource.attributes.service.name' 'assistant' 1",
        ] {
            assert!(out.contains(label), "{label} missing:\n{out}");
        }
    }

    #[test]
    fn charts_a_call_from_a_log_record() {
        let export = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![text("service.name", "claude-code")],
                    ..Default::default()
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "com.anthropic.claude_code.events".into(),
                        ..Default::default()
                    }),
                    log_records: vec![LogRecord {
                        time_unix_nano: 100_000_000_000,
                        attributes: vec![
                            text("event.name", "api_request"),
                            text("model", "claude-opus-4-5"),
                            int("input_tokens", 2),
                            int("cache_read_tokens", 30),
                            int("cache_creation_tokens", 10),
                            int("output_tokens", 5),
                            int("duration_ms", 1500),
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let genai = GenAiCalls::new(10).unwrap();
        let mut pending = PendingLogs::new();
        genai.record_logs(&mut pending, "default", &export);
        genai.commit_logs(pending, 100);
        let mut chunks = Vec::new();
        genai.emit(115, &mut chunks);
        let out = netdata_plugin_protocol::readable_chart_stream(&chunks.concat());
        netdata_plugin_protocol::validate_chart_stream(out.as_bytes()).unwrap();
        assert_eq!(first(&out, "success"), "1");
        assert_eq!(first(&out, "input"), "42");
        assert_eq!(first(&out, "cache_read"), "30");
        assert_eq!(first(&out, "avg"), "1.5");
        assert!(
            out.contains("CLABEL 'gen_ai.request.model' 'claude-opus-4-5' 1"),
            "{out}"
        );
    }

    #[test]
    fn spans_that_are_not_calls_chart_nothing() {
        let out = render(vec![call(3, 0, vec![text("http.request.method", "POST")])]);
        assert!(!out.contains("genai"), "{out}");
    }
}
