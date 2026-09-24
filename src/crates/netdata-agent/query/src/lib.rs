//! The data query engine, ported from `src/web/api/queries/`, `src/web/api/formatters/`,
//! `src/database/contexts/query_target.c` and the data handlers of `src/web/api/v1/` and `src/web/api/v2/`.
//! Spec: `knowledge/spec-query.md` in the status repository (decisions D22).

#![forbid(unsafe_code)]

/// `nd_profile.storage_tiers`: one tier (ram) until dbengine brings more (decision D15).
pub const STORAGE_TIERS: u64 = 1;

pub mod execute;
pub mod finalize;
pub mod format;
pub mod grouping;
pub mod id;
pub mod jsonwrap;
pub mod output;
pub mod request;
pub mod rrdr;
pub mod tables;
pub mod target;
#[cfg(test)]
mod testing;
pub mod window;
