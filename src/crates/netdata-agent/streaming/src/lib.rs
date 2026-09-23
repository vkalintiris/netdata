//! Streaming between agents: the receiver that accepts children and the sender that connects to parents
//! (C: `src/streaming/`). Protocol handlers that apply the received keywords live in the ingest code (decisions D9).

#![forbid(unsafe_code)]

pub mod caps;
pub mod conf;
pub mod handshake;
