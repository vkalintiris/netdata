//! The agent's data model: hosts, charts and dimensions (C: `src/database/rrd*.c`), with per-host extension slots
//! for the subsystems that attach state to a host (decisions D9).

#![forbid(unsafe_code)]

pub mod host;
pub mod mode;
pub mod system_info;
