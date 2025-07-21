use crate::chart_config::ChartConfigManager;
use std::path::PathBuf;

pub struct PluginConfig {
    pub netdata_user_config_dir: Option<PathBuf>,
    pub print_flattened_metrics: bool,
    pub buffer_samples: usize,
    pub endpoint: String,
    pub chart_config_manager: ChartConfigManager,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            netdata_user_config_dir: None,
            print_flattened_metrics: false,
            buffer_samples: 10,
            endpoint: String::from("localhost:4317"),
            chart_config_manager: Default::default(),
        }
    }
}

impl PluginConfig {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut netdata_user_config_dir = None;
        let mut print_flattened_metrics = false;
        let mut buffer_samples = 10;
        let mut endpoint = "localhost:4317".to_string();

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;

        while i < args.len() {
            match args[i].as_str() {
                "--netdata-user-config-dir" => {
                    if i + 1 >= args.len() {
                        eprintln!("Error: --netdata-user-config-dir requires a value");
                        std::process::exit(1);
                    }
                    netdata_user_config_dir = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                }
                "--print-flattened-metrics" => {
                    print_flattened_metrics = true;
                    i += 1;
                }
                "--buffer-samples" => {
                    if i + 1 >= args.len() {
                        eprintln!("Error: --buffer-samples requires a value");
                        std::process::exit(1);
                    }
                    buffer_samples = args[i + 1].parse().unwrap_or_else(|_| {
                        eprintln!("Error: --buffer-samples must be a number");
                        std::process::exit(1);
                    });
                    i += 2;
                }
                "--endpoint" => {
                    if i + 1 >= args.len() {
                        eprintln!("Error: --endpoint requires a value");
                        std::process::exit(1);
                    }
                    endpoint = args[i + 1].clone();
                    i += 2;
                }
                "--help" | "-h" => {
                    Self::print_help(&args[0]);
                    std::process::exit(0);
                }
                arg if arg.starts_with("--") => {
                    eprintln!("Error: Unknown option: {}", arg);
                    std::process::exit(1);
                }
                _ => {
                    eprintln!("Error: Unexpected argument: {}", args[i]);
                    std::process::exit(1);
                }
            }
        }

        // Handle netdata user config directory
        let config_dir = netdata_user_config_dir.clone().or_else(|| {
            std::env::var("NETDATA_USER_CONFIG_DIR")
                .ok()
                .map(PathBuf::from)
        });

        if config_dir.is_none() && !atty::is(atty::Stream::Stdout) {
            eprintln!("Error: NETDATA_USER_CONFIG_DIR environment variable is not set and no --netdata-user-config-dir provided");
            std::process::exit(1);
        }

        // Initialize chart config manager
        let mut chart_config_manager = ChartConfigManager::with_default_configs();

        // Load user configs if directory is available
        if let Some(dir) = &config_dir {
            chart_config_manager.load_user_configs(dir)?;
        }

        Ok(PluginConfig {
            netdata_user_config_dir,
            print_flattened_metrics,
            buffer_samples,
            endpoint,
            chart_config_manager,
        })
    }

    fn print_help(program_name: &str) {
        println!("Usage: {} [OPTIONS]", program_name);
        println!("Options:");
        println!("  --netdata-user-config-dir <DIR>    Override NETDATA_USER_CONFIG_DIR");
        println!("  --print-flattened-metrics          Print flattened metrics to stderr");
        println!("  --buffer-samples <N>               Number of samples to buffer (default: 10)");
        println!("  --endpoint <ENDPOINT>              gRPC endpoint (default: localhost:4317)");
        println!("  --help, -h                         Show this help message");
    }

    pub fn netdata_user_config_dir(&self) -> Option<PathBuf> {
        self.netdata_user_config_dir.clone().or_else(|| {
            std::env::var("NETDATA_USER_CONFIG_DIR")
                .ok()
                .map(PathBuf::from)
        })
    }
}
