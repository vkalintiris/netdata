//! Netdata Plugin Runtime
//!
//! This crate provides a simplified runtime layer for writing Netdata plugins.
//! It abstracts away the complexities of gRPC services, protocol transport,
//! and connection management, allowing plugin authors to focus on implementing
//! their plugin functionality.
//!
//! # Example
//!
//! ```rust
//! use netdata_plugin_runtime::{PluginRuntime, Function, FunctionCall, FunctionResult, PluginContext, FunctionContext};
//! use std::sync::Arc;
//!
//! async fn hello_handler(plugin_ctx: Arc<PluginContext>, fn_ctx: FunctionContext) -> FunctionResult {
//!     let stats = plugin_ctx.get_stats().await;
//!     FunctionResult::success(format!(
//!         "Hello from {}! Plugin: {} | Total calls: {} | Elapsed: {:?}",
//!         fn_ctx.function_name(),
//!         plugin_ctx.plugin_name(),
//!         stats.total_calls,
//!         fn_ctx.elapsed()
//!     ))
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = PluginRuntime::new("my-plugin");
//!     
//!     // Register a function with its handler
//!     runtime.register_function(Function {
//!         name: "hello".to_string(),
//!         help: "Returns a friendly greeting".to_string(),
//!         timeout: 10,
//!         tags: Some("greeting".to_string()),
//!         access: Some(0),
//!         priority: Some(100),
//!         version: Some(1),
//!         global: false,
//!     }, hello_handler).await;
//!     
//!     // Run the plugin
//!     runtime.run().await
//! }
//! ```

mod context;
mod error;
mod function_context;
mod registry;
mod runtime;

pub use context::{PluginContext, PluginStats, Transaction, TransactionId};
pub use error::{Result, RuntimeError};
pub use function_context::{FunctionContext, FunctionMetadata};
pub use registry::{FunctionRegistry, RegisteredFunction};
pub use runtime::PluginRuntime;

// Re-export commonly used types from the protocol
pub use netdata_plugin_proto::v1::{FunctionCall, FunctionDeclaration, FunctionResult};
