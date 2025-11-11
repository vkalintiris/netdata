//! Log entry formatting and display.
//!
//! This module provides types and transformations for converting log entries
//! into formatted tables suitable for display in the Netdata dashboard.

pub mod table;
pub mod transformations;

pub use table::{log_entries_to_table, CellValue, ColumnInfo, Table};
pub use transformations::{create_systemd_journal_transformations, FieldTransformation, TransformationRegistry};
