use crate::entry::StreamEntry;

/// A time-range + optional stream filter over a [`crate::Catalog`].
///
/// Range endpoints are inclusive on both sides. An entry matches if its
/// `[min_timestamp_s, max_timestamp_s]` overlaps the query's range. When
/// `stream` is `Some`, the entry must also list that exact `(namespace, name)`
/// pair in its `streams`.
#[derive(Debug, Clone)]
pub struct CatalogQuery {
    pub min_timestamp_s: u32,
    pub max_timestamp_s: u32,
    pub stream: Option<StreamEntry>,
}
