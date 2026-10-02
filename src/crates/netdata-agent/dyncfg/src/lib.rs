//! DynCfg, ported from `src/daemon/dyncfg/` and `src/libnetdata/inicfg/dyncfg.c`: the dynamic configuration plugins
//! and the agent offer through the `config` Function, and the user's changes saved across restarts.

#![forbid(unsafe_code)]

pub mod files;
pub mod model;
