//! Shared helpers.

use file_registry::TimestampNs;

/// Convert a summary's `min_timestamp_s` to the calendar date used for
/// catalog partitioning.
///
/// Returns `None` for an empty SFST (`total_logs == 0`) or a timestamp
/// outside the representable chrono range. Callers fall back to the
/// current date when `None` — encoded once at each call site rather than
/// hidden inside this helper, so the fallback is visible.
pub(crate) fn date_from_summary(summary: &sfst::FileSummary) -> Option<chrono::NaiveDate> {
    if summary.total_logs == 0 {
        return None;
    }
    chrono::DateTime::from_timestamp(summary.min_timestamp_s as i64, 0).map(|dt| dt.date_naive())
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
