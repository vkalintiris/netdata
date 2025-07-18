use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectCriteria {
    #[serde(with = "serde_regex", skip_serializing_if = "Option::is_none", default)]
    pub instrumentation_scope_name: Option<Regex>,

    #[serde(with = "serde_regex", skip_serializing_if = "Option::is_none", default)]
    pub instrumentation_scope_version: Option<Regex>,

    #[serde(with = "serde_regex")]
    pub metric_name: Regex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractPattern {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart_instance_pattern: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartConfig {
    pub select: SelectCriteria,
    pub extract: ExtractPattern,
}

impl ChartConfig {
    pub fn new(
        scope_name: Option<&str>,
        scope_version: Option<&str>,
        metric_name: &str,
        chart_instance_pattern: Option<&str>,
        dimension_name: Option<&str>,
    ) -> Result<Self, regex::Error> {
        let instrumentation_scope_name = match scope_name {
            Some(pattern) => Some(Regex::new(pattern)?),
            None => None,
        };

        let instrumentation_scope_version = match scope_version {
            Some(pattern) => Some(Regex::new(pattern)?),
            None => None,
        };

        let metric_name = Regex::new(metric_name)?;

        Ok(ChartConfig {
            select: SelectCriteria {
                instrumentation_scope_name,
                instrumentation_scope_version,
                metric_name,
            },
            extract: ExtractPattern {
                chart_instance_pattern: chart_instance_pattern.map(String::from),
                dimension_name: dimension_name.map(String::from),
            },
        })
    }

    pub fn matches(&self, json_map: &JsonMap<String, JsonValue>) -> bool {
        if let Some(scope_regex) = &self.select.instrumentation_scope_name {
            if let Some(JsonValue::String(scope_name)) = json_map.get("scope.name") {
                if !scope_regex.is_match(scope_name) {
                    return false;
                }
            } else {
                return false;
            }
        }

        if let Some(version_regex) = &self.select.instrumentation_scope_version {
            if let Some(JsonValue::String(scope_version)) = json_map.get("scope.version") {
                if !version_regex.is_match(scope_version) {
                    return false;
                }
            } else {
                return false;
            }
        }

        if let Some(JsonValue::String(metric_name)) = json_map.get("metric.name") {
            self.select.metric_name.is_match(metric_name)
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Stock,
    User,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ChartConfigs {
    configs: Vec<ChartConfig>,
}

#[derive(Debug, Default)]
pub struct ChartConfigManager {
    stock: ChartConfigs,
    user: ChartConfigs,
}

impl ChartConfigManager {
    pub fn with_default_configs() -> Self {
        let mut manager = Self::default();
        manager.load_stock_config();
        manager
    }

    // pub fn from_yaml_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
    //     let contents = fs::read_to_string(path)?;
    //     let chart_configs: ChartConfigs = serde_yaml::from_str(&contents)?;
    //     Ok(manager)
    // }

    pub fn to_yaml_file<P: AsRef<Path>>(&self, path: P) -> Result<(), Box<dyn std::error::Error>> {
        let yaml_string = serde_yaml::to_string(&self.stock)?;
        fs::write(path, yaml_string)?;
        Ok(())
    }

    pub fn find_matching_config(
        &self,
        json_map: &JsonMap<String, JsonValue>,
    ) -> Option<&ChartConfig> {
        self.user
            .configs
            .iter()
            .chain(self.stock.configs.iter())
            .find(|config| config.matches(json_map))
    }

    fn load_stock_config(&mut self) {
        const DEFAULT_CONFIGS_YAML: &str = include_str!("../configs/stock.yml");

        match serde_yaml::from_str::<ChartConfigs>(DEFAULT_CONFIGS_YAML) {
            Ok(configs) => {
                self.stock = configs;
            }
            Err(e) => {
                eprintln!("Failed to parse default configs YAML: {}", e);
                // Handle error as appropriate
            }
        }
    }

    pub fn load_user_configs<P: AsRef<Path>>(
        &mut self,
        config_dir: P,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // check dir
        let config_path = config_dir.as_ref();
        if !config_path.exists() || !config_path.is_dir() {
            return Ok(());
        }

        // collect them
        let mut config_files: Vec<_> = std::fs::read_dir(config_path)?
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

        // deserialize them
        self.user = ChartConfigs::default();
        for path in config_files {
            match fs::read_to_string(&path) {
                Ok(contents) => match serde_yaml::from_str::<ChartConfigs>(&contents) {
                    Ok(chart_configs) => {
                        self.user.configs.extend(chart_configs.configs);
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

        // profit
        Ok(())
    }
}
