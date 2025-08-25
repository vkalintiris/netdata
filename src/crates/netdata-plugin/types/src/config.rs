use crate::{DynCfgCmds, DynCfgSourceType, DynCfgStatus, DynCfgType, HttpAccess};
use std::hash::{Hash, Hasher};

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

impl PartialEq for ConfigDeclaration {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.path == other.path
    }
}

impl Eq for ConfigDeclaration {}

impl Hash for ConfigDeclaration {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.path.hash(state);
    }
}
