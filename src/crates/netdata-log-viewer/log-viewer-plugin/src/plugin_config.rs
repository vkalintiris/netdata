use anyhow::{Context, Result};
use bytesize::ByteSize;
use clap::Parser;
use rt::NetdataEnv;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Default value for workers (number of CPU cores)
fn default_workers() -> usize {
    num_cpus::get()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalConfig {
    /// Path to systemd journal directory to watch
    pub path: String,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            path: String::from("/var/log/journal"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// Directory to store the hybrid cache (memory + disk)
    pub directory: String,

    /// Memory cache capacity (number of entries to cache in memory)
    pub memory_capacity: usize,

    /// Disk cache size (total size of disk-backed cache)
    #[serde(with = "bytesize_serde")]
    pub disk_capacity: ByteSize,

    /// Cache block size (size of cache blocks)
    #[serde(with = "bytesize_serde")]
    pub block_size: ByteSize,

    /// Number of background workers for indexing journal files
    #[serde(default = "default_workers")]
    pub workers: usize,

    /// Queue capacity for pending indexing requests
    pub queue_capacity: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            directory: String::from("/var/cache/netdata/log-viewer"),
            memory_capacity: 1000,
            disk_capacity: ByteSize::mb(32),
            block_size: ByteSize::mb(4),
            workers: default_workers(),
            queue_capacity: 100,
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Journal source configuration
    #[serde(rename = "journal")]
    pub journal: JournalConfig,

    /// Cache configuration
    #[serde(rename = "cache")]
    pub cache: CacheConfig,
}

#[derive(Debug, Parser)]
#[command(name = "log-viewer-plugin")]
#[command(about = "Netdata systemd journal log viewer plugin")]
#[command(version = "0.1")]
pub struct CliArgs {
    /// Path to configuration file (overrides automatic Netdata config lookup)
    #[arg(long = "config")]
    pub config: Option<PathBuf>,

    /// Collection interval (ignored, kept for compatibility with Netdata)
    #[arg(hide = true, help = "Collection interval in seconds (ignored)")]
    pub _update_frequency: Option<u32>,
}

pub struct PluginConfig {
    pub config: Config,
    pub netdata_env: NetdataEnv,
}

impl PluginConfig {
    /// Load configuration from Netdata environment or CLI arguments
    pub fn new() -> Result<Self> {
        let netdata_env = NetdataEnv::from_environment();
        let cli_args = CliArgs::parse();

        let mut config = if let Some(config_path) = cli_args.config {
            // Explicit config file provided via --config
            Config::from_yaml_file(&config_path).with_context(|| {
                format!("Loading config from {}", config_path.display())
            })?
        } else if netdata_env.running_under_netdata() {
            // Running under Netdata - try user config first, fallback to stock config
            let user_config = netdata_env
                .user_config_dir
                .as_ref()
                .map(|path| path.join("log-viewer.yml"))
                .and_then(|path| {
                    if path.exists() {
                        Config::from_yaml_file(&path)
                            .with_context(|| format!("Loading user config from {}", path.display()))
                            .ok()
                    } else {
                        None
                    }
                });

            if let Some(config) = user_config {
                config
            } else if let Some(stock_path) = netdata_env
                .stock_config_dir
                .as_ref()
                .map(|p| p.join("log-viewer.yml"))
            {
                if stock_path.exists() {
                    Config::from_yaml_file(&stock_path).with_context(|| {
                        format!("Loading stock config from {}", stock_path.display())
                    })?
                } else {
                    // No config files found, use defaults
                    Config::default()
                }
            } else {
                // No config directories available, use defaults
                Config::default()
            }
        } else {
            // Not running under Netdata and no --config provided, use defaults
            Config::default()
        };

        // Resolve relative paths
        config.cache.directory = resolve_relative_path(
            &config.cache.directory,
            netdata_env.cache_dir.as_deref(),
        );

        // Validate configuration
        Self::validate(&config)?;

        Ok(PluginConfig {
            config,
            netdata_env,
        })
    }

    /// Validate configuration values
    fn validate(config: &Config) -> Result<()> {
        if config.cache.memory_capacity == 0 {
            anyhow::bail!("cache.memory_capacity must be greater than 0");
        }

        if config.cache.disk_capacity.as_u64() == 0 {
            anyhow::bail!("cache.disk_capacity must be greater than 0");
        }

        if config.cache.block_size.as_u64() == 0 {
            anyhow::bail!("cache.block_size must be greater than 0");
        }

        if config.cache.workers == 0 {
            anyhow::bail!("cache.workers must be greater than 0");
        }

        if config.cache.queue_capacity == 0 {
            anyhow::bail!("cache.queue_capacity must be greater than 0");
        }

        // Validate that journal path exists (warning only)
        if !Path::new(&config.journal.path).exists() {
            eprintln!(
                "WARNING: Journal path does not exist: {}",
                config.journal.path
            );
        }

        Ok(())
    }
}

impl Config {
    /// Load configuration from a YAML file
    pub fn from_yaml_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Config = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse YAML config file: {}", path.display()))?;
        Ok(config)
    }
}

/// Helper function to resolve relative paths against a base directory
fn resolve_relative_path(path: &str, base_dir: Option<&Path>) -> String {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_string_lossy().to_string()
    } else if let Some(base) = base_dir {
        base.join(path).to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}
