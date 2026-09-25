//! Hosts, ported from `src/database/rrdhost.c`: localhost plus one host per child that ever streamed here, indexed
//! by machine GUID and kept in creation order (localhost first).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use netdata_agent_nrpc::Registry;

use crate::chart::Charts;
use crate::contexts::{self, Contexts};
use crate::labels::Labels;
use crate::mode::DbMode;
use crate::system_info::SystemInfo;

/// What a host is and how it is stored; the mutable part of `struct rrdhost`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub hostname: String,
    pub registry_hostname: String,
    pub os: String,
    pub timezone: String,
    pub abbrev_timezone: String,
    pub utc_offset: i32,
    pub program_name: String,
    pub program_version: String,
    pub update_every: i32,
    pub db_mode: DbMode,
    /// `rrd_history_entries` after `align_entries_to_pagesize()`.
    pub history_entries: i64,
    pub health_enabled: bool,
    pub system_info: SystemInfo,
    /// `RRDHOST_OPTION_REPLICATION` and `host->stream.replication.{period,step}`.
    pub replication_enabled: bool,
    pub replication_period: i64,
    pub replication_step: i64,
}

impl HostInfo {
    /// `rrdhost_set_replication_parameters()`: a ring cannot serve more than it holds, so for every mode but
    /// dbengine the period is capped at `history × update every`.
    pub fn set_replication(&mut self, enabled: bool, period: i64, step: i64) {
        self.replication_enabled = enabled;
        self.replication_step = step;
        let cap = self.history_entries * i64::from(self.update_every);
        self.replication_period = if self.db_mode != DbMode::Dbengine && period > cap {
            cap
        } else {
            period
        };
    }
}

/// The receiver attached to a host (`host->receiver`): what admission needs to judge a second connection.
pub struct ReceiverSlot {
    /// `rpt->thread.last_traffic_ut`, monotonic microseconds.
    pub last_traffic_ut: AtomicU64,
    /// `rpt->exit.shutdown`.
    pub stop_requested: AtomicBool,
    /// `rpt->remote_ip` and `rpt->remote_port`, for the records about this receiver.
    pub remote: (String, String),
    /// Shuts the connection down so its stream thread notices at once.
    shutdown: Box<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for ReceiverSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceiverSlot")
            .field("last_traffic_ut", &self.last_traffic_ut)
            .field("stop_requested", &self.stop_requested)
            .finish_non_exhaustive()
    }
}

impl ReceiverSlot {
    pub fn new(
        now_ut: u64,
        remote: (String, String),
        shutdown: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        ReceiverSlot {
            last_traffic_ut: AtomicU64::new(now_ut),
            stop_requested: AtomicBool::new(false),
            remote,
            shutdown,
        }
    }

    /// The first half of `stream_receiver_signal_to_stop_and_wait()`: flag it and shut the socket down, once.
    pub fn stop(&self) {
        if !self
            .stop_requested
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            (self.shutdown)();
        }
    }
}

