//! Shared helpers.

use file_registry::TimestampNs;

/// Derive the catalog partition date from an SFST file's summary.
///
/// Uses the file's earliest timestamp. On an empty summary (an SFST with no
/// logs — shouldn't happen in practice) falls back to the current date.
pub(super) fn derive_date_from_summary(summary: &sfst::FileSummary) -> chrono::NaiveDate {
    if summary.total_logs == 0 {
        return chrono::Utc::now().date_naive();
    }
    chrono::DateTime::from_timestamp(summary.min_timestamp_s as i64, 0)
        .map(|dt| dt.date_naive())
        .unwrap_or_else(|| chrono::Utc::now().date_naive())
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

/// Build a [`otel_catalog::CatalogEntry`] from a registered SFST file.
///
/// All summary fields come from `sfst_file.summary`, which the registry
/// populated either at indexing time (`Registry::track`) or at recovery time
/// (`Registry::recover`). No reads against the SFST file itself.
pub(crate) fn build_catalog_entry(
    sfst_file: &sfst::File,
    remote_key: String,
    uploaded_at_ns: TimestampNs,
) -> otel_catalog::CatalogEntry {
    let summary = &sfst_file.summary;
    otel_catalog::CatalogEntry {
        id: sfst_file.id,
        remote_key,
        min_timestamp_s: summary.min_timestamp_s,
        max_timestamp_s: summary.max_timestamp_s,
        total_logs: summary.total_logs,
        stream: summary.stream.clone(),
        size: sfst_file.size,
        uploaded_at_ns,
    }
}
