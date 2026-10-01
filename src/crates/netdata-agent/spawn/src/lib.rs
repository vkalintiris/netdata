//! The spawn server (`src/libnetdata/spawn_server/`, decisions D12, D134, D136 in the status repository): the helper
//! process that starts every plugin and script on the daemon's behalf, the wire between them, and the child contract.
//! This crate holds no `unsafe` (D12): the system calls no safe crate wraps are `netdata-agent-sys`'s.
#![forbid(unsafe_code)]

pub mod client;
pub mod env;
pub mod exec;
pub mod popen;
pub mod server;
pub mod wire;
