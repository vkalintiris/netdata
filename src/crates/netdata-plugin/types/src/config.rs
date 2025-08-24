use crate::{DynCfgCmds, DynCfgSourceType, DynCfgStatus, DynCfgType, HttpAccess};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
