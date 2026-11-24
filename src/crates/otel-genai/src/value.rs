//! The VRL view of an OTLP record: what a rule reads as its event.
//!
//! A span becomes an object with `scope.name`, `scope.version`, `name`, `kind`, `status_code` and `attributes` (one
//! entry per attribute key, last value wins). Timestamps are left out: the host derives the call's end time and
//! duration itself, so rules only deal with the producer's vocabulary.

use opentelemetry_proto::tonic::common::v1::any_value::Value as Av;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::trace::v1::Span;
use vrl::value::{KeyString, ObjectMap, Value};

pub(crate) fn span(scope: Option<&InstrumentationScope>, span: &Span) -> Value {
    let mut root = ObjectMap::new();
    root.insert("scope".into(), scope_object(scope));
    root.insert("name".into(), Value::from(span.name.as_str()));
    root.insert("kind".into(), Value::Integer(i64::from(span.kind)));
    let status_code = span.status.as_ref().map_or(0, |status| status.code);
    root.insert("status_code".into(), Value::Integer(i64::from(status_code)));
    root.insert("attributes".into(), attributes(&span.attributes));
    Value::Object(root)
}

fn scope_object(scope: Option<&InstrumentationScope>) -> Value {
    let mut object = ObjectMap::new();
    let (name, version) = scope.map_or(("", ""), |scope| {
        (scope.name.as_str(), scope.version.as_str())
    });
    object.insert("name".into(), Value::from(name));
    object.insert("version".into(), Value::from(version));
    Value::Object(object)
}

fn attributes(attributes: &[KeyValue]) -> Value {
    let mut object = ObjectMap::new();
    for attribute in attributes {
        object.insert(
            KeyString::from(attribute.key.as_str()),
            any_value(attribute.value.as_ref()),
        );
    }
    Value::Object(object)
}

fn any_value(value: Option<&AnyValue>) -> Value {
    match value.and_then(|value| value.value.as_ref()) {
        Some(Av::StringValue(string)) => Value::from(string.as_str()),
        Some(Av::BoolValue(boolean)) => Value::Boolean(*boolean),
        Some(Av::IntValue(integer)) => Value::Integer(*integer),
        // NaN has no VRL value; a rule sees it as absent.
        Some(Av::DoubleValue(double)) if double.is_nan() => Value::Null,
        Some(Av::DoubleValue(double)) => Value::from_f64_or_zero(*double),
        Some(Av::ArrayValue(array)) => Value::Array(
            array
                .values
                .iter()
                .map(|value| any_value(Some(value)))
                .collect(),
        ),
        Some(Av::KvlistValue(list)) => attributes(&list.values),
        Some(Av::BytesValue(bytes)) => Value::Bytes(bytes.clone().into()),
        // A profiles dictionary index: no dictionary outside profiles, so no value.
        Some(Av::StringValueStrindex(_)) | None => Value::Null,
    }
}
