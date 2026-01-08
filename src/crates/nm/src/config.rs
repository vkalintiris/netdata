//! Configuration types and management for metric processing.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::iter::MetricRef;

/// Pattern matching for instrumentation scope fields
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstrumentationScopePattern {
    #[serde(with = "serde_regex", skip_serializing_if = "Option::is_none", default)]
    pub name: Option<Regex>,

    #[serde(with = "serde_regex", skip_serializing_if = "Option::is_none", default)]
    pub version: Option<Regex>,
}

impl InstrumentationScopePattern {
    /// Check if this pattern matches the given instrumentation scope
    pub fn matches(&self, scope: Option<&InstrumentationScope>) -> bool {
        let scope = match scope {
            Some(s) => s,
            None => return self.name.is_none() && self.version.is_none(),
        };

        if let Some(r) = &self.name {
            if !r.is_match(&scope.name) {
                return false;
            }
        }

        if let Some(r) = &self.version {
            if !r.is_match(&scope.version) {
                return false;
            }
        }

        true
    }
}

/// Individual configuration for a metric under specific instrumentation scope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricConfig {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub instrumentation_scope: Option<InstrumentationScopePattern>,

    /// The attribute key in DataPoint attributes whose value becomes the dimension name
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dimension_attribute_key: Option<String>,
}

impl MetricConfig {
    /// Check if this config matches the given instrumentation scope
    pub fn matches_scope(&self, scope: Option<&InstrumentationScope>) -> bool {
        match &self.instrumentation_scope {
            Some(pattern) => pattern.matches(scope),
            None => true, // No pattern means match any scope
        }
    }
}

/// Type alias for the config storage: metric name -> list of Arc-wrapped configs
pub type ConfigMap = HashMap<String, Vec<Arc<MetricConfig>>>;

/// Root configuration structure for YAML deserialization
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricConfigs {
    /// Map from exact metric name to list of configurations
    #[serde(default)]
    pub metrics: ConfigMap,
}

#[derive(Debug, Default, Clone)]
pub struct ChartConfigManager {
    /// Stock configs wrapped in Arc for cheap cloning
    stock: Arc<ConfigMap>,
    /// User configs wrapped in Arc for cheap cloning
    user: Arc<ConfigMap>,
}

impl ChartConfigManager {
    pub fn with_default_configs() -> Self {
        let mut manager = Self::default();
        manager.load_stock_config();
        manager
    }

    /// Find matching config for a metric. Returns Arc<MetricConfig> for zero-copy access.
    pub fn find_matching_config(&self, m: &MetricRef<'_>) -> Option<Arc<MetricConfig>> {
        let scope = m.scope_metrics.scope.as_ref();

        // Check user configs first (priority)
        if let Some(configs) = self.user.get(&m.metric.name) {
            if let Some(cfg) = configs.iter().find(|c| c.matches_scope(scope)) {
                return Some(Arc::clone(cfg));
            }
        }

        // Fall back to stock configs
        if let Some(configs) = self.stock.get(&m.metric.name) {
            if let Some(cfg) = configs.iter().find(|c| c.matches_scope(scope)) {
                return Some(Arc::clone(cfg));
            }
        }

        None
    }

    fn load_stock_config(&mut self) {
        const DEFAULT_CONFIGS_YAML: &str =
            include_str!("../configs/otel.d/v1/metrics/hostmetrics-receiver.yaml");

        match serde_yaml::from_str::<MetricConfigs>(DEFAULT_CONFIGS_YAML) {
            Ok(configs) => {
                self.stock = Arc::new(configs.metrics);
            }
            Err(e) => {
                eprintln!("Failed to parse default configs YAML: {}", e);
            }
        }
    }

    pub fn load_user_configs<P: AsRef<Path>>(&mut self, config_dir: P) -> Result<()> {
        // check dir
        let config_path = config_dir.as_ref();
        if !config_path.exists() {
            return Err(anyhow::anyhow!(
                "Configuration directory does not exist: {}",
                config_path.display()
            ));
        }
        if !config_path.is_dir() {
            return Err(anyhow::anyhow!(
                "Configuration path is not a directory: {}",
                config_path.display()
            ));
        }

        // collect the yaml files
        let mut config_files: Vec<_> = std::fs::read_dir(config_path)
            .with_context(|| {
                format!(
                    "Failed to read chart config directory: {}",
                    config_path.display()
                )
            })?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.is_file()
                    && matches!(
                        path.extension().and_then(|s| s.to_str()),
                        Some("yml" | "yaml")
                    )
                {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();
        config_files.sort();

        // deserialize and merge user configs
        let mut merged_configs: ConfigMap = HashMap::new();
        for path in config_files {
            match fs::read_to_string(&path) {
                Ok(contents) => match serde_yaml::from_str::<MetricConfigs>(&contents) {
                    Ok(metric_configs) => {
                        for (metric_name, configs) in metric_configs.metrics {
                            merged_configs
                                .entry(metric_name)
                                .or_default()
                                .extend(configs);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to parse YAML file {}: {}", path.display(), e);
                    }
                },
                Err(e) => {
                    eprintln!("Failed to read file {}: {}", path.display(), e);
                }
            }
        }
        self.user = Arc::new(merged_configs);

        Ok(())
    }
}
