use crate::{DynCfgCmds, DynCfgSourceType, DynCfgStatus, DynCfgType, HttpAccess};
use std::convert::TryFrom;

#[derive(Debug, Clone)]
pub struct ConfigDeclaration {
    pub id: String,
    pub status: DynCfgStatus,
    pub type_: DynCfgType,
    pub path: String,
    pub source_type: DynCfgSourceType,
    pub source: String,
    pub cmds: DynCfgCmds,
    pub view_access: HttpAccess,
    pub edit_access: HttpAccess,
}

impl TryFrom<&serde_json::Value> for ConfigDeclaration {
    type Error = &'static str;

    fn try_from(schema: &serde_json::Value) -> Result<Self, Self::Error> {
        let config_decl = schema
            .get("configDeclaration")
            .ok_or("Missing configDeclaration field")?;

        let id = config_decl
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid id field")?
            .to_string();

        let status = config_decl
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(DynCfgStatus::from_name)
            .unwrap_or(DynCfgStatus::Running);

        let type_ = config_decl
            .get("type")
            .and_then(|v| v.as_str())
            .and_then(DynCfgType::from_name)
            .unwrap_or(DynCfgType::Single);

        let path = config_decl
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid path field")?
            .to_string();

        let source_type = config_decl
            .get("sourceType")
            .and_then(|v| v.as_str())
            .and_then(DynCfgSourceType::from_name)
            .unwrap_or(DynCfgSourceType::Stock);

        let source = config_decl
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or("Missing or invalid source field")?
            .to_string();

        let cmds = config_decl
            .get("cmds")
            .and_then(|v| v.as_str())
            .and_then(DynCfgCmds::from_str_multi)
            .unwrap_or(DynCfgCmds::SCHEMA | DynCfgCmds::GET);

        let view_access = config_decl
            .get("viewAccess")
            .and_then(|v| v.as_u64())
            .map(|v| HttpAccess::from_u32(v as u32))
            .unwrap_or(HttpAccess::empty());

        let edit_access = config_decl
            .get("editAccess")
            .and_then(|v| v.as_u64())
            .map(|v| HttpAccess::from_u32(v as u32))
            .unwrap_or(HttpAccess::empty());

        Ok(ConfigDeclaration {
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
}
