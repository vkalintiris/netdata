//! The METASYNC thread of `src/database/sqlite/sqlite_metadata.c` (`metadata_event_loop()`) as far as D4 S1 needs
//! it: its lifecycle records, and the context load of the archived hosts, which it hands to the `UV_WORKER` pool
//! (`ctx_hosts_load()`), one `CTXLOAD` thread per host while slots last (D59.5). The metadata writer comes with S1b;
//! the contexts loader itself is wired with dbengine (S2), so the load of an alloc or ram host only clears its flag,
//! as in C.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{Priority, Source, nd_log, netdata_log_info};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::duration::duration_to_string;

use crate::startup::now_ut;

/// `NETDATA_VIRTUAL_HOST`.
const VIRTUAL_HOST_OS: &str = "Netdata Virtual Host 1.0";

enum Cmd {
    /// `METADATA_LOAD_HOST_CONTEXT`: load the pending hosts' contexts; vnodes report on the channel.
    LoadHostContexts(Arc<Hosts>, mpsc::Sender<()>),
    /// `METADATA_STORE_CLAIM_ID`: a host's node instance with no claim id (D61.3).
    StoreClaimId(Option<Arc<MetaDb>>, [u8; 16]),
    Shutdown,
}

/// The running METASYNC thread.
pub struct MetaSync {
    tx: mpsc::Sender<Cmd>,
    done: mpsc::Receiver<()>,
    thread: JoinHandle<()>,
}

fn duration(us: u64) -> String {
    duration_to_string(i64::try_from(us).unwrap_or(i64::MAX), "us", true).unwrap_or_default()
}

fn is_vnode(host: &Host) -> bool {
    host.info().os == VIRTUAL_HOST_OS
}

impl MetaSync {
    /// `metadata_sync_init()`: the thread, once it runs. `None` when it cannot start (C's creation asserts).
    pub fn start(pool: &WorkPool, cpus: usize, stack_size: usize) -> std::io::Result<MetaSync> {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let pool = pool.clone();
        let thread = std::thread::Builder::new()
            .name("METASYNC".into())
            .stack_size(stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "Starting metadata sync thread"
                );
                netdata_log_info!("METADATA: Synchronization thread is up and running");
                let _ = done_tx.send(());
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Cmd::LoadHostContexts(hosts, vnodes) => {
                            let _ = pool
                                .queue(move || ctx_hosts_load(&hosts, cpus, stack_size, &vnodes));
                        }
                        Cmd::StoreClaimId(meta, id) => {
                            crate::meta_store::store_claim_id(meta.as_deref(), &id);
                        }
                        Cmd::Shutdown => break,
                    }
                }
                let _ = done_tx.send(());
                netdata_agent_log::thread_finished();
            })?;
        let _ = done.recv();
        Ok(MetaSync { tx, done, thread })
    }

    /// `metadata_queue_load_host_context()`: false when the command cannot be queued.
    pub fn load_host_contexts(&self, hosts: &Arc<Hosts>, vnodes: mpsc::Sender<()>) -> bool {
        self.tx
            .send(Cmd::LoadHostContexts(Arc::clone(hosts), vnodes))
            .is_ok()
    }

    /// `metaqueue_store_claim_id()`.
    pub fn store_claim_id(&self, meta: Option<Arc<MetaDb>>, id: [u8; 16]) {
        let _ = self.tx.send(Cmd::StoreClaimId(meta, id));
    }

    /// `metadata_sync_shutdown()`, at the exit step "stop metasync threads".
    pub fn shutdown(self) {
        if self.tx.send(Cmd::Shutdown).is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "METADATA: Failed to send a shutdown command"
            );
            return;
        }
        netdata_log_info!("METADATA: Submitted shutdown command, waiting for ACK");
        let _ = self.done.recv();
        if self.thread.join().is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "METADATA: Failed to join synchronization thread"
            );
        } else {
            netdata_log_info!("METADATA: synchronization thread shutdown completed");
        }
    }
}

/// `restore_host_context()`: the host's contexts (from SQL for dbengine hosts, with S2), then the host no longer
/// waits for them.
fn restore_host_context(host: &Host, vnodes: &mpsc::Sender<()>) {
    let started = now_ut();
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Contexts for host {} loaded in {}",
        host.hostname(),
        duration(now_ut().saturating_sub(started))
    );
    host.clear_pending_context_load();
    if is_vnode(host) {
        let _ = vnodes.send(());
    }
}

