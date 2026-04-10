use std::collections::HashMap;
use std::time::Duration;

use bridge::config::{LogsConfig, RetentionEntry};
use bytesize::ByteSize;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub(super) struct LogsOverride {
    #[serde(default)]
    pub(super) wal: Option<WalOverride>,
    #[serde(default)]
    pub(super) index: Option<IndexOverride>,
    #[serde(default)]
    pub(super) storage: Option<StorageOverride>,
    #[serde(default)]
    pub(super) retention: Option<HashMap<String, RetentionEntry>>,
    #[serde(default)]
    pub(super) auth: Option<AuthOverride>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct IndexOverride {
    #[serde(default)]
    pub(super) dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct WalOverride {
    #[serde(default)]
    pub(super) dir: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_bytesize")]
    pub(super) max_file_size: Option<ByteSize>,
    #[serde(default)]
    pub(super) max_log_entries: Option<usize>,
    #[serde(default, deserialize_with = "deserialize_opt_duration")]
    pub(super) max_file_duration: Option<Duration>,
    #[serde(default)]
    pub(super) crc_enabled: Option<bool>,
    #[serde(default)]
    pub(super) compression_enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct StorageOverride {
    #[serde(default)]
    pub(super) enabled: Option<bool>,
    #[serde(default)]
    pub(super) uri: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct AuthOverride {
    #[serde(default)]
    pub(super) enabled: Option<bool>,
}

impl AuthOverride {
    pub(super) fn has_any(&self) -> bool {
        self.enabled.is_some()
    }
}

impl LogsOverride {
    pub(super) fn has_any(&self) -> bool {
        self.wal.as_ref().is_some_and(|w| w.has_any())
            || self.index.as_ref().is_some_and(|i| i.has_any())
            || self.storage.as_ref().is_some_and(|s| s.has_any())
            || self.retention.is_some()
            || self.auth.as_ref().is_some_and(|a| a.has_any())
    }
}

impl StorageOverride {
    pub(super) fn has_any(&self) -> bool {
        self.enabled.is_some() || self.uri.is_some()
    }
}

impl IndexOverride {
    pub(super) fn has_any(&self) -> bool {
        self.dir.is_some()
    }
}

impl WalOverride {
    pub(super) fn has_any(&self) -> bool {
        self.dir.is_some()
            || self.max_file_size.is_some()
            || self.max_log_entries.is_some()
            || self.max_file_duration.is_some()
            || self.crc_enabled.is_some()
            || self.compression_enabled.is_some()
    }
}

pub(super) fn apply(config: &mut LogsConfig, o: &LogsOverride) {
    if let Some(w) = &o.wal {
        if let Some(v) = &w.dir {
            config.wal.dir = v.clone();
        }
        if let Some(v) = w.max_file_size {
            config.wal.max_file_size = v;
        }
        if let Some(v) = w.max_log_entries {
            config.wal.max_log_entries = v;
        }
        if let Some(v) = w.max_file_duration {
            config.wal.max_file_duration = v;
        }
        if let Some(v) = w.crc_enabled {
            config.wal.crc_enabled = v;
        }
        if let Some(v) = w.compression_enabled {
            config.wal.compression_enabled = v;
        }
    }
    if let Some(i) = &o.index {
        if let Some(v) = &i.dir {
            config.index.dir = v.clone();
        }
    }
    if let Some(s) = &o.storage {
        if let Some(v) = s.enabled {
            config.storage.enabled = v;
        }
        if let Some(v) = &s.uri {
            config.storage.uri = v.clone();
        }
    }
    if let Some(r) = &o.retention {
        // Merge per-tenant entries: override fields replace stock fields.
        for (tenant, entry) in r {
            let target = config.retention.entry(tenant.clone()).or_default();
            if let Some(v) = entry.max_files {
                target.max_files = Some(v);
            }
            if let Some(v) = entry.max_total_size {
                target.max_total_size = Some(v);
            }
            if let Some(v) = entry.max_age {
                target.max_age = Some(v);
            }
        }
    }
    if let Some(a) = &o.auth {
        if let Some(v) = a.enabled {
            config.auth.enabled = v;
        }
    }
}

fn deserialize_opt_bytesize<'de, D>(d: D) -> Result<Option<ByteSize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
    }
}

fn deserialize_opt_duration<'de, D>(d: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => humantime::parse_duration(&s)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}
