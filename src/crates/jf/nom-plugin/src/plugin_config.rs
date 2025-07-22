use crate::chart_config::ChartConfigManager;
use std::path::PathBuf;

pub struct PluginConfig {
    pub netdata_user_config_dir: Option<PathBuf>,
    pub print_flattened_metrics: bool,
    pub buffer_samples: usize,
    pub throttle_charts: usize,
    pub endpoint: String,
    pub chart_config_manager: ChartConfigManager,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            netdata_user_config_dir: None,
            print_flattened_metrics: false,
            buffer_samples: 10,
            throttle_charts: 100,
            endpoint: String::from("0.0.0.0:21213"),
            chart_config_manager: ChartConfigManager::with_default_configs(),
        }
    }
}

impl PluginConfig {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut pc = PluginConfig::default();

        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;

        while i < args.len() {
            match args[i].as_str() {
                "--netdata-user-config-dir" => {
                    if i + 1 >= args.len() {
                        eprintln!("Error: --netdata-user-config-dir requires a value");
                        std::process::exit(1);
                    }
                    pc.netdata_user_config_dir = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                }
                "--print-flattened-metrics" => {
                    pc.print_flattened_metrics = true;
                    i += 1;
                }
                "--buffer-samples" => {
                    if i + 1 >= args.len() {
                        eprintln!("Error: --buffer-samples requires a value");
                        std::process::exit(1);
                    }
                    pc.buffer_samples = args[i + 1].parse().unwrap_or_else(|_| {
                        eprintln!("Error: --buffer-samples must be a number");
                        std::process::exit(1);
                    });
                    i += 2;
                }
                "--throttle-charts" => {
                    if i + 1 >= args.len() {
                        eprintln!("Error: --throttle-samples requires a value");
                        std::process::exit(1);
                    }
                    pc.throttle_charts = args[i + 1].parse().unwrap_or_else(|_| {
                        eprintln!("Error: --throttle-samples  must be a number");
                        std::process::exit(1);
                    });
                    i += 2;
                }
                "--endpoint" => {
                    if i + 1 >= args.len() {
                        eprintln!("Error: --endpoint requires a value");
                        std::process::exit(1);
                    }
                    pc.endpoint = args[i + 1].clone();
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
                    eprintln!("Ignoring unexpected argument: argv [{}]={}", i, args[i]);
                    i += 1
                }
            }
        }

        // Load config
        let config_dir = pc.netdata_user_config_dir.clone().or_else(|| {
            std::env::var("NETDATA_USER_CONFIG_DIR")
                .ok()
                .map(PathBuf::from)
        });
        if config_dir.is_none() && !atty::is(atty::Stream::Stdout) {
            eprintln!("Error: NETDATA_USER_CONFIG_DIR environment variable is not set and no --netdata-user-config-dir provided");
            std::process::exit(1);
        }
        let mut chart_config_manager = ChartConfigManager::with_default_configs();
        if let Some(dir) = &config_dir {
            chart_config_manager.load_user_configs(dir)?;
        }

        Ok(pc)
    }

    fn print_help(program_name: &str) {
        println!("Usage: {} [OPTIONS]", program_name);
        println!("Options:");
        println!("  --netdata-user-config-dir <DIR>    Override NETDATA_USER_CONFIG_DIR");
        println!("  --print-flattened-metrics          Print flattened metrics to stderr");
        println!("  --buffer-samples <N>               Number of samples to buffer (default: 10)");
        println!("  --throttle-charts <N>              Throttle charts created per second (default: 100)");
        println!("  --endpoint <ENDPOINT>              gRPC endpoint (default: localhost:4317)");
        println!("  --help, -h                         Show this help message");
    }
}
