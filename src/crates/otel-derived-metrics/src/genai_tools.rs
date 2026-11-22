//! GenAI tool charts from spans: tool calls and their duration, per service and tool.
//!
//! `otel-genai` turns each span that describes one run of a tool into a normalized tool call, whatever dialect its
//! producer speaks; [`crate::genai_calls`] finds them while it looks for model calls, and this set counts them. A
//! tool span carries no model or provider, so the labels are the service's and the tool's name; a producer that does
//! not name the tool is charted without the name. A tool call counts at its end time, weighted like a request by its
//! span's sampling threshold.

use crate::genai_calls::DURATION_BOUNDS;
use crate::service::service_settings;
use crate::shared_aggregator::SharedAggregator;
use netdata_plugin_metrics::{
    Batch, ChartDescriptor, ChartType, Counter, Histogram, Labels, MetricSet, MetricSetBuilder,
    Stats, columns,
};
use otel_genai::ToolCall;

/// Label sets (service and tool) over this share the overflow entity.
pub(crate) const MAX_SERIES: usize = 1000;

columns! {
    /// How a tool call ended.
    enum Outcome {
        Success = "success",
        Error = "error",
    }
}

/// One tool call among the spans of an export request, not yet counted.
pub(crate) struct PendingTool {
    pub(crate) service: Labels,
    pub(crate) tool: ToolCall,
    /// The span's adjusted count.
    pub(crate) weight: f64,
}

pub(crate) struct GenAiTools {
    metric_set: MetricSet,
    aggregator: SharedAggregator,
    calls: Counter<Outcome>,
    duration: Histogram,
}

impl GenAiTools {
    pub(crate) fn new(max_series: usize) -> Result<Self, netdata_plugin_metrics::Error> {
        let mut builder = MetricSetBuilder::new(service_settings(max_series))?;
        let calls = builder.counter::<Outcome>(ChartDescriptor {
            title: "GenAI tool calls".into(),
            units: "calls/s".into(),
            family: "genai".into(),
            context: "otel.derived_metrics.genai.tool_calls".into(),
            chart_type: ChartType::Stacked,
            priority: 60_109,
        })?;
        // Charts `otel.derived_metrics.genai.tool_call_duration_avg`, `_max` and `_histogram`. The bounds are those
        // of the model calls: a tool that calls a service or a model takes as long as one.
        let duration = builder.histogram(
            DURATION_BOUNDS,
            ChartDescriptor {
                title: "GenAI tool call duration".into(),
                units: "seconds".into(),
                family: "genai".into(),
                context: "otel.derived_metrics.genai.tool_call_duration".into(),
                chart_type: ChartType::Line,
                priority: 60_110,
            },
            "calls/s".into(),
        )?;
        let metric_set = builder.build()?;
        let aggregator = SharedAggregator::new(&metric_set);
        Ok(Self {
            metric_set,
            aggregator,
            calls,
            duration,
        })
    }

    /// Count `pending` at `now` (Unix seconds): its spans are durable.
    pub(crate) fn commit(&self, pending: Vec<PendingTool>, now: u64) {
        if pending.is_empty() {
            return;
        }
        let mut batch = Batch::new(&self.metric_set);
        for PendingTool {
            service,
            tool,
            weight,
        } in pending
        {
            let mut labels = service;
            if let Some(name) = &tool.name {
                labels.insert("gen_ai.tool.name", name);
            }
            let outcome = if tool.error {
                Outcome::Error
            } else {
                Outcome::Success
            };
            let mut entity = batch.entity(labels);
            let mut event = entity.record(tool.end_time_unix_nano / 1_000_000_000);
            event.add(self.calls, outcome, weight);
            event.observe(self.duration, tool.duration_seconds, weight);
        }
        self.aggregator.merge(batch, now);
    }

    /// Add to `chunks` every window whose allowed lateness has passed by `now` (Unix seconds).
    pub(crate) fn emit(&self, now: u64, chunks: &mut Vec<Vec<u8>>) -> Stats {
        self.aggregator.emit(now, chunks)
    }
}
