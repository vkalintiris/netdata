use super::transformations::TransformationRegistry;
use journal::Result;
use journal::file::{JournalFile, Mmap};
use journal::index::{FieldValuePair, LogEntry};
use journal::repository::File;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZeroU64;

/// A cell value with both raw and display representations
#[derive(Debug, Clone)]
pub struct CellValue {
    pub raw: Option<String>,
    pub display: Option<String>,
}

impl CellValue {
    /// Create a new cell value with no transformation
    pub fn new(value: Option<String>) -> Self {
        Self {
            raw: value.clone(),
            display: value,
        }
    }

    /// Create a new cell value with separate raw and display representations
    pub fn with_display(raw: Option<String>, display: Option<String>) -> Self {
        Self { raw, display }
    }
}

/// Column metadata for a table, compatible with the JSON response format
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub index: usize,
}

impl ColumnInfo {
    pub fn new(name: String, index: usize) -> Self {
        Self { name, index }
    }
}

/// A table representation of log entries with extracted field values
#[derive(Debug, Clone)]
pub struct Table {
    pub columns: Vec<ColumnInfo>,
    pub data: Vec<Vec<CellValue>>,
}

impl Table {
    /// Create a new empty table with the given column names
    pub fn new(column_names: Vec<String>) -> Self {
        let columns = column_names
            .into_iter()
            .enumerate()
            .map(|(index, name)| ColumnInfo::new(name, index))
            .collect();

        Self {
            columns,
            data: Vec::new(),
        }
    }

    /// Add a row to the table
    pub fn add_row(&mut self, row: Vec<CellValue>) {
        self.data.push(row);
    }

    /// Get the number of rows in the table
    pub fn row_count(&self) -> usize {
        self.data.len()
    }

    /// Get the number of columns in the table
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Calculate the optimal column widths for display
    fn calculate_column_widths(&self) -> Vec<usize> {
        const MESSAGE_MAX_WIDTH: usize = 80;

        let mut widths: Vec<usize> = self.columns.iter().map(|col| col.name.len()).collect();

        // Check each row to find the maximum width needed for each column
        for row in &self.data {
            for (col_idx, cell) in row.iter().enumerate() {
                let display_len = cell.display.as_deref().unwrap_or("-").len();
                if display_len > widths[col_idx] {
                    widths[col_idx] = display_len;
                }
            }
        }

        // Cap the MESSAGE column width at MESSAGE_MAX_WIDTH
        for (col_idx, col) in self.columns.iter().enumerate() {
            if col.name == "MESSAGE" && widths[col_idx] > MESSAGE_MAX_WIDTH {
                widths[col_idx] = MESSAGE_MAX_WIDTH;
            }
        }

        widths
    }
}

impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.columns.is_empty() {
            return writeln!(f, "(empty table)");
        }

        let widths = self.calculate_column_widths();
        let total_width: usize = widths.iter().sum::<usize>() + (widths.len() - 1) * 3 + 2;

        // Print top border
        writeln!(f, "{}", "=".repeat(total_width))?;

        // Print header
        write!(f, "|")?;
        for (col, width) in self.columns.iter().zip(&widths) {
            write!(f, " {:<width$} |", col.name, width = width)?;
        }
        writeln!(f)?;

        // Print separator
        writeln!(f, "{}", "=".repeat(total_width))?;

        // Print rows
        for row in &self.data {
            write!(f, "|")?;
            for (cell, width) in row.iter().zip(&widths) {
                let display = cell.display.as_deref().unwrap_or("-");
                // Truncate if the value is longer than the column width
                if display.len() > *width {
                    let truncated = &display[..*width];
                    write!(f, " {:<width$} |", truncated, width = width)?;
                } else {
                    write!(f, " {:<width$} |", display, width = width)?;
                }
            }
            writeln!(f)?;
        }

        // Print bottom border
        writeln!(f, "{}", "=".repeat(total_width))?;

        Ok(())
    }
}

/// Convert a vector of LogEntry items into a Table by reading and extracting field values
/// The first column will always be "timestamp" taken from the LogEntry's timestamp field.
pub fn log_entries_to_table(
    log_entries: Vec<LogEntry>,
    column_names: Vec<String>,
    transformations: &TransformationRegistry,
) -> Result<Table> {
    // Always prepend "timestamp" as the first column
    let mut all_columns = vec!["timestamp".to_string()];
    all_columns.extend(column_names.clone());

    let mut table = Table::new(all_columns);

    // Pre-allocate all rows (we know exactly how many we need)
    let num_rows = log_entries.len();
    let num_cols = column_names.len() + 1;
    let mut rows: Vec<Vec<CellValue>> = vec![vec![CellValue::new(None); num_cols]; num_rows];

    // (a) Collect unique files
    let unique_files: HashSet<&File> = log_entries.iter().map(|entry| &entry.file).collect();

    // Create a mapping from column name to index for fast lookup (offset by 1 since timestamp is first)
    let column_map: HashMap<&str, usize> = column_names
        .iter()
        .enumerate()
        .map(|(idx, name)| (name.as_str(), idx + 1)) // +1 because timestamp is at index 0
        .collect();

    // (b) Process each file once
    for file in unique_files {
        let journal_file = JournalFile::<Mmap>::open(file, 2 * 1024 * 1024)?;

        // Keeps all data offset of each entry object
        let mut data_offsets = Vec::new();

        // (c) For each entry that matches this file, populate its row
        for (row_idx, log_entry) in log_entries.iter().enumerate() {
            if &log_entry.file != file {
                continue;
            }

            // Read the entry at the specified offset
            let entry_offset =
                NonZeroU64::new(log_entry.offset).ok_or(journal::JournalError::InvalidOffset)?;
            let entry_guard = journal_file.entry_ref(entry_offset)?;

            data_offsets.clear();
            entry_guard.collect_offsets(&mut data_offsets)?;

            // Drop entry_guard before reading data objects to avoid borrow conflicts
            drop(entry_guard);

            // First column: timestamp from LogEntry (in microseconds)
            let timestamp_str = log_entry.timestamp.to_string();
            rows[row_idx][0] = transformations.transform_field("timestamp", Some(timestamp_str));

            // Read each data object and extract field values
            for data_offset in data_offsets.iter().copied() {
                let data_guard = journal_file.data_ref(data_offset)?;
                let payload_bytes = data_guard.payload_bytes();
                let payload_str = String::from_utf8_lossy(payload_bytes);

                // Parse the field=value pair
                if let Some(pair) = FieldValuePair::parse(payload_str.as_ref()) {
                    let field_name = pair.field();
                    let field_value = pair.value();

                    // If this field is one of our requested columns, transform and store it
                    if let Some(&col_idx) = column_map.get(field_name) {
                        let raw_value = Some(field_value.to_string());
                        rows[row_idx][col_idx] =
                            transformations.transform_field(field_name, raw_value);
                    }
                }
            }
        }
    }

    // Add all rows to table
    for row in rows {
        table.add_row(row);
    }

    Ok(table)
}
