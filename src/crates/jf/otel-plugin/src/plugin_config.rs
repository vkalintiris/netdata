use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EndpointConfig {
    /// gRPC endpoint to listen on
    #[arg(long = "otel-endpoint", default_value = "0.0.0.0:21213")]
    pub path: String,

    /// Path to TLS certificate file (enables TLS when provided)
    #[arg(long = "otel-tls-cert-path")]
    pub tls_cert_path: Option<String>,

    /// Path to TLS private key file (required when TLS certificate is provided)
    #[arg(long = "otel-tls-key-path")]
    pub tls_key_path: Option<String>,

    /// Path to TLS CA certificate file for client authentication (optional)
    #[arg(long = "otel-tls-ca-cert-path")]
    pub tls_ca_cert_path: Option<String>,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            path: String::from("0.0.0.0:21213"),
            tls_cert_path: None,
            tls_key_path: None,
            tls_ca_cert_path: None,
        }
    }
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    /// Print flattened metrics to stdout for debugging
    #[arg(long = "otel-metrics-print-flattened")]
    pub print_flattened: bool,

    /// Number of samples to buffer for collection interval detection
    #[arg(long = "otel-metrics-buffer-samples", default_value = "10")]
    pub buffer_samples: usize,

    /// Maximum number of new charts to create per collection interval
    #[arg(long = "otel-metrics-throttle-charts", default_value = "100")]
    pub throttle_charts: usize,

    /// Directory to store journal files for logs
    #[arg(long = "otel-metrics-charts-configs-dir", default_value = Some("/foo/otel.d"))]
    pub chart_configs_dir: Option<String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            print_flattened: false,
            buffer_samples: 10,
            throttle_charts: 100,
            chart_configs_dir: None,
        }
    }
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LogsConfig {
    /// Directory to store journal files for logs
    #[arg(
        long = "otel-logs-journal-dir",
        default_value = "/tmp/netdata-journals"
    )]
    pub journal_dir: String,

    /// Maximum file size for journal files (in MB)
    #[arg(long = "otel-logs-max-file-size-mb", default_value = "100")]
    pub max_file_size_mb: u64,

    /// Maximum number of journal files to keep
    #[arg(long = "otel-logs-max-files", default_value = "10")]
    pub max_files: usize,

    /// Maximum total size for all journal files (in MB)
    #[arg(long = "otel-logs-max-total-size-mb", default_value = "1000")]
    pub max_total_size_mb: u64,

    /// Maximum age for journal entries (in days)
    #[arg(long = "otel-logs-max-entry-age-days", default_value = "7")]
    pub max_entry_age_days: u64,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            journal_dir: String::from("/tmp/netdata-journals"),
            max_file_size_mb: 100,
            max_files: 10,
            max_total_size_mb: 1000,
            max_entry_age_days: 7,
        }
    }
}

#[derive(Debug, Parser, Clone, Serialize, Deserialize)]
#[command(name = "otel-plugin")]
#[command(about = "OpenTelemetry metrics and logs plugin.")]
#[command(version = "0.1")]
#[serde(default)]
pub struct PluginConfig {
    // endpoint configuration (includes grpc endpoint and tls)
    #[command(flatten)]
    #[serde(rename = "endpoint")]
    pub endpoint: EndpointConfig,

    // metrics
    #[command(flatten)]
    #[serde(rename = "metrics")]
    pub metrics: MetricsConfig,

    // logs
    #[command(flatten)]
    #[serde(rename = "logs")]
    pub logs: LogsConfig,

    /// Collection interval (ignored)
    #[arg(help = "Collection interval in seconds (ignored)")]
    #[serde(skip)]
    pub _update_frequency: Option<u32>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            endpoint: EndpointConfig::default(),
            metrics: MetricsConfig::default(),
            logs: LogsConfig::default(),
            _update_frequency: None,
        }
    }
}

impl PluginConfig {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let config = if atty::is(atty::Stream::Stdout) {
            // Running from terminal, use CLI arguments
            Self::parse()
        } else {
            // Not running from terminal, load from YAML file
            match Self::from_yaml_file("/tmp/foo.yaml") {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("Failed to load config from /tmp/foo.yaml: {}", e);
                    eprintln!("Falling back to CLI argument parsing");
                    Self::parse()
                }
            }
        };

        // Validate configuration
        if config.metrics.buffer_samples == 0 {
            return Err("buffer_samples must be greater than 0".into());
        }

        if config.metrics.throttle_charts == 0 {
            return Err("throttle_charts must be greater than 0".into());
        }

        // Validate endpoint format (basic check)
        if !config.endpoint.path.contains(':') {
            return Err("endpoint must be in format host:port".into());
        }

        // Validate TLS configuration
        match (
            &config.endpoint.tls_cert_path,
            &config.endpoint.tls_key_path,
        ) {
            (Some(cert_path), Some(key_path)) => {
                if cert_path.is_empty() {
                    return Err("TLS certificate path cannot be empty when provided".into());
                }
                if key_path.is_empty() {
                    return Err("TLS private key path cannot be empty when provided".into());
                }
            }
            (Some(_), None) => {
                return Err(
                    "TLS private key path must be provided when TLS certificate is provided".into(),
                );
            }
            (None, Some(_)) => {
                return Err(
                    "TLS certificate path must be provided when TLS private key is provided".into(),
                );
            }
            (None, None) => {
                // TLS disabled, which is fine
            }
        }

        Ok(config)
    }

    pub fn from_yaml_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let config: PluginConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }
}
