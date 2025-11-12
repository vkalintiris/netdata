//! Log entry formatting and display.
//!
//! This module provides generic types for converting log entries
//! into formatted tables.

pub mod query;
pub mod table;

pub use query::{LogEntryData, LogQuery};
pub use table::{CellValue, ColumnInfo, Table, entry_data_to_table};

// Re-export transformations from netdata module for backward compatibility
pub use crate::netdata::{FieldTransformation, TransformationRegistry, systemd_transformations};
