//! GenAI charts from spans: model calls, tokens, token details, call duration and time to first chunk, per service,
//! provider, operation, requested model and answering model.
//!
//! `otel-genai` turns each span that describes a model call into a normalized call, whatever dialect its producer
//! speaks, and drops the second span of a call that arrives twice. This set counts what is left. Token details (cache
//! reads, cache writes, reasoning) are parts of the input and output tokens, never additions to them. A call counts
//! at its end time, weighted like a request by its sampling threshold. The time to first chunk is the producer's own
//! measurement, charted for the streamed calls of the producers that report one.
//!
//! Each call that reports tokens is priced from a table of published prices ([`otel_genai::Prices`]): the cost
//! chart adds up the calls the table knows, and a second chart counts the calls it knows and the calls it does not,
//! so a cost that leaves calls out says so.
//!
//! The same pass over the spans finds the tool calls, which [`crate::genai_tools`] counts in its own charts.
//!
//! The pass also writes each call onto its span ([`otel_genai::annotation`]), with whether it counts, before the
//! span is stored: the stored spans marked as counted are the calls of the charts.
//!
//! A call a gateway reported while forwarding it is marked but not counted: the charts count what applications
//! report, and a traced application behind the gateway reports the same call again.

use crate::genai_tools::{GenAiTools, PendingTool};
use crate::service::{service_labels, service_settings};
use crate::service_requests::adjusted_count;
use crate::shared_aggregator::SharedAggregator;
use netdata_plugin_metrics::{
    Batch, ChartDescriptor, ChartType, Counter, Histogram, Labels, MetricSet, MetricSetBuilder,
    Stats, columns,
};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use otel_genai::{Call, Candidate, Normalizer, Pairs, Price, Prices, Remembered, annotation};
use std::sync::atomic::{AtomicU64, Ordering};

/// Label sets (service, provider, operation and models) over this share the overflow entity.
pub(crate) const MAX_SERIES: usize = 1000;

/// Bucket bounds in seconds: the GenAI conventions' advice for `gen_ai.client.inference.duration` and for
/// `gen_ai.client.inference.time_to_first_chunk`.
pub(crate) const DURATION_BOUNDS: &[f64] = &[
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

columns! {
    /// What the priced calls cost.
    enum Cost {
        Cost = "cost",
    }
}

columns! {
    /// Whether the price table knows the model of a call that reports tokens.
    enum Priced {
        Priced = "priced",
        Unpriced = "unpriced",
    }
}

/// What a counted call carries besides itself.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Counted {
    /// The call's sampling weight.
    weight: f64,
    price: Option<Price>,
}

/// The model calls and tool calls among the spans of one export request, normalized and paired but not yet counted.
#[derive(Default)]
pub(crate) struct Pending {
    /// The calls that count, with their service labels and weight.
    calls: Vec<Candidate<Labels, Counted>>,
    /// Call spans that are the second span of a call.
    duplicates: u64,
    /// Call spans a gateway reported.
    gateway_calls: u64,
    tools: Vec<PendingTool>,
    /// What pairing remembers of the request, to forget if the request is not stored.
    remembered: Vec<Remembered<Labels>>,
}

pub(crate) struct GenAiCalls {
    metric_set: MetricSet,
    aggregator: SharedAggregator,
    normalizer: Normalizer,
    prices: Prices,
    tools: GenAiTools,
    pairs: Pairs<Labels>,
    calls: Counter<Outcome>,
    tokens: Counter<Tokens>,
    token_details: Counter<TokenDetails>,
    duration: Histogram,
    time_to_first_chunk: Histogram,
    cost: Counter<Cost>,
    priced_calls: Counter<Priced>,
    rule_error_logged_at: AtomicU64,
    duplicates: AtomicU64,
    gateway_calls: AtomicU64,
    failed_rules: AtomicU64,
}

/// What the GenAI charts did not count as it arrived, since the start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Uncounted {
    /// Call spans dropped as the second span of a call already counted.
    pub(crate) duplicates: u64,
    /// Call spans a gateway reported.
    pub(crate) gateway_calls: u64,
    /// Records a rule matched and then failed on.
    pub(crate) failed_rules: u64,
}