/// `struct rrdhost`.
#[derive(Debug)]
pub struct Host {
    machine_guid: String,
    is_localhost: bool,
    /// `host->node_id`: zero until the host is claimed.
    node_id: RwLock<[u8; 16]>,
    info: RwLock<HostInfo>,
    receiver: Mutex<Option<Arc<ReceiverSlot>>>,
    /// `RRDHOST_FLAG_ORPHAN`: a child whose receiver has gone.
    orphan: AtomicBool,
    charts: Charts,
    /// `host->rrdctx`.
    contexts: Arc<Contexts>,
    /// `host->rrdlabels`.
    labels: RwLock<Labels>,
    /// The claim id a child reported (`CLAIMED_ID`), zero when unclaimed.
    claim_id_of_origin: RwLock<[u8; 16]>,
    /// Host variables (`VARIABLE HOST`), used by health.
    variables: Mutex<HashMap<String, f64>>,
    /// The functions registered for this host (`rrdhost_nrpc_owner()`).
    functions: Registry,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the host index down with it: the data stays usable.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Host {
    pub fn new(machine_guid: &str, is_localhost: bool, info: HostInfo) -> Self {
        let contexts = Arc::new(Contexts::default());
        Host {
            machine_guid: machine_guid.to_string(),
            is_localhost,
            node_id: RwLock::new([0; 16]),
            info: RwLock::new(info),
            receiver: Mutex::new(None),
            orphan: AtomicBool::new(false),
            charts: Charts::new(Arc::clone(&contexts)),
            contexts,
            labels: RwLock::new(Labels::default()),
            claim_id_of_origin: RwLock::new([0; 16]),
            variables: Mutex::new(HashMap::new()),
            functions: Registry::default(),
        }
    }

    pub fn contexts(&self) -> &Contexts {
        &self.contexts
    }

    pub fn functions(&self) -> &Registry {
        &self.functions
    }

    pub fn machine_guid(&self) -> &str {
        &self.machine_guid
    }

    pub fn is_localhost(&self) -> bool {
        self.is_localhost
    }

    pub fn labels(&self) -> Labels {
        self.labels
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn update_labels<T>(&self, update: impl FnOnce(&mut Labels) -> T) -> T {
        update(&mut self.labels.write().unwrap_or_else(PoisonError::into_inner))
    }

    /// `rrdhost_claim_id_get()` for a child: what it reported, if anything.
    pub fn claim_id(&self) -> Option<[u8; 16]> {
        let id = *self
            .claim_id_of_origin
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        (id != [0; 16]).then_some(id)
    }

    /// `rrdhost_claim_id_of_origin_set()`.
    pub fn set_claim_id_of_origin(&self, id: [u8; 16]) {
        *self
            .claim_id_of_origin
            .write()
            .unwrap_or_else(PoisonError::into_inner) = id;
    }

    /// `rrdvar_host_variable_set()`.
    pub fn set_variable(&self, name: &str, value: f64) {
        lock(&self.variables).insert(name.to_string(), value);
    }

    /// `host->rrdset_root_index`.
    pub fn charts(&self) -> &Charts {
        &self.charts
    }

    pub fn node_id(&self) -> [u8; 16] {
        *self.node_id.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn info(&self) -> HostInfo {
        self.info
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn hostname(&self) -> String {
        self.info
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .hostname
            .clone()
    }

    pub fn update_info(&self, update: impl FnOnce(&mut HostInfo)) {
        update(&mut self.info.write().unwrap_or_else(PoisonError::into_inner));
    }

    /// `host->receiver`.
    pub fn receiver(&self) -> Option<Arc<ReceiverSlot>> {
        lock(&self.receiver).clone()
    }

    /// `rrdhost_set_receiver()`: false when another receiver is already attached.
    pub fn set_receiver(&self, slot: Arc<ReceiverSlot>) -> bool {
        let mut receiver = lock(&self.receiver);
        if receiver.is_some() {
            return false;
        }
        *receiver = Some(slot);
        self.orphan
            .store(false, std::sync::atomic::Ordering::Release);
        drop(receiver);
        // rrdcontext_host_child_connected(): every chart and dimension reports collection again.
        for chart in self.charts.all() {
            contexts::rrdset_not_collected(&chart);
        }
        true
    }

    /// `RRDHOST_FLAG_ORPHAN`.
    pub fn is_orphan(&self) -> bool {
        self.orphan.load(std::sync::atomic::Ordering::Acquire)
    }

    /// `rrdhost_ingestion_hops()`: 0 for localhost, else what the child reported.
    pub fn ingestion_hops(&self) -> i16 {
        if self.is_localhost {
            0
        } else {
            self.info
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .system_info
                .hops
        }
    }

    /// `rrdhost_clear_receiver()`: detaches `slot` if it is still the attached one.
    pub fn clear_receiver(&self, slot: &Arc<ReceiverSlot>) {
        let mut receiver = lock(&self.receiver);
        if receiver.as_ref().is_some_and(|r| Arc::ptr_eq(r, slot)) {
            *receiver = None;
            self.orphan
                .store(true, std::sync::atomic::Ordering::Release);
            drop(receiver);
            self.contexts.child_disconnected();
        }
    }
}

/// The host index (`rrdhost_root_index` and the `localhost` list).
#[derive(Debug)]
pub struct Hosts {
    localhost: Arc<Host>,
    inner: RwLock<Index>,
    /// `dictionary_version(rrdhost_root_index)`: one per insert (and delete).
    version: std::sync::atomic::AtomicU32,
}

#[derive(Debug, Default)]
struct Index {
    /// Creation order, localhost first.
    ordered: Vec<Arc<Host>>,
    by_guid: HashMap<String, Arc<Host>>,
}

impl Hosts {
    pub fn new(localhost: Host) -> Self {
        let localhost = Arc::new(localhost);
        let index = Index {
            ordered: vec![Arc::clone(&localhost)],
            by_guid: HashMap::from([(localhost.machine_guid.clone(), Arc::clone(&localhost))]),
        };
        Hosts {
            localhost,
            inner: RwLock::new(index),
            version: std::sync::atomic::AtomicU32::new(1),
        }
    }

    /// `dictionary_version(rrdhost_root_index)`.
    pub fn version(&self) -> u32 {
        self.version.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn localhost(&self) -> &Arc<Host> {
        &self.localhost
    }

    /// `rrdhost_find_by_guid()`: an exact match.
    pub fn find_by_guid(&self, guid: &str) -> Option<Arc<Host>> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .by_guid
            .get(guid)
            .cloned()
    }

    /// `rrdhost_find_by_hostname()`: `localhost` is always this agent; otherwise the first host in creation order
    /// with this name (an empty name matches none).
    pub fn find_by_hostname(&self, hostname: &str) -> Option<Arc<Host>> {
        if hostname == "localhost" {
            return Some(Arc::clone(&self.localhost));
        }
        if hostname.is_empty() {
            return None;
        }
        let index = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        index
            .ordered
            .iter()
            .find(|h| h.hostname() == hostname)
            .cloned()
    }

    /// `rrdhost_find_by_node_id()`: the first host whose node ID equals the parsed UUID. Unclaimed hosts have a
    /// zero node ID, so the nil UUID finds the first of them.
    pub fn find_by_node_id(&self, node_id: &[u8; 16]) -> Option<Arc<Host>> {
        let index = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        index
            .ordered
            .iter()
            .find(|h| h.node_id() == *node_id)
            .cloned()
    }

    /// Every host, localhost first, then in creation order.
    pub fn all(&self) -> Vec<Arc<Host>> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .ordered
            .clone()
    }

    /// The find half of `rrdhost_find_or_create()`: an existing host is updated by `update`; otherwise `create` makes
    /// the new one, appended after the others. The whole step holds the index lock, as `rrd_wrlock()` does in C, so
    /// two connections for one GUID cannot both create it.
    pub fn find_or_create(
        &self,
        guid: &str,
        create: impl FnOnce() -> HostInfo,
        update: impl FnOnce(&Host),
    ) -> Arc<Host> {
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(host) = index.by_guid.get(guid) {
            let host = Arc::clone(host);
            drop(index);
            update(&host);
            return host;
        }
        let host = Arc::new(Host::new(guid, false, create()));
        index.ordered.push(Arc::clone(&host));
        index.by_guid.insert(guid.to_string(), Arc::clone(&host));
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        host
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(hostname: &str) -> HostInfo {
        HostInfo {
            hostname: hostname.into(),
            registry_hostname: hostname.into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 4096,
            health_enabled: false,
            system_info: SystemInfo::default(),
            replication_enabled: true,
            replication_period: 86400,
            replication_step: 3600,
        }
    }

    #[test]
    fn index_keeps_creation_order_and_single_receivers() {
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let a = hosts.find_or_create("guid-a", || info("a"), |_| panic!("new host"));
        hosts.find_or_create("guid-b", || info("b"), |_| panic!("new host"));
        let again = hosts.find_or_create(
            "guid-a",
            || panic!("exists"),
            |h| h.update_info(|i| i.hostname = "a2".into()),
        );
        assert!(Arc::ptr_eq(&a, &again));
        let names: Vec<_> = hosts.all().iter().map(|h| h.hostname()).collect();
        assert_eq!(names, ["parent", "a2", "b"]);
        assert_eq!(
            hosts
                .find_by_hostname("b")
                .map(|h| h.machine_guid().to_string())
                .as_deref(),
            Some("guid-b")
        );
        let first = Arc::new(ReceiverSlot::new(1, Default::default(), Box::new(|| {})));
        let second = Arc::new(ReceiverSlot::new(2, Default::default(), Box::new(|| {})));
        assert!(a.set_receiver(Arc::clone(&first)));
        assert!(!a.set_receiver(Arc::clone(&second)));
        a.clear_receiver(&second);
        assert!(a.receiver().is_some());
        a.clear_receiver(&first);
        assert!(a.receiver().is_none());
    }
}
