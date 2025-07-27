use clap::Parser;
use std::path::PathBuf;

use crate::chart_config::ChartConfigManager;

#[derive(Debug, Parser)]
#[command(name = "otel-plugin")]
#[command(about = "OpenTelemetry metrics and logs plugin.")]
#[command(version = "0.1")]
pub struct CliConfig {
    /// Print flattened metrics to stdout for debugging
    #[arg(long)]
    pub otel_metrics_print_flattened: bool,

    /// Number of samples to buffer for collection interval detection
    #[arg(long, default_value = "10")]
    pub otel_metrics_buffer_samples: usize,

    /// Maximum number of new charts to create per collection interval
    #[arg(long, default_value = "100")]
    pub otel_metrics_throttle_charts: usize,

    /// gRPC endpoint to listen on
    #[arg(long, default_value = "0.0.0.0:21213")]
    pub otel_endpoint: String,

    /// Collection interval (ignored)
    #[arg(help = "Collection interval in seconds (ignored)")]
    pub _update_frequency: Option<u32>,

    /// Chart configuration manager (not part of CLI)
    #[clap(skip)]
    pub chart_config_manager: ChartConfigManager,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            otel_metrics_print_flattened: false,
            otel_metrics_buffer_samples: 10,
            otel_metrics_throttle_charts: 100,
            otel_endpoint: String::from("0.0.0.0:21213"),
            _update_frequency: None,
            chart_config_manager: ChartConfigManager::with_default_configs(),
        }
    }
}

impl CliConfig {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut config = Self::parse();

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

        // Load chart configurations
        let config_dir = None.clone().or_else(|| {
            std::env::var("NETDATA_USER_CONFIG_DIR")
                .ok()
                .map(PathBuf::from)
        });

        if config_dir.is_none() && !atty::is(atty::Stream::Stdout) {
            return Err("NETDATA_USER_CONFIG_DIR environment variable is not set and no --netdata-user-config-dir provided".into());
        }

        let mut chart_config_manager = ChartConfigManager::with_default_configs();
        if let Some(dir) = &config_dir {
            chart_config_manager.load_user_configs(dir)?;
        }
        config.chart_config_manager = chart_config_manager;

        Ok(config)
    }
}

#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Print flattened metrics to stdout for debugging
    pub otel_metrics_print_flattened: bool,

    /// Number of samples to buffer for collection interval detection
    pub otel_metrics_buffer_samples: usize,

    /// Maximum number of new charts to create per collection interval
    pub otel_metrics_throttle_charts: usize,

    /// Chart configuration manager (not part of CLI)
    pub chart_config_manager: ChartConfigManager,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            otel_metrics_print_flattened: false,
            otel_metrics_buffer_samples: 10,
            otel_metrics_throttle_charts: 100,
            chart_config_manager: ChartConfigManager::with_default_configs(),
        }
    }
}

impl MetricsConfig {
    pub fn from_cli_config(cli_config: &CliConfig) -> Self {
        Self {
            otel_metrics_print_flattened: cli_config.otel_metrics_print_flattened,
            otel_metrics_buffer_samples: cli_config.otel_metrics_buffer_samples,
            otel_metrics_throttle_charts: cli_config.otel_metrics_throttle_charts,
            chart_config_manager: ChartConfigManager::with_default_configs(),
        }
    }
}

#[derive(Default, Debug)]
pub struct LogsConfig(());

#[derive(Debug, Default)]
pub struct PluginConfig {
    pub metrics_config: MetricsConfig,
    pub logs_config: LogsConfig,
}

impl PluginConfig {
    pub fn new(metrics_config: &MetricsConfig) -> Self {
        Self {
            metrics_config: metrics_config.clone(),
            logs_config: LogsConfig(()),
        }
    }
}
