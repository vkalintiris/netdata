//! Hosts, ported from `src/database/rrdhost.c`: localhost plus one host per child that ever streamed here, indexed
//! by machine GUID and kept in creation order (localhost first).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use netdata_agent_log::{Priority, REDACTED, Source, nd_log, netdata_log_error};
use netdata_agent_nrpc::Registry;
use netdata_agent_text::parse::uuid_parse_flexi;

use crate::chart::{self, Charts};
use crate::contexts::Metric;
use crate::contexts::{self, Contexts};
use crate::labels::Labels;
use crate::mode::DbMode;
use crate::storage::{StorageLayout, TierHandle};
use crate::stream_path::PathEntry;
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
    /// `host->stream.snd`: where the host streams to, when its sender structures were set up.
    pub stream_send: Option<StreamSend>,
    /// `host->cache_dir`: localhost only.
    pub cache_dir: Option<String>,
}

/// `host->stream.snd.destination` and `api_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSend {
    /// As configured: parents separated by whitespace or commas, each optionally with `:SSL`.
    pub destination: String,
    pub api_key: String,
}

impl StreamSend {
    /// `stream_sender_structures_init()`: a sender only with streaming on, a destination and an API key (an empty
    /// setting is NULL in C).
    pub fn new(enabled: bool, destination: &str, api_key: &str) -> Option<StreamSend> {
        (enabled && !destination.is_empty() && !api_key.is_empty()).then(|| StreamSend {
            destination: destination.to_string(),
            api_key: api_key.to_string(),
        })
    }

    /// The parents as `stream_parent_add_one_unsafe()` records them from `foreach_entry_in_connection_string()`:
    /// the first `:SSL` cuts an entry short.
    fn parents(&self) -> impl Iterator<Item = &str> {
        self.destination
            .split(|c: char| c == ',' || (c.is_ascii() && netdata_agent_text::c::is_space(c as u8)))
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.find(":SSL").map_or(entry, |at| &entry[..at]))
    }
}

/// `rrdhost_init_hostname()`: an empty name is `localhost`.
fn init_hostname(hostname: &str) -> String {
    if hostname.is_empty() {
        "localhost".to_string()
    } else {
        hostname.to_string()
    }
}

/// The record `rrdhost_create()` writes. The health thread copies the alarm defaults into the host only later, so
/// they print empty; C prints a child's unset cache directory with a raw `%s`.
fn initialized_record(guid: &str, info: &HostInfo) -> String {
    let (streaming, to, key) = match &info.stream_send {
        Some(send) => ("enabled", send.destination.as_str(), REDACTED),
        None => ("disabled", "", ""),
    };
    format!(
        "Host '{}' (at registry as '{}') with guid '{guid}' initialized, os '{}', timezone '{}', program_name '{}', \
         program_version '{}', update every {}, memory mode {}, history entries {}, streaming {streaming} (to '{to}' \
         with api key '{key}'), health {}, cache_dir '{}', alarms default handler '', alarms default recipient ''",
        info.hostname,
        info.registry_hostname,
        info.os,
        info.timezone,
        info.program_name,
        info.program_version,
        info.update_every,
        info.db_mode.name(),
        info.history_entries,
        if info.health_enabled {
            "enabled"
        } else {
            "disabled"
        },
        info.cache_dir.as_deref().unwrap_or("(null)"),
    )
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

/// `netdata_start_time`: the wall-clock second the daemon started.
static NETDATA_START_TIME: AtomicI64 = AtomicI64::new(0);

/// `netdata_start_time`.
pub fn netdata_start_time() -> i64 {
    NETDATA_START_TIME.load(Ordering::Relaxed)
}

/// Sets `netdata_start_time`, once at startup.
pub fn set_netdata_start_time(seconds: i64) {
    NETDATA_START_TIME.store(seconds, Ordering::Relaxed);
}

/// `now_realtime_sec()`.
fn now_realtime_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `get_agent_event_time_median()` of the start and shutdown events, in microseconds: cached from the agent event log
/// at startup, 0 without events.
static AGENT_EVENT_MEDIANS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

pub fn agent_event_medians_us() -> (u64, u64) {
    (
        AGENT_EVENT_MEDIANS[0].load(Ordering::Relaxed),
        AGENT_EVENT_MEDIANS[1].load(Ordering::Relaxed),
    )
}

pub fn set_agent_event_medians_us(start: u64, shutdown: u64) {
    AGENT_EVENT_MEDIANS[0].store(start, Ordering::Relaxed);
    AGENT_EVENT_MEDIANS[1].store(shutdown, Ordering::Relaxed);
}

/// What a receiver's connection negotiated, for what the parent tells the child about itself (the stream path).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReceiverLink {
    /// `rpt->hops`: the child's `hops=`.
    pub hops: i16,
    /// `rpt->connected_since_s`: when the request was accepted.
    pub connected_since_s: i64,
    /// `rpt->capabilities`: the negotiated capabilities.
    pub capabilities: u32,
}

