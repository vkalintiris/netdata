//! Value suggestions (`{"values": {...}}`): the stored values of one field
//! starting with a prefix, for the explorer's search box.

use serde::{Deserialize, Serialize};

use super::StatusWire;
use super::explore::RequestWindow;

/// Values returned when the request names no limit.
pub const VALUES_DEFAULT_LIMIT: usize = 100;

/// The request, validated.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawValuesParams")]
pub struct ValuesParams {
    pub window: RequestWindow,
    /// A storage field name.
    pub field: String,
    /// Case-sensitive; empty keeps every value.
    pub prefix: String,
    pub limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawValuesParams {
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    before: Option<i64>,
    field: String,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    limit: Option<usize>,
}

impl TryFrom<RawValuesParams> for ValuesParams {
    type Error = String;

    fn try_from(raw: RawValuesParams) -> Result<Self, String> {
        let window = RequestWindow::parse(raw.after, raw.before)?;
        if raw.field.is_empty() {
            return Err("`field` is empty".into());
        }
        let limit = raw.limit.unwrap_or(VALUES_DEFAULT_LIMIT);
        let max = sfsq::traces::explore::VALUES_LIMIT_MAX;
        if limit == 0 || limit > max {
            return Err(format!("`limit` must be 1 to {max}"));
        }
        Ok(ValuesParams {
            window,
            field: raw.field,
            prefix: raw.prefix,
            limit,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ValuesResponse {
    pub mode: &'static str,
    pub version: u32,
    pub field: String,
    /// The first values in byte order.
    pub values: Vec<String>,
    /// Whether more values matched than returned.
    pub truncated: bool,
    pub status: StatusWire,
}
