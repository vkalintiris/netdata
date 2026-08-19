//! Where the notifier finds its configuration and state.
//!
//! Precedence matches the shell script: the environment the daemon exports wins,
//! and the compile-time install paths are the fallback for manual runs. Unlike the
//! script, the registry directory is also environment-derived, because the
//! compile-time value is wrong on Windows (it is an MSYS-style build path).

use std::path::PathBuf;

pub struct Paths {
    pub user_config_dir: PathBuf,
    pub stock_config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub registry_dir: PathBuf,
}

fn from_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Paths {
    pub fn from_environment() -> Self {
        let registry_dir = from_env("NETDATA_REGISTRY_DIR")
            .or_else(|| from_env("NETDATA_LIB_DIR").map(|p| p.join("registry")))
            .unwrap_or_else(|| PathBuf::from(env!("NETDATA_BUILD_REGISTRY_DIR")));

        Self {
            user_config_dir: from_env("NETDATA_USER_CONFIG_DIR")
                .unwrap_or_else(|| PathBuf::from(env!("NETDATA_BUILD_CONFIG_DIR"))),
            stock_config_dir: from_env("NETDATA_STOCK_CONFIG_DIR")
                .unwrap_or_else(|| PathBuf::from(env!("NETDATA_BUILD_STOCK_CONFIG_DIR"))),
            cache_dir: from_env("NETDATA_CACHE_DIR")
                .unwrap_or_else(|| PathBuf::from(env!("NETDATA_BUILD_CACHE_DIR"))),
            registry_dir,
        }
    }

    /// Stock file first, user file second - the user's copy overrides.
    pub fn config_files(&self) -> [PathBuf; 2] {
        [
            self.stock_config_dir.join("health_alarm_notify.conf"),
            self.user_config_dir.join("health_alarm_notify.conf"),
        ]
    }

    pub fn machine_guid_file(&self) -> PathBuf {
        self.registry_dir.join("netdata.public.unique.id")
    }

    /// Per-recipient state for the `|critical` severity filter.
    pub fn criticality_tracking_dir(&self, method: &str, recipient: &str) -> PathBuf {
        self.cache_dir
            .join("alarm-notify")
            .join(method)
            .join(recipient)
    }
}