/// The receiver attached to a host (`host->receiver`): what admission needs to judge a second connection.
pub struct ReceiverSlot {
    /// `rpt->thread.last_traffic_ut`, monotonic microseconds.
    pub last_traffic_ut: AtomicU64,
    /// `rpt->exit.shutdown`.
    pub stop_requested: AtomicBool,
    /// `rpt->remote_ip` and `rpt->remote_port`, for the records about this receiver.
    pub remote: (String, String),
    pub link: ReceiverLink,
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
        link: ReceiverLink,
        shutdown: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        ReceiverSlot {
            last_traffic_ut: AtomicU64::new(now_ut),
            stop_requested: AtomicBool::new(false),
            remote,
            link,
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

/// The host flags the metadata writer consumes (`RRDHOST_FLAG_METADATA_*`).
pub mod meta_flags {
    /// `RRDHOST_FLAG_METADATA_UPDATE`: something of the host (its info, labels, charts or dimensions) waits to be
    /// stored.
    pub const UPDATE: u32 = 1 << 0;
    /// `RRDHOST_FLAG_METADATA_LABELS`.
    pub const LABELS: u32 = 1 << 1;
    /// `RRDHOST_FLAG_METADATA_INFO`: the host row and its system info.
    pub const INFO: u32 = 1 << 2;
    /// `RRDHOST_FLAG_METADATA_CLAIMID`.
    pub const CLAIMID: u32 = 1 << 3;
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
    /// `host->stream.rcv.status.replication.percent`, as `f64` bits: kept across reconnections.
    replication_percent: AtomicU64,
    /// `host->stream.path`: what the child last reported, sorted by hops.
    stream_path: RwLock<Vec<PathEntry>>,
    /// `RRDHOST_OPTION_EPHEMERAL_HOST`.
    ephemeral: AtomicBool,
    /// `host->stream.rcv.min_update_every`: the smallest update every among the child's charts since it connected
    /// (`u32::MAX` before one), and the one its receiver's keepalive last used (`min_update_every_applied`).
    min_update_every: AtomicU32,
    min_update_every_applied: AtomicU32,
    /// `host->stream.rcv.status.replication.counter_out`: replication requests sent since the child connected.
    replication_requests: AtomicU32,
    /// `RRDHOST_FLAG_ARCHIVED`: loaded from the metadata database, not connected since this start.
    archived: AtomicBool,
    /// `RRDHOST_FLAG_PENDING_CONTEXT_LOAD`: its contexts are still loading; a child connecting now is refused.
    pending_context_load: AtomicBool,
    /// `host->stream.snd.status.last_connected`, in wall-clock seconds.
    last_connected_s: AtomicI64,
    /// `RRDHOST_FLAG_METADATA_*` (`meta_flags`), shared with the charts, whose changes raise `UPDATE`.
    meta_flags: Arc<AtomicU32>,
    /// `host->metadata_lifetime_lock`: the metadata writer stores the host under its read side, a netdatacli removal
    /// frees it under its write side; true once freed.
    metadata_lifetime: RwLock<bool>,
    /// `host->db[]`.
    storage: Arc<StorageLayout>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the host index down with it: the data stays usable.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Host {
    /// `rrdhost_create()` of a host whose storage has its tiers from `storage` (the engine's, when it runs).
    pub fn with_storage(
        machine_guid: &str,
        is_localhost: bool,
        info: HostInfo,
        storage: &Arc<StorageLayout>,
    ) -> Self {
        let mode = info.db_mode;
        let mut host = Host::new(machine_guid, is_localhost, info);
        let tiers = storage.tiers_for(mode, host.contexts.ram_index());
        if !tiers.is_empty() {
            host.contexts.set_tiers(tiers);
        }
        host.storage = Arc::clone(storage);
        host
    }

    /// `host->db[]`: the storage the host's tiers come from.
    pub fn storage(&self) -> &Arc<StorageLayout> {
        &self.storage
    }

    /// `qn->rrdhost->db[tier]` for a metric (`metric_dup()` of the dimension's, else `metric_get_by_id()`): its
    /// storage on `tier`, `None` past the tiers in use or when the tier does not hold it. Tier 0 of a host that is
    /// not dbengine is the ram ring of the dimension (or of the RAM index by UUID).
    pub fn tier_handle(&self, tier: usize, rm: &Metric) -> Option<TierHandle> {
        if tier >= self.storage.storage_tiers() {
            return None;
        }
        match self.storage.dbengine() {
            Some(engine) if tier > 0 || self.info().db_mode == DbMode::Dbengine => {
                let metric = engine.mrg.get_and_acquire(&rm.uuid(), tier)?;
                Some(TierHandle::Dbengine {
                    engine: Arc::clone(engine),
                    metric,
                })
            }
            _ => {
                let dim = rm.storage_dim()?;
                dim.ring()?;
                Some(TierHandle::Ram(dim))
            }
        }
    }

    /// A host without the dbengine: its contexts take retention from the RAM index.
    pub fn new(machine_guid: &str, is_localhost: bool, mut info: HostInfo) -> Self {
        info.hostname = init_hostname(&info.hostname);
        let contexts = Arc::new(Contexts::default());
        let meta_flags = Arc::new(AtomicU32::new(0));
        Host {
            machine_guid: machine_guid.to_string(),
            is_localhost,
            node_id: RwLock::new([0; 16]),
            info: RwLock::new(info),
            receiver: Mutex::new(None),
            orphan: AtomicBool::new(false),
            charts: Charts::new(Arc::clone(&contexts), Arc::clone(&meta_flags)),
            contexts,
            labels: RwLock::new(Labels::default()),
            claim_id_of_origin: RwLock::new([0; 16]),
            variables: Mutex::new(HashMap::new()),
            functions: Registry::default(),
            replication_percent: AtomicU64::new(100f64.to_bits()),
            stream_path: RwLock::new(Vec::new()),
            ephemeral: AtomicBool::new(false),
            min_update_every: AtomicU32::new(u32::MAX),
            min_update_every_applied: AtomicU32::new(u32::MAX),
            replication_requests: AtomicU32::new(0),
            archived: AtomicBool::new(false),
            pending_context_load: AtomicBool::new(false),
            last_connected_s: AtomicI64::new(0),
            meta_flags,
            metadata_lifetime: RwLock::new(false),
            storage: Arc::default(),
        }
    }

    /// `rw_spinlock_tryread_lock(&host->metadata_lifetime_lock)`: held while the writer stores the host or a command
    /// marks it; `None` while it is being freed, or once it is.
    pub fn metadata_try_read(&self) -> Option<std::sync::RwLockReadGuard<'_, bool>> {
        self.metadata_lifetime
            .try_read()
            .ok()
            .filter(|freed| !**freed)
    }

    /// `rw_spinlock_trywrite_lock(&host->metadata_lifetime_lock)`: for freeing the host (the guard's value is set to
    /// true); `None` while the writer stores it.
    pub fn metadata_try_write(&self) -> Option<std::sync::RwLockWriteGuard<'_, bool>> {
        self.metadata_lifetime
            .try_write()
            .ok()
            .filter(|freed| !**freed)
    }

    /// What `rrdhost_create()` does for a host that is not archived: it connected now, and its info waits to be
    /// stored.
    fn created_connected(&self) {
        self.set_last_connected_s(now_realtime_s());
        self.set_meta_flags(meta_flags::INFO | meta_flags::UPDATE);
    }

    /// `rrdhost_flag_set()` of `meta_flags`.
    pub fn set_meta_flags(&self, flags: u32) {
        self.meta_flags.fetch_or(flags, Ordering::AcqRel);
    }

    /// `rrdhost_flag_check()` then `rrdhost_flag_clear()` of `meta_flags`: whether any of them was set.
    pub fn take_meta_flags(&self, flags: u32) -> bool {
        self.meta_flags.fetch_and(!flags, Ordering::AcqRel) & flags != 0
    }

    pub fn meta_flags(&self) -> u32 {
        self.meta_flags.load(Ordering::Acquire)
    }

    /// `rrdset_observe_receiver_update_every()`: a chart's update every lowers the receiver's minimum.
    pub fn observe_receiver_update_every(&self, update_every: i32) {
        if update_every > 0 {
            self.min_update_every
                .fetch_min(update_every as u32, std::sync::atomic::Ordering::Release);
        }
    }

    pub fn receiver_min_update_every(&self) -> u32 {
        self.min_update_every
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// The minimum update every the receiver's keepalive last used; `set_` records a new one.
    pub fn receiver_min_update_every_applied(&self) -> u32 {
        self.min_update_every_applied
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_receiver_min_update_every_applied(&self, update_every: u32) {
        self.min_update_every_applied
            .store(update_every, std::sync::atomic::Ordering::Relaxed);
    }

    /// A replication request went to the child (`counter_out`).
    pub fn count_replication_request(&self) {
        self.replication_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn replication_requests(&self) -> u32 {
        self.replication_requests
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// `host->stream.rcv.status.replication.percent`: 100 from creation, then the receiver's replication progress.
    pub fn replication_percent(&self) -> f64 {
        f64::from_bits(
            self.replication_percent
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    pub fn set_replication_percent(&self, percent: f64) {
        self.replication_percent
            .store(percent.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn contexts(&self) -> &Contexts {
        &self.contexts
    }

    /// The records of `rrdhost_create()` for a new host: the sender's parents, an invalid machine GUID, the function
    /// registry (`nrpc_registry_init()`, which prints the host's address), then `Host ... initialized`.
    fn log_created(&self) {
        self.log_created_with(true);
    }

    /// The records of `rrdhost_create()`; an archived host gets no function registry, so no NRPC record.
    fn log_created_with(&self, registry: bool) {
        let info = self.info();
        if let Some(send) = &info.stream_send {
            for (n, parent) in send.parents().enumerate() {
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "STREAM PARENTS '{}': added streaming destination No {}: '{parent}'",
                    info.hostname,
                    n + 1
                );
            }
        }
        if uuid_parse_flexi(self.machine_guid.as_bytes()).is_none() {
            netdata_log_error!("Host machine GUID {} is not valid", self.machine_guid);
        }
        if registry {
            self.log_registry_created(&info.hostname);
        }
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "{}",
            initialized_record(&self.machine_guid, &info)
        );
    }

    fn log_registry_created(&self, hostname: &str) {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "NRPC: function registry 0x{:016X} created for host '{hostname}'",
            std::ptr::from_ref(self) as usize
        );
    }

    /// `RRDHOST_FLAG_ARCHIVED`.
    pub fn is_archived(&self) -> bool {
        self.archived.load(Ordering::Acquire)
    }

    /// `RRDHOST_FLAG_PENDING_CONTEXT_LOAD`.
    pub fn is_pending_context_load(&self) -> bool {
        self.pending_context_load.load(Ordering::Acquire)
    }

    pub fn clear_pending_context_load(&self) {
        self.pending_context_load.store(false, Ordering::Release);
    }

    /// `host->stream.snd.status.last_connected`.
    pub fn last_connected_s(&self) -> i64 {
        self.last_connected_s.load(Ordering::Relaxed)
    }

    pub fn set_last_connected_s(&self, seconds: i64) {
        self.last_connected_s.store(seconds, Ordering::Relaxed);
    }

    /// The last record of `rrdhost_cleanup_data_collection_and_health()`, which runs as a host is freed.
    fn log_archive_mode(&self) {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "RRD: 'host:{}' is now in archive mode...",
            self.hostname()
        );
    }

    /// `rrdhost_update()` for a child that connects again: what it reports about itself replaces the stored values,
    /// and what needs a restart (update every, memory mode, history) is only warned about. `update_every` and
    /// `history` are the configured values before `rrdhost_create()` normalizes them, as C compares them.
    pub fn update(&self, wanted: &HostInfo, update_every: i64, history: i64) {
        let mut records = Vec::new();
        {
            let mut info = self.info.write().unwrap_or_else(PoisonError::into_inner);
            info.health_enabled = wanted.health_enabled;
            info.system_info = wanted.system_info.clone();
            // under the same lock as the flags, as rrdhost_update() does: a store that takes INFO sees it
            self.set_last_connected_s(now_realtime_s());
            self.set_meta_flags(meta_flags::INFO | meta_flags::CLAIMID | meta_flags::UPDATE);
            info.os.clone_from(&wanted.os);
            info.timezone.clone_from(&wanted.timezone);
            info.abbrev_timezone.clone_from(&wanted.abbrev_timezone);
            info.utc_offset = wanted.utc_offset;
            info.registry_hostname = if wanted.registry_hostname.is_empty() {
                wanted.hostname.clone()
            } else {
                wanted.registry_hostname.clone()
            };
            if info.hostname != wanted.hostname {
                records.push((
                    Priority::Warning,
                    format!(
                        "Host '{}' has been renamed to '{}'. If this is not intentional it may mean multiple hosts \
                         are using the same machine_guid.",
                        info.hostname, wanted.hostname
                    ),
                ));
                info.hostname = init_hostname(&wanted.hostname);
            }
            if info.program_name != wanted.program_name {
                records.push((
                    Priority::Notice,
                    format!(
                        "Host '{}' switched program name from '{}' to '{}'",
                        info.hostname, info.program_name, wanted.program_name
                    ),
                ));
                info.program_name.clone_from(&wanted.program_name);
            }
            if info.program_version != wanted.program_version {
                records.push((
                    Priority::Notice,
                    format!(
                        "Host '{}' switched program version from '{}' to '{}'",
                        info.hostname, info.program_version, wanted.program_version
                    ),
                ));
                info.program_version.clone_from(&wanted.program_version);
            }
            if i64::from(info.update_every) != update_every {
                records.push((
                    Priority::Warning,
                    format!(
                        "Host '{}' has an update frequency of {} seconds, but the wanted one is {update_every} \
                         seconds. Restart netdata here to apply the new settings.",
                        info.hostname, info.update_every
                    ),
                ));
            }
            if info.db_mode != wanted.db_mode {
                records.push((
                    Priority::Warning,
                    format!(
                        "Host '{}' has memory mode '{}', but the wanted one is '{}'. Restart netdata here to apply \
                         the new settings.",
                        info.hostname,
                        info.db_mode.name(),
                        wanted.db_mode.name()
                    ),
                ));
            } else if info.db_mode != DbMode::Dbengine && info.history_entries < history {
                records.push((
                    Priority::Warning,
                    format!(
                        "Host '{}' has history of {} entries, but the wanted one is {history} entries. Restart \
                         netdata here to apply the new settings.",
                        info.hostname, info.history_entries
                    ),
                ));
            }
        }
        for (priority, text) in records {
            nd_log!(Source::Daemon, priority, "{text}");
        }
        // the host connected again: it gets its function registry back
        if self.archived.swap(false, Ordering::AcqRel) {
            let hostname = self.hostname();
            self.log_registry_created(&hostname);
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "Host {hostname} is not in archived mode anymore"
            );
        }
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

    /// `set_host_node_id()`: zero clears it.
    pub fn set_node_id(&self, node_id: [u8; 16]) {
        *self.node_id.write().unwrap_or_else(PoisonError::into_inner) = node_id;
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

    /// `host->stream.path`.
    pub fn stream_path(&self) -> Vec<PathEntry> {
        self.stream_path
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Stores a new stream path; true when it differs from the stored one (C compares a hash of the entries).
    pub fn replace_stream_path(&self, path: Vec<PathEntry>) -> bool {
        let mut stored = self
            .stream_path
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = *stored != path;
        *stored = path;
        changed
    }

    /// `RRDHOST_OPTION_EPHEMERAL_HOST`.
    pub fn is_ephemeral(&self) -> bool {
        self.ephemeral.load(Ordering::Relaxed)
    }

    pub fn set_ephemeral(&self, ephemeral: bool) {
        self.ephemeral.store(ephemeral, Ordering::Relaxed);
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
        self.replication_reset();
        // the child's charts report their update every again
        self.min_update_every
            .store(u32::MAX, std::sync::atomic::Ordering::Release);
        self.min_update_every_applied
            .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        drop(receiver);
        // rrdcontext_host_child_connected(): every chart and dimension reports collection again.
        for chart in self.charts.all() {
            contexts::rrdset_not_collected(&chart);
        }
        true
    }

    /// `rrdhost_is_online()`: localhost, or a child whose receiver is attached (no vnodes here).
    pub fn is_online(&self) -> bool {
        self.is_localhost || (self.receiver().is_some() && !self.is_orphan())
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

    /// `stream_receiver_replication_reset()`: no chart is being replicated by a receiver that just came or went, so
    /// the next connection asks for every chart's missing data again.
    fn replication_reset(&self) {
        for chart in self.charts.all() {
            chart.update_meta(|m| {
                m.flags |= chart::flags::RECEIVER_REPLICATION_FINISHED;
                m.flags &= !chart::flags::RECEIVER_REPLICATION_IN_PROGRESS;
            });
        }
        self.replication_requests
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// `rrdhost_clear_receiver()`: detaches `slot` if it is still the attached one.
    pub fn clear_receiver(&self, slot: &Arc<ReceiverSlot>) {
        let mut receiver = lock(&self.receiver);
        if receiver.as_ref().is_some_and(|r| Arc::ptr_eq(r, slot)) {
            *receiver = None;
            self.orphan
                .store(true, std::sync::atomic::Ordering::Release);
            self.contexts.record_first_time_changes(false);
            // stream_path_child_disconnected()
            self.replace_stream_path(Vec::new());
            self.replication_reset();
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
    /// `is_parent_label_cached_state` under its commit lock: whether localhost's `_is_parent` says a child is connected.
    is_parent: Mutex<bool>,
    /// The storage every host it creates gets.
    storage: Arc<StorageLayout>,
}

#[derive(Debug, Default)]
struct Index {
    /// Creation order, localhost first.
    ordered: Vec<Arc<Host>>,
    by_guid: HashMap<String, Arc<Host>>,
}

impl Hosts {
    /// The index of hosts without the dbengine.
    pub fn new(localhost: Host) -> Self {
        Hosts::with_storage(localhost, Arc::default())
    }

    /// The index of hosts with this storage; `localhost` was created with it.
    pub fn with_storage(localhost: Host, storage: Arc<StorageLayout>) -> Self {
        let localhost = Arc::new(localhost);
        let index = Index {
            ordered: vec![Arc::clone(&localhost)],
            by_guid: HashMap::from([(localhost.machine_guid.clone(), Arc::clone(&localhost))]),
        };
        localhost.log_created();
        localhost.created_connected();
        Hosts {
            localhost,
            inner: RwLock::new(index),
            version: std::sync::atomic::AtomicU32::new(1),
            is_parent: Mutex::new(false),
            storage,
        }
    }

    pub fn storage(&self) -> &Arc<StorageLayout> {
        &self.storage
    }

    /// `stream_receivers_currently_connected()`: hosts with a receiver attached.
    pub fn receivers_connected(&self) -> usize {
        self.all().iter().filter(|h| h.receiver().is_some()).count()
    }

    /// The forced half of `rrdhost_update_is_parent_label()` (a labels reload): the `_is_parent` value to store now,
    /// remembered as the last one stored. The caller writes it under the labels lock it already holds.
    pub fn is_parent_label(&self) -> &'static [u8] {
        let mut cached = lock(&self.is_parent);
        *cached = self.receivers_connected() > 0;
        if *cached { b"true" } else { b"false" }
    }

    /// `rrdhost_set_is_parent_label()`: after a receiver attached or detached, the label follows when the answer
    /// changed; decided and written under one lock, as C's commit lock, so the last writer writes the current state.
    pub fn update_is_parent_label(&self) {
        let mut cached = lock(&self.is_parent);
        let desired = self.receivers_connected() > 0;
        if *cached == desired {
            return;
        }
        *cached = desired;
        let value: &[u8] = if desired { b"true" } else { b"false" };
        self.localhost
            .update_labels(|labels| labels.add(b"_is_parent", value, crate::labels::SRC_AUTO));
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

    /// `rrdhost_unlink___while_having_rrd_wrlock()`: the host leaves the index (never localhost). Holders of its
    /// `Arc` keep it until they drop it.
    pub fn remove(&self, guid: &str) -> Option<Arc<Host>> {
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if guid == self.localhost.machine_guid {
            return None;
        }
        let host = index.by_guid.remove(guid)?;
        index.ordered.retain(|h| !Arc::ptr_eq(h, &host));
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        drop(index);
        host.log_archive_mode();
        Some(host)
    }

    /// `rrdhost_find_or_create(archived = true)` for a host of the metadata database: appended as archived, orphan
    /// and pending its contexts, with no function registry. `before_record` runs before the host's record (its node
    /// id). An existing host is returned as it is.
    pub fn add_archived(
        &self,
        guid: &str,
        info: HostInfo,
        before_record: impl FnOnce(&Host),
    ) -> Arc<Host> {
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(host) = index.by_guid.get(guid) {
            return Arc::clone(host);
        }
        let host = Arc::new(Host::with_storage(guid, false, info, &self.storage));
        host.archived.store(true, Ordering::Release);
        host.pending_context_load.store(true, Ordering::Release);
        host.orphan.store(true, Ordering::Release);
        before_record(&host);
        index.ordered.push(Arc::clone(&host));
        index.by_guid.insert(guid.to_string(), Arc::clone(&host));
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        drop(index);
        host.log_created_with(false);
        host
    }

    /// The find half of `rrdhost_find_or_create()`: an existing host is updated by `update`; otherwise `create` makes
    /// the new one, appended after the others, and its records follow once the index is unlocked. The whole step
    /// holds the index lock, as `rrd_wrlock()` does in C, so two connections for one GUID cannot both create it.
    ///
    /// An archived host still loading its contexts is returned untouched (the receiver refuses it); one of another
    /// memory mode is discarded and created again.
    pub fn find_or_create(
        &self,
        guid: &str,
        mode: DbMode,
        create: impl FnOnce() -> HostInfo,
        update: impl FnOnce(&Host),
    ) -> Arc<Host> {
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        let found = index.by_guid.get(guid).cloned();
        let found = match found {
            Some(host) if host.is_archived() && host.info().db_mode != mode => {
                if host.is_pending_context_load() {
                    return host;
                }
                nd_log!(
                    Source::Daemon,
                    Priority::Info,
                    "Archived host '{}' has memory mode '{}', but the wanted one is '{}'. Discarding archived state.",
                    host.hostname(),
                    host.info().db_mode.name(),
                    mode.name()
                );
                index.by_guid.remove(guid);
                index.ordered.retain(|h| !Arc::ptr_eq(h, &host));
                self.version
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                host.log_archive_mode();
                None
            }
            found => found,
        };
        if let Some(host) = found {
            drop(index);
            if !host.is_pending_context_load() {
                update(&host);
            }
            return host;
        }
        let host = Arc::new(Host::with_storage(guid, false, create(), &self.storage));
        host.created_connected();
        index.ordered.push(Arc::clone(&host));
        index.by_guid.insert(guid.to_string(), Arc::clone(&host));
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        drop(index);
        host.log_created();
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
            stream_send: None,
            cache_dir: None,
        }
    }

    /// An engine of `tiers` empty tiers over temporary directories, and its registry.
    fn engine(tiers: usize) -> (Vec<tempfile::TempDir>, Arc<StorageLayout>) {
        use netdata_agent_storage::dbengine::engine::cache::{ExtentCache, MainCache};
        use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
        use netdata_agent_storage::dbengine::engine::mrg::Mrg;
        use netdata_agent_storage::dbengine::engine::query::{Dbengine, TierData};
        let mrg = Mrg::new();
        let dirs: Vec<_> = (0..tiers).map(|_| tempfile::tempdir().unwrap()).collect();
        let tiers = dirs
            .iter()
            .enumerate()
            .map(|(tier, dir)| {
                let cfg = TierConfig {
                    tier,
                    path: dir.path().to_path_buf(),
                    direct_io: false,
                    max_disk_space: 0,
                    journal_check: false,
                };
                TierData::new(load(cfg, &mrg, 1_800_000_000).unwrap())
            })
            .collect();
        let engine = Dbengine {
            mrg,
            tiers,
            main: MainCache::new(1 << 20),
            extents: ExtentCache::new(1 << 20),
            pool: None,
            update_every_s: 1,
        };
        (dirs, Arc::new(StorageLayout::new(Some(Arc::new(engine)))))
    }

    /// `rrdhost_create()`'s tiers: every tier from the engine for a dbengine host, tier 0 from the RAM index for the
    /// other modes, the RAM index alone without the engine; hosts the index creates get the same.
    #[test]
    fn hosts_take_their_tiers_from_the_storage() {
        const A: [u8; 16] = [0xaa; 16];
        let (_dirs, storage) = engine(2);
        let mrg = &storage.dbengine().unwrap().mrg;
        drop(mrg.add_and_acquire(&A, 0, 150, 300, 1));
        drop(mrg.add_and_acquire(&A, 1, 100, 200, 60));
        let dbengine = HostInfo {
            db_mode: DbMode::Dbengine,
            ..info("d")
        };
        let retention = |host: &Host| host.contexts().metric_retention(&A);
        let host = Host::with_storage("guid-d", false, dbengine.clone(), &storage);
        assert_eq!(retention(&host), (100, 300, true));
        let alloc = Host::with_storage("guid-a", false, info("a"), &storage);
        assert_eq!(
            retention(&alloc),
            (100, 200, false),
            "tier 0 is the RAM index"
        );
        let plain = Host::with_storage("guid-p", false, dbengine.clone(), &Arc::default());
        assert_eq!(retention(&plain), (i64::MAX, 0, false));
        assert_eq!(
            (
                storage.storage_tiers(),
                StorageLayout::default().storage_tiers()
            ),
            (2, 1)
        );
        let hosts = Hosts::with_storage(
            Host::with_storage("guid-l", true, info("l"), &storage),
            Arc::clone(&storage),
        );
        let archived = hosts.add_archived("guid-x", dbengine, |_| {});
        assert_eq!(retention(&archived), (100, 300, true));
    }

    /// `stream_receiver_replication_reset()` on attach and on detach, each on its own.
    #[test]
    fn receivers_reset_replication_flags() {
        use crate::chart::{ChartSpec, ChartType, flags};
        let host = Host::new("guid-r", false, info("r"));
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "t",
            id: "c",
            name: None,
            family: None,
            context: None,
            title: "t",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 60,
            page_size: 4096,
        });
        let replicating = |c: &crate::chart::Chart| {
            c.update_meta(|m| {
                m.flags |= flags::RECEIVER_REPLICATION_IN_PROGRESS;
                m.flags &= !flags::RECEIVER_REPLICATION_FINISHED;
            })
        };
        let reset = |c: &crate::chart::Chart| {
            c.flags()
                & (flags::RECEIVER_REPLICATION_IN_PROGRESS | flags::RECEIVER_REPLICATION_FINISHED)
                == flags::RECEIVER_REPLICATION_FINISHED
        };
        let slot = || {
            Arc::new(ReceiverSlot::new(
                0,
                Default::default(),
                ReceiverLink::default(),
                Box::new(|| {}),
            ))
        };
        let first = slot();
        assert!(host.set_receiver(Arc::clone(&first)));
        replicating(&chart);
        host.clear_receiver(&first);
        assert!(reset(&chart), "detach resets");
        replicating(&chart);
        assert!(host.set_receiver(slot()));
        assert!(reset(&chart), "attach resets");
    }

    #[test]
    fn index_keeps_creation_order_and_single_receivers() {
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let a = hosts.find_or_create("guid-a", DbMode::Ram, || info("a"), |_| panic!("new host"));
        hosts.find_or_create("guid-b", DbMode::Ram, || info("b"), |_| panic!("new host"));
        let again = hosts.find_or_create(
            "guid-a",
            DbMode::Ram,
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
        let first = Arc::new(ReceiverSlot::new(
            1,
            Default::default(),
            ReceiverLink::default(),
            Box::new(|| {}),
        ));
        let second = Arc::new(ReceiverSlot::new(
            2,
            Default::default(),
            ReceiverLink::default(),
            Box::new(|| {}),
        ));
        assert!(a.set_receiver(Arc::clone(&first)));
        assert!(!a.set_receiver(Arc::clone(&second)));
        a.clear_receiver(&second);
        assert!(a.receiver().is_some());
        a.clear_receiver(&first);
        assert!(a.receiver().is_none());
    }

    fn texts(records: &[netdata_agent_log::Captured]) -> Vec<(Priority, String)> {
        records
            .iter()
            .map(|r| (r.priority, r.message.clone().unwrap_or_default()))
            .collect()
    }

    /// B8's child on a ram parent with health off, and a localhost that streams (brief-host-records §1.6).
    #[test]
    fn a_new_host_logs_its_records_as_c() {
        let guid = "09d5302f-8bdc-4fae-b017-d305d7666a42";
        let mut local = info("parent");
        local.program_version = "v2.11.0-458-g1e97a0fc9e".into();
        local.timezone = "Etc/UTC".into();
        local.health_enabled = false;
        local.stream_send = StreamSend::new(true, "127.0.0.1:29181:SSL, other:19999", "a-key");
        local.cache_dir = Some("/var/cache/netdata".into());
        let (hosts, records) = netdata_agent_log::capture(|| {
            Hosts::new(Host::new(
                "5a1e0000-0000-4000-8000-000000000000",
                true,
                local,
            ))
        });
        let address = format!("0x{:016X}", Arc::as_ptr(hosts.localhost()) as usize);
        assert_eq!(
            texts(&records),
            [
                (
                    Priority::Debug,
                    "STREAM PARENTS 'parent': added streaming destination No 1: '127.0.0.1:29181'".to_string()
                ),
                (
                    Priority::Debug,
                    "STREAM PARENTS 'parent': added streaming destination No 2: 'other:19999'".to_string()
                ),
                (
                    Priority::Debug,
                    format!("NRPC: function registry {address} created for host 'parent'")
                ),
                (
                    Priority::Info,
                    "Host 'parent' (at registry as 'parent') with guid '5a1e0000-0000-4000-8000-000000000000' \
                     initialized, os 'linux', \
                     timezone 'Etc/UTC', program_name 'netdata', program_version 'v2.11.0-458-g1e97a0fc9e', update \
                     every 1, memory mode ram, history entries 4096, streaming enabled (to '127.0.0.1:29181:SSL, \
                     other:19999' with api key '[REDACTED]'), health disabled, cache_dir '/var/cache/netdata', \
                     alarms default handler '', alarms default recipient ''"
                        .to_string()
                ),
            ]
        );
        let mut child = info("b8-child");
        child.timezone = "Etc/UTC".into();
        child.program_version = "v2.11.0-458-g1e97a0fc9e".into();
        let (host, records) = netdata_agent_log::capture(|| {
            hosts.find_or_create(guid, DbMode::Ram, || child, |_| panic!("new host"))
        });
        assert_eq!(
            texts(&records)[1..],
            [(
                Priority::Info,
                "Host 'b8-child' (at registry as 'b8-child') with guid '09d5302f-8bdc-4fae-b017-d305d7666a42' \
                 initialized, os 'linux', timezone 'Etc/UTC', program_name 'netdata', program_version \
                 'v2.11.0-458-g1e97a0fc9e', update every 1, memory mode ram, history entries 4096, streaming \
                 disabled (to '' with api key ''), health disabled, cache_dir '(null)', alarms default handler '', \
                 alarms default recipient ''"
                    .to_string()
            )]
        );
        // a malformed GUID is only reported, and an empty hostname is localhost
        let (_, records) = netdata_agent_log::capture(|| {
            hosts.find_or_create(
                "not-a-guid",
                DbMode::Ram,
                || info(""),
                |_| panic!("new host"),
            )
        });
        let texts = texts(&records);
        assert_eq!(
            texts[0],
            (
                Priority::Err,
                "Host machine GUID not-a-guid is not valid".to_string()
            )
        );
        assert!(
            texts[2]
                .1
                .starts_with("Host 'localhost' (at registry as '') with guid 'not-a-guid'")
        );
        drop(host);
    }

    /// `rrdhost_update()`: each record of C's table, in C's order, against the values before the update.
    #[test]
    fn an_update_logs_what_changed_as_c() {
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let host =
            hosts.find_or_create("guid-a", DbMode::Ram, || info("a"), |_| panic!("new host"));
        let mut wanted = info("renamed");
        wanted.program_name = "other".into();
        wanted.program_version = "v1".into();
        let ((), records) = netdata_agent_log::capture(|| host.update(&wanted, 2, 8192));
        assert_eq!(
            texts(&records),
            [
                (
                    Priority::Warning,
                    "Host 'a' has been renamed to 'renamed'. If this is not intentional it may mean multiple hosts \
                     are using the same machine_guid."
                        .to_string()
                ),
                (
                    Priority::Notice,
                    "Host 'renamed' switched program name from 'netdata' to 'other'".to_string()
                ),
                (
                    Priority::Notice,
                    "Host 'renamed' switched program version from 'v0' to 'v1'".to_string()
                ),
                (
                    Priority::Warning,
                    "Host 'renamed' has an update frequency of 1 seconds, but the wanted one is 2 seconds. Restart \
                     netdata here to apply the new settings."
                        .to_string()
                ),
                (
                    Priority::Warning,
                    "Host 'renamed' has history of 4096 entries, but the wanted one is 8192 entries. Restart netdata \
                     here to apply the new settings."
                        .to_string()
                ),
            ]
        );
        assert_eq!(host.info().registry_hostname, "renamed");
        wanted.db_mode = DbMode::Alloc;
        let ((), records) = netdata_agent_log::capture(|| host.update(&wanted, 1, 8192));
        assert_eq!(
            texts(&records),
            [(
                Priority::Warning,
                "Host 'renamed' has memory mode 'ram', but the wanted one is 'alloc'. Restart netdata here to apply \
                 the new settings."
                    .to_string()
            )]
        );
    }

    #[test]
    fn archived_hosts_wait_for_their_contexts_then_follow_cs_branches() {
        let messages = |records: Vec<netdata_agent_log::Captured>| -> Vec<String> {
            records
                .into_iter()
                .filter_map(|r| r.message)
                .filter(|m| !m.starts_with("Host 'child' (at registry"))
                .map(|m| {
                    if m.starts_with("NRPC: function registry") {
                        "NRPC".to_string()
                    } else {
                        m
                    }
                })
                .collect()
        };
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let alloc = HostInfo {
            db_mode: DbMode::Alloc,
            ..info("child")
        };
        let (host, records) = netdata_agent_log::capture(|| {
            hosts.add_archived(
                "5a1e0000-0000-4000-8000-0000000000c1",
                alloc.clone(),
                |_| {},
            )
        });
        assert!(host.is_archived() && host.is_pending_context_load() && host.is_orphan());
        // no function registry for an archived host
        assert_eq!(messages(records), Vec::<String>::new());
        // while its contexts load, the host comes back as it is
        let same = hosts.find_or_create(
            "5a1e0000-0000-4000-8000-0000000000c1",
            DbMode::Ram,
            || panic!("created"),
            |_| panic!("updated"),
        );
        assert!(Arc::ptr_eq(&same, &host));
        host.clear_pending_context_load();
        // the same memory mode: updated and no longer archived
        let (again, records) = netdata_agent_log::capture(|| {
            hosts.find_or_create(
                "5a1e0000-0000-4000-8000-0000000000c1",
                DbMode::Alloc,
                || panic!("created"),
                |h| h.update(&alloc, 1, 3600),
            )
        });
        assert!(Arc::ptr_eq(&again, &host) && !again.is_archived());
        assert_eq!(
            messages(records),
            ["NRPC", "Host child is not in archived mode anymore"]
        );
        // another memory mode: the archived state is discarded and the host created again
        let archived = hosts.add_archived(
            "5a1e0000-0000-4000-8000-0000000000c2",
            HostInfo {
                db_mode: DbMode::Alloc,
                ..info("other")
            },
            |_| {},
        );
        archived.clear_pending_context_load();
        let (created, records) = netdata_agent_log::capture(|| {
            hosts.find_or_create(
                "5a1e0000-0000-4000-8000-0000000000c2",
                DbMode::Ram,
                || info("other"),
                |_| panic!("updated"),
            )
        });
        assert!(!Arc::ptr_eq(&created, &archived) && !created.is_archived());
        let records = messages(records);
        assert_eq!(
            records[..2],
            [
                "Archived host 'other' has memory mode 'alloc', but the wanted one is 'ram'. Discarding archived state.",
                "RRD: 'host:other' is now in archive mode..."
            ]
        );
        assert_eq!(hosts.all().len(), 3);
    }

    /// `RRDHOST_FLAG_METADATA_*` as C raises them: at the creation of a host that connected (not an archived one),
    /// at a reconnection, and never for `_is_parent`.
    #[test]
    fn metadata_flags_follow_cs_setters() {
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let localhost = hosts.localhost();
        assert_eq!(
            localhost.meta_flags(),
            meta_flags::INFO | meta_flags::UPDATE
        );
        assert!(localhost.last_connected_s() > 0);
        let child =
            hosts.find_or_create("guid-a", DbMode::Ram, || info("a"), |_| panic!("new host"));
        assert_eq!(child.meta_flags(), meta_flags::INFO | meta_flags::UPDATE);
        assert!(child.last_connected_s() > 0);
        assert!(
            child.take_meta_flags(meta_flags::INFO) && !child.take_meta_flags(meta_flags::LABELS)
        );
        assert_eq!(child.meta_flags(), meta_flags::UPDATE);
        child.update(&info("a"), 1, 3600);
        assert_eq!(
            child.meta_flags(),
            meta_flags::INFO | meta_flags::CLAIMID | meta_flags::UPDATE
        );
        let archived = hosts.add_archived("guid-b", info("b"), |_| {});
        assert_eq!((archived.meta_flags(), archived.last_connected_s()), (0, 0));
        localhost.take_meta_flags(u32::MAX);
        hosts.update_is_parent_label();
        assert_eq!(localhost.meta_flags(), 0);
    }

    /// The metadata lifetime lock: the writer and a marking command share it, a removal takes it alone and frees the
    /// host, after which neither side gets it.
    #[test]
    fn metadata_lifetime_as_c() {
        let host = Host::new("guid-a", false, info("a"));
        let read = host.metadata_try_read().unwrap();
        assert!(host.metadata_try_read().is_some() && host.metadata_try_write().is_none());
        drop(read);
        let mut freed = host.metadata_try_write().unwrap();
        assert!(host.metadata_try_read().is_none());
        *freed = true;
        drop(freed);
        assert!(host.metadata_try_read().is_none() && host.metadata_try_write().is_none());
    }
}
