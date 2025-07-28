use clap::Parser;

use crate::chart_config::ChartConfigManager;

#[derive(Debug, Parser)]
#[command(name = "otel-plugin")]
#[command(about = "OpenTelemetry metrics and logs plugin.")]
#[command(version = "0.1")]
pub struct CliConfig {
    /// gRPC endpoint to listen on
    #[arg(long, default_value = "0.0.0.0:21213")]
    pub otel_endpoint: String,

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

    /// Collection interval (ignored)
    #[arg(help = "Collection interval in seconds (ignored)")]
    pub _update_frequency: Option<u32>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            otel_metrics_print_flattened: false,
            otel_metrics_buffer_samples: 10,
            otel_metrics_throttle_charts: 100,
            otel_endpoint: String::from("0.0.0.0:21213"),
            otel_logs_journal_dir: String::from("/tmp/netdata-journals"),
            otel_logs_max_file_size_mb: 100,
            otel_logs_max_files: 10,
            otel_logs_max_total_size_mb: 1000,
            otel_logs_max_entry_age_days: 7,
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

        Ok(config)
    }
}

#[derive(Debug, Clone)]
pub struct MetricsConfig {
    pub print_flattened: bool,
    pub buffer_samples: usize,
    pub throttle_charts: usize,
    pub chart_config_manager: ChartConfigManager,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            print_flattened: false,
            buffer_samples: 10,
            throttle_charts: 100,
            chart_config_manager: ChartConfigManager::with_default_configs(),
        }
    }
}

impl MetricsConfig {
    pub fn from_cli_config(cli_config: &CliConfig) -> Self {
        Self {
            print_flattened: cli_config.otel_metrics_print_flattened,
            buffer_samples: cli_config.otel_metrics_buffer_samples,
            throttle_charts: cli_config.otel_metrics_throttle_charts,
            chart_config_manager: ChartConfigManager::with_default_configs(),
        }
    }
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Default)]
pub struct PluginConfig {
    pub metrics_config: MetricsConfig,
    pub logs_config: LogsConfig,
}

impl PluginConfig {
    pub fn new(metrics_config: &MetricsConfig, logs_config: &LogsConfig) -> Self {
        Self {
            metrics_config: metrics_config.clone(),
            logs_config: logs_config.clone(),
        }
    }
}
