#![allow(dead_code)]

use async_trait::async_trait;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::FunctionDeclaration;
use rt::{FunctionHandler, PluginRuntime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Deserialize)]
struct EmptyRequest {}

#[derive(Serialize)]
struct HelloFastResponse {
    message: String,
}

struct HelloFastHandler;

#[async_trait]
impl FunctionHandler for HelloFastHandler {
    type Request = EmptyRequest;
    type Response = HelloFastResponse;

    async fn on_call(&self, _request: Self::Request) -> Result<Self::Response> {
        info!("Fast function called");

        Ok(HelloFastResponse {
            message: "Fast response!".to_string(),
        })
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        // Fast function doesn't really need cancellation handling
        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Cancelled".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress requested for fast function");
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration::new("hello_fast", "A fast function that responds immediately")
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JournalRequest {
    pub info: bool,

    /// Unix timestamp for the start of the time range
    pub after: i64,

    /// Unix timestamp for the end of the time range
    pub before: i64,

    /// Maximum number of results to return
    pub last: Option<u32>,

    /// List of facets to include in the response
    #[serde(default)]
    pub facets: Vec<String>,

    /// Whether to slice the results
    pub slice: Option<bool>,

    /// Query string (empty in your example)
    #[serde(default)]
    pub query: String,

    /// Selection filters
    #[serde(default)]
    pub selections: HashMap<String, Vec<String>>,

    /// Timeout in milliseconds
    pub timeout: Option<u32>,
}

impl Default for JournalRequest {
    fn default() -> Self {
        Self {
            info: true,
            after: 0,
            before: 0,
            last: Some(200),
            facets: Vec::new(),
            slice: None,
            query: String::new(),
            selections: HashMap::new(),
            timeout: None,
        }
    }
}

#[derive(Serialize)]
struct HelloSlowResponse {
    message: String,
    progress: u32,
}

struct Journal;

#[async_trait]
impl FunctionHandler for Journal {
    type Request = JournalRequest;
    type Response = HelloSlowResponse;

    async fn on_call(&self, request: Self::Request) -> Result<Self::Response> {
        info!("Slow function request: {:#?}", request);
        // info!("Slow function started - simulating 10 seconds of work");

        // Simulate slow work - 10 seconds total
        // The framework will automatically handle cancellation and progress
        for i in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            info!("Slow function progress: {}%", (i + 1) * 5);
        }

        info!("Slow function completed");
        Ok(HelloSlowResponse {
            message: "Slow work completed successfully!".to_string(),
            progress: 100,
        })
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        warn!("Slow function was cancelled!");
        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Operation cancelled by user".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress report requested for slow function");
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration::new(
            "systemd-journal",
            "A slow function that takes 10 seconds and respects cancellation",
        )
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,netdata_plugin_channels=debug".to_string()),
        )
        .init();

    info!("Starting example plugin");

    let mut runtime = PluginRuntime::new("example");
    runtime.register_handler(HelloFastHandler);
    runtime.register_handler(Journal);
    runtime.run().await?;

    Ok(())
}
