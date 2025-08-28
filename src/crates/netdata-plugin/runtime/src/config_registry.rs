use crate::ConfigDeclaration;
use netdata_plugin_protocol::{DynCfgCmds, DynCfgSourceType, DynCfgStatus, DynCfgType, HttpAccess};
use netdata_plugin_schema::NetdataSchema;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Trait for types that can be used as plugin configuration
/// This is a marker trait that combines the necessary traits for config types
pub trait ConfigDeclarable: Send + Sync + Serialize + DeserializeOwned + NetdataSchema {}

/// Blanket implementation for all types that implement the required traits
impl<T> ConfigDeclarable for T where T: Send + Sync + Serialize + DeserializeOwned + NetdataSchema {}

/// Extract ConfigDeclaration from a Netdata schema object
fn extract_config_declaration_from_schema(schema: &serde_json::Value) -> Option<ConfigDeclaration> {
    let config_decl = schema.get("configDeclaration")?;
    
    let id = config_decl.get("id")?.as_str()?.to_string();
    let status = config_decl.get("status")?.as_str()
        .and_then(DynCfgStatus::from_name)
        .unwrap_or(DynCfgStatus::Running);
    let type_ = config_decl.get("type")?.as_str()
        .and_then(DynCfgType::from_name)
        .unwrap_or(DynCfgType::Single);
    let path = config_decl.get("path")?.as_str()?.to_string();
    let source_type = config_decl.get("sourceType")?.as_str()
        .and_then(DynCfgSourceType::from_name)
        .unwrap_or(DynCfgSourceType::Stock);
    let source = config_decl.get("source")?.as_str()?.to_string();
    let cmds = config_decl.get("cmds")?.as_str()
        .and_then(|s| DynCfgCmds::from_str_multi(s))
        .unwrap_or(DynCfgCmds::SCHEMA | DynCfgCmds::GET);
    let view_access = config_decl.get("viewAccess")?.as_u64()
        .map(|v| HttpAccess::from_u32(v as u32))
        .unwrap_or(HttpAccess::empty());
    let edit_access = config_decl.get("editAccess")?.as_u64()
        .map(|v| HttpAccess::from_u32(v as u32))
        .unwrap_or(HttpAccess::empty());
    
    Some(ConfigDeclaration {
        id,
        status,
        type_,
        path,
        source_type,
        source,
        cmds,
        view_access,
        edit_access,
    })
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
    pub fn new<T>(initial_value: Option<T>) -> Option<Self>
    where
        T: ConfigDeclarable,
    {
        let schema = T::netdata_schema();
        
        // Extract config declaration from the schema
        let declaration = extract_config_declaration_from_schema(&schema)?;

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
