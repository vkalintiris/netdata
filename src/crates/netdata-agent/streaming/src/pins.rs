//! `stream_thread_globals.assign` (`stream-thread.c`): which stream thread serves a host. A host is pinned to the least
//! loaded thread (the first of equals) when its receiver or its sender is first queued, both then share that thread,
//! and the pin goes with the last of them (`stream_thread_get_unsafe()`, `stream_thread_node_queued()`,
//! `stream_thread_node_removed()`).

use std::collections::HashMap;

/// `STREAM_MAX_THREADS`.
pub const MAX_THREADS: usize = 2048;

/// `stream_thread_get_unsafe()`'s `assign.cores`: one stream thread per CPU but one, 4..=`MAX_THREADS`.
pub fn threads_for(cpus: u64) -> usize {
    cpus.saturating_sub(1).clamp(4, MAX_THREADS as u64) as usize
}

#[derive(Debug)]
pub struct Pins {
    /// Each thread's `nodes_count`: the hosts pinned to it.
    nodes: Vec<usize>,
    /// Each pinned host's thread and how many of its receiver and sender are queued there, by machine GUID.
    hosts: HashMap<String, (usize, u32)>,
}

impl Pins {
    /// A table for `threads` stream threads.
    pub fn new(threads: usize) -> Pins {
        Pins { nodes: vec![0; threads.max(1)], hosts: HashMap::new() }
    }

    /// The host's thread, pinned unless it has one, with one more user.
    pub fn queue(&mut self, host: &str) -> usize {
        let nodes = &mut self.nodes;
        let pin = self.hosts.entry(host.to_string()).or_insert_with(|| {
            let thread = (0..nodes.len()).min_by_key(|&i| nodes[i]).unwrap_or(0);
            nodes[thread] += 1;
            (thread, 0)
        });
        pin.1 += 1;
        pin.0
    }

    /// Whether the host is pinned.
    #[cfg(test)]
    pub(crate) fn is_pinned(&self, host: &str) -> bool {
        self.hosts.contains_key(host)
    }

    /// One user less; the last one unpins the host.
    pub fn remove(&mut self, host: &str) {
        let Some(pin) = self.hosts.get_mut(host) else { return };
        pin.1 = pin.1.saturating_sub(1);
        if pin.1 == 0 {
            let thread = pin.0;
            self.hosts.remove(host);
            self.nodes[thread] = self.nodes[thread].saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host keeps its thread while its receiver or sender is queued; new hosts go to the least loaded thread, the
    /// first of equals.
    /// `netdata_conf_cpus()` - 1, at least 4 and at most 2048; a negative `[global] cpu cores` wraps to the most.
    #[test]
    fn stream_threads_follow_the_cpus_as_c() {
        let got: Vec<_> = [1, 4, 5, 6, 17, 2049, 2050, u64::MAX].map(threads_for).into();
        assert_eq!(got, [4, 4, 4, 5, 16, 2048, 2048, 2048]);
    }

    #[test]
    fn hosts_pin_as_c() {
        let mut p = Pins::new(3);
        assert_eq!((p.queue("a"), p.queue("b"), p.queue("c"), p.queue("d")), (0, 1, 2, 0));
        // a's sender joins its receiver
        assert_eq!(p.queue("a"), 0);
        p.remove("a");
        assert_eq!(p.queue("e"), 1, "a still holds thread 0");
        p.remove("a");
        p.remove("d");
        assert_eq!(p.nodes, vec![0, 2, 1]);
        assert_eq!(p.queue("a"), 0);
        p.remove("unknown");
        assert_eq!(p.nodes, vec![1, 2, 1]);
    }
}
