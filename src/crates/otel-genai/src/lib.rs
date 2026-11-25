//! GenAI normalization: turns a span that describes one model call into a [`Call`], whatever dialect its producer
//! speaks.
//!
//! The rules are VRL programs under `rules/`, compiled into the binary and compiled by VRL once, in
//! [`Normalizer::new`]. Rules are tried in order and the first one that returns an object wins; a rule returns `null`
//! for a span it does not describe, and `false` for one of its producer's spans that is not a call, so no later rule
//! counts it. A rule only maps the producer's vocabulary: the host supplies the call's end time and duration from the
//! span itself, unless the rule knows a better duration. A rule also passes on the producer's time to the first chunk
//! of a streamed answer, when the producer reports one: in seconds, or as the instant of the first chunk, which the
//! host measures from the span's start.
//!
//! Token counts follow the OpenTelemetry GenAI conventions: input includes cache reads and writes, output includes
//! reasoning, and the cache and reasoning counts are parts of those totals, never additions to them.
//!
//! A second list of rules, under `rules/tools/`, turns a span that describes one run of a tool into a [`ToolCall`].
//! A tool rule maps the tool's name and whether the run failed; the host supplies the times. A span is a model call
//! or a tool call, never both: the two lists read different markers.

pub mod annotation;
mod gate;
mod pairs;
mod value;

use gate::Gate;
pub use pairs::{Candidate, Pairs, Remembered};

