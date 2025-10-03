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
    #[serde(default)]
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

        // Simulate slow work - 2 seconds total
        for i in 0..4 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            info!("Slow function progress: {i}",);
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
        let mut func_decl = FunctionDeclaration::new(
            "systemd-journal",
            "A slow function that takes 10 seconds and respects cancellation",
        );
        func_decl.global = true;
        func_decl.tags = Some(String::from("logs"));
        func_decl
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

    // Check if we should use TCP or stdio based on command line argument
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--tcp" {
        // TCP mode: expect address as second argument
        let addr = args.get(2).unwrap_or(&"127.0.0.1:9999".to_string()).clone();
        info!("Running in TCP mode, connecting to {}", addr);
        run_tcp_mode(&addr).await?;
    } else {
        // Default stdio mode
        info!("Running in stdio mode");
        run_stdio_mode().await?;
    }

    Ok(())
}

/// Run the plugin using stdin/stdout (default Netdata mode)
async fn run_stdio_mode() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut runtime = PluginRuntime::new("example");
    runtime.register_handler(HelloFastHandler);
    runtime.register_handler(Journal);
    runtime.run().await?;
    Ok(())
}

/// Run the plugin using a TCP connection
async fn run_tcp_mode(addr: &str) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use tokio::net::TcpStream;

    info!("Connecting to TCP server at {}", addr);
    let stream = TcpStream::connect(addr).await?;
    info!("Connected to TCP server");

    let (reader, writer) = stream.into_split();

    let mut runtime = PluginRuntime::with_streams("example", reader, writer);
    runtime.register_handler(HelloFastHandler);
    runtime.register_handler(Journal);
    runtime.run().await?;

    Ok(())
}
