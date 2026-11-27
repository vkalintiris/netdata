//! Keeps the GenAI price table current: once a day, the table's published data replaces the table model calls are
//! priced from. Until the first download succeeds, and on a host that cannot reach the address, calls are priced
//! from the table that is part of the binary.

use anyhow::Context;
use otel_derived_metrics::{DerivedMetrics, GENAI_PRICES_URL};
use std::sync::Arc;
use std::time::Duration;

/// How long a downloaded table is used before the next download.
const REFRESH_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after a failed download the next one is tried.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// How long one download may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// The largest body read: the published table is under 1 MiB.
const MAX_TABLE_BYTES: usize = 16 * 1024 * 1024;

/// Download the table now and then once a day, for as long as the task lives.
pub(crate) async fn refresh(derived_metrics: Arc<DerivedMetrics>) {
    let client = match reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                "GenAI prices: no HTTP client ({error}); the table that is part of the plugin stays in use"
            );
            return;
        }
    };

    loop {
        let wait = match download(&client).await {
            Ok(table) => match derived_metrics.replace_genai_prices(&table) {
                Ok(()) => {
                    tracing::info!(
                        "GenAI prices: now using the table downloaded from {GENAI_PRICES_URL} ({} bytes)",
                        table.len()
                    );
                    REFRESH_EVERY
                }
                Err(error) => {
                    tracing::warn!(
                        "GenAI prices: {GENAI_PRICES_URL} answered data that is {error}; the table in use stays"
                    );
                    RETRY_AFTER
                }
            },
            Err(error) => {
                tracing::warn!("GenAI prices: {error:#}; the table in use stays");
                RETRY_AFTER
            }
        };
        tokio::time::sleep(wait).await;
    }
}

async fn download(client: &reqwest::Client) -> anyhow::Result<Vec<u8>> {
    let mut response = client
        .get(GENAI_PRICES_URL)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("downloading {GENAI_PRICES_URL}"))?;
    let mut table = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("reading {GENAI_PRICES_URL}"))?
    {
        anyhow::ensure!(
            table.len() + chunk.len() <= MAX_TABLE_BYTES,
            "{GENAI_PRICES_URL} answered more than {MAX_TABLE_BYTES} bytes"
        );
        table.extend_from_slice(&chunk);
    }
    Ok(table)
}
