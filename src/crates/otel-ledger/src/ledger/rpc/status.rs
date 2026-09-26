//! The answer status a data-mode response carries beside its data: the wire
//! form of [`sfsq::QueryStatus`], shared by `otel-traces` and the
//! trace-filtered `otel-logs` answer.

use serde::{Deserialize, Serialize};

use sfsq::{PartialReason, QueryStatus};

/// Wire form of the engine's [`QueryStatus`]: `{"complete": true}` or
/// `{"partial": [{"reason": "source_failure", "count": 2, "of": 14}, ...]}`.
/// Untagged — the distinct field names select the variant.
/// Every data-mode response carries one beside its data; one definition
/// (pinned by round-trip tests) so they all share one serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StatusWire {
    Complete { complete: CompleteTrue },
    Partial { partial: Vec<ReasonWire> },
}

/// One partial reason on the wire: how often it happened, out of how many
/// where that applies, and the names it applies to (e.g. fields left out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasonWire {
    pub reason: PartialReasonWire,
    pub count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<String>,
}

/// The `complete` field's value — the JSON literal `true`, as a type.
/// A complete status has no other truth, so `{"complete": false}` is an
/// unrepresentable (and un-deserializable) shape rather than a value a
/// future producer could construct by mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "bool", into = "bool")]
pub struct CompleteTrue;

impl From<CompleteTrue> for bool {
    fn from(_: CompleteTrue) -> bool {
        true
    }
}

impl TryFrom<bool> for CompleteTrue {
    type Error = &'static str;
    fn try_from(v: bool) -> Result<Self, Self::Error> {
        if v {
            Ok(CompleteTrue)
        } else {
            Err("`complete` must be true")
        }
    }
}

impl From<&QueryStatus> for StatusWire {
    fn from(status: &QueryStatus) -> Self {
        match status {
            QueryStatus::Complete => StatusWire::Complete {
                complete: CompleteTrue,
            },
            QueryStatus::Partial(reasons) => {
                // BTreeMap iteration keeps the wire rendering deterministic.
                let mut partial = Vec::with_capacity(reasons.len());
                for (&reason, counted) in reasons {
                    partial.push(ReasonWire {
                        reason: reason.into(),
                        count: counted.count,
                        of: counted.of,
                        detail: counted.detail.iter().cloned().collect(),
                    });
                }
                StatusWire::Partial { partial }
            }
        }
    }
}

/// Wire names of the engine's [`PartialReason`] variants (snake_case).
/// The `From` match is exhaustive on purpose: a new engine reason fails
/// compilation here until its wire name is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialReasonWire {
    SizeCap,
    SourceFailure,
    Cancelled,
    RemoteUnavailable,
    LegacyFile,
    StackFieldHighCard,
    FacetHighCard,
    FacetValueCap,
    GroupsCap,
    LivePassFailed,
}

impl From<PartialReason> for PartialReasonWire {
    fn from(reason: PartialReason) -> Self {
        match reason {
            PartialReason::SizeCap => PartialReasonWire::SizeCap,
            PartialReason::SourceFailure => PartialReasonWire::SourceFailure,
            PartialReason::Cancelled => PartialReasonWire::Cancelled,
            PartialReason::RemoteUnavailable => PartialReasonWire::RemoteUnavailable,
            PartialReason::LegacyFile => PartialReasonWire::LegacyFile,
            PartialReason::StackFieldHighCard => PartialReasonWire::StackFieldHighCard,
            PartialReason::FacetHighCard => PartialReasonWire::FacetHighCard,
            PartialReason::FacetValueCap => PartialReasonWire::FacetValueCap,
            PartialReason::GroupsCap => PartialReasonWire::GroupsCap,
            PartialReason::LivePassFailed => PartialReasonWire::LivePassFailed,
        }
    }
}

#[cfg(test)]
mod tests;
