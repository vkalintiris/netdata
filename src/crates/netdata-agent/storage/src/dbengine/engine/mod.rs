//! The running dbengine (briefs `knowledge/brief-dbengine-s2.md` §4 and `knowledge/brief-dbengine-s3-reground.md` in
//! the status repository): the metric registry, the tiers' startup and v2 indexes, the read path, collection, extent
//! writes, journal indexing and the runtime.

pub mod cache;
pub mod collect;
mod evict;
mod flush;
pub mod index;
pub mod io;
pub mod load;
pub mod mrg;
pub mod query;
mod rotate;
pub mod runtime;
pub mod stats;
pub mod tier;
pub mod v2index;

/// `USEC_PER_SEC`.
const USEC_PER_SEC: u64 = 1_000_000;

#[cfg(test)]
mod testutil;
