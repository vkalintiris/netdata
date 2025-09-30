use netdata_plugin_error::{NetdataPluginError, Result};
use netdata_plugin_protocol::{ConfigDeclaration, DynCfgCmds};
use netdata_plugin_schema::NetdataSchema;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Trait for types that can be used as plugin configuration
/// This is a marker trait that combines the necessary traits for config types
pub trait ConfigDeclarable: Send + Sync + Serialize + DeserializeOwned + NetdataSchema {}

/// Blanket implementation for all types that implement the required traits
impl<T> ConfigDeclarable for T where T: Send + Sync + Serialize + DeserializeOwned + NetdataSchema {}

#[derive(Debug, Clone)]
pub struct ConfigInner {
    /// The configuration declaration (metadata)
    pub declaration: ConfigDeclaration,

    /// Schema for the configuration
    pub schema: serde_json::Value,

    /// Optional current configuration value
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct Config {
    inner: Arc<ConfigInner>,
}

impl Config {
    pub fn new<T>(initial_value: Option<T>) -> Result<Self>
    where
        T: ConfigDeclarable,
    {
        let schema = T::netdata_schema();
        let declaration = ConfigDeclaration::try_from(&schema)?;

        Ok(Self {
            inner: Arc::new(ConfigInner {
                declaration,
                schema,
                value: initial_value
                    .as_ref()
                    .map(|v| serde_json::to_value(v).unwrap()),
            }),
        })
    }

    pub fn id(&self) -> &str {
        &self.inner.declaration.id
    }

    pub fn schema(&self) -> &serde_json::Value {
        &self.inner.schema
    }

    pub fn value(&self) -> Option<&serde_json::Value> {
        self.inner.value.as_ref()
    }

    pub fn instance<T>(&self) -> Result<Option<T>>
    where
        T: ConfigDeclarable,
    {
        let Some(value) = self.inner.value.as_ref() else {
            return Ok(None);
        };

        let config =
            serde_json::from_value::<T>(value.clone()).map_err(|e| NetdataPluginError::Schema {
                message: format!("Malformed configuration instance value: {:#?}", e),
            })?;
        Ok(Some(config))
    }

    pub fn dyncfg_commands(&self) -> DynCfgCmds {
        self.inner.declaration.cmds
    }
}

#[derive(Default, Debug)]
pub struct ConfigRegistry {
    config_declarations: Arc<RwLock<HashMap<String, Config>>>,
}

impl ConfigRegistry {
    pub async fn add(&self, cfg: Config) {
        let mut config_decls = self.config_declarations.write().await;
        config_decls.insert(String::from(cfg.id()), cfg);
    }

    pub async fn get(&self, id: &str) -> Option<Config> {
        let config_decls = self.config_declarations.read().await;
        config_decls.get(id).cloned()
    }
}
