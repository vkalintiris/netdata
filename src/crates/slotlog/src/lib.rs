//! Slot Log Protocol types and service definitions.
//!
//! This crate provides the Protocol Buffer generated types for the slot log
//! protocol, which defines communication between a metrics producer and consumer.
//!
//! The protocol is designed around these principles:
//! - Slot-based batching: All metric changes are grouped into fixed-interval time slots
//! - Consumer-assigned identifiers: The consumer assigns numeric IDs to optimize storage
//! - Registration before reference: Charts/dimensions must be registered before use
//! - Late data as a separate concern: Late-arriving data is handled via policy

pub mod v1 {
    tonic::include_proto!("nm.slotlog.v1");
}

pub use v1::*;
