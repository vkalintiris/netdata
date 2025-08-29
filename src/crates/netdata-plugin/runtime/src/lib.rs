//! Netdata Plugin Runtime
//!
//! This crate provides a runtime layer for writing Netdata plugins using gRPC services.
//! It abstracts the complexities of the Netdata protocol, gRPC communication, and
//! provides a clean API for plugin authors to focus on implementing their business logic.
//!
//! # Architecture
//!
//! The runtime consists of several key components:
//!
//! - **Plugin Runtime**: The main orchestrator that manages gRPC services and message handling
//! - **Function Registry**: Type-erased async function handler registry
//! - **Transaction Registry**: Manages active function calls with timeout and cancellation support
//! - **Plugin Context**: Maintains plugin state and statistics
//! - **Function Context**: Provides function-specific context to handlers
//!
//! The runtime uses an in-memory gRPC channel to communicate between the Plugin and Agent
//! services, while using the tokio codec to handle stdin/stdout communication with Netdata.
//!
//! # Example
//!
//! ```rust,no_run
//! use netdata_plugin_runtime::{
//!     PluginRuntime, FunctionContext, FunctionDeclaration,
//!     FunctionResult, PluginContext
//! };
//! use std::sync::Arc;
//!
//! async fn my_function(
//!     plugin_ctx: Arc<PluginContext>,
//!     fn_ctx: FunctionContext,
//! ) -> FunctionResult {
//!     // Check if transaction was cancelled
//!     if plugin_ctx.is_transaction_cancelled(fn_ctx.transaction_id()).await {
//!         return FunctionResult {
//!             transaction: fn_ctx.transaction_id().clone(),
//!             status: 499,
//!             format: "text/plain".to_string(),
//!             expires: 0,
//!             payload: b"Cancelled".to_vec(),
//!         };
//!     }
//!
//!     // Get plugin statistics
//!     let stats = plugin_ctx.get_stats().await;
//!     
//!     // Return result
//!     FunctionResult {
//!         transaction: fn_ctx.transaction_id().clone(),
//!         status: 200,
//!         format: "text/plain".to_string(),
//!         expires: 0,
//!         payload: format!(
//!             "Hello from {}! Total calls: {}",
//!             plugin_ctx.plugin_name(),
//!             stats.total_calls
//!         ).into_bytes(),
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = PluginRuntime::new("my-plugin");
//!
//!     // Register function
//!     runtime.register_function(
//!         FunctionDeclaration {
//!             name: "hello".to_string(),
//!             help: "Says hello".to_string(),
//!             timeout: 30,
//!             tags: Some("greeting".to_string()),
//!             access: Some(0),
//!             priority: Some(100),
//!             version: Some(1),
//!             global: false,
//!         },
//!         my_function,
//!     ).await?;
//!
//!     // Run the plugin
//!     runtime.run().await?;
//!     Ok(())
//! }
//! ```

mod config_registry;
mod error;
mod function_context;
mod function_registry;
mod plugin_context;
mod plugin_runtime;

// Public exports
pub use config_registry::ConfigDeclarable;
pub use error::{NetdataPluginError, Result};
pub use function_context::FunctionContext;
pub use function_registry::FunctionRegistry;
pub use plugin_context::{PluginContext, PluginStats, Transaction, TransactionId};
pub use plugin_runtime::PluginRuntime;

// Re-export commonly used types from proto
pub use netdata_plugin_protocol::{
    ConfigDeclaration, DynCfgCmds, DynCfgSourceType, DynCfgStatus, DynCfgType, FunctionCall,
    FunctionCancel, FunctionDeclaration, FunctionResult, HttpAccess,
};
