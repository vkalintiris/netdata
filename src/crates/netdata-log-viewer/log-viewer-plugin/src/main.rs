#![allow(dead_code)]

use async_trait::async_trait;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_schema::HttpAccess;
use rt::{FunctionHandler, PluginRuntime};
use tracing::{Level, info, instrument, span, warn};
use types::{EmptyRequest, HealthResponse, JournalRequest, JournalResponse};

#[derive(Debug, Default)]
struct HealthHandler;

#[async_trait]
impl FunctionHandler for HealthHandler {
    type Request = EmptyRequest;
    type Response = HealthResponse;

    #[instrument(name = "health_call")]
    async fn on_call(&self, _request: Self::Request) -> Result<Self::Response> {
        info!("health function called");

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

    #[instrument(name = "health_declaration")]
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

    #[instrument(name = "journal_call", skip(self))]
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

    #[instrument(name = "journal_declaration", skip(self))]
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

fn initialize_tracing() {
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::WithExportConfig;
    use tracing_perfetto::PerfettoLayer;
    use tracing_subscriber::{EnvFilter, prelude::*};

    // Create the Perfetto layer
    let perfetto_layer = PerfettoLayer::new(std::sync::Mutex::new(
        std::fs::File::create("/tmp/test.pftrace").unwrap(),
    ));

    // Create Otel layer
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint("http://localhost:4317") // Jaeger's OTLP gRPC endpoint
        .build()
        .expect("Failed to build OTLP exporter");

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name("log-viewer-plugin")
        .build();

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource)
        .build();

    let tracer = tracer_provider.tracer("log-viewer-plugin");
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Create the fmt layer with your existing configuration
    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    // Create the environment filter
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,netdata_plugin_channels=debug"));

    // Combine all layers
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(perfetto_layer)
        .with(telemetry_layer)
        .init();
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    initialize_tracing();

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
#[instrument(skip_all)]
async fn run_stdio_mode() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut runtime = PluginRuntime::new("example");

    {
        let _ = span!(Level::INFO, "register_handlers").entered();
        runtime.register_handler(Journal::default());
        runtime.register_handler(HealthHandler {});
    }

    {
        runtime.run().await?;
    }
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
