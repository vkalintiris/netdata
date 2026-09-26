//! The running dbengine (brief `knowledge/brief-dbengine-s2.md` §4 in the status repository): the metric registry, the
//! tiers' startup and v2 indexes, the read path and the runtime.

pub mod cache;
pub mod collect;
pub mod io;
pub mod load;
pub mod mrg;
pub mod query;
pub mod runtime;
pub mod v2index;

#[cfg(test)]
mod testutil;
