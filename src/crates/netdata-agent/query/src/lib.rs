//! The data query engine, ported from `src/web/api/queries/`, `src/web/api/formatters/`,
//! `src/database/contexts/query_target.c` and the data handlers of `src/web/api/v1/` and `src/web/api/v2/`.
//! Spec: `knowledge/spec-query.md` in the status repository (decisions D22).

#![forbid(unsafe_code)]

pub mod execute;
pub mod finalize;
pub mod format;
pub mod groupby;
pub mod grouping;
pub mod id;
pub mod jsonwrap;
pub mod jsonwrap_v2;
pub mod keys;
pub mod output;
pub mod request;
pub mod rrdr;
pub mod tables;
pub mod target;
#[cfg(test)]
mod testing;
pub mod window;

/// `now_realtime_sec()`.
pub fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