use opentelemetry_proto::tonic::common::v1::{InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::trace::v1::Span;
use vrl::compiler::state::RuntimeState;
use vrl::compiler::{Context, Function, Program, TargetValue, TimeZone};
use vrl::value::{ObjectMap, Secrets, Value};

struct RuleSource {
    name: &'static str,
    source: &'static str,
    /// Admits every record the rule can match, so records that concern no rule never reach VRL; the corpus tests
    /// check the gates against the rules.
    gate: Gate,
}

/// The rules, in the order they are tried: producer rules first, the conventions last, as the fallback for any
/// producer that follows them.
const RULES: &[RuleSource] = &[
    RuleSource {
        name: "litellm",
        source: include_str!("../rules/litellm.vrl"),
        gate: Gate::scope_in(&["litellm"]).and_attribute_in(
            "gen_ai.operation.name",
            &[
                "chat",
                "completion",
                "acompletion",
                "text_completion",
                "atext_completion",
                "embedding",
                "aembedding",
                "embeddings",
            ],
        ),
    },
    RuleSource {
        name: "anthropic_raw_input",
        source: include_str!("../rules/anthropic_raw_input.vrl"),
        // OpenLIT's error span has no attributes: the scope is all there is to go by.
        gate: Gate::scope_in(&[
            "openlit.instrumentation.anthropic",
            "@traceloop/instrumentation-anthropic",
        ]),
    },
    RuleSource {
        name: "openinference",
        source: include_str!("../rules/openinference.vrl"),
        gate: Gate::attribute_in("openinference.span.kind", &["LLM", "EMBEDDING"]),
    },
    RuleSource {
        name: "langfuse",
        source: include_str!("../rules/langfuse.vrl"),
        gate: Gate::scope_in(&["langfuse-sdk"])
            .and_attribute_in("langfuse.observation.type", &["generation", "embedding"]),
    },
    RuleSource {
        name: "mlflow",
        source: include_str!("../rules/mlflow.vrl"),
        gate: Gate::scope_in(&["mlflow.tracing.provider"]).and_attribute_in(
            "mlflow.spanType",
            &["\"CHAT_MODEL\"", "\"LLM\"", "\"EMBEDDING\""],
        ),
    },
    RuleSource {
        name: "logfire",
        source: include_str!("../rules/logfire.vrl"),
        gate: Gate::scope_in(&["logfire.openai", "logfire.anthropic"])
            .and_attribute_in("gen_ai.operation.name", CALL_OPERATIONS),
    },
    RuleSource {
        name: "langsmith",
        source: include_str!("../rules/langsmith.vrl"),
        // Every LangSmith run carries an operation; the rule drops the non-call ones the conventions would count.
        gate: Gate::scope_in(&["langsmith"]),
    },
    RuleSource {
        name: "logfire_openai_agents",
        source: include_str!("../rules/logfire_openai_agents.vrl"),
        gate: Gate::scope_in(&["logfire.openai_agents"]),
    },
    RuleSource {
        name: "laminar",
        source: include_str!("../rules/laminar.vrl"),
        gate: Gate::scope_in(&["lmnr.tracer"]).and_attribute_in("lmnr.span.type", &["LLM"]),
    },
    RuleSource {
        name: "datadog",
        source: include_str!("../rules/datadog.vrl"),
        gate: Gate::attribute_in("gen_ai.operation.name", &["llm", "embedding"]),
    },
    RuleSource {
        name: "conventions",
        source: include_str!("../rules/conventions.vrl"),
        gate: Gate::attribute_in("gen_ai.operation.name", CALL_OPERATIONS),
    },
];

/// The tool rules, in the order they are tried. A span that carries both markers gives the same name either way.
const TOOL_RULES: &[RuleSource] = &[
    RuleSource {
        name: "tools/openinference",
        source: include_str!("../rules/tools/openinference.vrl"),
        gate: Gate::attribute_in("openinference.span.kind", &["TOOL"]),
    },
    RuleSource {
        name: "tools/conventions",
        source: include_str!("../rules/tools/conventions.vrl"),
        gate: Gate::attribute_in("gen_ai.operation.name", &["execute_tool"]),
    },
];

/// The call operations the charts count: agent and tool operations repeat the totals of the calls inside them.
pub const CALL_OPERATIONS: &[&str] = &["chat", "text_completion", "generate_content", "embeddings"];

/// One model call, in the normalized vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// One of [`CALL_OPERATIONS`].
    pub operation: String,
    /// `None` when the producer does not report it; [`Self::provider_label`] is what the charts show.
    pub provider: Option<String>,
    /// `None` when the producer does not report it; [`Self::request_model_label`] is what the charts show.
    pub request_model: Option<String>,
    /// `None` when the producer does not report it or no answer came; [`Self::response_model_label`] is what the
    /// charts show.
    pub response_model: Option<String>,
    pub tokens: Tokens,
    pub error: bool,
    /// When the call ended: the second it counts in.
    pub end_time_unix_nano: u64,
    pub duration_seconds: f64,
    /// For a streamed answer, the time from the start of the call to the first chunk of the answer, as the producer
    /// measured it; `None` when the producer does not report it.
    pub time_to_first_chunk_seconds: Option<f64>,
    /// The provider's id of the answer, which identifies one call across records.
    pub response_id: Option<String>,
}

/// The label value of what the producer of a call did not report. A missing label cannot be filtered on; this
/// value can.
pub const UNKNOWN: &str = "unknown";

impl Call {
    /// The provider, as a label of the charts and on the stored span.
    pub fn provider_label(&self) -> &str {
        self.provider.as_deref().unwrap_or(UNKNOWN)
    }

    /// The requested model, as a label of the charts and on the stored span. A producer that reports one model
    /// name and not which model was requested reports, in every capture, the name that was asked for: that name
    /// stands in.
    pub fn request_model_label(&self) -> &str {
        self.request_model
            .as_deref()
            .or(self.response_model.as_deref())
            .unwrap_or(UNKNOWN)
    }

    /// The answering model, as a label of the charts and on the stored span. The requested model does not stand
    /// in: a call can end without an answer.
    pub fn response_model_label(&self) -> &str {
        self.response_model.as_deref().unwrap_or(UNKNOWN)
    }
}

/// Token counts of one call; `None` is absent, not zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// All input, including cache reads and writes.
    pub input: Option<u64>,
    /// All output, including reasoning.
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
}

/// One run of a tool, in the normalized vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// `None` when the producer does not name the tool.
    pub name: Option<String>,
    pub error: bool,
    /// When the run ended: the second it counts in.
    pub end_time_unix_nano: u64,
    pub duration_seconds: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GenAI rule {rule} does not compile: {message}")]
    Compile { rule: &'static str, message: String },
}

