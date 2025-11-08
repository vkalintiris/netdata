//! Schema types for communicating with Netdata.
//!
//! This module contains all the types used for the protocol/schema when
//! communicating with Netdata (both the agent and frontend):
//! - Request/response envelope types
//! - UI-specific data structures (facets, histograms, charts)

pub mod types;
pub mod ui;
