//! Native gates: the cheap checks that keep records no rule can match away from VRL.

use opentelemetry_proto::tonic::common::v1::any_value::Value as Av;
use opentelemetry_proto::tonic::common::v1::{InstrumentationScope, KeyValue};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Gate {
    /// A string attribute `key` whose value is one of `values`.
    AttributeIn {
        key: &'static str,
        values: &'static [&'static str],
    },
}

impl Gate {
    pub(crate) fn admits(
        &self,
        _scope: Option<&InstrumentationScope>,
        attributes: &[KeyValue],
    ) -> bool {
        match *self {
            Gate::AttributeIn { key, values } => attributes.iter().any(|attribute| {
                attribute.key == key
                    && matches!(
                        attribute.value.as_ref().and_then(|value| value.value.as_ref()),
                        Some(Av::StringValue(value)) if values.contains(&value.as_str())
                    )
            }),
        }
    }
}