/// Why a rule produced no call for a record it matched.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RuleError {
    #[error("GenAI rule {rule} failed: {message}")]
    Runtime { rule: &'static str, message: String },
    #[error("GenAI rule {rule} returned a bad {field}: {value}")]
    Output {
        rule: &'static str,
        field: &'static str,
        value: String,
    },
}

/// The times of a call that the host reads from the span itself.
#[derive(Debug, Clone, Copy)]
struct Times {
    start_unix_nano: u64,
    end_unix_nano: u64,
}

struct Rule {
    name: &'static str,
    gate: Gate,
    program: Program,
}

/// The compiled rules.
pub struct Normalizer {
    rules: Vec<Rule>,
    tool_rules: Vec<Rule>,
}

impl Normalizer {
    /// # Errors
    ///
    /// A rule that does not compile: a defect of this crate, caught by its tests.
    pub fn new() -> Result<Self, Error> {
        let functions = functions();
        Ok(Self {
            rules: compile(RULES, &functions)?,
            tool_rules: compile(TOOL_RULES, &functions)?,
        })
    }

    /// The call `span` describes, if a rule matches it.
    ///
    /// # Errors
    ///
    /// The first rule that matched failed or returned a value of the wrong type; later rules are not tried.
    pub fn span(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
    ) -> Result<Option<Call>, RuleError> {
        self.span_gated(scope, span, true)
    }

    /// The tool call `span` describes, if a tool rule matches it.
    ///
    /// # Errors
    ///
    /// As for [`Self::span`].
    pub fn tool_span(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
    ) -> Result<Option<ToolCall>, RuleError> {
        self.tool_span_gated(scope, span, true)
    }

    /// Whether any tool rule's gate admits `span`: when not, [`Self::tool_span`] never runs VRL for it.
    pub fn is_tool_candidate(&self, scope: Option<&InstrumentationScope>, span: &Span) -> bool {
        self.tool_rules
            .iter()
            .any(|rule| rule.gate.admits(scope, &span.attributes))
    }

    /// Whether any rule's gate admits `span`: when not, [`Self::span`] never runs VRL for it.
    pub fn is_candidate(&self, scope: Option<&InstrumentationScope>, span: &Span) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.gate.admits(scope, &span.attributes))
    }

    fn span_gated(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
        gated: bool,
    ) -> Result<Option<Call>, RuleError> {
        let start = span.start_time_unix_nano;
        let times = Times {
            start_unix_nano: start,
            // A span that ends before it starts has no duration.
            end_unix_nano: span.end_time_unix_nano.max(start),
        };
        let matched = resolve(
            &self.rules,
            scope,
            &span.attributes,
            || value::span(scope, span),
            gated,
        )?;
        matched
            .map(|(rule, fields)| call(rule, &fields, times))
            .transpose()
    }

    fn tool_span_gated(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
        gated: bool,
    ) -> Result<Option<ToolCall>, RuleError> {
        let matched = resolve(
            &self.tool_rules,
            scope,
            &span.attributes,
            || value::span(scope, span),
            gated,
        )?;
        matched
            .map(|(rule, fields)| tool_call(rule, &fields, span))
            .transpose()
    }
}

fn compile(sources: &[RuleSource], functions: &[Box<dyn Function>]) -> Result<Vec<Rule>, Error> {
    let mut rules = Vec::with_capacity(sources.len());
    for rule in sources {
        let compiled = vrl::compiler::compile(rule.source, functions).map_err(|diagnostics| {
            Error::Compile {
                rule: rule.name,
                message: vrl::diagnostic::Formatter::new(rule.source, diagnostics).to_string(),
            }
        })?;
        rules.push(Rule {
            name: rule.name,
            gate: rule.gate,
            program: compiled.program,
        });
    }
    Ok(rules)
}