impl GenAiCalls {
    pub(crate) fn new(
        max_series: usize,
        max_tool_series: usize,
    ) -> Result<Self, netdata_plugin_metrics::Error> {
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
        // Charts `otel.derived_metrics.genai.time_to_first_chunk_avg`, `_max` and `_histogram`.
        let time_to_first_chunk = builder.histogram(
            DURATION_BOUNDS,
            ChartDescriptor {
                title: "GenAI time to first chunk".into(),
                units: "seconds".into(),
                family: "genai".into(),
                context: "otel.derived_metrics.genai.time_to_first_chunk".into(),
                chart_type: ChartType::Line,
                priority: 60_106,
            },
            "calls/s".into(),
        )?;
        // Millionths of a US dollar: the Agent's first storage tier keeps about seven digits and nothing under
        // one ten-millionth, which whole dollars a second would fall below.
        let cost = builder.counter::<Cost>(ChartDescriptor {
            title: "GenAI estimated cost".into(),
            units: "microUSD/s".into(),
            family: "genai".into(),
            context: "otel.derived_metrics.genai.cost".into(),
            chart_type: ChartType::Line,
            priority: 60_107,
        })?;
        let priced_calls = builder.counter::<Priced>(ChartDescriptor {
            title: "GenAI model calls by whether their model has a known price".into(),
            units: "calls/s".into(),
            family: "genai".into(),
            context: "otel.derived_metrics.genai.priced_calls".into(),
            chart_type: ChartType::Stacked,
            priority: 60_108,
        })?;
        let metric_set = builder.build()?;
        let aggregator = SharedAggregator::new(&metric_set);
        // The rules are part of this binary and their tests compile them.
        let normalizer = Normalizer::new().expect("built-in GenAI rules compile");
        Ok(Self {
            metric_set,
            aggregator,
            normalizer,
            prices: Prices::new(),
            tools: GenAiTools::new(max_tool_series)?,
            pairs: Pairs::new(),
            calls,
            tokens,
            token_details,
            duration,
            time_to_first_chunk,
            cost,
            priced_calls,
            rule_error_logged_at: AtomicU64::new(0),
            duplicates: AtomicU64::new(0),
            gateway_calls: AtomicU64::new(0),
            failed_rules: AtomicU64::new(0),
        })
    }

    /// Normalize the model calls and tool calls among the spans of `export` into `pending`, decide at `now` (Unix
    /// seconds) which calls count, and write each call onto its span. `export` is one group of an export request:
    /// calls of the same request in another group pair as calls of another request do.
    pub(crate) fn record(
        &self,
        pending: &mut Pending,
        tenant: &str,
        export: &mut ExportTraceServiceRequest,
        now: u64,
    ) {
        let mut candidates = Vec::new();
        // Where each candidate's span is: its resource, scope and span indexes.
        let mut places = Vec::new();
        for (resource_index, resource_spans) in export.resource_spans.iter_mut().enumerate() {
            let mut service = None;
            let resource = resource_spans.resource.as_ref();
            for (scope_index, scope_spans) in resource_spans.scope_spans.iter_mut().enumerate() {
                let scope = scope_spans.scope.as_ref();
                for (span_index, span) in scope_spans.spans.iter_mut().enumerate() {
                    annotation::strip(span);
                    let mut service = || {
                        service
                            .get_or_insert_with(|| service_labels(tenant, resource))
                            .clone()
                    };
                    match self.normalizer.span(scope, span) {
                        Ok(Some(call)) if call.gateway => {
                            // A gateway prices its own calls; this table is for what applications report.
                            annotation::annotate(span, &call, false, None);
                            pending.gateway_calls += 1;
                            continue;
                        }
                        Ok(Some(call)) => {
                            let price = self.prices.price(&call);
                            candidates.push(Candidate {
                                call,
                                trace_id: span.trace_id.clone(),
                                span_id: span.span_id.clone(),
                                parent_span_id: span.parent_span_id.clone(),
                                scope: scope.map(|scope| scope.name.clone()).unwrap_or_default(),
                                group: service(),
                                data: Counted {
                                    weight: adjusted_count(&span.trace_state),
                                    price,
                                },
                            });
                            places.push((resource_index, scope_index, span_index));
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            self.log_rule_error(&error);
                            continue;
                        }
                    }
                    match self.normalizer.tool_span(scope, span) {
                        Ok(Some(tool)) => {
                            annotation::annotate_tool(span, &tool);
                            pending.tools.push(PendingTool {
                                service: service(),
                                tool,
                                weight: adjusted_count(&span.trace_state),
                            });
                        }
                        Ok(None) => {}
                        Err(error) => self.log_rule_error(&error),
                    }
                }
            }
        }
        if candidates.is_empty() {
            return;
        }

        let (keep, remembered) = self.pairs.decide(now, &candidates);
        pending.remembered.push(remembered);
        for ((candidate, place), keep) in candidates.into_iter().zip(places).zip(keep) {
            let (resource_index, scope_index, span_index) = place;
            let span = &mut export.resource_spans[resource_index].scope_spans[scope_index].spans
                [span_index];
            annotation::annotate(span, &candidate.call, keep, candidate.data.price);
            if keep {
                pending.calls.push(candidate);
            } else {
                pending.duplicates += 1;
            }
        }
    }

