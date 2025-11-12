//! High-level builder for Netdata UI responses.
//!
//! This module provides convenience functions for building complete Netdata UI
//! responses from log data and histogram information.

use crate::histogram::HistogramResponse;
use crate::logs::{LogEntryData, entry_data_to_table, systemd_transformations};
use serde_json;
use tracing::{info, warn};

/// Build a complete Netdata UI response from log entries and histogram data.
///
/// This is a high-level convenience function that:
/// 1. Generates column schema from histogram response
/// 2. Builds a table with the log entries and discovered fields
/// 3. Transforms the table to Netdata UI JSON format
/// 4. Returns both the column schema and data for the UI
///
/// # Arguments
///
/// * `histogram_response` - The histogram response containing discovered fields
/// * `log_entries` - The log entry data to format
///
/// # Returns
///
/// A tuple of `(columns, data)` where:
/// - `columns` is the JSON serialization of the column schema
/// - `data` is the JSON array of formatted log rows
///
/// # Example
///
/// ```ignore
/// use journal_function::netdata::build_ui_response;
///
/// let (columns, data) = build_ui_response(&histogram_response, &log_entries);
/// ```
pub fn build_ui_response(
    histogram_response: &HistogramResponse,
    log_entries: &[LogEntryData],
) -> (serde_json::Value, serde_json::Value) {
    // Generate column schema from histogram response
    let field_names = histogram_response.discovered_field_names();
    let column_schema = super::columns::generate_column_schema(&field_names);
    let columns = serde_json::to_value(&column_schema).unwrap_or_else(|e| {
        warn!("Failed to serialize column schema: {}", e);
        serde_json::json!({})
    });

    if log_entries.is_empty() {
        return (columns, serde_json::json!([]));
    }

    let transformations = systemd_transformations();

    match entry_data_to_table(log_entries, field_names, &transformations) {
        Ok(table) => {
            info!(
                "Table has {} rows and {} columns",
                table.row_count(),
                table.column_count()
            );

            // Transform to UI format
            let ui_data_rows = super::response::table_to_netdata_response(&table, &column_schema);

            info!("Transformed to {} UI data rows", ui_data_rows.len());

            (columns, serde_json::json!(ui_data_rows))
        }
        Err(e) => {
            warn!("Failed to create table from log entries: {}", e);
            (columns, serde_json::json!([]))
        }
    }
}
