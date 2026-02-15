use std::time::Duration;

/// When to rotate a WAL file and start a new one.
#[derive(Debug, Clone)]
pub struct RotationConfig {
    pub max_log_entries: usize,
    pub max_file_size: u64,
    pub max_duration: Option<Duration>,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            max_log_entries: 100_000,
            max_file_size: 256 * 1024 * 1024,
            max_duration: Some(Duration::from_secs(3600)),
        }
    }
}

/// Configuration for the WAL writer.
#[derive(Debug, Clone)]
pub struct Config {
    pub rotation: RotationConfig,
    pub crc_enabled: bool,
    pub compression_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rotation: RotationConfig::default(),
            crc_enabled: true,
            compression_enabled: true,
        }
    }
}
