//! Keeps the GenAI price table current: once a day, the table's published data replaces the table model calls are
//! priced from.
//!
//! The downloaded table is kept in the Agent's cache directory, so a restart prices from it at once and does not
//! download again before the table is a day old. Until a first download succeeds, and on a host that cannot reach
//! the address, calls are priced from the table that is part of the binary.

use anyhow::Context;
use otel_derived_metrics::{DerivedMetrics, GENAI_PRICES_URL};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// How long a downloaded table is used before the next download.
const REFRESH_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after a failed download the next one is tried.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// How long one download may take: enough for the table over a link of a few kilobytes a second. Nothing waits
/// for it.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The largest table read, from the address or from the cache directory: the published table is under 1 MiB.
const MAX_TABLE_BYTES: usize = 16 * 1024 * 1024;

/// The environment variable in which the Agent names its cache directory.
const CACHE_DIR_VARIABLE: &str = "NETDATA_CACHE_DIR";

/// Where the downloaded table is kept, under the cache directory.
const STORED_TABLE: &str = "otel/genai-prices.json";

/// The file the downloaded table is kept in; `None` when the Agent names no cache directory, and then every start
/// downloads.
pub(crate) fn stored_table_path() -> Option<PathBuf> {
    let cache_dir = std::env::var_os(CACHE_DIR_VARIABLE)?;
    Some(Path::new(&cache_dir).join(STORED_TABLE))
}

/// Use the stored table if there is one, then download the table whenever the one in use is a day old, for as long
/// as the task lives.
pub(crate) async fn refresh(derived_metrics: Arc<DerivedMetrics>, stored: Option<PathBuf>) {
    let client = match reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                "GenAI prices: no HTTP client ({error}); the table that is part of the plugin stays in use"
            );
            return;
        }
    };

    let mut wait = Duration::ZERO;
    if let Some(path) = &stored {
        let newer_than = binary_modified();
        if let Some((table, age)) = read_stored(path, newer_than, SystemTime::now()) {
            match derived_metrics.replace_genai_prices(&table) {
                Ok(()) => {
                    tracing::info!(
                        "GenAI prices: using the table stored in {} {} hours ago",
                        path.display(),
                        age.as_secs() / 3600
                    );
                    wait = REFRESH_EVERY.saturating_sub(age);
                }
                Err(error) => {
                    tracing::warn!(
                        "GenAI prices: {} is {error}; a download replaces it",
                        path.display()
                    );
                }
            }
        }
    }

    loop {
        tokio::time::sleep(wait).await;
        wait = match download(&client).await {
            Ok(table) => match derived_metrics.replace_genai_prices(&table) {
                Ok(()) => {
                    tracing::info!(
                        "GenAI prices: now using the table downloaded from {GENAI_PRICES_URL} ({} bytes)",
                        table.len()
                    );
                    if let Some(path) = &stored
                        && let Err(error) = store(path, &table)
                    {
                        // The table is in use all the same; only the next start downloads again.
                        tracing::warn!(
                            "GenAI prices: the table is not kept for the next start: {error:#}"
                        );
                    }
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
    }
}

/// When this binary was written to disk; `None` when that cannot be told.
fn binary_modified() -> Option<SystemTime> {
    std::fs::metadata(std::env::current_exe().ok()?)
        .ok()?
        .modified()
        .ok()
}

/// The table stored in `path` and how long ago, as of `now`, it was stored. `None` when there is none to use:
/// no file, one that cannot be read or is too large, or one stored before `newer_than`, when this binary was
/// installed: the table that is part of a newer binary is the newer of the two.
fn read_stored(
    path: &Path,
    newer_than: Option<SystemTime>,
    now: SystemTime,
) -> Option<(Vec<u8>, Duration)> {
    let metadata = std::fs::metadata(path).ok()?;
    let stored_at = metadata.modified().ok()?;
    if newer_than.is_some_and(|newer_than| stored_at < newer_than) {
        return None;
    }
    if metadata.len() > MAX_TABLE_BYTES as u64 {
        return None;
    }
    let table = std::fs::read(path).ok()?;
    // A file dated in the future counts as just stored.
    let age = now.duration_since(stored_at).unwrap_or(Duration::ZERO);
    Some((table, age))
}

/// Keep `table` in `path`, whole or not at all.
fn store(path: &Path, table: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    file_registry::durable::write_atomic(path, table)
        .with_context(|| format!("writing {}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn a_stored_table_is_read_back_with_its_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STORED_TABLE);
        assert_eq!(read_stored(&path, None, SystemTime::now()), None);

        store(&path, b"table").unwrap();
        let stored_at = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            read_stored(&path, None, stored_at + 3 * HOUR),
            Some((b"table".to_vec(), 3 * HOUR))
        );
        // A clock that went back does not make the table older than new.
        assert_eq!(
            read_stored(&path, None, stored_at - HOUR),
            Some((b"table".to_vec(), Duration::ZERO))
        );

        // A second table replaces the first and leaves nothing else behind.
        store(&path, b"newer table").unwrap();
        assert_eq!(
            read_stored(&path, None, stored_at).map(|(table, _)| table),
            Some(b"newer table".to_vec())
        );
        let files: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, vec!["genai-prices.json"]);
    }

    #[test]
    fn a_table_stored_before_the_binary_was_installed_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STORED_TABLE);
        store(&path, b"table").unwrap();
        let stored_at = std::fs::metadata(&path).unwrap().modified().unwrap();
        let now = stored_at + 2 * HOUR;

        assert_eq!(read_stored(&path, Some(stored_at + HOUR), now), None);
        assert!(read_stored(&path, Some(stored_at - HOUR), now).is_some());
        assert!(read_stored(&path, Some(stored_at), now).is_some());
    }
}
