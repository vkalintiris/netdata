//! The normalized call, written onto the span it came from.
//!
//! A stored span that a rule recognised as a model call carries its [`Call`] under [`PREFIX`], in one vocabulary
//! whatever dialect the producer speaks, and says whether the call counted. A query that lists the spans marked
//! [`COUNTED`] lists the calls the charts count; a call a gateway reported is marked [`GATEWAY`] instead. A span recognised as one run of a tool carries its [`ToolCall`]
//! the same way; [`KIND`] tells the two apart.

use crate::{Call, Price, ToolCall};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::trace::v1::Span;

/// The namespace of the attributes this module writes. Where spans are annotated, no attribute a sender puts under
/// it is stored.
pub const PREFIX: &str = "netdata.genai.";

/// [`KIND_CALL`] or [`KIND_TOOL`], on every span a rule recognised.
pub const KIND: &str = "netdata.genai.kind";
pub const KIND_CALL: &str = "call";
pub const KIND_TOOL: &str = "tool";

/// [`COUNTED`], [`DUPLICATE`] or [`GATEWAY`], on every span that is a model call.
pub const CALL: &str = "netdata.genai.call";
/// The span is the one its call counts by.
pub const COUNTED: &str = "counted";
/// The span is the second span of a call that another span counts.
pub const DUPLICATE: &str = "duplicate";
/// A gateway reported the call while forwarding it. The charts leave such calls out: the application behind the
/// gateway may report the same call, and the gateway's own view shows what the gateway handled.
pub const GATEWAY: &str = "gateway";

/// The tool's name, on a tool span whose producer names the tool.
pub const TOOL: &str = "netdata.genai.tool";
pub const OPERATION: &str = "netdata.genai.operation";
pub const PROVIDER: &str = "netdata.genai.provider";
pub const REQUEST_MODEL: &str = "netdata.genai.request_model";
pub const RESPONSE_MODEL: &str = "netdata.genai.response_model";
/// [`SUCCESS`] or [`ERROR`].
pub const OUTCOME: &str = "netdata.genai.outcome";
pub const SUCCESS: &str = "success";
pub const ERROR: &str = "error";
pub const INPUT_TOKENS: &str = "netdata.genai.input_tokens";
pub const OUTPUT_TOKENS: &str = "netdata.genai.output_tokens";
pub const CACHE_READ_TOKENS: &str = "netdata.genai.cache_read_tokens";
pub const CACHE_WRITE_TOKENS: &str = "netdata.genai.cache_write_tokens";
pub const REASONING_TOKENS: &str = "netdata.genai.reasoning_tokens";
/// Seconds.
pub const TIME_TO_FIRST_CHUNK: &str = "netdata.genai.time_to_first_chunk";
/// [`YES`] or [`NO`], on every model call that reports tokens and that no gateway reported: whether the price table
/// knows the call's model.
pub const PRICED: &str = "netdata.genai.priced";
pub const YES: &str = "yes";
pub const NO: &str = "no";
/// What a priced call cost, in billionths of a US dollar: a whole number, so that a query can add it up.
pub const COST_NANO_USD: &str = "netdata.genai.cost_nano_usd";

/// Remove the attributes of `span` under [`PREFIX`]: the namespace holds only what [`annotate`] writes.
pub fn strip(span: &mut Span) {
    strip_under(&mut span.attributes, PREFIX);
}

/// Remove from `attributes` what is stored under a key that starts with `prefix`. The keys inside a nested value
/// are stored joined to its own key with a dot, so a leading part of the prefix can be an attribute's key and the
/// rest be spelled inside its value.
fn strip_under(attributes: &mut Vec<KeyValue>, prefix: &str) {
    attributes.retain_mut(|attribute| {
        if attribute.key.starts_with(prefix) {
            return false;
        }
        let rest = prefix
            .strip_prefix(attribute.key.as_str())
            .and_then(|rest| rest.strip_prefix('.'));
        if let Some(rest) = rest
            && let Some(Value::KvlistValue(nested)) = attribute
                .value
                .as_mut()
                .and_then(|value| value.value.as_mut())
        {
            strip_under(&mut nested.values, rest);
        }
        true
    });
}