    /// Price later calls from `data`, the price table's current data; data that is not a table changes nothing.
    pub(crate) fn replace_prices(&self, data: &[u8]) -> Result<(), otel_genai::InvalidTable> {
        self.prices.replace(data)
    }

    /// Count `pending` at `now` (Unix seconds): its spans are durable.
    pub(crate) fn commit(&self, pending: Pending, now: u64) {
        self.tools.commit(pending.tools, now);
        self.duplicates
            .fetch_add(pending.duplicates, Ordering::Relaxed);
        self.gateway_calls
            .fetch_add(pending.gateway_calls, Ordering::Relaxed);
        if pending.calls.is_empty() {
            return;
        }
        let mut batch = Batch::new(&self.metric_set);
        for candidate in pending.calls {
            self.count(&mut batch, candidate.group, &candidate.call, candidate.data);
        }
        self.aggregator.merge(batch, now);
    }

    /// Drop `pending`: its spans were not stored, and the sender sends them again.
    pub(crate) fn discard(&self, pending: Pending) {
        // Newest first: a later group may have paired with what an earlier one added.
        for remembered in pending.remembered.into_iter().rev() {
            self.pairs.forget(remembered);
        }
    }

    pub(crate) fn uncounted(&self) -> Uncounted {
        Uncounted {
            duplicates: self.duplicates.load(Ordering::Relaxed),
            gateway_calls: self.gateway_calls.load(Ordering::Relaxed),
            failed_rules: self.failed_rules.load(Ordering::Relaxed),
        }
    }

    /// Add to `chunks` every window whose allowed lateness has passed by `now` (Unix seconds). Returns the
    /// statistics of the model call charts and of the tool charts.
    pub(crate) fn emit(&self, now: u64, chunks: &mut Vec<Vec<u8>>) -> (Stats, Stats) {
        let calls = self.aggregator.emit(now, chunks);
        (calls, self.tools.emit(now, chunks))
    }

    fn count(&self, batch: &mut Batch, service: Labels, call: &Call, counted: Counted) {
        let weight = counted.weight;
        let mut labels = service;
        labels.insert("gen_ai.operation.name", &call.operation);
        labels.insert("gen_ai.provider.name", call.provider_label());
        labels.insert("gen_ai.request.model", call.request_model_label());
        labels.insert("gen_ai.response.model", call.response_model_label());
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
        event.observe(self.duration, call.duration_seconds, weight);
        if let Some(seconds) = call.time_to_first_chunk_seconds {
            event.observe(self.time_to_first_chunk, seconds, weight);
        }
        match counted.price {
            Some(Price::Usd(usd)) => {
                event.add(self.cost, Cost::Cost, usd * 1e6 * weight);
                event.add(self.priced_calls, Priced::Priced, weight);
            }
            Some(Price::Unknown) => {
                event.add(self.priced_calls, Priced::Unpriced, weight);
            }
            None => {}
        }
    }

