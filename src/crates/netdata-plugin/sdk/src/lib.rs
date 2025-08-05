//! Netdata Plugin SDK
//!
//! This crate provides a high-level SDK for writing Netdata plugins with automatic
//! function registration using procedural macros.
//!
//! # Example
//!
//! ```rust
//! use netdata_plugin_sdk::{PluginRuntime, Function, FunctionCall, FunctionResult, Context, PluginContext};
//! use std::sync::Arc;
//!
//! async fn hello_handler(plugin_ctx: Arc<PluginContext>, call: FunctionCall) -> FunctionResult {
//!     let ctx = Context::new(call);
//!     let name = ctx.get_parameter("name").unwrap_or("World");
//!     let stats = plugin_ctx.get_stats().await;
//!     FunctionResult::success(format!(
//!         "Hello, {}! Plugin: {} | Total calls: {}", 
//!         name, 
//!         plugin_ctx.plugin_name(), 
//!         stats.total_calls
//!     ))
//! }
//!
//! async fn list_processes(plugin_ctx: Arc<PluginContext>, call: FunctionCall) -> FunctionResult {
//!     let ctx = Context::new(call);
//!     let processes = serde_json::json!({"processes": []});
//!     FunctionResult::json(processes)
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = PluginRuntime::new("my-plugin");
//!     
//!     runtime.register_function(Function {
//!         name: "hello".to_string(),
//!         help: "Returns a greeting".to_string(),
//!         timeout: 30,
//!         tags: Some("greeting".to_string()),
//!         access: Some(0),
//!         priority: Some(100),
//!         version: Some(1),
//!         global: false,
//!     }, hello_handler).await;
//!     
//!     runtime.register_function(Function {
//!         name: "processes".to_string(),
//!         help: "Lists running processes".to_string(),
//!         timeout: 60,
//!         tags: Some("system".to_string()),
//!         access: Some(0),
//!         priority: Some(100),
//!         version: Some(1),
//!         global: false,
//!     }, list_processes).await;
//!     
//!     runtime.run().await
//! }
//! ```

mod context;
mod response;
// mod builder; // TODO: Fix builder implementation

pub use context::Context;
pub use response::FunctionResponse;

// Re-export the procedural macro
// pub use netdata_plugin_sdk_macros::netdata_function;

// Re-export commonly used runtime types
pub use netdata_plugin_runtime::{
    Function, FunctionCall, FunctionResult, PluginContext, PluginRuntime, PluginStats,
    Result as RuntimeResult, RuntimeError, Transaction, TransactionId, FunctionContext, FunctionMetadata,
};

// Re-export commonly used external crates
pub use serde_json;
pub use tokio;
pub use tracing;
