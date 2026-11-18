//! Native gates: the cheap checks that keep records no rule can match away from VRL.

use opentelemetry_proto::tonic::common::v1::any_value::Value as Av;
use opentelemetry_proto::tonic::common::v1::{InstrumentationScope, KeyValue};

/// Admits a record when every condition it has holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Gate {
    /// The instrumentation scope's name.
    scope: Option<&'static str>,
    /// A string attribute whose value is one of a set.
    attribute: Option<(&'static str, &'static [&'static str])>,
}

impl Gate {
    pub(crate) const fn scope(name: &'static str) -> Self {
        Self {
            scope: Some(name),
            attribute: None,
        }
    }

    pub(crate) const fn attribute_in(key: &'static str, values: &'static [&'static str]) -> Self {
        Self {
            scope: None,
            attribute: Some((key, values)),
        }
    }

    pub(crate) const fn and_attribute_in(
        self,
        key: &'static str,
        values: &'static [&'static str],
    ) -> Self {
        Self {
            scope: self.scope,
            attribute: Some((key, values)),
        }
    }

    pub(crate) fn admits(
        &self,
        scope: Option<&InstrumentationScope>,
        attributes: &[KeyValue],
    ) -> bool {
        if let Some(name) = self.scope
            && scope_name(scope) != name
        {
            return false;
        }
        let Some((key, values)) = self.attribute else {
            return true;
        };
        attributes.iter().any(|attribute| {
            attribute.key == key
                && matches!(
                    attribute.value.as_ref().and_then(|value| value.value.as_ref()),
                    Some(Av::StringValue(value)) if values.contains(&value.as_str())
                )
        })
    }
}

pub(crate) fn scope_name(scope: Option<&InstrumentationScope>) -> &str {
    scope.map_or("", |scope| scope.name.as_str())
}
