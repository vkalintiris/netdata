use crate::ConfigDeclaration;
use netdata_plugin_protocol::DynCfgCmds;
use netdata_plugin_schema::NetdataSchema;
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub trait ConfigDeclarable: Send + Sync + Serialize + DeserializeOwned + JsonSchema {
    fn config_declaration() -> ConfigDeclaration;
}

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
    pub fn new<T>(initial_value: Option<T>) -> Self
    where
        T: ConfigDeclarable + NetdataSchema,
    {
        let declaration = T::config_declaration();

        Self {
            inner: Arc::new(ConfigInner {
                declaration,
                schema: T::netdata_schema(),
                instance: initial_value
                    .as_ref()
                    .map(|v| serde_json::to_value(v).unwrap()),
            }),
        }
    }

    pub fn id(&self) -> &str {
        &self.inner.declaration.id
    }

    pub fn schema(&self) -> &serde_json::Value {
        &self.inner.schema
    }

    pub fn initial_value(&self) -> Option<&serde_json::Value> {
        self.inner.instance.as_ref()
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
