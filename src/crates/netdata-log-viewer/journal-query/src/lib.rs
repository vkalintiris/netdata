mod error;

pub mod indexing;
pub use crate::indexing::IndexingService;

pub mod histogram;
pub use crate::histogram::{HistogramRequest, HistogramResponse, HistogramService};

pub mod ui;

// Re-export types from journal crate
pub use journal::index::Filter;
pub use journal::{FieldName, FieldValuePair};
