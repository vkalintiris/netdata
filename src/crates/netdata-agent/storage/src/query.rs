//! A query on either engine (`storage-engine.h`'s `seb` dispatch): a ram ring or a dbengine tier. Dropping a dbengine
//! query is `rrdeng_load_metric_finalize()`.

use crate::dbengine::engine::query::Query as DbengineQuery;
pub use crate::dbengine::engine::query::Priority;
use crate::ram::RamQuery;
use crate::storage_point::StoragePoint;

#[derive(Debug)]
pub enum StorageQuery<'a> {
    Ram(RamQuery<'a>),
    Dbengine(DbengineQuery),
}

impl StorageQuery<'_> {
    /// `storage_engine_query_next_metric()`.
    pub fn next_metric(&mut self) -> StoragePoint {
        match self {
            StorageQuery::Ram(q) => q.next_metric(),
            StorageQuery::Dbengine(q) => q.next_metric(),
        }
    }

    /// `storage_engine_query_is_finished()`.
    pub fn is_finished(&self) -> bool {
        match self {
            StorageQuery::Ram(q) => q.is_finished(),
            StorageQuery::Dbengine(q) => q.is_finished(),
        }
    }
}
