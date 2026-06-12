use std::time::Duration;

use opentelemetry_proto::tonic::{
    collector::logs::v1::logs_service_client::LogsServiceClient, logs::v1::LogRecord,
};
use tokio::sync::mpsc;
use tokio::time;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tracing::{error, info};

use crate::otel::build_export_request;

pub struct OtelConfig {
    pub endpoint: String,
    pub batch_size: usize,
    pub flush_interval: Duration,
    pub tenant_id: Option<String>,
    pub service_name: &'static str,
    pub scope_name: &'static str,
    pub scope_version: &'static str,
}

pub struct Sender {
    client: LogsServiceClient<Channel>,
    batch: Vec<LogRecord>,
    batch_size: usize,
    flush_interval: Duration,
    rx: mpsc::Receiver<LogRecord>,
    tenant_header: Option<MetadataValue<tonic::metadata::Ascii>>,
    service_name: &'static str,
    scope_name: &'static str,
    scope_version: &'static str,
}

impl Sender {
    pub async fn new(config: OtelConfig, rx: mpsc::Receiver<LogRecord>) -> anyhow::Result<Self> {
        let tenant_header = match config.tenant_id {
            Some(ref id) => Some(
                id.parse()
                    .map_err(|e| anyhow::anyhow!("invalid --tenant-id: {e}"))?,
            ),
            None => None,
        };

        let endpoint = &config.endpoint;
        let channel_endpoint = Channel::from_shared(endpoint.to_string())?;
        let mut attempt = 0u64;
        let mut backoff = Duration::from_secs(1);
        const MAX_CONNECT_BACKOFF: Duration = Duration::from_secs(30);
        let channel = loop {
            attempt += 1;
            info!(
                "Connecting to OTel endpoint: {} (attempt {attempt})",
                endpoint
            );
            match channel_endpoint.connect().await {
                Ok(ch) => break ch,
                Err(e) => {
                    info!(
                        "OTel endpoint not ready: {e}, retrying in {}s...",
                        backoff.as_secs()
                    );
                    time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_CONNECT_BACKOFF);
                }
            }
        };
        let client = LogsServiceClient::new(channel);
        info!("Connected to OTel endpoint");

        Ok(Self {
            client,
            batch: Vec::with_capacity(config.batch_size),
            batch_size: config.batch_size,
            flush_interval: config.flush_interval,
            rx,
            tenant_header,
            service_name: config.service_name,
            scope_name: config.scope_name,
            scope_version: config.scope_version,
        })
    }

    pub async fn run(&mut self) {
        let mut flush_timer = time::interval(self.flush_interval);
        flush_timer.tick().await;

        loop {
            tokio::select! {
                record = self.rx.recv() => {
                    match record {
                        Some(record) => {
                            self.batch.push(record);
                            if self.batch.len() >= self.batch_size {
                                self.flush().await;
                                flush_timer.reset();
                            }
                        }
                        None => {
                            if !self.batch.is_empty() {
                                self.flush().await;
                            }
                            info!("Event channel closed, sender shutting down");
                            return;
                        }
                    }
                }
                _ = flush_timer.tick() => {
                    if !self.batch.is_empty() {
                        self.flush().await;
                    }
                }
            }
        }
    }

    async fn flush(&mut self) {
        let records = std::mem::replace(&mut self.batch, Vec::with_capacity(self.batch_size));
        let count = records.len();
        let export = build_export_request(
            records,
            self.service_name,
            self.scope_name,
            self.scope_version,
        );
        let mut request = tonic::Request::new(export);

        if let Some(ref value) = self.tenant_header {
            request
                .metadata_mut()
                .insert("x-scope-orgid", value.clone());
        }

        match self.client.export(request).await {
            Ok(_response) => {
                info!(count, "Flushed batch");
            }
            Err(e) => {
                error!(count, error = %e, "Failed to send batch");
            }
        }
    }
}
