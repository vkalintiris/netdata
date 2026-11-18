//! GenAI normalization: turns a span that describes one model call into a [`Call`], whatever dialect its producer
//! speaks.
//!
//! The rules are VRL programs under `rules/`, compiled into the binary and compiled by VRL once, in
//! [`Normalizer::new`]. Rules are tried in order and the first one that returns an object wins; a rule returns `null`
//! for a record it does not describe. A rule only maps the producer's vocabulary: the host supplies the call's end
//! time and duration from the span itself.
//!
//! Token counts follow the OpenTelemetry GenAI conventions: input includes cache reads and writes, output includes
//! reasoning, and the cache and reasoning counts are parts of those totals, never additions to them.

mod gate;
mod pairs;
mod value;

use gate::Gate;
pub use pairs::{Candidate, Pairs};

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::trace::v1::Span;
use vrl::compiler::state::RuntimeState;
use vrl::compiler::{Context, Function, Program, TargetValue, TimeZone};
use vrl::value::{ObjectMap, Secrets, Value};

/// The rules, in the order they are tried. Each has a native gate that admits every record the rule can match, so
/// records that concern no rule never reach VRL; the corpus tests check the gates against the rules.
const RULES: &[(&str, &str, Gate)] = &[(
    "conventions",
    include_str!("../rules/conventions.vrl"),
    Gate::AttributeIn {
        key: "gen_ai.operation.name",
        values: CALL_OPERATIONS,
    },
)];

/// The call operations the charts count: agent and tool operations repeat the totals of the calls inside them.
pub const CALL_OPERATIONS: &[&str] = &["chat", "text_completion", "generate_content", "embeddings"];

/// One model call, in the normalized vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// One of [`CALL_OPERATIONS`].
    pub operation: String,
    pub provider: Option<String>,
    pub request_model: Option<String>,
    pub response_model: Option<String>,
    pub tokens: Tokens,
    pub error: bool,
    /// When the call ended: the second it counts in.
    pub end_time_unix_nano: u64,
    /// `None` when the record says nothing about how long the call took.
    pub duration_seconds: Option<f64>,
    /// The provider's id of the answer, which identifies one call across records.
    pub response_id: Option<String>,
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

struct Rule {
    name: &'static str,
    gate: Gate,
    program: Program,
}

/// The compiled rules.
pub struct Normalizer {
    rules: Vec<Rule>,
}

impl Normalizer {
    /// # Errors
    ///
    /// A rule that does not compile: a defect of this crate, caught by its tests.
    pub fn new() -> Result<Self, Error> {
        let functions = functions();
        let mut rules = Vec::with_capacity(RULES.len());
        for &(name, source, gate) in RULES {
            let compiled = vrl::compiler::compile(source, &functions).map_err(|diagnostics| {
                Error::Compile {
                    rule: name,
                    message: vrl::diagnostic::Formatter::new(source, diagnostics).to_string(),
                }
            })?;
            rules.push(Rule {
                name,
                gate,
                program: compiled.program,
            });
        }
        Ok(Self { rules })
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
        self.resolve(scope, span, true)
    }

    /// Whether any rule's gate admits `span`: when not, [`Self::span`] never runs VRL for it.
    pub fn is_candidate(&self, scope: Option<&InstrumentationScope>, span: &Span) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.gate.admits(scope, &span.attributes))
    }

    fn resolve(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
        gated: bool,
    ) -> Result<Option<Call>, RuleError> {
        let timezone = TimeZone::default();
        let mut event = None;
        for rule in &self.rules {
            if gated && !rule.gate.admits(scope, &span.attributes) {
                continue;
            }
            let event = event.get_or_insert_with(|| value::span(scope, span));
            let mut target = TargetValue {
                value: event.clone(),
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
            let Value::Object(fields) = output else {
                continue;
            };
            let start = span.start_time_unix_nano;
            // A span that ends before it starts has no duration.
            let end = span.end_time_unix_nano.max(start);
            return call(rule.name, &fields, end, Some((end - start) as f64 / 1e9)).map(Some);
        }
        Ok(None)
    }
}

#[doc(hidden)]
impl Normalizer {
    /// [`Self::span`] with every gate open: the corpus tests compare it with the gated path.
    pub fn span_ungated(
        &self,
        scope: Option<&InstrumentationScope>,
        span: &Span,
    ) -> Result<Option<Call>, RuleError> {
        self.resolve(scope, span, false)
    }
}

/// The VRL functions rules may call: only what they use, so a rule cannot reach the environment or the network.
fn functions() -> Vec<Box<dyn Function>> {
    vec![
        Box::new(vrl::stdlib::Includes),
        Box::new(vrl::stdlib::Exists),
    ]
}

fn call(
    rule: &'static str,
    fields: &ObjectMap,
    end_time_unix_nano: u64,
    duration_seconds: Option<f64>,
) -> Result<Call, RuleError> {
    let operation = string(rule, fields, "operation")?;
    let Some(operation) =
        operation.filter(|operation| CALL_OPERATIONS.contains(&operation.as_str()))
    else {
        return Err(bad(rule, "operation", fields.get("operation")));
    };
    let error = match fields.get("error") {
        None | Some(Value::Null) => false,
        Some(Value::Boolean(error)) => *error,
        other => return Err(bad(rule, "error", other)),
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
        end_time_unix_nano,
        duration_seconds,
        response_id: string(rule, fields, "response_id")?,
    })
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
            duration_seconds: Some(1.5),
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
}
