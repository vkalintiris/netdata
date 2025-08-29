use crate::ConfigDeclaration;
use netdata_plugin_protocol::DynCfgCmds;
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

    /// Optional current configuration instance
    pub instance: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct Config {
    inner: Arc<ConfigInner>,
}

impl Config {
    pub fn new<T>(initial_value: Option<T>) -> Option<Self>
    where
        T: ConfigDeclarable,
    {
        let schema = T::netdata_schema();
        let Ok(declaration) = ConfigDeclaration::try_from(&schema) else {
            panic!(
                "Could not create config declaration from schema: {:?}",
                serde_json::to_string_pretty(&schema)
            );
        };

        Some(Self {
            inner: Arc::new(ConfigInner {
                declaration,
                schema,
                instance: initial_value
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

    pub fn instance<T>(&self) -> Result<Option<T>, serde_json::Error>
    where
        T: ConfigDeclarable,
    {
        match &self.inner.instance {
            Some(value) => {
                let config = serde_json::from_value::<T>(value.clone())?;
                Ok(Some(config))
            }
            None => Ok(None),
        }
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
        let inner = cfg.inner.clone();
        let id = inner.declaration.id.clone();

        {
            let mut guard = self.config_declarations.write().await;
            guard.insert(id, cfg);
        }
    }

    pub async fn get(&self, id: &str) -> Option<Config> {
        let guard = self.config_declarations.read().await;
        guard.get(id).cloned()
    }
}
