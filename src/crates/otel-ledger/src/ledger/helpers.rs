//! Shared helpers.

use file_registry::{ByteSize, FileId, TimestampNs};

pub(super) fn derive_date_from_metadata(metadata: &log_index::IndexMetadata) -> chrono::NaiveDate {
    match metadata.histogram.timestamps.first() {
        Some(&sec) => chrono::DateTime::from_timestamp(sec as i64, 0)
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| chrono::Utc::now().date_naive()),
        None => chrono::Utc::now().date_naive(),
    }
}

/// Catalog retention window (whole days) derived from a tenant's SFST
/// retention policy. Ceiling division so a non-integer `max_age` in days
/// doesn't trim catalog coverage below SFST coverage. There is no
/// independent knob — this is the single source of truth.
pub(crate) fn catalog_retention_days(retention: &bridge::config::RetentionConfig) -> u32 {
    retention
        .max_age
        .as_secs()
        .div_ceil(86_400)
        .try_into()
        .unwrap_or(u32::MAX)
}

pub(crate) fn build_catalog_entry(
    id: FileId,
    remote_key: String,
    metadata: &log_index::IndexMetadata,
    size: ByteSize,
    uploaded_at_ns: TimestampNs,
) -> otel_catalog::CatalogEntry {
    // On an empty histogram (an SFST with no logs — shouldn't happen in
    // practice) the 0 fallback yields a [0, 0] epoch range that no real
    // query will match.
    let min_timestamp_s = metadata.histogram.timestamps.first().copied().unwrap_or(0);
    let max_timestamp_s = metadata.histogram.timestamps.last().copied().unwrap_or(0);
    let streams = metadata
        .streams
        .iter()
        .map(|s| otel_catalog::StreamEntry {
            namespace: s.namespace.clone(),
            name: s.name.clone(),
        })
        .collect();
    otel_catalog::CatalogEntry {
        id,
        remote_key,
        min_timestamp_s,
        max_timestamp_s,
        total_logs: metadata.total_logs,
        streams,
        size,
        uploaded_at_ns,
    }
}