    fn log_rule_error(&self, error: &otel_genai::RuleError) {
        self.failed_rules.fetch_add(1, Ordering::Relaxed);
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

    fn double(key: &str, value: f64) -> KeyValue {
        attr(key, Value::DoubleValue(value))
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
        let mut export = export;
        let genai = GenAiCalls::new(10, 10).unwrap();
        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut export, 100);
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
    fn charts_what_a_call_cost_and_counts_the_calls_it_cannot_price() {
        let mut unknown = chat_attributes();
        unknown[2] = text("gen_ai.request.model", "no-such-model");
        unknown[3] = text("gen_ai.response.model", "no-such-model");
        let out = render(vec![
            call(1, 0, chat_attributes()),
            call(2, 0, unknown),
            // A failed call reports no tokens: there is nothing to price.
            call(
                3,
                0,
                vec![
                    text("gen_ai.operation.name", "chat"),
                    text("gen_ai.provider.name", "anthropic"),
                    text("gen_ai.request.model", "claude-sonnet-4-5"),
                    text("error.type", "timeout"),
                ],
            ),
        ]);
        for context in [
            "otel.derived_metrics.genai.cost",
            "otel.derived_metrics.genai.priced_calls",
        ] {
            assert!(out.contains(context), "{context} missing:\n{out}");
        }
        // Per million tokens: 3 USD input, 0.30 cache read, 3.75 cache write, 15 output. 13 plain input tokens,
        // 29 read, 7 written, 17 output: 328.95 millionths of a dollar.
        let total = |dimension: &str| -> f64 {
            let prefix = format!("SET {dimension} =");
            out.lines()
                .filter_map(|line| line.strip_prefix(&prefix))
                .map(|value| value.trim().parse::<f64>().unwrap())
                .sum()
        };
        assert!((total("cost") - 328.95).abs() < 1e-6, "{out}");
        assert_eq!(total("priced"), 1.0);
        assert_eq!(total("unpriced"), 1.0);
    }

    #[test]
    fn charts_a_call_whose_producer_names_one_model() {
        let out = render(vec![call(
            1,
            0,
            vec![
                text("gen_ai.operation.name", "chat"),
                text("gen_ai.response.model", "qwen3.8-flash"),
            ],
        )]);
        for label in [
            "CLABEL 'gen_ai.provider.name' 'unknown' 1",
            "CLABEL 'gen_ai.request.model' 'qwen3.8-flash' 1",
            "CLABEL 'gen_ai.response.model' 'qwen3.8-flash' 1",
        ] {
            assert!(out.contains(label), "{label} missing:\n{out}");
        }
    }

    /// The value of the time-to-first-chunk average chart in `out`; empty when the chart has a gap.
    fn first_chunk_avg(out: &str) -> String {
        let chart = out
            .split("BEGIN ")
            .find(|chart| chart.starts_with("otel.derived_metrics.genai.time_to_first_chunk_avg"))
            .unwrap_or_else(|| panic!("no time to first chunk chart:\n{out}"));
        first(chart, "avg")
    }

    #[test]
    fn charts_the_time_to_first_chunk_of_a_streamed_call() {
        let mut streamed = chat_attributes();
        streamed.push(double("gen_ai.response.time_to_first_chunk", 0.25));
        assert_eq!(first_chunk_avg(&render(vec![call(1, 0, streamed)])), "0.25");

        // A call that was not streamed leaves a gap.
        assert_eq!(
            first_chunk_avg(&render(vec![call(1, 0, chat_attributes())])),
            ""
        );
    }

    #[test]
    fn reports_what_it_did_not_count() {
        let genai = GenAiCalls::new(10, 10).unwrap();
        let negative = vec![
            text("gen_ai.operation.name", "chat"),
            int("gen_ai.usage.input_tokens", -1),
        ];
        let mut export = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    // A nested pair, and a call whose rule fails on a negative count.
                    spans: vec![
                        call(1, 0, chat_attributes()),
                        call(2, 1, chat_attributes()),
                        call(3, 0, negative),
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut export, 100);
        genai.commit(pending, 100);
        assert_eq!(
            genai.uncounted(),
            Uncounted {
                duplicates: 1,
                failed_rules: 1,
                ..Uncounted::default()
            }
        );
    }

    /// The marker of each span of `export`, by the first byte of its span id.
    fn markers(export: &ExportTraceServiceRequest) -> Vec<(u8, Option<String>)> {
        let mut markers = Vec::new();
        for span in &export.resource_spans[0].scope_spans[0].spans {
            let marker = span.attributes.iter().find_map(|attribute| {
                match (
                    attribute.key.as_str(),
                    attribute.value.as_ref()?.value.as_ref()?,
                ) {
                    (annotation::CALL, Value::StringValue(marker)) => Some(marker.clone()),
                    _ => None,
                }
            });
            markers.push((span.span_id[0], marker));
        }
        markers
    }

    fn export_of(spans: Vec<Span>) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    #[test]
    fn marks_each_call_span_as_counted_or_duplicate() {
        let genai = GenAiCalls::new(10, 10).unwrap();
        // A wrapper's span around the SDK's, a span that is no call, one that only claims to be counted, and a tool.
        let mut export = export_of(vec![
            call(1, 0, chat_attributes()),
            call(2, 1, chat_attributes()),
            call(3, 0, vec![text("http.request.method", "POST")]),
            call(4, 0, vec![text(annotation::CALL, annotation::COUNTED)]),
            call(
                5,
                0,
                vec![
                    text("gen_ai.operation.name", "execute_tool"),
                    text("gen_ai.tool.name", "get_weather"),
                ],
            ),
        ]);
        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut export, 100);
        assert_eq!(
            markers(&export),
            vec![
                (1, Some(annotation::DUPLICATE.to_string())),
                (2, Some(annotation::COUNTED.to_string())),
                (3, None),
                (4, None),
                (5, None),
            ]
        );
        let tool: Vec<&str> = export.resource_spans[0].scope_spans[0].spans[4].attributes[2..]
            .iter()
            .map(|attribute| attribute.key.as_str())
            .collect();
        assert_eq!(
            tool,
            vec![annotation::KIND, annotation::TOOL, annotation::OUTCOME]
        );
        let written: Vec<&str> = export.resource_spans[0].scope_spans[0].spans[1].attributes
            [chat_attributes().len()..]
            .iter()
            .map(|attribute| attribute.key.as_str())
            .collect();
        assert_eq!(
            written,
            vec![
                annotation::KIND,
                annotation::CALL,
                annotation::OPERATION,
                annotation::PROVIDER,
                annotation::REQUEST_MODEL,
                annotation::RESPONSE_MODEL,
                annotation::OUTCOME,
                annotation::INPUT_TOKENS,
                annotation::OUTPUT_TOKENS,
                annotation::CACHE_READ_TOKENS,
                annotation::CACHE_WRITE_TOKENS,
                annotation::PRICED,
                annotation::COST_NANO_USD,
            ]
        );
    }

