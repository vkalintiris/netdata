mod error;

mod indexing;
pub use crate::indexing::IndexingService;

mod request;
pub use crate::request::BucketRequest;
pub use crate::request::HistogramRequest;

mod response;
pub use crate::response::BucketResponse;
pub use crate::response::HistogramResponse;

mod service;
pub use crate::service::HistogramService;

pub mod ui;

// Re-export types from journal crate
pub use journal::index::Filter;
pub use journal::{FieldName, FieldValuePair};
