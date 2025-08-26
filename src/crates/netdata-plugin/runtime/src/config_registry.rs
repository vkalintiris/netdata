use crate::ConfigDeclaration;
use netdata_plugin_protocol::DynCfgCmds;
use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings};
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
    pub schema: schemars::Schema,

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
        T: ConfigDeclarable,
    {
        let declaration = T::config_declaration();

        let settings = SchemaSettings::draft07();
        let generator = SchemaGenerator::new(settings);
        let schema = generator.into_root_schema_for::<T>();

        Self {
            inner: Arc::new(ConfigInner {
                declaration,
                schema,
                instance: initial_value
                    .as_ref()
                    .map(|v| serde_json::to_value(v).unwrap()),
            }),
        }
    }

    pub fn id(&self) -> &str {
        &self.inner.declaration.id
    }

    pub fn schema(&self) -> &schemars::Schema {
        &self.inner.schema
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
        guard.get(id).map(|x| x.clone())
    }
}
