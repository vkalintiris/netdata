//! otel-plugin library - can be called from multi-call binaries or standalone

use anyhow::{Context, Result};
use opentelemetry_proto::tonic::collector::{
    logs::v1::logs_service_server::LogsServiceServer,
    metrics::v1::metrics_service_server::MetricsServiceServer,
};
use tonic::transport::{Identity, Server, ServerTlsConfig};

mod chart_config;
mod flattened_point;
mod netdata_chart;
mod netdata_env;
mod regex_cache;
mod samples_table;

mod plugin_config;
use crate::plugin_config::PluginConfig;

mod logs_service;
use crate::logs_service::NetdataLogsService;

mod metrics_service;
use crate::metrics_service::NetdataMetricsService;

async fn send_keepalive_periodically() {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

    loop {
        interval.tick().await;
        println!("PLUGIN_KEEPALIVE");
    }
}

fn initialize_tracing() {
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::WithExportConfig;
    use tracing_subscriber::{EnvFilter, prelude::*};

    // Create Otel layer
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint("http://localhost:4318")
        .build()
        .expect("Failed to build OTLP exporter");

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name("otel-plugin")
        .build();

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource)
        .build();

    let tracer = tracer_provider.tracer("otel-plugin");
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Create the fmt layer with your existing configuration
    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    // Create the environment filter
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("debug,histogram-backend=debug,tokio=trace,runtime=trace")
    });

    // Combine all layers
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(telemetry_layer)
        .init();
}

/// Entry point for otel-plugin - can be called from multi-call binary
///
/// # Arguments
/// * `args` - Command-line arguments (should include argv[0] as "otel-plugin")
///
/// # Returns
/// Exit code (0 for success, non-zero for errors)
pub fn run(args: Vec<String>) -> i32 {
    // otel-plugin is async, so we need a tokio runtime
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async_run(args))
}

async fn async_run(_args: Vec<String>) -> i32 {
    initialize_tracing();

    match run_internal().await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {:#}", e);
            1
        }
    }
}

async fn run_internal() -> Result<()> {
    let config = PluginConfig::new().context("Failed to initialize plugin configuration")?;

    let addr =
        config.endpoint.path.parse().with_context(|| {
            format!("Failed to parse endpoint address: {}", config.endpoint.path)
        })?;
    let metrics_service =
        NetdataMetricsService::new(config.clone()).context("Failed to create metrics service")?;
    let logs_service =
        NetdataLogsService::new(config.clone()).context("Failed to create logs service")?;

    println!("TRUST_DURATIONS 1");

    let mut server_builder = Server::builder();

    // Configure TLS if provided
    if let (Some(cert_path), Some(key_path)) = (
        &config.endpoint.tls_cert_path,
        &config.endpoint.tls_key_path,
    ) {
        let cert = std::fs::read(cert_path)
            .with_context(|| format!("Failed to read TLS certificate from: {}", cert_path))?;
        let key = std::fs::read(key_path)
            .with_context(|| format!("Failed to read TLS private key from: {}", key_path))?;
        let identity = Identity::from_pem(cert, key);

        let mut tls_config_builder = ServerTlsConfig::new().identity(identity);

        // If CA certificate is provided, enable client authentication
        if let Some(ref ca_cert_path) = config.endpoint.tls_ca_cert_path {
            let ca_cert = std::fs::read(ca_cert_path)
                .with_context(|| format!("Failed to read CA certificate from: {}", ca_cert_path))?;
            tls_config_builder =
                tls_config_builder.client_ca_root(tonic::transport::Certificate::from_pem(ca_cert));
        }

        server_builder = server_builder
            .tls_config(tls_config_builder)
            .context("Failed to configure TLS")?;
    } else {
        eprintln!(
            "TLS disabled, using insecure connection on endpoint: {}",
            config.endpoint.path
        );
    }

    let server = server_builder
        .add_service(
            MetricsServiceServer::new(metrics_service)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .add_service(
            LogsServiceServer::new(logs_service)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .serve(addr);

    let keepalive = send_keepalive_periodically();

    tokio::select! {
        result = server => {
            result.with_context(|| format!("Failed to serve gRPC server on {}", addr))?;
        }
        _ = keepalive => {
            // This should never complete
        }
    }

    Ok(())
}
