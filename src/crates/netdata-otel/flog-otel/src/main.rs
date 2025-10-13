use anyhow::Result;
use chrono::DateTime;
use clap::Parser;
use opentelemetry::KeyValue;
use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use serde::Deserialize;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "1048576")]
    rate_limit_bytes: u64,

    #[arg(short, long, default_value = "http://127.0.0.1:19998")]
    otel_endpoint: String,

    #[arg(short, long, default_value = "1000")]
    log_count: u32,

    #[arg(long, default_value = "false")]
    loop_forever: bool,
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

struct RateLimiter {
    bytes_sent: u64,
    last_reset: Instant,
    limit: u64,
}

impl RateLimiter {
    fn new(limit: u64) -> Self {
        Self {
            bytes_sent: 0,
            last_reset: Instant::now(),
            limit,
        }
    }

    async fn can_send(&mut self, size: u64) -> bool {
        if self.last_reset.elapsed() >= Duration::from_secs(1) {
            self.bytes_sent = 0;
            self.last_reset = Instant::now();
        }

        if self.bytes_sent + size <= self.limit {
            self.bytes_sent += size;
            true
        } else {
            false
        }
    }
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
    rate_limiter: Arc<Mutex<RateLimiter>>,
    args: &Args,
) -> Result<()> {
    let log_count_str = args.log_count.to_string();
    let mut flog_args = vec!["-f", "json", "-t", "stdout"];

    if args.loop_forever {
        flog_args.extend(&["-l"]);
    } else {
        flog_args.extend(&["-n", &log_count_str]);
    }

    let mut child = tokio::process::Command::new("flog")
        .args(&flog_args)
        .stdout(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    const BATCH_SIZE: usize = 100;
    let mut batch_count = 0;

    while let Some(line) = lines.next_line().await? {
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

                    batch_count += 1;

                    if batch_count >= BATCH_SIZE {
                        let batch_size = (batch_count * 1024) as u64;
                        let mut limiter = rate_limiter.lock().await;

                        if !limiter.can_send(batch_size).await {
                            drop(limiter);
                            info!("Rate limit reached, waiting 1 second...");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }

                        batch_count = 0;
                    }
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
        "Starting flog-otel-wrapper with rate limit: {} bytes/sec",
        args.rate_limit_bytes
    );
    info!("OTEL endpoint: {}", args.otel_endpoint);

    let provider = initialize_logger(&args.otel_endpoint)?;
    let logger = provider.logger("flog-otel-wrapper");
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new(args.rate_limit_bytes)));

    loop {
        if let Err(e) = process_flog_stream(&logger, rate_limiter.clone(), &args).await {
            error!("Error processing flog stream: {}", e);
        }

        if !args.loop_forever {
            break;
        }
    }

    // Ensure all logs are flushed before shutdown
    let _ = provider.force_flush();
    let _ = provider.shutdown();

    info!("Successfully sent logs to OTEL collector via gRPC");
    Ok(())
}