/// The first rule of `rules` that matches, with the fields it returned.
fn resolve(
    rules: &[Rule],
    scope: Option<&InstrumentationScope>,
    attributes: &[KeyValue],
    event: impl Fn() -> Value,
    gated: bool,
) -> Result<Option<(&'static str, ObjectMap)>, RuleError> {
    let timezone = TimeZone::default();
    let mut converted = None;
    for rule in rules {
        if gated && !rule.gate.admits(scope, attributes) {
            continue;
        }
        let converted = converted.get_or_insert_with(&event);
        let mut target = TargetValue {
            value: converted.clone(),
            metadata: Value::Object(ObjectMap::new()),
            secrets: Secrets::default(),
        };
        let mut state = RuntimeState::default();
        let mut context = Context::new(&mut target, &mut state, &timezone);
        let output =
            rule.program
                .resolve(&mut context)
                .map_err(|terminate| RuleError::Runtime {
                    rule: rule.name,
                    message: terminate.to_string(),
                })?;
        match output {
            Value::Object(fields) => return Ok(Some((rule.name, fields))),
            Value::Boolean(false) => return Ok(None),
            _ => continue,
        }
    }
    Ok(None)
}

#[doc(hidden)]
impl Normalizer {
    /// [`Self::span`] with every gate open: the corpus tests compare it with the gated path.
    pub fn span_ungated(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
    ) -> Result<Option<Call>, RuleError> {
        self.span_gated(scope, span, false)
    }

    /// [`Self::tool_span`] with every gate open.
    pub fn tool_span_ungated(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
    ) -> Result<Option<ToolCall>, RuleError> {
        self.tool_span_gated(scope, span, false)
    }
}

/// The VRL functions rules may call: only what they use, so a rule cannot reach the environment or the network.
fn functions() -> Vec<Box<dyn Function>> {
    vec![
        Box::new(vrl::stdlib::Includes),
        Box::new(vrl::stdlib::Exists),
        Box::new(vrl::stdlib::EndsWith),
        Box::new(vrl::stdlib::IsInteger),
        Box::new(vrl::stdlib::IsString),
        Box::new(vrl::stdlib::IsObject),
        Box::new(vrl::stdlib::ToInt),
        Box::new(vrl::stdlib::String),
        Box::new(vrl::stdlib::ParseInt),
        Box::new(vrl::stdlib::ParseJson),
        Box::new(vrl::stdlib::ParseTimestamp),
    ]
}

fn call(rule: &'static str, fields: &ObjectMap, times: Times) -> Result<Call, RuleError> {
    let operation = string(rule, fields, "operation")?;
    let Some(operation) =
        operation.filter(|operation| CALL_OPERATIONS.contains(&operation.as_str()))
    else {
        return Err(bad(rule, "operation", fields.get("operation")));
    };
    let error = flag(rule, fields, "error")?;
    let own_duration = (times.end_unix_nano - times.start_unix_nano) as f64 / 1e9;
    let duration_seconds = seconds(rule, fields, "duration_seconds")?.unwrap_or(own_duration);
    let time_to_first_chunk_seconds = match seconds(rule, fields, "time_to_first_chunk_seconds")? {
        Some(seconds) => Some(seconds),
        None => seconds_from_start(rule, fields, "first_chunk_time", times.start_unix_nano)?,
    };
    Ok(Call {
        operation,
        provider: string(rule, fields, "provider")?,
        request_model: string(rule, fields, "request_model")?,
        response_model: string(rule, fields, "response_model")?,
        tokens: Tokens {
            input: count(rule, fields, "input_tokens")?,
            output: count(rule, fields, "output_tokens")?,
            cache_read: count(rule, fields, "cache_read_tokens")?,
            cache_write: count(rule, fields, "cache_write_tokens")?,
            reasoning: count(rule, fields, "reasoning_tokens")?,
        },
        error,
        end_time_unix_nano: times.end_unix_nano,
        duration_seconds,
        time_to_first_chunk_seconds,
        response_id: string(rule, fields, "response_id")?,
    })
}