/// `ctx_hosts_load()`, on a pool thread: the pending vnodes first, then the other pending hosts, most recently
/// connected first; each on a free `CTXLOAD` slot (one per CPU, when there is more than one), or here when none is.
fn ctx_hosts_load(hosts: &Hosts, cpus: usize, stack_size: usize, vnodes: &mpsc::Sender<()>) {
    let started = now_ut();
    let max_threads = cpus.max(1);
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Using {max_threads} threads for context loading"
    );
    let all = hosts.all();
    let mut order: Vec<Arc<Host>> = all
        .iter()
        .filter(|h| is_vnode(h) && h.is_pending_context_load())
        .cloned()
        .collect();
    let mut others: Vec<Arc<Host>> = all
        .iter()
        .filter(|h| !is_vnode(h) && h.is_pending_context_load())
        .cloned()
        .collect();
    others.sort_by_key(|h| std::cmp::Reverse(h.last_connected_s()));
    order.extend(others);
    let mut slots: Vec<Option<JoinHandle<()>>> = (0..if max_threads > 1 { max_threads } else { 0 })
        .map(|_| None)
        .collect();
    let (mut delegated, mut direct) = (0, 0);
    for host in &order {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Loading context for host {}",
            host.hostname()
        );
        // cleanup_finished_threads(): a slot whose thread has finished is free again
        let free = slots.iter_mut().find(|slot| match slot {
            None => true,
            Some(thread) => thread.is_finished(),
        });
        let spawned = free.and_then(|slot| {
            if let Some(thread) = slot.take() {
                let _ = thread.join();
            }
            let (host, vnodes) = (Arc::clone(host), vnodes.clone());
            let thread = std::thread::Builder::new()
                .name("CTXLOAD".into())
                .stack_size(stack_size)
                .spawn(move || {
                    netdata_agent_log::thread_created();
                    restore_host_context(&host, &vnodes);
                    netdata_agent_log::thread_finished();
                })
                .ok()?;
            *slot = Some(thread);
            Some(())
        });
        match spawned {
            Some(()) => delegated += 1,
            None => {
                direct += 1;
                restore_host_context(host, vnodes);
            }
        }
    }
    for thread in slots.into_iter().flatten() {
        let _ = thread.join();
    }
    netdata_log_info!(
        "Contexts for {} hosts loaded: {delegated} delegated to {max_threads} threads, {direct} handled directly, in \
         {}.",
        order.len(),
        duration(now_ut().saturating_sub(started))
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_rrd::host::HostInfo;
    use netdata_agent_rrd::mode::DbMode;

    fn info(hostname: &str, os: &str) -> HostInfo {
        HostInfo {
            hostname: hostname.into(),
            registry_hostname: hostname.into(),
            os: os.into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Alloc,
            history_entries: 5,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        }
    }

    fn hosts() -> Arc<Hosts> {
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            info("parent", "linux"),
        )));
        for (n, (name, os, last_connected)) in [
            ("old", "linux", 100),
            ("vnode", VIRTUAL_HOST_OS, 50),
            ("recent", "linux", 300),
        ]
        .into_iter()
        .enumerate()
        {
            let host = hosts.add_archived(
                &format!("5a1e0000-0000-4000-8000-0000000000b{n}"),
                info(name, os),
                |_| {},
            );
            host.set_last_connected_s(last_connected);
        }
        hosts
    }

    #[test]
    fn vnodes_load_first_then_the_most_recently_connected() {
        let hosts = hosts();
        let (tx, rx) = mpsc::channel();
        let (_, records) =
            netdata_agent_log::capture(|| ctx_hosts_load(&hosts, 1, 256 * 1024, &tx));
        let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
        let loading: Vec<&str> = messages
            .iter()
            .filter_map(|m| m.strip_prefix("Loading context for host "))
            .collect();
        assert_eq!(loading, ["vnode", "recent", "old"]);
        assert!(messages.last().unwrap().starts_with(
            "Contexts for 3 hosts loaded: 0 delegated to 1 threads, 3 handled directly, in "
        ));
        assert!(hosts.all().iter().all(|h| !h.is_pending_context_load()));
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn hosts_load_on_ctxload_threads_while_slots_last() {
        let hosts = hosts();
        let (tx, rx) = mpsc::channel();
        let (_, records) =
            netdata_agent_log::capture(|| ctx_hosts_load(&hosts, 2, 256 * 1024, &tx));
        let summary = records
            .into_iter()
            .filter_map(|r| r.message)
            .next_back()
            .unwrap();
        // two slots: a third host waits for a finished thread or runs here
        assert!(
            summary.starts_with("Contexts for 3 hosts loaded: "),
            "{summary}"
        );
        assert!(summary.contains(" delegated to 2 threads, "), "{summary}");
        assert!(hosts.all().iter().all(|h| !h.is_pending_context_load()));
        assert_eq!(rx.try_iter().count(), 1);
    }
}
