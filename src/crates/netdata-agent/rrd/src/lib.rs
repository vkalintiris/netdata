//! The agent's data model: hosts, charts and dimensions (C: `src/database/rrd*.c`), with per-host extension slots
//! for the subsystems that attach state to a host (decisions D9).

#![forbid(unsafe_code)]

pub mod chart;
pub mod collection;
pub mod contexts;
pub mod host;
pub mod labels;
pub mod mode;
pub mod system_info;
