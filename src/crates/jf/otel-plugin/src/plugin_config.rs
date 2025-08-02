use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Parser)]
#[command(name = "otel-plugin")]
#[command(about = "OpenTelemetry metrics and logs plugin.")]
#[command(version = "0.1")]
pub struct CliConfig {
    /// gRPC endpoint to listen on
    #[arg(long, default_value = "0.0.0.0:21213")]
    pub otel_endpoint: String,

    // metrics
    #[command(flatten)]
    metrics: MetricsConfig,

    /// Enable TLS/SSL for secure connections
    #[arg(long)]
    pub otel_tls_enabled: bool,

    /// Path to TLS certificate file
    #[arg(long)]
    pub otel_tls_cert_path: Option<String>,

    /// Path to TLS private key file
    #[arg(long)]
    pub otel_tls_key_path: Option<String>,

    /// Path to TLS CA certificate file for client authentication (optional)
    #[arg(long)]
    pub otel_tls_ca_cert_path: Option<String>,

    /// Print flattened metrics to stdout for debugging
    #[arg(long)]
    pub otel_metrics_print_flattened: bool,

    /// Number of samples to buffer for collection interval detection
    #[arg(long, default_value = "10")]
    pub otel_metrics_buffer_samples: usize,

    /// Maximum number of new charts to create per collection interval
    #[arg(long, default_value = "100")]
    pub otel_metrics_throttle_charts: usize,

    /// Directory to store journal files for logs
    #[arg(long, default_value = "/tmp/netdata-journals")]
    pub otel_logs_journal_dir: String,

    /// Maximum file size for journal files (in MB)
    #[arg(long, default_value = "100")]
    pub otel_logs_max_file_size_mb: u64,

    /// Maximum number of journal files to keep
    #[arg(long, default_value = "10")]
    pub otel_logs_max_files: usize,

    /// Maximum total size for all journal files (in MB)
    #[arg(long, default_value = "1000")]
    pub otel_logs_max_total_size_mb: u64,

    /// Maximum age for journal entries (in days)
    #[arg(long, default_value = "7")]
    pub otel_logs_max_entry_age_days: u64,

    /// Path to YAML configuration file to override default settings
    #[arg(long)]
    pub config_file: Option<String>,

    /// Directory containing user chart configuration YAML files
    #[arg(long)]
    pub top_chart_configs_dir: Option<String>,

    /// Collection interval (ignored)
    #[arg(help = "Collection interval in seconds (ignored)")]
    pub _update_frequency: Option<u32>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            metrics: MetricsConfig::default(),
            otel_metrics_print_flattened: false,
            otel_metrics_buffer_samples: 10,
            otel_metrics_throttle_charts: 100,
            otel_endpoint: String::from("0.0.0.0:21213"),
            otel_tls_enabled: false,
            otel_tls_cert_path: None,
            otel_tls_key_path: None,
            otel_tls_ca_cert_path: None,
            otel_logs_journal_dir: String::from("/tmp/netdata-journals"),
            otel_logs_max_file_size_mb: 100,
            otel_logs_max_files: 10,
            otel_logs_max_total_size_mb: 1000,
            otel_logs_max_entry_age_days: 7,
            config_file: None,
            top_chart_configs_dir: None,
            _update_frequency: None,
        }
    }
}

impl CliConfig {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let config = Self::parse();

        // Validate configuration
        if config.otel_metrics_buffer_samples == 0 {
            return Err("buffer_samples must be greater than 0".into());
        }

        if config.otel_metrics_throttle_charts == 0 {
            return Err("throttle_charts must be greater than 0".into());
        }

        // Validate endpoint format (basic check)
        if !config.otel_endpoint.contains(':') {
            return Err("endpoint must be in format host:port".into());
        }

        // Validate TLS configuration
        if config.otel_tls_enabled {
            if config.otel_tls_cert_path.is_none() {
                return Err("TLS certificate path must be provided when TLS is enabled".into());
            }
            if config.otel_tls_key_path.is_none() {
                return Err("TLS private key path must be provided when TLS is enabled".into());
            }
        }

        Ok(config)
    }

    pub fn create_plugin_config(&self) -> Result<PluginConfig, Box<dyn std::error::Error>> {
        if let Some(config_file) = &self.config_file {
            PluginConfig::from_yaml_file(config_file)
        } else {
            Ok(PluginConfig::from_cli_config(self))
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
    #[arg(long, default_value = "10")]
    pub buffer_samples: usize,

    /// Maximum number of new charts to create per collection interval
    #[arg(long, default_value = "100")]
    pub throttle_charts: usize,

    /// Directory to store journal files for logs
    #[arg(long, default_value = Some("/foo/otel.d"))]
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

impl MetricsConfig {
    pub fn from_cli_config(cli_config: &CliConfig) -> Self {
        Self {
            print_flattened: cli_config.otel_metrics_print_flattened,
            buffer_samples: cli_config.otel_metrics_buffer_samples,
            throttle_charts: cli_config.otel_metrics_throttle_charts,
            chart_configs_dir: cli_config.top_chart_configs_dir.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LogsConfig {
    pub journal_dir: String,
    pub max_file_size_mb: u64,
    pub max_files: usize,
    pub max_total_size_mb: u64,
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

impl LogsConfig {
    pub fn from_cli_config(cli_config: &CliConfig) -> Self {
        Self {
            journal_dir: cli_config.otel_logs_journal_dir.clone(),
            max_file_size_mb: cli_config.otel_logs_max_file_size_mb,
            max_files: cli_config.otel_logs_max_files,
            max_total_size_mb: cli_config.otel_logs_max_total_size_mb,
            max_entry_age_days: cli_config.otel_logs_max_entry_age_days,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    pub enabled: bool,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub ca_cert_path: Option<String>,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cert_path: None,
            key_path: None,
            ca_cert_path: None,
        }
    }
}

impl TlsConfig {
    pub fn from_cli_config(cli_config: &CliConfig) -> Self {
        Self {
            enabled: cli_config.otel_tls_enabled,
            cert_path: cli_config.otel_tls_cert_path.clone(),
            key_path: cli_config.otel_tls_key_path.clone(),
            ca_cert_path: cli_config.otel_tls_ca_cert_path.clone(),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    pub metrics_config: MetricsConfig,
    pub logs_config: LogsConfig,
    pub tls_config: TlsConfig,
}

impl PluginConfig {
    pub fn new(
        metrics_config: &MetricsConfig,
        logs_config: &LogsConfig,
        tls_config: &TlsConfig,
    ) -> Self {
        Self {
            metrics_config: metrics_config.clone(),
            logs_config: logs_config.clone(),
            tls_config: tls_config.clone(),
        }
    }

    pub fn from_yaml_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let config: PluginConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    pub fn from_cli_config(cli_config: &CliConfig) -> Self {
        let metrics_config = MetricsConfig::from_cli_config(cli_config);
        let logs_config = LogsConfig::from_cli_config(cli_config);
        let tls_config = TlsConfig::from_cli_config(cli_config);

        Self::new(&metrics_config, &logs_config, &tls_config)
    }

    pub fn to_yaml_file<P: AsRef<Path>>(&self, path: P) -> Result<(), Box<dyn std::error::Error>> {
        let yaml_string = serde_yaml::to_string(self)?;
        fs::write(path, yaml_string)?;
        Ok(())
    }
}
