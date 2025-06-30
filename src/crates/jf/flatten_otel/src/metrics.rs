use serde_json::{json, Map as JsonMap, Value as JsonValue};

use opentelemetry_proto::tonic::{
    collector::metrics::v1::ExportMetricsServiceRequest,
    metrics::v1::{
        metric::Data, AggregationTemporality, Gauge, Histogram, HistogramDataPoint, Metric,
        NumberDataPoint, ResourceMetrics, Sum,
    },
};

use crate::{json_from_instrumentation_scope, json_from_key_value_list, json_from_resource};

pub fn flatten_metrics_request(req: &ExportMetricsServiceRequest) -> JsonValue {
    let mut flattened_metrics = Vec::new();

    for resource_metric in &req.resource_metrics {
        flattened_metrics.extend(flatten_resource_metrics(resource_metric));
    }

    JsonValue::Array(flattened_metrics)
}

fn flatten_resource_metrics(resource_metrics: &ResourceMetrics) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();

    for scope_metrics in &resource_metrics.scope_metrics {
        let mut jm = JsonMap::new();

        if let Some(resource) = &resource_metrics.resource {
            json_from_resource(&mut jm, resource);
        }

        // Add scope info
        if let Some(scope) = &scope_metrics.scope {
            json_from_instrumentation_scope(&mut jm, scope);
        }

        // Add individual metrics
        for metric in &scope_metrics.metrics {
            flattened_metrics.push(flatten_metric(metric));
        }
    }

    flattened_metrics
}

fn flatten_metric(metric: &Metric) -> JsonValue {
    let mut jm = JsonMap::new();

    // Add metric metadata
    jm.insert(
        "metric.name".to_string(),
        JsonValue::String(metric.name.clone()),
    );
    jm.insert(
        "metric.description".to_string(),
        JsonValue::String(metric.description.clone()),
    );
    jm.insert(
        "metric.unit".to_string(),
        JsonValue::String(metric.unit.clone()),
    );

    for (key, value) in json_from_key_value_list(&metric.metadata) {
        jm.insert(format!("metric.metadata.{}", key), value);
    }

    if let Some(data) = &metric.data {
        let v = match data {
            Data::Gauge(gauge) => flatten_gauge(gauge),
            Data::Sum(sum) => flatten_sum(sum),
            Data::Histogram(histogram) => flatten_histogram(histogram),
            Data::ExponentialHistogram(_) | Data::Summary(_) => {
                unreachable!();
            }
        };

        jm.insert("dfjlkdasjf".to_string(), JsonValue::Array(v));
    }

    JsonValue::Object(jm)
}

fn flatten_gauge(gauge: &Gauge) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();

    for data_point in &gauge.data_points {
        let mut jm = JsonMap::new();

        jm.insert(
            "metric.type".to_string(),
            JsonValue::String("gauge".to_string()),
        );
        let v = flatten_number_data_point(data_point);
        jm.insert("gvd.dp".to_string(), v);
        flattened_metrics.push(JsonValue::Object(jm));
    }

    flattened_metrics
}

fn flatten_sum(sum: &Sum) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();

    let aggregation_temporality = match sum.aggregation_temporality {
        x if x == AggregationTemporality::Unspecified as i32 => "unspecified",
        x if x == AggregationTemporality::Delta as i32 => "delta",
        x if x == AggregationTemporality::Cumulative as i32 => "cumulative",
        _ => "unknown",
    };

    for data_point in &sum.data_points {
        let mut jm = JsonMap::new();

        jm.insert(
            "metric.type".to_string(),
            JsonValue::String("sum".to_string()),
        );
        jm.insert(
            "metric.aggregation_temporality".to_string(),
            JsonValue::String(aggregation_temporality.to_string()),
        );
        jm.insert(
            "metric.is_monotonic".to_string(),
            JsonValue::Bool(sum.is_monotonic),
        );
        let v = flatten_number_data_point(data_point);
        jm.insert("jsldfkjsdfl".to_string(), v);
        flattened_metrics.push(JsonValue::Object(jm));
    }

    flattened_metrics
}

fn flatten_histogram(histogram: &Histogram) -> Vec<JsonValue> {
    todo!();
}

fn flatten_number_data_point(data_point: &NumberDataPoint) -> JsonValue {
    let mut jm = JsonMap::new();

    for (key, value) in json_from_key_value_list(&data_point.attributes) {
        jm.insert(format!("metric.attributes.{}", key), value);
    }

    // Add timestamps
    jm.insert(
        "metric.start_time_unix_nano".to_string(),
        JsonValue::Number(data_point.start_time_unix_nano.into()),
    );
    jm.insert(
        "metric.time_unix_nano".to_string(),
        JsonValue::Number(data_point.time_unix_nano.into()),
    );

    // Add value
    if let Some(value) = &data_point.value {
        match value {
            opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d) => {
                jm.insert("metric.value".to_string(), json!(d));
            }
            opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(i) => {
                jm.insert("metric.value".to_string(), JsonValue::Number((*i).into()));
            }
        }
    }

    // Add exemplars count if present
    if !data_point.exemplars.is_empty() {
        jm.insert(
            "metric.exemplars_count".to_string(),
            JsonValue::Number(data_point.exemplars.len().into()),
        );
    }

    // Add flags
    if data_point.flags != 0 {
        jm.insert(
            "metric.flags".to_string(),
            JsonValue::Number(data_point.flags.into()),
        );
    }

    JsonValue::Object(jm)
}

fn flatten_histogram_data_point(data_point: &HistogramDataPoint) -> JsonValue {
    let mut jm = JsonMap::new();

    // Add attributes
    for (key, value) in json_from_key_value_list(&data_point.attributes) {
        jm.insert(format!("metric.attributes.{}", key), value);
    }

    // Add timestamps
    jm.insert(
        "metric.start_time_unix_nano".to_string(),
        JsonValue::Number(data_point.start_time_unix_nano.into()),
    );
    jm.insert(
        "metric.time_unix_nano".to_string(),
        JsonValue::Number(data_point.time_unix_nano.into()),
    );

    // Add histogram values
    jm.insert(
        "metric.count".to_string(),
        JsonValue::Number(data_point.count.into()),
    );
    if let Some(sum) = data_point.sum {
        jm.insert("metric.sum".to_string(), json!(sum));
    }
    if let Some(min) = data_point.min {
        jm.insert("metric.min".to_string(), json!(min));
    }
    if let Some(max) = data_point.max {
        jm.insert("metric.max".to_string(), json!(max));
    }

    // Add bucket counts
    for (i, bucket_count) in data_point.bucket_counts.iter().enumerate() {
        jm.insert(
            format!("metric.bucket_counts.{}", i),
            JsonValue::Number((*bucket_count).into()),
        );
    }

    // Add explicit bounds
    for (i, bound) in data_point.explicit_bounds.iter().enumerate() {
        jm.insert(format!("metric.explicit_bounds.{}", i), json!(bound));
    }

    // Add exemplars count if present
    if !data_point.exemplars.is_empty() {
        jm.insert(
            "metric.exemplars_count".to_string(),
            JsonValue::Number(data_point.exemplars.len().into()),
        );
    }

    // Add flags
    if data_point.flags != 0 {
        jm.insert(
            "metric.flags".to_string(),
            JsonValue::Number(data_point.flags.into()),
        );
    }

    JsonValue::Object(jm)
}