    #[test]
    fn a_call_a_gateway_reports_is_marked_and_not_counted() {
        let genai = GenAiCalls::new(10, 10).unwrap();
        // The application's call span, and below it, in the same trace, the LiteLLM proxy's span of the same call.
        let mut export = export_of(vec![call(1, 0, chat_attributes())]);
        let mut gateway_attributes = chat_attributes();
        gateway_attributes[0] = text("gen_ai.operation.name", "acompletion");
        gateway_attributes.push(text("http.route", "/v1/chat/completions"));
        export.resource_spans[0].scope_spans.push(ScopeSpans {
            scope: Some(InstrumentationScope {
                name: "litellm".into(),
                ..Default::default()
            }),
            spans: vec![call(2, 1, gateway_attributes)],
            ..Default::default()
        });
        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut export, 100);
        let marker = |scope: usize| {
            export.resource_spans[0].scope_spans[scope].spans[0]
                .attributes
                .iter()
                .find(|attribute| attribute.key == annotation::CALL)
                .and_then(|attribute| attribute.value.clone())
        };
        assert_eq!(
            [marker(0), marker(1)],
            [
                Some(AnyValue {
                    value: Some(Value::StringValue(annotation::COUNTED.into()))
                }),
                Some(AnyValue {
                    value: Some(Value::StringValue(annotation::GATEWAY.into()))
                }),
            ]
        );
        genai.commit(pending, 100);
        assert_eq!(
            genai.uncounted(),
            Uncounted {
                gateway_calls: 1,
                ..Uncounted::default()
            }
        );
    }

    #[test]
    fn every_call_agentgateway_reports_is_a_gateways() {
        let genai = GenAiCalls::new(10, 10).unwrap();
        let mut export = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    scope: Some(InstrumentationScope {
                        name: "agentgateway".into(),
                        ..Default::default()
                    }),
                    spans: vec![call(1, 0, chat_attributes())],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut export, 100);
        assert_eq!(
            markers(&export),
            vec![(1, Some(annotation::GATEWAY.to_string()))]
        );
    }

    #[test]
    fn the_litellm_sdk_inside_an_application_is_counted() {
        let genai = GenAiCalls::new(10, 10).unwrap();
        // The proxy's tracer name, but no route served: the SDK making the call itself.
        let mut attributes = chat_attributes();
        attributes[0] = text("gen_ai.operation.name", "acompletion");
        let mut export = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    scope: Some(InstrumentationScope {
                        name: "litellm".into(),
                        ..Default::default()
                    }),
                    spans: vec![call(1, 0, attributes)],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut export, 100);
        assert_eq!(
            markers(&export),
            vec![(1, Some(annotation::COUNTED.to_string()))]
        );
    }

    #[test]
    fn a_discarded_request_counts_when_it_comes_again() {
        let genai = GenAiCalls::new(10, 10).unwrap();
        let mut answered = chat_attributes();
        answered.push(text("gen_ai.response.id", "msg_1"));
        let request = || export_of(vec![call(1, 0, answered.clone())]);

        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut request(), 100);
        genai.discard(pending);

        let mut again = request();
        let mut pending = Pending::default();
        genai.record(&mut pending, "default", &mut again, 100);
        assert_eq!(
            markers(&again),
            vec![(1, Some(annotation::COUNTED.to_string()))]
        );
        genai.commit(pending, 100);
        assert_eq!(genai.uncounted(), Uncounted::default());

        // Stored, the same answer is a duplicate from then on.
        let mut third = request();
        genai.record(&mut Pending::default(), "default", &mut third, 100);
        assert_eq!(
            markers(&third),
            vec![(1, Some(annotation::DUPLICATE.to_string()))]
        );
    }

    /// The part of `out` that belongs to the chart `context`.
    fn chart<'a>(out: &'a str, context: &str) -> &'a str {
        out.split("BEGIN ")
            .find(|chart| chart.starts_with(context))
            .unwrap_or_else(|| panic!("no {context} chart:\n{out}"))
    }

    #[test]
    fn charts_a_tool_call_under_its_name() {
        let mut failed = call(
            2,
            0,
            vec![
                text("openinference.span.kind", "TOOL"),
                text("tool.name", "get_weather"),
            ],
        );
        failed.status = Some(opentelemetry_proto::tonic::trace::v1::Status {
            code: 2,
            ..Default::default()
        });
        let out = render(vec![
            call(
                1,
                0,
                vec![
                    text("gen_ai.operation.name", "execute_tool"),
                    text("gen_ai.tool.name", "get_weather"),
                ],
            ),
            failed,
        ]);
        // Both dialects land in the same series: one tool of one service.
        let calls = chart(&out, "otel.derived_metrics.genai.tool_calls");
        assert_eq!(first(calls, "success"), "1");
        assert_eq!(first(calls, "error"), "1");
        let duration = chart(&out, "otel.derived_metrics.genai.tool_call_duration_avg");
        assert_eq!(first(duration, "avg"), "0.5");
        for label in [
            "CLABEL 'gen_ai.tool.name' 'get_weather' 1",
            "CLABEL 'resource.attributes.service.name' 'assistant' 1",
        ] {
            assert!(out.contains(label), "{label} missing:\n{out}");
        }
        // A tool call is not a model call.
        assert!(!out.contains("otel.derived_metrics.genai.calls"), "{out}");
        assert!(!out.contains("gen_ai.request.model"), "{out}");
    }

    #[test]
    fn charts_a_tool_call_without_a_name_without_the_label() {
        let out = render(vec![call(
            1,
            0,
            vec![text("openinference.span.kind", "TOOL")],
        )]);
        let calls = chart(&out, "otel.derived_metrics.genai.tool_calls");
        assert_eq!(first(calls, "success"), "1");
        assert!(!out.contains("gen_ai.tool.name"), "{out}");
    }

    #[test]
    fn a_model_call_charts_no_tool_call() {
        let out = render(vec![call(1, 0, chat_attributes())]);
        assert!(!out.contains("tool_call"), "{out}");
    }

    #[test]
    fn spans_that_are_not_calls_chart_nothing() {
        let out = render(vec![call(3, 0, vec![text("http.request.method", "POST")])]);
        assert!(!out.contains("genai"), "{out}");
    }
}