fn tool_call(rule: &'static str, fields: &ObjectMap, span: &Span) -> Result<ToolCall, RuleError> {
    let start = span.start_time_unix_nano;
    // A span that ends before it starts has no duration.
    let end = span.end_time_unix_nano.max(start);
    Ok(ToolCall {
        name: string(rule, fields, "name")?,
        error: flag(rule, fields, "error")?,
        end_time_unix_nano: end,
        duration_seconds: (end - start) as f64 / 1e9,
    })
}

/// A yes-or-no field; absent is no.
fn flag(rule: &'static str, fields: &ObjectMap, field: &'static str) -> Result<bool, RuleError> {
    match fields.get(field) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Boolean(flag)) => Ok(*flag),
        other => Err(bad(rule, field, other)),
    }
}

/// A span of time in seconds: a non-negative, finite number.
fn seconds(
    rule: &'static str,
    fields: &ObjectMap,
    field: &'static str,
) -> Result<Option<f64>, RuleError> {
    match fields.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Integer(seconds)) if *seconds >= 0 => Ok(Some(*seconds as f64)),
        Some(Value::Float(seconds)) if seconds.into_inner() >= 0.0 && seconds.is_finite() => {
            Ok(Some(seconds.into_inner()))
        }
        other => Err(bad(rule, field, other)),
    }
}

/// The seconds from `start_unix_nano` to the instant in `field`: a timestamp at or after the start.
fn seconds_from_start(
    rule: &'static str,
    fields: &ObjectMap,
    field: &'static str,
    start_unix_nano: u64,
) -> Result<Option<f64>, RuleError> {
    let instant = match fields.get(field) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Timestamp(instant)) => instant,
        other => return Err(bad(rule, field, other)),
    };
    let start = i128::from(start_unix_nano);
    match instant.timestamp_nanos_opt() {
        Some(nanos) if i128::from(nanos) >= start => {
            Ok(Some((i128::from(nanos) - start) as f64 / 1e9))
        }
        _ => Err(bad(rule, field, fields.get(field))),
    }
}

/// A string field; empty counts as absent, as a blank chart label would.
fn string(
    rule: &'static str,
    fields: &ObjectMap,
    field: &'static str,
) -> Result<Option<String>, RuleError> {
    match fields.get(field) {
        None | Some(Value::Null) => Ok(None),
        // VRL strings are `Bytes` or `String` depending on where they came from.
        Some(value @ (Value::Bytes(_) | Value::String(_))) => Ok(value
            .to_str_lossy()
            .filter(|text| !text.is_empty())
            .map(|text| text.into_owned())),
        other => Err(bad(rule, field, other)),
    }
}

/// A token count: a non-negative integer, or a float with no fraction (some producers send counts as doubles).
fn count(
    rule: &'static str,
    fields: &ObjectMap,
    field: &'static str,
) -> Result<Option<u64>, RuleError> {
    match fields.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Integer(integer)) if *integer >= 0 => Ok(Some(integer.unsigned_abs())),
        Some(Value::Float(float))
            if float.into_inner() >= 0.0
                && float.fract() == 0.0
                && float.into_inner() < 2f64.powi(53) =>
        {
            Ok(Some(float.into_inner() as u64))
        }
        other => Err(bad(rule, field, other)),
    }
}

