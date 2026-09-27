//! The agent's data model: hosts, charts and dimensions (C: `src/database/rrd*.c`), with per-host extension slots
//! for the subsystems that attach state to a host (decisions D9).

#![forbid(unsafe_code)]

pub mod backfill;
pub mod chart;
pub mod collection;
pub mod contexts;
pub mod host;
pub mod labels;
pub mod mode;
pub mod pulse;
pub mod retention;
pub mod status;
pub mod storage;
pub mod stream_path;
pub mod system_info;
#[cfg(test)]
mod testutil;
pub mod tiers;
