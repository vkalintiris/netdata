//! Slot Log Producer implementation.
//!
//! This crate provides the producer side of the slot log protocol.
//! The producer defines charts and dimensions, accumulates updates,
//! and sends them to a consumer.
//!
//! # Example
//!
//! ```ignore
//! use slotlog_producer::{Producer, GrpcSender, ProducerError};
//! use slotlog::ChartType;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), ProducerError> {
//!     let sender = GrpcSender::connect("http://[::1]:50051").await?;
//!     let mut producer = Producer::new(sender);
//!
//!     // Define charts and dimensions
//!     producer.define_chart("cpu.usage", ChartType::Gauge)?;
//!     producer.define_dimension("cpu.usage", "user")?;
//!     producer.define_dimension("cpu.usage", "system")?;
//!
//!     // Update values
//!     producer.begin_slot(1000);
//!     producer.update("cpu.usage", "user", Some(25.5))?;
//!     producer.update("cpu.usage", "system", Some(10.2))?;
//!     producer.send().await?;
//!
//!     Ok(())
//! }
//! ```

mod producer;

pub use producer::{GrpcSender, InMemorySender, Producer, ProducerError, SlotLogSender};
