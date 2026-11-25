//! The normalized call, written onto the span it came from.
//!
//! A stored span that a rule recognised as a model call carries its [`Call`] under [`PREFIX`], in one vocabulary
//! whatever dialect the producer speaks, and says whether the call counted. A query that lists the spans marked
//! [`COUNTED`] lists the calls the charts count.

use crate::Call;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::trace::v1::Span;

/// The namespace of the attributes this module writes. No attribute a sender puts under it is stored.
pub const PREFIX: &str = "netdata.genai.";

/// [`COUNTED`] or [`DUPLICATE`], on every span that is a model call.
pub const CALL: &str = "netdata.genai.call";
/// The span is the one its call counts by.
pub const COUNTED: &str = "counted";
/// The span is the second span of a call that another span counts.
pub const DUPLICATE: &str = "duplicate";

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

/// Remove the attributes of `span` under [`PREFIX`]: the namespace holds only what [`annotate`] writes.
pub fn strip(span: &mut Span) {
    span.attributes
        .retain(|attribute| !attribute.key.starts_with(PREFIX));
}

/// Write `call` onto `span`, the span it was normalized from; `counted` is whether the call counts by this span.
/// A value the call does not have is left out.
pub fn annotate(span: &mut Span, call: &Call, counted: bool) {
    let mut add = |key: &str, value: Value| {
        span.attributes.push(KeyValue {
            key: key.to_string(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        });
    };
    let text = |value: &str| Value::StringValue(value.to_string());

    add(CALL, text(if counted { COUNTED } else { DUPLICATE }));
    add(OPERATION, text(&call.operation));
    for (key, value) in [
        (PROVIDER, &call.provider),
        (REQUEST_MODEL, &call.request_model),
        (RESPONSE_MODEL, &call.response_model),
    ] {
        if let Some(value) = value {
            add(key, text(value));
        }
    }
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
        };
        strip(&mut span);
        annotate(&mut span, &call, false);
        assert_eq!(
            span.attributes,
            vec![
                text("llm.model_name", "gpt-5"),
                text(CALL, DUPLICATE),
                text(OPERATION, "chat"),
                text(PROVIDER, "openai"),
                text(REQUEST_MODEL, "gpt-5"),
                text(OUTCOME, ERROR),
                attribute(INPUT_TOKENS, Value::IntValue(49)),
                attribute(OUTPUT_TOKENS, Value::IntValue(17)),
                attribute(CACHE_READ_TOKENS, Value::IntValue(29)),
                attribute(TIME_TO_FIRST_CHUNK, Value::DoubleValue(0.25)),
            ]
        );
    }
}
