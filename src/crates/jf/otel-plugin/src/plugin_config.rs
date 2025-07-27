use clap::Parser;
use std::path::PathBuf;

use crate::chart_config::ChartConfigManager;

#[derive(Debug, Parser)]
#[command(name = "otel-plugin")]
#[command(about = "OpenTelemetry metrics and logs plugin.")]
#[command(version = "0.1")]
pub struct CliConfig {
    /// Netdata user config directory
    #[arg(long, env = "NETDATA_USER_CONFIG_DIR")]
    pub netdata_user_config_dir: Option<PathBuf>,

    /// Print flattened metrics to stdout for debugging
    #[arg(long)]
    pub print_flattened_metrics: bool,

    /// Number of samples to buffer for collection interval detection
    #[arg(long, default_value = "10")]
    pub buffer_samples: usize,

    /// Maximum number of new charts to create per collection interval
    #[arg(long, default_value = "100")]
    pub throttle_charts: usize,

    /// gRPC endpoint to listen on
    #[arg(long, default_value = "0.0.0.0:21213")]
    pub endpoint: String,

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
            netdata_user_config_dir: None,
            print_flattened_metrics: false,
            buffer_samples: 10,
            throttle_charts: 100,
            endpoint: String::from("0.0.0.0:21213"),
            _update_frequency: None,
            chart_config_manager: ChartConfigManager::with_default_configs(),
        }
    }
}

impl CliConfig {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut config = Self::parse();

        // Validate configuration
        if config.buffer_samples == 0 {
            return Err("buffer_samples must be greater than 0".into());
        }

        if config.throttle_charts == 0 {
            return Err("throttle_charts must be greater than 0".into());
        }

        // Validate endpoint format (basic check)
        if !config.endpoint.contains(':') {
            return Err("endpoint must be in format host:port".into());
        }

        // Load chart configurations
        let config_dir = config.netdata_user_config_dir.clone().or_else(|| {
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
