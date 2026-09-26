//! The netdata agent's SQLite databases (`src/database/sqlite/`): `netdata-meta.db` and `context-meta.db`, read and
//! written as the C agent does, on the SQLite the C agent builds (`vendor/libsqlite3-sys`, D59.1). The crate knows
//! rows and files; the daemon turns rows into hosts, charts and contexts.

#![forbid(unsafe_code)]

pub mod conn;
pub mod functions;
pub mod library;
pub mod migrate;
pub mod open;
pub mod read;
pub mod recover;
pub mod schema;
