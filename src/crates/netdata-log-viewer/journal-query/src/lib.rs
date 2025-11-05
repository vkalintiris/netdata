pub mod error;
pub mod indexing;
pub mod request;
pub mod response;
pub mod service;
pub mod ui;

pub use crate::error::Result;
pub use crate::indexing::{IndexingRequest, IndexingService};
pub use crate::request::{BucketRequest, HistogramRequest, RequestMetadata};
pub use crate::response::{
    BucketCompleteResponse, BucketPartialResponse, BucketResponse, HistogramResult,
};
pub use crate::service::{FileTimeRange, FileWithRange, HistogramService};

// Re-export field types from journal crate
pub use journal::{FieldName, FieldValuePair};
