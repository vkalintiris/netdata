//! Slot Log Consumer implementation.
//!
//! This crate provides the consumer side of the slot log protocol.
//! The consumer receives slot logs from a producer, assigns IDs to new
//! charts and dimensions, and stores the metric data using dense vectors.
//!
//! # Example
//!
//! ```ignore
//! use slotlog_consumer::{SharedSlotLogConsumer, ConsumerConfig};
//! use slotlog::metrics_processor_server::MetricsProcessorServer;
//! use tonic::transport::Server;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let consumer = SharedSlotLogConsumer::with_defaults();
//!
//!     Server::builder()
//!         .add_service(MetricsProcessorServer::new(consumer))
//!         .serve("[::1]:50051".parse()?)
//!         .await?;
//!
//!     Ok(())
//! }
//! ```

mod service;
mod storage;

pub use service::{ConsumerConfig, SharedSlotLogConsumer, SlotLogConsumer};
pub use storage::{ChartData, DimensionMeta, Storage};
