//! The running dbengine (brief `knowledge/brief-dbengine-s2.md` §4 in the status repository): the metric registry
//! first; the tiers' startup, the read path and the runtime follow.

pub mod cache;
pub mod io;
pub mod load;
pub mod mrg;
pub mod query;
pub mod v2index;

#[cfg(test)]
mod testutil;
