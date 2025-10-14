use anyhow::Result;
use chrono::DateTime;
use clap::Parser;
use governor::{Quota, RateLimiter as GovernorRateLimiter};
use opentelemetry::KeyValue;
use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use serde::Deserialize;
use std::num::NonZeroU32;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::Duration;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Rate limit in messages per second
    #[arg(long, default_value = "1000")]
    rate_limit_messages: u32,

    #[arg(short, long, default_value = "http://127.0.0.1:4317")]
    otel_endpoint: String,
}

#[derive(Deserialize, Debug)]
struct FlogEntry {
    host: String,
    #[serde(rename = "user-identifier")]
    user_identifier: String,
    datetime: String,
    method: String,
    request: String,
    protocol: String,
    status: u16,
    bytes: u64,
    referer: String,
}

fn parse_flog_datetime(datetime_str: &str) -> Result<DateTime<chrono::Utc>> {
    let dt = chrono::DateTime::parse_from_str(datetime_str, "%d/%b/%Y:%H:%M:%S %z")?;
    Ok(dt.with_timezone(&chrono::Utc))
}

fn status_to_severity(status: u16) -> Severity {
    match status {
        200..=299 => Severity::Info,
        300..=399 => Severity::Debug,
        400..=499 => Severity::Warn,
        500..=599 => Severity::Error,
        _ => Severity::Info,
    }
}

fn initialize_logger(endpoint: &str) -> Result<opentelemetry_sdk::logs::SdkLoggerProvider> {
    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let resource = Resource::builder()
        .with_service_name("flog-otel-wrapper")
        .with_attributes(vec![KeyValue::new("service.version", "0.1.0")])
        .build();

    let provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    Ok(provider)
}

async fn process_flog_stream<L: Logger>(
    logger: &L,
    rate_limiter: Arc<
        GovernorRateLimiter<
            governor::state::direct::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        >,
    >,
) -> Result<()> {
    let mut child = tokio::process::Command::new("flog")
        .args(["-f", "json", "-t", "stdout", "-l"])
        .stdout(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        while rate_limiter.check().is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        match serde_json::from_str::<FlogEntry>(&line) {
            Ok(entry) => match parse_flog_datetime(&entry.datetime) {
                Ok(dt) => {
                    let severity = status_to_severity(entry.status);

                    let log_message = format!(
                        "{} {} {} {} {} {}",
                        entry.host,
                        entry.method,
                        entry.request,
                        entry.protocol,
                        entry.status,
                        entry.bytes
                    );

                    let mut log_record = logger.create_log_record();
                    log_record.set_severity_number(severity);
                    log_record.set_timestamp(
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_nanos(
                                dt.timestamp_nanos_opt().unwrap_or(0) as u64,
                            ),
                    );
                    log_record.set_body(log_message.into());
                    log_record.add_attribute("host", entry.host);
                    log_record.add_attribute("method", entry.method);
                    log_record.add_attribute("request", entry.request);
                    log_record.add_attribute("protocol", entry.protocol);
                    log_record.add_attribute("status", entry.status as i64);
                    log_record.add_attribute("response_bytes", entry.bytes as i64);
                    log_record.add_attribute("referer", entry.referer);
                    log_record.add_attribute("user_identifier", entry.user_identifier);

                    logger.emit(log_record);
                }
                Err(e) => error!("Failed to parse datetime: {}", e),
            },
            Err(e) => error!("Failed to parse JSON: {}", e),
        }
    }

    child.wait().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    info!(
        "Starting flog-otel-wrapper with rate limit: {} messages/sec",
        args.rate_limit_messages
    );
    info!("OTEL endpoint: {}", args.otel_endpoint);

    let provider = initialize_logger(&args.otel_endpoint)?;
    let logger = provider.logger("flog-otel-wrapper");

    // Create governor rate limiter with messages per second
    let quota = Quota::per_second(NonZeroU32::new(args.rate_limit_messages).unwrap());
    let rate_limiter = Arc::new(GovernorRateLimiter::direct(quota));

    // Process stream forever, restarting on errors
    loop {
        if let Err(e) = process_flog_stream(&logger, rate_limiter.clone()).await {
            error!("Error processing flog stream: {}", e);
        }
    }
}