/// Write `call` onto `span`, the span it was normalized from; `counted` is whether the call counts by this span,
/// and is ignored for a call a gateway reported. `price` is what the price table says of the call, if it was asked.
/// A count or time the call does not have is left out; a provider or model it does not have is named as the
/// charts name it.
pub fn annotate(span: &mut Span, call: &Call, counted: bool, price: Option<Price>) {
    let mut add = |key: &str, value: Value| {
        span.attributes.push(KeyValue {
            key: key.to_string(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        });
    };
    let text = |value: &str| Value::StringValue(value.to_string());

    add(KIND, text(KIND_CALL));
    let status = if call.gateway {
        GATEWAY
    } else if counted {
        COUNTED
    } else {
        DUPLICATE
    };
    add(CALL, text(status));
    add(OPERATION, text(&call.operation));
    add(PROVIDER, text(call.provider_label()));
    add(REQUEST_MODEL, text(call.request_model_label()));
    add(RESPONSE_MODEL, text(call.response_model_label()));
    add(OUTCOME, text(if call.error { ERROR } else { SUCCESS }));
    let tokens = call.tokens;
    for (key, count) in [
        (INPUT_TOKENS, tokens.input),
        (OUTPUT_TOKENS, tokens.output),
        (CACHE_READ_TOKENS, tokens.cache_read),
        (CACHE_WRITE_TOKENS, tokens.cache_write),
        (REASONING_TOKENS, tokens.reasoning),
    ] {
        if let Some(count) = count {
            add(
                key,
                Value::IntValue(i64::try_from(count).unwrap_or(i64::MAX)),
            );
        }
    }
    if let Some(seconds) = call.time_to_first_chunk_seconds {
        add(TIME_TO_FIRST_CHUNK, Value::DoubleValue(seconds));
    }
    match price {
        Some(Price::Usd(usd)) => {
            add(PRICED, text(YES));
            add(COST_NANO_USD, Value::IntValue((usd * 1e9).round() as i64));
        }
        Some(Price::Unknown) => add(PRICED, text(NO)),
        None => {}
    }
}

/// Write `tool` onto `span`, the span it was normalized from. A tool the producer does not name gets no name, as
/// on the charts.
pub fn annotate_tool(span: &mut Span, tool: &ToolCall) {
    let mut add = |key: &str, value: &str| {
        span.attributes.push(KeyValue {
            key: key.to_string(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(value.to_string())),
            }),
            ..Default::default()
        });
    };

    add(KIND, KIND_TOOL);
    if let Some(name) = &tool.name {
        add(TOOL, name);
    }
    add(OUTCOME, if tool.error { ERROR } else { SUCCESS });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tokens;

    fn attribute(key: &str, value: Value) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        }
    }

    fn text(key: &str, value: &str) -> KeyValue {
        attribute(key, Value::StringValue(value.to_string()))
    }

    #[test]
    fn strips_the_namespace_spelled_as_nested_values() {
        use opentelemetry_proto::tonic::common::v1::KeyValueList;

        let list = |values: Vec<KeyValue>| Value::KvlistValue(KeyValueList { values });
        let mut span = Span {
            attributes: vec![
                // All of these flatten to keys under `netdata.genai.`.
                attribute(
                    "netdata",
                    list(vec![text("genai.call", COUNTED), text("other", "kept")]),
                ),
                attribute(
                    "netdata",
                    list(vec![attribute("genai", list(vec![text("call", COUNTED)]))]),
                ),
                attribute("netdata.genai", list(vec![text("call", COUNTED)])),
                text("netdata.genaix", "kept"),
            ],
            ..Default::default()
        };
        strip(&mut span);
        assert_eq!(
            span.attributes,
            vec![
                // The emptied values stay: their own keys are outside the namespace.
                attribute("netdata", list(vec![text("other", "kept")])),
                attribute("netdata", list(vec![attribute("genai", list(vec![]))])),
                attribute("netdata.genai", list(vec![])),
                text("netdata.genaix", "kept"),
            ]
        );
    }

    #[test]
    fn writes_what_the_call_has_and_nothing_a_sender_put_in_the_namespace() {
        let mut span = Span {
            attributes: vec![
                text("llm.model_name", "gpt-5"),
                text(CALL, COUNTED),
                text("netdata.genai.anything", "x"),
            ],
            ..Default::default()
        };
        let call = Call {
            operation: "chat".into(),
            provider: Some("openai".into()),
            request_model: Some("gpt-5".into()),
            response_model: None,
            tokens: Tokens {
                input: Some(49),
                output: Some(17),
                cache_read: Some(29),
                cache_write: None,
                reasoning: None,
            },
            error: true,
            end_time_unix_nano: 0,
            duration_seconds: 0.0,
            time_to_first_chunk_seconds: Some(0.25),
            response_id: None,
            gateway: false,
        };
        strip(&mut span);
        annotate(&mut span, &call, false, Some(Price::Usd(0.000_017_025)));
        assert_eq!(
            span.attributes,
            vec![
                text("llm.model_name", "gpt-5"),
                text(KIND, KIND_CALL),
                text(CALL, DUPLICATE),
                text(OPERATION, "chat"),
                text(PROVIDER, "openai"),
                text(REQUEST_MODEL, "gpt-5"),
                text(RESPONSE_MODEL, crate::UNKNOWN),
                text(OUTCOME, ERROR),
                attribute(INPUT_TOKENS, Value::IntValue(49)),
                attribute(OUTPUT_TOKENS, Value::IntValue(17)),
                attribute(CACHE_READ_TOKENS, Value::IntValue(29)),
                attribute(TIME_TO_FIRST_CHUNK, Value::DoubleValue(0.25)),
                text(PRICED, YES),
                attribute(COST_NANO_USD, Value::IntValue(17_025)),
            ]
        );

        let mut unpriced = Span::default();
        annotate(&mut unpriced, &call, true, Some(Price::Unknown));
        assert_eq!(unpriced.attributes.last(), Some(&text(PRICED, NO)));

        let mut unasked = Span::default();
        annotate(&mut unasked, &call, true, None);
        assert_eq!(
            unasked.attributes.last(),
            Some(&attribute(TIME_TO_FIRST_CHUNK, Value::DoubleValue(0.25)))
        );
    }

    #[test]
    fn writes_a_tool_call_with_or_without_its_name() {
        let tool = |name: Option<&str>, error| ToolCall {
            name: name.map(str::to_string),
            error,
            end_time_unix_nano: 0,
            duration_seconds: 0.0,
        };

        let mut named = Span::default();
        annotate_tool(&mut named, &tool(Some("get_weather"), true));
        assert_eq!(
            named.attributes,
            vec![
                text(KIND, KIND_TOOL),
                text(TOOL, "get_weather"),
                text(OUTCOME, ERROR),
            ]
        );

        let mut unnamed = Span::default();
        annotate_tool(&mut unnamed, &tool(None, false));
        assert_eq!(
            unnamed.attributes,
            vec![text(KIND, KIND_TOOL), text(OUTCOME, SUCCESS)]
        );
    }
}
