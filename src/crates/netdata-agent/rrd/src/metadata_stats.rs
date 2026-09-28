//! `rrdstats_metadata_collect()` (`src/database/rrd-metadata.c`): the nodes, metrics, instances and contexts of every
//! host, as `/api/v2/info` and the status file report them. C adds up per-host counters; the Rust agent counts the
//! same things in the contexts trees (D92.2).

use std::collections::HashSet;

use crate::host::Hosts;

/// A collected and available count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub collected: u64,
    pub available: u64,
}

impl Counts {
    pub(crate) fn add(&mut self, collected: bool) {
        self.available += 1;
        self.collected += u64::from(collected);
    }

    fn merge(&mut self, other: Counts, online: bool) {
        self.available += other.available;
        if online {
            self.collected += other.collected;
        }
    }
}

/// One host's tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeCounts {
    pub contexts: Counts,
    pub instances: Counts,
    pub metrics: Counts,
}

/// `RRDSTATS_METADATA`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataStats {
    pub nodes_total: u64,
    pub nodes_receiving: u64,
    /// `RRDHOST_FLAG_STREAM_SENDER_CONNECTED`: the Rust agent has no stream sender yet.
    pub nodes_sending: u64,
    pub nodes_archived: u64,
    pub metrics: Counts,
    pub instances: Counts,
    pub contexts: Counts,
    /// `rrdcontext_context_registry_unique_count()`: the context ids of all hosts, each once.
    pub contexts_unique: u64,
}

impl Hosts {
    /// `rrdstats_metadata_collect()`: every host counts; the collected counts only of online hosts; a host online
    /// but localhost receives, one offline is archived.
    pub fn metadata_stats(&self) -> MetadataStats {
        let mut stats = MetadataStats::default();
        let mut ids = HashSet::new();
        for host in self.all() {
            stats.nodes_total += 1;
            let tree = host.contexts().counts(|id| {
                if !ids.contains(id) {
                    ids.insert(id.to_string());
                }
            });
            let online = host.is_online();
            stats.metrics.merge(tree.metrics, online);
            stats.instances.merge(tree.instances, online);
            stats.contexts.merge(tree.contexts, online);
            if online {
                stats.nodes_receiving += u64::from(!host.is_localhost());
            } else {
                stats.nodes_archived += 1;
            }
        }
        stats.contexts_unique = ids.len() as u64;
        stats
    }
}