fn bad(rule: &'static str, field: &'static str, value: Option<&Value>) -> RuleError {
    RuleError::Output {
        rule,
        field,
        value: value.map_or_else(|| "nothing".to_string(), ToString::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::any_value::Value as Av;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
    use opentelemetry_proto::tonic::trace::v1::Status;

    fn attribute(key: &str, value: Av) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        }
    }

    fn string(key: &str, value: &str) -> KeyValue {
        attribute(key, Av::StringValue(value.to_string()))
    }

    fn int(key: &str, value: i64) -> KeyValue {
        attribute(key, Av::IntValue(value))
    }

    fn double(key: &str, value: f64) -> KeyValue {
        attribute(key, Av::DoubleValue(value))
    }

    fn span(attributes: Vec<KeyValue>) -> Span {
        Span {
            start_time_unix_nano: 1_000_000_000,
            end_time_unix_nano: 2_500_000_000,
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn rules_compile() {
        Normalizer::new().unwrap();
    }

    #[test]
    fn conventions_rule_maps_current_and_old_names() {
        let normalizer = Normalizer::new().unwrap();
        let current = span(vec![
            string("gen_ai.operation.name", "chat"),
            string("gen_ai.provider.name", "anthropic"),
            string("gen_ai.request.model", "claude-sonnet-4-5"),
            string("gen_ai.response.model", "claude-sonnet-4-5-20250929"),
            int("gen_ai.usage.input_tokens", 49),
            int("gen_ai.usage.output_tokens", 17),
            int("gen_ai.usage.cache_read.input_tokens", 29),
            int("gen_ai.usage.cache_write.input_tokens", 7),
            string("gen_ai.response.id", "msg_1"),
        ]);
        let expected = Call {
            operation: "chat".into(),
            provider: Some("anthropic".into()),
            request_model: Some("claude-sonnet-4-5".into()),
            response_model: Some("claude-sonnet-4-5-20250929".into()),
            tokens: Tokens {
                input: Some(49),
                output: Some(17),
                cache_read: Some(29),
                cache_write: Some(7),
                reasoning: None,
            },
            error: false,
            end_time_unix_nano: 2_500_000_000,
            duration_seconds: 1.5,
            time_to_first_chunk_seconds: None,
            response_id: Some("msg_1".into()),
        };
        assert_eq!(
            normalizer.span(None, &current).unwrap(),
            Some(expected.clone())
        );

        let old = span(vec![
            string("gen_ai.operation.name", "chat"),
            string("gen_ai.system", "anthropic"),
            string("gen_ai.request.model", "claude-sonnet-4-5"),
            string("gen_ai.response.model", "claude-sonnet-4-5-20250929"),
            int("gen_ai.usage.prompt_tokens", 49),
            int("gen_ai.usage.completion_tokens", 17),
            int("gen_ai.usage.cache_read.input_tokens", 29),
            int("gen_ai.usage.cache_creation.input_tokens", 7),
            string("gen_ai.response.id", "msg_1"),
        ]);
        assert_eq!(normalizer.span(None, &old).unwrap(), Some(expected));
    }

    #[test]
    fn labels_name_what_the_producer_left_out() {
        let normalizer = Normalizer::new().unwrap();
        let labels = |attributes: Vec<KeyValue>| {
            let call = normalizer.span(None, &span(attributes)).unwrap().unwrap();
            [
                call.provider_label().to_string(),
                call.request_model_label().to_string(),
                call.response_model_label().to_string(),
            ]
        };
        let chat = || string("gen_ai.operation.name", "chat");
        assert_eq!(labels(vec![chat()]), ["unknown", "unknown", "unknown"]);
        // The one model name a producer reports stands in for the requested model, not for the answering one.
        assert_eq!(
            labels(vec![chat(), string("gen_ai.response.model", "gpt-5")]),
            ["unknown", "gpt-5", "gpt-5"]
        );
        assert_eq!(
            labels(vec![
                chat(),
                string("gen_ai.provider.name", "openai"),
                string("gen_ai.request.model", "gpt-5"),
            ]),
            ["openai", "gpt-5", "unknown"]
        );
    }

    #[test]
    fn errors_from_status_or_error_type() {
        let normalizer = Normalizer::new().unwrap();
        let mut by_status = span(vec![string("gen_ai.operation.name", "chat")]);
        by_status.status = Some(Status {
            code: 2,
            ..Default::default()
        });
        assert!(normalizer.span(None, &by_status).unwrap().unwrap().error);

        let by_type = span(vec![
            string("gen_ai.operation.name", "chat"),
            string("error.type", "500"),
        ]);
        assert!(normalizer.span(None, &by_type).unwrap().unwrap().error);

        let fine = span(vec![string("gen_ai.operation.name", "chat")]);
        assert!(!normalizer.span(None, &fine).unwrap().unwrap().error);
    }

    #[test]
    fn time_to_first_chunk_under_the_conventions_names() {
        let normalizer = Normalizer::new().unwrap();
        let first_chunk = |attributes: Vec<KeyValue>| {
            let mut all = vec![string("gen_ai.operation.name", "chat")];
            all.extend(attributes);
            normalizer
                .span(None, &span(all))
                .unwrap()
                .unwrap()
                .time_to_first_chunk_seconds
        };
        assert_eq!(
            first_chunk(vec![double("gen_ai.response.time_to_first_chunk", 0.75)]),
            Some(0.75)
        );
        // The metric's former name, which some producers write on the span.
        assert_eq!(
            first_chunk(vec![double(
                "gen_ai.client.operation.time_to_first_chunk",
                0.5
            )]),
            Some(0.5)
        );
        // A call that was not streamed has none.
        assert_eq!(first_chunk(vec![]), None);
        // OpenLIT's attribute is not read: its clock starts after the response headers arrived.
        assert_eq!(
            first_chunk(vec![double("gen_ai.server.time_to_first_token", 0.002)]),
            None
        );
    }

    #[test]
    fn strands_time_to_first_chunk_is_in_milliseconds() {
        let normalizer = Normalizer::new().unwrap();
        let call = span(vec![
            string("gen_ai.operation.name", "chat"),
            int("gen_ai.server.time_to_first_token", 1353),
        ]);
        let strands = scope("strands.telemetry.tracer");
        assert_eq!(
            normalizer
                .span(Some(&strands), &call)
                .unwrap()
                .unwrap()
                .time_to_first_chunk_seconds,
            Some(1.353)
        );
        // The same attribute from any other producer is not read.
        assert_eq!(
            normalizer
                .span(None, &call)
                .unwrap()
                .unwrap()
                .time_to_first_chunk_seconds,
            None
        );
    }

    #[test]
    fn langfuse_time_to_first_chunk_from_the_instant_of_the_first_chunk() {
        let normalizer = Normalizer::new().unwrap();
        let langfuse = scope("langfuse-sdk");
        // The span starts at second 1 and ends at second 2.5.
        let generation = |completion_start: &str| {
            normalizer.span(
                Some(&langfuse),
                &span(vec![
                    string("langfuse.observation.type", "generation"),
                    string(
                        "langfuse.observation.completion_start_time",
                        completion_start,
                    ),
                ]),
            )
        };
        let first_chunk = |completion_start: &str| {
            generation(completion_start)
                .unwrap()
                .unwrap()
                .time_to_first_chunk_seconds
        };
        // The JavaScript SDK writes milliseconds, the Python SDK microseconds.
        assert_eq!(first_chunk("\"1970-01-01T00:00:01.750Z\""), Some(0.75));
        assert_eq!(first_chunk("\"1970-01-01T00:00:02.250000Z\""), Some(1.25));
        assert_eq!(first_chunk("\"not a time\""), None);
        // A first chunk before the call started is a producer defect: the record is not counted.
        assert!(matches!(
            generation("\"1970-01-01T00:00:00.500Z\""),
            Err(RuleError::Output {
                rule: "langfuse",
                field: "first_chunk_time",
                ..
            })
        ));
    }

    #[test]
    fn non_call_spans_are_not_calls() {
        let normalizer = Normalizer::new().unwrap();
        for operation in ["invoke_agent", "execute_tool", "create_agent"] {
            let agent = span(vec![
                string("gen_ai.operation.name", operation),
                int("gen_ai.usage.input_tokens", 10),
            ]);
            assert_eq!(normalizer.span(None, &agent).unwrap(), None);
        }
        assert_eq!(
            normalizer
                .span(None, &span(vec![string("http.method", "POST")]))
                .unwrap(),
            None
        );
    }

    fn scope(name: &str) -> InstrumentationScope {
        InstrumentationScope {
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn a_bad_count_is_an_error() {
        let normalizer = Normalizer::new().unwrap();
        let negative = span(vec![
            string("gen_ai.operation.name", "chat"),
            int("gen_ai.usage.input_tokens", -1),
        ]);
        assert_eq!(
            normalizer.span(None, &negative),
            Err(RuleError::Output {
                rule: "conventions",
                field: "input_tokens",
                value: "-1".into()
            })
        );
    }

    #[test]
    fn a_bad_time_to_first_chunk_is_an_error() {
        let normalizer = Normalizer::new().unwrap();
        let negative = span(vec![
            string("gen_ai.operation.name", "chat"),
            double("gen_ai.response.time_to_first_chunk", -0.5),
        ]);
        assert_eq!(
            normalizer.span(None, &negative),
            Err(RuleError::Output {
                rule: "conventions",
                field: "time_to_first_chunk_seconds",
                value: "-0.5".into()
            })
        );
    }

    #[test]
    fn tool_rules_map_the_name_under_either_marker() {
        let normalizer = Normalizer::new().unwrap();
        let expected = ToolCall {
            name: Some("get_weather".into()),
            error: false,
            end_time_unix_nano: 2_500_000_000,
            duration_seconds: 1.5,
        };
        let conventions = span(vec![
            string("gen_ai.operation.name", "execute_tool"),
            string("gen_ai.tool.name", "get_weather"),
        ]);
        let openinference = span(vec![
            string("openinference.span.kind", "TOOL"),
            string("tool.name", "get_weather"),
        ]);
        // OpenInference decorating a framework's own span: one span, one call.
        let both = span(vec![
            string("gen_ai.operation.name", "execute_tool"),
            string("gen_ai.tool.name", "get_weather"),
            string("openinference.span.kind", "TOOL"),
            string("tool.name", "get_weather"),
        ]);
        for tool in [conventions, openinference, both] {
            assert!(normalizer.is_tool_candidate(None, &tool));
            assert_eq!(
                normalizer.tool_span(None, &tool).unwrap(),
                Some(expected.clone())
            );
            assert_eq!(normalizer.span(None, &tool).unwrap(), None);
        }
    }

    #[test]
    fn a_tool_call_without_a_name_still_counts() {
        let normalizer = Normalizer::new().unwrap();
        for marker in [
            string("gen_ai.operation.name", "execute_tool"),
            string("openinference.span.kind", "TOOL"),
        ] {
            let tool = normalizer.tool_span(None, &span(vec![marker])).unwrap();
            assert_eq!(tool.unwrap().name, None);
        }
    }

    #[test]
    fn tool_errors_from_status_or_error_type() {
        let normalizer = Normalizer::new().unwrap();
        for marker in [
            string("gen_ai.operation.name", "execute_tool"),
            string("openinference.span.kind", "TOOL"),
        ] {
            let mut by_status = span(vec![marker.clone()]);
            by_status.status = Some(Status {
                code: 2,
                ..Default::default()
            });
            assert!(
                normalizer
                    .tool_span(None, &by_status)
                    .unwrap()
                    .unwrap()
                    .error
            );

            let by_type = span(vec![marker, string("error.type", "RuntimeError")]);
            assert!(normalizer.tool_span(None, &by_type).unwrap().unwrap().error);
        }
    }

    #[test]
    fn model_calls_and_other_spans_are_not_tool_calls() {
        let normalizer = Normalizer::new().unwrap();
        for other in [
            span(vec![string("gen_ai.operation.name", "chat")]),
            span(vec![string("gen_ai.operation.name", "invoke_agent")]),
            span(vec![string("openinference.span.kind", "LLM")]),
            span(vec![string("http.method", "POST")]),
        ] {
            assert!(!normalizer.is_tool_candidate(None, &other));
            assert_eq!(normalizer.tool_span(None, &other).unwrap(), None);
            assert_eq!(normalizer.tool_span_ungated(None, &other).unwrap(), None);
        }
    }

    #[test]
    fn a_bad_tool_name_is_an_error() {
        let normalizer = Normalizer::new().unwrap();
        let numbered = span(vec![
            string("gen_ai.operation.name", "execute_tool"),
            int("gen_ai.tool.name", 7),
        ]);
        assert_eq!(
            normalizer.tool_span(None, &numbered),
            Err(RuleError::Output {
                rule: "tools/conventions",
                field: "name",
                value: "7".into()
            })
        );
    }
}
