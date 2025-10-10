#![allow(dead_code)]

use async_trait::async_trait;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_schema::HttpAccess;
use rt::{FunctionHandler, PluginRuntime};
use tracing::{info, warn};
use types::{EmptyRequest, HealthResponse, JournalRequest, JournalResponse};

#[derive(Default)]
struct HealthHandler;

#[async_trait]
impl FunctionHandler for HealthHandler {
    type Request = EmptyRequest;
    type Response = HealthResponse;

    async fn on_call(&self, _request: Self::Request) -> Result<Self::Response> {
        info!("Health function called");

        let response = reqwest::get("http://localhost:8080/health").await.unwrap();
        let resp = response.json::<HealthResponse>().await.unwrap();
        println!("Response: {:?}", resp);

        Ok(resp)
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        // Health  function doesn't really need cancellation handling
        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Cancelled".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress requested for health function");
    }

    fn declaration(&self) -> FunctionDeclaration {
        let mut func_decl =
            FunctionDeclaration::new("health", "A health function that responds immediately");
        func_decl.global = true;
        func_decl.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        func_decl
    }
}

#[derive(Default)]
struct Journal {}

#[async_trait]
impl FunctionHandler for Journal {
    type Request = JournalRequest;
    type Response = JournalResponse;

    async fn on_call(&self, request: Self::Request) -> Result<Self::Response> {
        info!("Systemd journal function called: {:#?}", request);

        let client = reqwest::Client::new();
        let response = client
            .post("http://localhost:8080/journal")
            .json(&request)
            .send()
            .await
            .unwrap();
        let resp = response.json::<JournalResponse>().await.unwrap();
        println!("Response: {:?}", resp);

        Ok(resp)
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
        func_decl.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
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

    if true {
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
    }

    Ok(())
}

/// Run the plugin using stdin/stdout (default Netdata mode)
async fn run_stdio_mode() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut runtime = PluginRuntime::new("example");
    runtime.register_handler(Journal::default());
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
    runtime.register_handler(Journal::default());
    runtime.register_handler(HealthHandler {});
    runtime.run().await?;

    Ok(())
}
