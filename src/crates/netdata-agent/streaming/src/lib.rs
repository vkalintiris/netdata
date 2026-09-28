//! Streaming between agents: the receiver that accepts children and the sender that connects to parents
//! (C: `src/streaming/`). Protocol handlers that apply the received keywords live in the ingest code (decisions D9).

#![forbid(unsafe_code)]

pub mod caps;
pub mod compress;
pub mod compression;
pub mod conf;
pub mod connect_to;
pub mod connector;
pub mod decompress;
pub mod h2o;
pub mod handshake;
pub mod parents;
pub mod pins;
pub mod random;
pub mod reason;
pub mod receiver;
pub mod records;
pub mod sender;
