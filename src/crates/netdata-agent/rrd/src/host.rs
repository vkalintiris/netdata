//! Hosts, ported from `src/database/rrdhost.c`: localhost plus one host per child that ever streamed here, indexed
//! by machine GUID and kept in creation order (localhost first).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, Weak};

use netdata_agent_log::{Priority, REDACTED, Source, nd_log, netdata_log_error};
use netdata_agent_nrpc::{self as nrpc, BuiltinDesc, MethodDesc, Registry, Unregistered};
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};

use crate::chart::{self, Charts};
use crate::clock::now_realtime_s;
use crate::contexts::Metric;
use crate::contexts::{self, Contexts, Taker};
use crate::index::Index;
use crate::labels::Labels;
use crate::mode::DbMode;
use crate::storage::{HealthEvent, StorageLayout, TierHandle};
use crate::stream_buffer::CircularBuffer;
use crate::stream_path::PathEntry;
use crate::system_info::SystemInfo;
use crate::upstream::{self, Traffic, Upstream};
use crate::variables::Variables;

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

impl HostInfo {
    /// What a vnode a plugin defines is created with (`pluginsd_host_define_end()`'s `rrdhost_find_or_create()`):
    /// localhost's settings under the vnode's name and system info, with the virtual OS and no cache directory.
    pub fn for_vnode(localhost: &HostInfo, hostname: &str, system_info: SystemInfo) -> HostInfo {
        HostInfo {
            hostname: hostname.to_string(),
            registry_hostname: hostname.to_string(),
            os: VIRTUAL_HOST_OS.to_string(),
            system_info,
            cache_dir: None,
            ..localhost.clone()
        }
    }
}

/// `host->stream.snd.destination`, `api_key` and `charts_matching`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSend {
    /// As configured: parents separated by whitespace or commas, each optionally with `:SSL`.
    pub destination: String,
    pub api_key: String,
    /// `send charts matching`: which charts go upstream, by context, name or id (none when empty).
    pub charts_matching: SimplePattern,
}

impl StreamSend {
    /// `stream_sender_structures_init()`: a sender only with streaming on, a destination and an API key (an empty
    /// setting is NULL in C).
    pub fn new(enabled: bool, destination: &str, api_key: &str, charts_matching: &str) -> Option<StreamSend> {
        (enabled && !destination.is_empty() && !api_key.is_empty()).then(|| StreamSend {
            destination: destination.to_string(),
            api_key: api_key.to_string(),
            charts_matching: SimplePattern::new(
                charts_matching.as_bytes(),
                Separators::Whitespace,
                SimplePatternMode::Exact,
                true,
            ),
        })
    }

    /// The parents as `stream_parent_add_one_unsafe()` records them from `foreach_entry_in_connection_string()`,
    /// each with whether it uses TLS: the first `:SSL` cuts an entry short.
    pub fn parents(&self) -> impl Iterator<Item = (&str, bool)> {
        self.destination
            .split(|c: char| c == ',' || (c.is_ascii() && netdata_agent_text::c::is_space(c as u8)))
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.find(":SSL").map_or((entry, false), |at| (&entry[..at], true)))
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

/// The records of `rrdhost_stream_parents_update_from_destination()` for a host that streams.
fn log_stream_parents(info: &HostInfo) {
    let Some(send) = &info.stream_send else {
        return;
    };
    for (n, (parent, _)) in send.parents().enumerate() {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "STREAM PARENTS '{}': added streaming destination No {}: '{parent}'",
            info.hostname,
            n + 1
        );
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

/// `STREAM_OPCODE_RECEIVER_*` (`stream-thread.h`): what a send to a child asks of the child's stream thread.
pub mod receiver_op {
    /// Bytes wait in a buffer that was empty: write them.
    pub const POLLOUT: u32 = 1 << 1;
    /// The buffer refused an add: the connection restarts.
    pub const BUFFER_OVERFLOW: u32 = 1 << 3;
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
    /// `rpt->thread.send_to_child.scb` under its spinlock: what is owed to the child (D166), from the move to running
    /// to the removal; `None` outside them, as C's NULL buffer.
    to_child: Mutex<Option<CircularBuffer>>,
    /// Set when the connection reaches its stream thread: given the opcode each send owes that thread
    /// (`stream_receiver_send_opcode()`, `receiver_op`).
    waker: OnceLock<Box<dyn Fn(u32) + Send + Sync>>,
    /// Its host is detaching it (`rrdhost_clear_receiver()` past its first step): no longer the host's receiver for
    /// the host's state, still in the slot until the detach ends.
    detaching: AtomicBool,
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
            to_child: Mutex::new(None),
            waker: OnceLock::new(),
            detaching: AtomicBool::new(false),
        }
    }

    /// `send_to_child()`: `bytes` for the child added to its buffer as `traffic`, the buffer autoscaled; their length,
    /// 0 without bytes or before the move to running (nothing is queued, D119.2), -1 when the buffer refuses them.
    /// After the lock its stream thread gets POLLOUT when the buffer was empty, BUFFER_OVERFLOW on a refusal.
    pub fn send_to_child(&self, bytes: &[u8], traffic: Traffic) -> isize {
        if bytes.is_empty() {
            return 0;
        }
        let (sent, op) = {
            let mut to_child = lock(&self.to_child);
            let Some(buffer) = to_child.as_mut() else {
                return 0;
            };
            let was_empty = buffer.stats().bytes_outstanding == 0;
            if !buffer.add(bytes, bytes.len(), traffic, true) {
                (-1, receiver_op::BUFFER_OVERFLOW)
            } else if was_empty {
                (bytes.len() as isize, receiver_op::POLLOUT)
            } else {
                (bytes.len() as isize, 0)
            }
        };
        if op != 0
            && let Some(wake) = self.waker.get()
        {
            wake(op);
        }
        sent
    }

    /// `stream_circular_buffer_create()` at the move to running, with the thread's `wake`: from here a send is
    /// queued.
    pub fn open_buffer(&self, wake: Box<dyn Fn(u32) + Send + Sync>) {
        let _ = self.waker.set(wake);
        *lock(&self.to_child) = Some(CircularBuffer::default());
    }

    /// `stream_receiver_free()`'s destroy: a send after it gets 0.
    pub fn close_buffer(&self) {
        *lock(&self.to_child) = None;
    }

    /// The child's buffer under its lock: its stream thread writes from it and its records read it.
    pub fn buffer(&self) -> MutexGuard<'_, Option<CircularBuffer>> {
        lock(&self.to_child)
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

/// `rrdhost_set_receiver()`'s outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attach {
    Attached,
    /// Another receiver serves the host.
    AlreadyServed,
    /// `RRDHOST_SET_RECEIVER_CLEANUP_BUSY`: the maintenance is marking the host's charts obsolete.
    CleanupBusy,
    /// `RRDHOST_SET_RECEIVER_VNODE_IS_LOCAL`: a plugin of this agent claimed the host as its vnode.
    VnodeIsLocal,
}

/// What claiming a host as a local vnode found (`pluginsd_host_claim_as_local_vnode()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// No receiver: nothing to do.
    Free,
    /// A receiver was streaming the host and was stopped.
    Evicted,
    /// A receiver was streaming the host and did not stop within 2 s.
    Stuck,
}

/// The host's `RRDHOST_FLAG_PENDING_*`: what its maintenance sweep reads (some chart or dimension turned obsolete
/// since the last one) and what its health pass reads (charts to give their alerts).
pub mod pending_flags {
    pub const OBSOLETE_CHARTS: u32 = 1 << 0;
    pub const OBSOLETE_DIMENSIONS: u32 = 1 << 1;
    /// `RRDHOST_FLAG_PENDING_HEALTH_INITIALIZATION`: some chart of the host carries a pending health flag.
    pub const HEALTH_INITIALIZATION: u32 = 1 << 2;
    /// `RRDHOST_FLAG_PENDING_LABEL_RECHECK`: the host's labels changed (or were loaded again): every chart's alerts
    /// are matched again.
    pub const LABEL_RECHECK: u32 = 1 << 3;

    /// The maintenance sweep's bits.
    pub const OBSOLETE: u32 = OBSOLETE_CHARTS | OBSOLETE_DIMENSIONS;
    /// The health pass's bits.
    pub const HEALTH: u32 = HEALTH_INITIALIZATION | LABEL_RECHECK;
}

/// `RRDHOST_FLAG_STREAM_SENDER_*` and `RRDHOST_FLAG_GLOBAL_FUNCTIONS_UPDATED`: the sender's state as the collectors
/// read it.
/// `RRDHOST_FLAG_VIRTUAL_HOST` and `RRDHOST_FLAG_COLLECTOR_ONLINE`, kept in one word: C clears them in one step, so a
/// status read never sees one without the other.
pub(crate) mod local_flags {
    /// A vnode this agent's plugins collect.
    pub const VIRTUAL: u8 = 1 << 0;
    /// The host's collector (this agent, a receiver, a plugin) is collecting it.
    pub const COLLECTOR_ONLINE: u8 = 1 << 1;
}

pub mod sender_flags {
    /// Queued for its parents (until the sender is removed).
    pub const ADDED: u32 = 1 << 0;
    pub const CONNECTED: u32 = 1 << 1;
    pub const READY_4_METRICS: u32 = 1 << 2;
    /// The "not ready" record was written and the "ready" one is due.
    pub const LOGGED_STATUS: u32 = 1 << 3;
    /// The host's functions changed since they were last sent (`rrdhost_nrpc_changed()`).
    pub const GLOBAL_FUNCTIONS_UPDATED: u32 = 1 << 4;
    /// `RRDHOST_OPTION_SENDER_ENABLED`: the host's sender is set up and not freed (`Host::upstream()` is it).
    pub const ENABLED: u32 = 1 << 5;
    /// `RRDHOST_FLAG_STREAM_SENDER_INITIALIZED`: a setup of the host's sender was elected, until the end of its free.
    pub const INITIALIZED: u32 = 1 << 6;
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
    /// `streaming_connected_receivers`: the receivers attached to the hosts of the index this host is in, counted as
    /// the slot fills and empties (set when the host joins an index; a host outside one counts nowhere).
    receivers_connected: OnceLock<Arc<AtomicU32>>,
    /// `host->stream.rcv.status.replication.backfill_pending`: charts whose replication waits for a backfill.
    backfill_pending: AtomicU32,
    /// `host->stream.rcv.status.connections`: the receivers attached since the agent started.
    receiver_connections: AtomicU32,
    /// `host->stream.rcv.status.last_connected` and `last_disconnected`, wall-clock seconds, written under the receiver
    /// lock: when the attached receiver came (0 without one), when the last one left (0 while one is attached).
    receiver_last_connected_s: AtomicI64,
    receiver_last_disconnected_s: AtomicI64,
    /// `host->health.evloop_iteration`: the HEALTH loop's pass when a receiver last attached or left; the host is
    /// archived only after more than 10 more.
    health_last_iteration: AtomicU64,
    /// `RRDHOST_FLAG_OBSOLETE_ALL_IN_PROGRESS`: the maintenance marks the host's charts obsolete; receivers wait.
    obsolete_all_busy: AtomicBool,
    /// `RRDHOST_FLAG_ORPHAN`: a child whose receiver has gone.
    orphan: AtomicBool,
    /// [`local_flags`].
    local: AtomicU8,
    charts: Charts,
    /// `host->rrdctx`.
    contexts: Arc<Contexts>,
    /// `host->rrdlabels`.
    labels: RwLock<Labels>,
    /// The claim id a child reported (`CLAIMED_ID`), zero when unclaimed.
    claim_id_of_origin: RwLock<[u8; 16]>,
    /// `host->aclk.claim_id_of_parent`: the claim id the parent sent with NODE_ID, zero when none.
    claim_id_of_parent: RwLock<[u8; 16]>,
    /// Host variables (`VARIABLE HOST`, `host->rrdvars`).
    variables: Variables,
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
    /// `counter_in`: the child's replication replies since it connected (a chart's first claim, every REND).
    replication_replies: AtomicU32,
    /// `RRDHOST_FLAG_ARCHIVED`: loaded from the metadata database, not connected since this start.
    archived: AtomicBool,
    /// `RRDHOST_FLAG_PENDING_CONTEXT_LOAD`: its contexts are still loading; a child connecting now is refused.
    pending_context_load: AtomicBool,
    /// `host->stream.snd.status.last_connected`, in wall-clock seconds.
    last_connected_s: AtomicI64,
    /// `RRDHOST_FLAG_METADATA_*` (`meta_flags`), shared with the charts, whose changes raise `UPDATE`.
    meta_flags: Arc<AtomicU32>,
    /// `pending_flags`, which the host's charts raise.
    pending_flags: Arc<AtomicU32>,
    /// `host->metadata_lifetime_lock`: the metadata writer stores the host under its read side, a netdatacli removal
    /// frees it under its write side; true once freed.
    metadata_lifetime: RwLock<bool>,
    /// `host->db[]`.
    storage: Arc<StorageLayout>,
    /// `host->stream.pulse_state`: the host's streaming state in the pulse charts (`pulse::host_status` bits).
    pulse_state: AtomicU32,
    /// `host->stream.rcv.status.running_latched`: the receiver reached running since it connected.
    running_latched: AtomicBool,
    /// `host->stream.rcv.status.state_changed_s`: when the inbound state last changed, in wall-clock seconds.
    state_changed_s: AtomicI64,
    /// `host->stream.rcv.status.bytes_in` and `bytes_out`: the stream bytes of every connection of this host.
    stream_bytes_in: AtomicU64,
    stream_bytes_out: AtomicU64,
    /// `host->stream.rcv.status.replication.charts`: the charts whose replication is in progress (it wraps below 0,
    /// as C's, until a reset zeroes it).
    replicating_charts: AtomicU32,
    /// `host->stream.rcv.status.labels_applied` and `labels_applied_version`: the pulse charts of this child took its
    /// labels, and at which version.
    labels_applied: AtomicBool,
    labels_applied_version: AtomicU32,
    /// `RRDHOST_FLAG_STREAM_SENDER_*` ([`sender_flags`]).
    sender_flags: AtomicU32,
    /// `host->sender`, with `RRDHOST_OPTION_SENDER_ENABLED` as its presence.
    upstream: OnceLock<Arc<dyn Upstream>>,
    /// `host->stream.snd.status.replication.charts`: the charts whose definition claimed a replication from the parent
    /// (it wraps below 0, as C's).
    sender_replicating_charts: AtomicU32,
    /// `sender->global_functions_spinlock`: the functions' render and commit, one call site at a time.
    global_functions: Mutex<()>,
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
        let host = Host::build(machine_guid, is_localhost, info, storage);
        if is_localhost {
            // rrd_init()'s object_state_activate(): localhost's functions belong to its first epoch
            host.functions.activate();
        }
        let tiers = storage.tiers_for(mode, host.contexts.ram_index());
        if !tiers.is_empty() {
            host.contexts.set_tiers(tiers);
        }
        host
    }

    /// The host shared, its contexts linked back to it (`rc->rrdhost`): the index makes its hosts' `Arc`s here.
    pub fn into_shared(self) -> Arc<Host> {
        let host = Arc::new(self);
        host.contexts().set_host(&host);
        host
    }

    /// `host->db[]`: the storage the host's tiers come from.
    pub fn storage(&self) -> &Arc<StorageLayout> {
        &self.storage
    }

    /// `rrdhost_finalize_collection()`: every chart's collection ends, dimensions included.
    pub fn finalize_collection(&self) {
        let hostname = self.hostname();
        let _frame = netdata_agent_log::push(vec![(
            netdata_agent_log::Field::NidlNode,
            netdata_agent_log::Value::txt(hostname.as_str()),
        )]);
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "RRD: 'host:{hostname}' stopping data collection..."
        );
        for chart in self.charts.all() {
            chart.finalize_collection(true);
        }
    }

    /// `qn->rrdhost->db[tier]` for a metric (`metric_dup()` of the dimension's, else `metric_get_by_id()`): its
    /// storage on `tier`, `None` past the tiers in use or when the tier does not hold it. Tier 0 of a host that is
    /// not dbengine is the ram ring of the dimension (or of the RAM index by UUID).
    pub fn tier_handle(&self, tier: usize, rm: &Metric) -> Option<TierHandle> {
        if tier >= self.storage.storage_tiers() {
            return None;
        }
        match self.storage.dbengine() {
            Some(engine) if self.storage.tier_is_dbengine(self.info().db_mode, tier) => {
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
    pub fn new(machine_guid: &str, is_localhost: bool, info: HostInfo) -> Self {
        Host::build(machine_guid, is_localhost, info, &Arc::default())
    }

    /// The host with its charts on `storage`.
    fn build(
        machine_guid: &str,
        is_localhost: bool,
        mut info: HostInfo,
        storage: &Arc<StorageLayout>,
    ) -> Self {
        info.hostname = init_hostname(&info.hostname);
        let contexts = Arc::new(Contexts::default());
        let meta_flags = Arc::new(AtomicU32::new(0));
        let pending_flags = Arc::new(AtomicU32::new(0));
        Host {
            machine_guid: machine_guid.to_string(),
            is_localhost,
            node_id: RwLock::new([0; 16]),
            info: RwLock::new(info),
            receiver: Mutex::new(None),
            receivers_connected: OnceLock::new(),
            backfill_pending: AtomicU32::new(0),
            receiver_connections: AtomicU32::new(0),
            receiver_last_connected_s: AtomicI64::new(0),
            receiver_last_disconnected_s: AtomicI64::new(0),
            health_last_iteration: AtomicU64::new(0),
            obsolete_all_busy: AtomicBool::new(false),
            orphan: AtomicBool::new(false),
            // rrd_init(): localhost's collector is this agent
            local: AtomicU8::new(if is_localhost { local_flags::COLLECTOR_ONLINE } else { 0 }),
            charts: Charts::new(
                Arc::clone(&contexts),
                Arc::clone(&meta_flags),
                Arc::clone(&pending_flags),
                Arc::clone(storage),
                machine_guid,
            ),
            contexts,
            labels: RwLock::new(Labels::default()),
            claim_id_of_origin: RwLock::new([0; 16]),
            claim_id_of_parent: RwLock::new([0; 16]),
            variables: Variables::default(),
            functions: Registry::default(),
            replication_percent: AtomicU64::new(100f64.to_bits()),
            stream_path: RwLock::new(Vec::new()),
            ephemeral: AtomicBool::new(false),
            min_update_every: AtomicU32::new(u32::MAX),
            min_update_every_applied: AtomicU32::new(u32::MAX),
            replication_requests: AtomicU32::new(0),
            replication_replies: AtomicU32::new(0),
            archived: AtomicBool::new(false),
            pending_context_load: AtomicBool::new(false),
            last_connected_s: AtomicI64::new(0),
            pulse_state: AtomicU32::new(0),
            running_latched: AtomicBool::new(false),
            state_changed_s: AtomicI64::new(0),
            stream_bytes_in: AtomicU64::new(0),
            stream_bytes_out: AtomicU64::new(0),
            replicating_charts: AtomicU32::new(0),
            labels_applied: AtomicBool::new(false),
            labels_applied_version: AtomicU32::new(0),
            sender_flags: AtomicU32::new(0),
            sender_replicating_charts: AtomicU32::new(0),
            global_functions: Mutex::new(()),
            upstream: OnceLock::new(),
            meta_flags,
            pending_flags,
            metadata_lifetime: RwLock::new(false),
            storage: Arc::clone(storage),
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

    /// The `pending_flags` its charts have raised.
    pub fn pending_flags(&self) -> u32 {
        self.pending_flags.load(Ordering::Acquire)
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

    /// A replication reply came from the child (`counter_in`).
    pub fn count_replication_reply(&self) {
        self.replication_replies.fetch_add(1, Ordering::Relaxed);
    }

    pub fn replication_replies(&self) -> u32 {
        self.replication_replies.load(Ordering::Relaxed)
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

    /// The records of `rrdhost_create()` before its index add: the sender's parents and an invalid machine GUID.
    fn log_created_before_index(&self) {
        log_stream_parents(&self.info());
        if uuid_parse_flexi(self.machine_guid.as_bytes()).is_none() {
            netdata_log_error!("Host machine GUID {} is not valid", self.machine_guid);
        }
    }

    /// The records of `rrdhost_create()`; an archived host gets no function registry, so no NRPC record.
    fn log_created_with(&self, registry: bool) {
        let info = self.info();
        self.log_created_before_index();
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
        // nrpc_owner_str(): the host's handle in hex, the registry's label in C's records
        let key = format!("0x{:016X}", std::ptr::from_ref(self) as usize);
        nd_log!(Source::Daemon, Priority::Debug, "NRPC: function registry {key} created for host '{hostname}'");
        self.functions.set_owner(key);
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
    /// `history` are the configured values before `rrdhost_create()` normalizes them, as C compares them; so are the
    /// replication settings, which only an archived host takes, capped for its own ring.
    pub fn update(
        &self,
        wanted: &HostInfo,
        update_every: i64,
        history: i64,
        replication: bool,
        replication_period: i64,
        replication_step: i64,
    ) {
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
        // the host connected again: it gets back its function registry, and the sender and replication settings
        // it was archived without
        if self.archived.swap(false, Ordering::AcqRel) {
            self.functions.init();
            let hostname = self.hostname();
            self.log_registry_created(&hostname);
            let sender_initialized = {
                let mut info = self.info.write().unwrap_or_else(PoisonError::into_inner);
                let initialized = info.stream_send.is_none();
                if initialized {
                    info.stream_send.clone_from(&wanted.stream_send);
                }
                info.set_replication(replication, replication_period, replication_step);
                initialized
            };
            if sender_initialized {
                log_stream_parents(&self.info());
            }
            self.set_replication_percent(100.0);
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

    /// `rrdlabels_version(host->rrdlabels)`.
    pub fn labels_version(&self) -> u32 {
        self.labels
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .version()
    }

    /// The pulse charts of this child take its labels of `version`: whether they must (again), because the version
    /// moved or they never took them (a host without labels stays at version 0).
    pub fn pulse_labels_refresh(&self, version: u32) -> bool {
        let old = self.labels_applied_version.swap(version, Ordering::Relaxed);
        let applied = self.labels_applied.swap(true, Ordering::Relaxed);
        old != version || !applied
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

    /// `rrdhost_claim_id_of_parent_get()`.
    pub fn claim_id_of_parent(&self) -> [u8; 16] {
        *self.claim_id_of_parent.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// `rrdhost_claim_id_of_parent_update()`: the previous id when it changed.
    pub fn update_claim_id_of_parent(&self, id: [u8; 16]) -> Option<[u8; 16]> {
        let mut current = self.claim_id_of_parent.write().unwrap_or_else(PoisonError::into_inner);
        let previous = *current;
        (previous != id).then(|| {
            *current = id;
            previous
        })
    }

    /// `rrdhost_claim_id_of_origin_set()`.
    pub fn set_claim_id_of_origin(&self, id: [u8; 16]) {
        *self
            .claim_id_of_origin
            .write()
            .unwrap_or_else(PoisonError::into_inner) = id;
    }

    /// A `VARIABLE HOST` line (`rrdvar_host_variable_add_and_acquire()`, then `rrdvar_host_variable_set()`): the
    /// value always counts as changed, so it goes to the parent at once.
    pub fn set_variable(&self, name: &str, value: f64) {
        let name = self.variables.put(name, value);
        upstream::send_host_variable(self, &name, value);
    }

    /// The host variables, in insertion order.
    pub fn variables(&self) -> Vec<(String, f64)> {
        self.variables.all()
    }

    /// `rrdvar_get_custom_host_variable_value()`.
    pub fn variable(&self, name: &[u8]) -> Option<f64> {
        self.variables.get(name)
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

    /// `stream_path_parent_disconnected()`: the entries after `host_id`'s go, under one lock; true when some went (C
    /// then sends the path to the child).
    pub fn cut_stream_path_after(&self, host_id: [u8; 16]) -> bool {
        let mut path = self
            .stream_path
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        match path.iter().position(|p| p.host_id == host_id) {
            Some(at) if at + 1 < path.len() => {
                path.truncate(at + 1);
                true
            }
            _ => false,
        }
    }

    /// `IS_VIRTUAL_HOST_OS()`: a virtual node, by its operating system.
    pub fn is_virtual_host_os(&self) -> bool {
        self.info().os == VIRTUAL_HOST_OS
    }

    /// `RRDHOST_OPTION_EPHEMERAL_HOST`.
    pub fn is_ephemeral(&self) -> bool {
        self.ephemeral.load(Ordering::Relaxed)
    }

    pub fn set_ephemeral(&self, ephemeral: bool) {
        self.ephemeral.store(ephemeral, Ordering::Relaxed);
    }

    /// `host->stream.rcv.status.last_connected`.
    pub fn receiver_last_connected_s(&self) -> i64 {
        self.receiver_last_connected_s.load(Ordering::Relaxed)
    }

    /// `host->stream.rcv.status.last_disconnected`.
    pub fn receiver_last_disconnected_s(&self) -> i64 {
        self.receiver_last_disconnected_s.load(Ordering::Relaxed)
    }

    /// Takes the `pending_flags` its charts raised, for a maintenance sweep.
    pub fn take_pending_flags(&self) -> u32 {
        self.pending_flags.fetch_and(!pending_flags::OBSOLETE, Ordering::AcqRel) & pending_flags::OBSOLETE
    }

    /// Takes the host's two health flags in one step (`rrdhost_flag_set_and_clear()` in
    /// `health_execute_delayed_initializations()`), so that one raised meanwhile is kept for the next pass.
    pub fn take_health_pending(&self) -> u32 {
        self.pending_flags.fetch_and(!pending_flags::HEALTH, Ordering::AcqRel) & pending_flags::HEALTH
    }

    /// `rrdhost_flag_set(host, RRDHOST_FLAG_PENDING_LABEL_RECHECK)`: the host's labels were set again.
    pub fn raise_label_recheck(&self) {
        self.pending_flags.fetch_or(pending_flags::LABEL_RECHECK, Ordering::AcqRel);
    }

    /// Raises `pending_flags` again: a sweep left work for the next one.
    pub fn raise_pending_flags(&self, flags: u32) {
        self.pending_flags.fetch_or(flags, Ordering::AcqRel);
    }

    /// `rrdhost_set_health_evloop_iteration()`.
    pub fn stamp_health_iteration(&self) {
        self.health_last_iteration
            .store(self.storage().health_iteration(), Ordering::Relaxed);
    }

    /// `rrdhost_health_evloop_last_iteration()`.
    pub fn health_last_iteration(&self) -> u64 {
        self.health_last_iteration.load(Ordering::Relaxed)
    }

    /// An ephemeral host loaded from the metadata database counts as disconnected at its load
    /// (`sql_create_aclk_table_for_host()`), so that its cleanup time runs from then.
    pub fn set_receiver_last_disconnected_s(&self, seconds: i64) {
        self.receiver_last_disconnected_s
            .store(seconds, Ordering::Relaxed);
    }

    /// `host->receiver`.
    pub fn receiver(&self) -> Option<Arc<ReceiverSlot>> {
        lock(&self.receiver).clone()
    }

    /// `rrdhost_set_receiver()`: refused while the maintenance marks the host's charts obsolete, or while another
    /// receiver is attached.
    pub fn set_receiver(&self, slot: Arc<ReceiverSlot>) -> Attach {
        let mut receiver = lock(&self.receiver);
        if self.obsolete_all_busy.load(Ordering::Acquire) {
            return Attach::CleanupBusy;
        }
        // a vnode has one writer, the plugin collecting it: a receiver that got here before the claim is refused
        if self.is_virtual() {
            return Attach::VnodeIsLocal;
        }
        if receiver.is_some() {
            return Attach::AlreadyServed;
        }
        // object_state_activate_if_not_activated(): what an earlier connection registered is no longer available
        self.functions.activate();
        *receiver = Some(slot);
        if let Some(count) = self.receivers_connected.get() {
            count.fetch_add(1, Ordering::Relaxed);
        }
        self.receiver_connections.fetch_add(1, Ordering::Relaxed);
        self.receiver_last_connected_s
            .store(now_realtime_s(), Ordering::Relaxed);
        self.receiver_last_disconnected_s.store(0, Ordering::Relaxed);
        self.stamp_health_iteration();
        self.orphan
            .store(false, std::sync::atomic::Ordering::Release);
        self.replication_reset();
        // the child's charts report their update every again
        self.min_update_every
            .store(u32::MAX, std::sync::atomic::Ordering::Release);
        self.min_update_every_applied
            .store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        self.local.fetch_or(local_flags::COLLECTOR_ONLINE, Ordering::AcqRel);
        drop(receiver);
        self.contexts_child_connected();
        Attach::Attached
    }

    /// `rrdcontext_host_child_connected()`: every chart and dimension reports collection again.
    pub fn contexts_child_connected(&self) {
        for chart in self.charts.all() {
            contexts::rrdset_not_collected(&chart);
        }
    }

    /// `pluginsd_host_claim_as_local_vnode()`'s eviction, with VIRTUAL already set so that no receiver attaches after
    /// it: a receiver still attached is stopped and waited for (`stream_receiver_signal_to_stop_and_wait()`).
    pub fn claim_as_local_vnode(&self) -> Claim {
        match self.receiver() {
            None => Claim::Free,
            Some(slot) if self.stop_receiver_and_wait(&slot) => Claim::Evicted,
            Some(_) => Claim::Stuck,
        }
    }

    /// The host is collected again (`RRDHOST_FLAG_ORPHAN` cleared), as a vnode's definition does.
    pub fn clear_orphan(&self) {
        self.orphan.store(false, Ordering::Release);
    }

    /// `svc_rrdhost_obsolete_all_charts()`: every chart is marked obsolete, so the charts a child does not define again
    /// are freed by the maintenance; each accepted connection does it, and so does the maintenance for a child gone.
    pub fn obsolete_all_charts(&self) {
        for chart in self.charts.all() {
            chart.is_obsolete(self);
        }
    }

    /// The maintenance's obsolete-all of a child gone for longer than `obsolete_s`: decided under the receiver lock (no
    /// receiver, none attached since, gone long enough, no pass running), the walk after it, a receiver that attaches
    /// meanwhile refused as busy. Whether it ran.
    pub fn obsolete_all_if_gone(&self, now_s: i64, obsolete_s: i64) -> bool {
        {
            let receiver = lock(&self.receiver);
            if receiver.is_some()
                || self.receiver_last_connected_s() != 0
                || self.receiver_last_disconnected_s().saturating_add(obsolete_s) >= now_s
                || self.obsolete_all_busy.load(Ordering::Acquire)
            {
                return false;
            }
            self.obsolete_all_busy.store(true, Ordering::Release);
        }
        self.obsolete_all_charts();
        self.obsolete_all_busy.store(false, Ordering::Release);
        true
    }

    /// `rrdhost_should_be_cleaned_up()`: a child other than `protected` that nothing replicates, orphan, loaded and not
    /// collecting, with more than 10 HEALTH passes and the orphan time since it was disconnected.
    pub fn should_be_cleaned_up(&self, protected: &Host, now_s: i64) -> bool {
        let disconnected = self.receiver_last_disconnected_s();
        !std::ptr::eq(self, protected)
            && !self.is_localhost
            && self.replicating_charts() == 0
            && self.sender_replicating_charts() == 0
            && self.is_orphan()
            && !self.is_pending_context_load()
            && !self.collector_online()
            && self.storage().health_iteration().saturating_sub(self.health_last_iteration()) > 10
            && disconnected != 0
            && disconnected.saturating_add(self.storage().cleanup_times().orphan_hosts_s) < now_s
    }

    /// `rrdhost_cleanup_data_collection_and_health()`: the host's receiver stopped (a wait of about 2 s), every chart
    /// freed, its stream path, variables and functions gone; archived and orphan, with C's record.
    pub fn cleanup_data_collection(&self) {
        if let Some(slot) = self.receiver() {
            self.stop_receiver_and_wait(&slot);
        }
        // rrdcalc_delete_all(), before the charts go
        self.storage().health_event(HealthEvent::HostCleanup(self.machine_guid()));
        self.charts.flush();
        self.variables.clear();
        self.replace_stream_path(Vec::new());
        // before the functions go (C frees the sender before the function registry)
        self.free_upstream();
        self.functions.destroy();
        self.archived.store(true, Ordering::Release);
        self.orphan.store(true, Ordering::Release);
        self.log_archive_mode();
    }

    /// `stream_receiver_signal_to_stop_and_wait()`: true when the receiver let go within 2 s, else C's error record.
    pub fn stop_receiver_and_wait(&self, slot: &Arc<ReceiverSlot>) -> bool {
        slot.stop();
        let attached = || self.receiver().is_some_and(|r| Arc::ptr_eq(&r, slot));
        for _ in 0..2000 {
            if !attached() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if !attached() {
            return true;
        }
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM RCV[x] '{}' [from [{}]:{}]: streaming thread takes too long to stop, giving up...",
            self.hostname(),
            slot.remote.0,
            slot.remote.1
        );
        false
    }

    /// `rrdhost_free_unlinked()` of a host out of the index: its data collection cleaned up, then marked deleted.
    fn freed_unlinked(&self) {
        self.cleanup_data_collection();
        self.storage().health_event(HealthEvent::HostFreed(self.machine_guid()));
        self.pulse_status(crate::pulse::host_status::DELETED);
    }

    /// `rrdhost_is_online()`: localhost or a vnode, or a host whose collector is online and that is not an orphan.
    pub fn is_online(&self) -> bool {
        self.is_local() || (self.collector_online() && !self.is_orphan())
    }

    /// The `local_flags` in one load, as C's status reads its `flags` once.
    pub(crate) fn local_flags(&self) -> u8 {
        self.local.load(Ordering::Acquire)
    }

    /// `rrdhost_is_virtual()`: a vnode this agent's plugins collect.
    pub fn is_virtual(&self) -> bool {
        self.local.load(Ordering::Acquire) & local_flags::VIRTUAL != 0
    }

    /// `rrdhost_is_local()`: localhost or a vnode.
    pub fn is_local(&self) -> bool {
        self.is_localhost || self.is_virtual()
    }

    /// `RRDHOST_FLAG_COLLECTOR_ONLINE`: the host's collector is collecting it.
    pub fn collector_online(&self) -> bool {
        self.local.load(Ordering::Acquire) & local_flags::COLLECTOR_ONLINE != 0
    }

    /// A plugin of this agent claims the host as its vnode (`RRDHOST_FLAG_VIRTUAL_HOST` set).
    pub fn set_virtual(&self) {
        self.local.fetch_or(local_flags::VIRTUAL, Ordering::AcqRel);
    }

    /// The vnode's claim failed.
    pub fn clear_virtual(&self) {
        self.local.fetch_and(!local_flags::VIRTUAL, Ordering::AcqRel);
    }

    /// The vnode's plugin collects it (`RRDHOST_FLAG_COLLECTOR_ONLINE` set).
    pub fn set_collector_online(&self) {
        self.local.fetch_or(local_flags::COLLECTOR_ONLINE, Ordering::AcqRel);
    }

    /// The vnode's plugin stopped collecting it: both flags cleared in one step.
    pub fn virtual_offline(&self) {
        self.local.fetch_and(!(local_flags::VIRTUAL | local_flags::COLLECTOR_ONLINE), Ordering::AcqRel);
    }

    /// `RRDHOST_FLAG_ORPHAN`.
    pub fn is_orphan(&self) -> bool {
        self.orphan.load(std::sync::atomic::Ordering::Acquire)
    }

    /// `rrdhost_ingestion_hops()`: 0 for localhost, 1 for a vnode, else what the child reported.
    pub fn ingestion_hops(&self) -> i16 {
        if self.is_localhost {
            0
        } else if self.is_virtual() {
            1
        } else {
            self.info
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .system_info
                .hops
        }
    }

    /// Installs the host's sender (the first one installed stays) and enables it.
    fn install_upstream(&self, upstream: Arc<dyn Upstream>) {
        let _ = self.upstream.set(upstream);
        self.sender_flags_set(sender_flags::ENABLED);
    }

    /// A host's sender for a test, without the settings [`Host::init_upstream`] needs.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_upstream(&self, upstream: Arc<dyn Upstream>) {
        self.install_upstream(upstream);
    }

    /// Holds the maintenance's obsolete-all pass as running, so a test's attach meets it.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_obsolete_all_busy(&self, busy: bool) {
        self.obsolete_all_busy.store(busy, Ordering::Release);
    }

    /// `stream_sender_structures_init()`: a host whose settings stream it gets its sender, `create`d the first time
    /// and set up again with the settings of now after a free (where C allocates a new one). One caller is elected
    /// to do it (INITIALIZED, set before the setup and cleared at the end of the free, as C's flag); nothing without
    /// settings.
    pub fn init_upstream(&self, create: impl FnOnce(&StreamSend) -> Arc<dyn Upstream>) {
        let Some(send) = self.info.read().unwrap_or_else(PoisonError::into_inner).stream_send.clone() else {
            return;
        };
        if self.sender_flags_set(sender_flags::INITIALIZED) & sender_flags::INITIALIZED != 0 {
            return;
        }
        match self.upstream.get() {
            Some(up) => {
                up.reinit(&send);
                self.sender_flags_set(sender_flags::ENABLED);
            }
            None => self.install_upstream(create(&send)),
        }
    }

    /// `stream_sender_structures_free()`: the sender stops and is emptied, and the host streams no more until a
    /// revival sets it up with the settings of that time (D118). A setup meanwhile does nothing (INITIALIZED): the
    /// host waits for its next revival, as C sets up only an archived host.
    fn free_upstream(&self) {
        let was = self.sender_flags_clear(sender_flags::ENABLED);
        if was & sender_flags::ENABLED != 0
            && let Some(up) = self.upstream.get()
        {
            up.free();
        }
        self.info.write().unwrap_or_else(PoisonError::into_inner).stream_send = None;
        self.sender_flags_clear(sender_flags::INITIALIZED);
    }

    /// `host->sender` when the host streams (`rrdhost_has_stream_sender_enabled()`).
    pub fn upstream(&self) -> Option<&Arc<dyn Upstream>> {
        self.upstream.get().filter(|_| self.sender_flags() & sender_flags::ENABLED != 0)
    }

    pub fn sender_flags(&self) -> u32 {
        self.sender_flags.load(Ordering::SeqCst)
    }

    /// Sets `bits`; the flags before.
    pub fn sender_flags_set(&self, bits: u32) -> u32 {
        self.sender_flags.fetch_or(bits, Ordering::SeqCst)
    }

    /// Clears `bits`; the flags before.
    pub fn sender_flags_clear(&self, bits: u32) -> u32 {
        self.sender_flags.fetch_and(!bits, Ordering::SeqCst)
    }

    /// `rrdhost_can_stream_metadata_to_parent()`: the host streams, its sender is ready and its collection is online.
    pub fn can_stream_metadata(&self) -> bool {
        self.upstream().is_some()
            && self.sender_flags() & sender_flags::READY_4_METRICS != 0
            && self.collector_online()
    }

    /// `host->stream.snd.charts_matching`, none when the host does not stream.
    pub fn with_charts_matching<T>(&self, f: impl FnOnce(Option<&SimplePattern>) -> T) -> T {
        let info = self.info.read().unwrap_or_else(PoisonError::into_inner);
        f(info.stream_send.as_ref().map(|s| &s.charts_matching))
    }

    /// `rrdhost_sender_replicating_charts()`.
    pub fn sender_replicating_charts(&self) -> u32 {
        self.sender_replicating_charts.load(Ordering::Relaxed)
    }

    /// `rrdhost_sender_replicating_charts_plus_one()`: the new count.
    pub fn sender_replicating_charts_plus_one(&self) -> u32 {
        self.sender_replicating_charts.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    /// `rrdhost_sender_replicating_charts_minus_one()`: the new count.
    pub fn sender_replicating_charts_minus_one(&self) -> u32 {
        self.sender_replicating_charts.fetch_sub(1, Ordering::Relaxed).wrapping_sub(1)
    }

    /// `sender->global_functions_spinlock`.
    pub(crate) fn lock_global_functions(&self) -> MutexGuard<'_, ()> {
        lock(&self.global_functions)
    }

    /// `nrpc_method_register()` on the host's registry, then `rrdhost_nrpc_changed()`: the functions go to the parent
    /// again at the next collection.
    pub fn register_function(&self, desc: &MethodDesc<'_>) -> Result<(), String> {
        self.functions.register(&self.hostname(), desc)?;
        self.sender_flags_set(sender_flags::GLOBAL_FUNCTIONS_UPDATED);
        Ok(())
    }

    /// `nrpc_method_register_builtin()` on the host's registry, which the parent hears of as any registration.
    pub fn register_builtin(&self, desc: &BuiltinDesc<'_>) -> Result<(), String> {
        self.functions.register_builtin(&self.hostname(), desc)?;
        self.sender_flags_set(sender_flags::GLOBAL_FUNCTIONS_UPDATED);
        Ok(())
    }

    /// `nrpc_method_unregister()` on the host's registry, queued for the parent while the host has a sender
    /// (`rrdhost_nrpc_wants_del_journal()`), then `rrdhost_nrpc_changed()` for a removal.
    pub fn unregister_function(&self, name: &[u8], source: nrpc::Source) -> Unregistered {
        let r = self.functions.unregister(name, source, self.upstream().is_some());
        if matches!(r, Unregistered::Removed { .. }) {
            self.sender_flags_set(sender_flags::GLOBAL_FUNCTIONS_UPDATED);
        }
        r
    }

    /// `stream_receiver_replication_reset()`: no chart is being replicated by a receiver that just came or went, so
    /// the next connection asks for every chart's missing data again.
    fn replication_reset(&self) {
        for chart in self.charts.all() {
            let old = chart.update_meta(|m| {
                let old = m.flags;
                m.flags |= chart::flags::RECEIVER_REPLICATION_FINISHED;
                m.flags &= !chart::flags::RECEIVER_REPLICATION_IN_PROGRESS;
                old
            });
            if old & chart::flags::RECEIVER_REPLICATION_FINISHED == 0 {
                self.replicating_charts_minus_one();
            }
        }
        let left = self.replicating_charts();
        if left != 0 {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "STREAM REPLAY ERROR: receiver replication instances counter should be zero, but it is {left} - \
                 resetting it to zero"
            );
            self.replicating_charts.store(0, Ordering::Relaxed);
        }
        self.replication_requests
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.replication_replies.store(0, Ordering::Relaxed);
        self.backfill_pending.store(0, Ordering::Relaxed);
    }

    /// `pulse_host_status()`, without its reason counters (they feed the extended charts only, D80.4): `status` 0
    /// detects the receiver's state from the host's status. A basic or receiver state takes the host's ephemerality;
    /// the running latch keeps a receiver that reached running there through its charts' replication ripples; the
    /// state replaces the flags of its class (all of them for a basic one, the inbound ones for a receiver one, the
    /// sender ones for a sender one), and a change of the inbound state restarts its age. As in C, a deletion takes
    /// the ephemerality bit before its test, so it is stored with it instead of clearing the state.
    pub fn pulse_status(&self, status: u32) {
        use crate::pulse::host_status::*;
        let now_s = now_realtime_s();
        let mut status = if status == 0 {
            self.detect_receiver_status(now_s)
        } else {
            status
        };
        if status & (BASIC | RECEIVER) != 0 && status & EPHEMERALITY == 0 {
            status |= if self.is_ephemeral() {
                EPHEMERAL
            } else {
                PERMANENT
            };
        }
        if status & RCV_RUNNING != 0 {
            self.running_latched.store(true, Ordering::Relaxed);
        } else if status & RCV_REPLICATING != 0 {
            if self.running_latched.load(Ordering::Relaxed)
                && self.pulse_state.load(Ordering::Relaxed) & RCV_RUNNING != 0
            {
                status = (status & !RCV_REPLICATING) | RCV_RUNNING;
            } else {
                self.running_latched.store(false, Ordering::Relaxed);
            }
        } else if status & RCV_OFFLINE != 0 {
            self.running_latched.store(false, Ordering::Relaxed);
        }
        let remove = if status & BASIC != 0 {
            BASIC | RECEIVER | EPHEMERALITY | SENDER
        } else if status & RECEIVER != 0 {
            BASIC | RECEIVER | EPHEMERALITY
        } else if status & SENDER != 0 {
            SENDER
        } else {
            0
        };
        let next = |cur: u32| {
            if status == DELETED {
                0
            } else {
                (cur & !remove) | status
            }
        };
        let old = self
            .pulse_state
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| Some(next(cur)))
            .unwrap_or_else(|cur| cur);
        if next(old) & (BASIC | RECEIVER) != old & (BASIC | RECEIVER) {
            self.state_changed_s.store(now_s, Ordering::Relaxed);
        }
    }

    /// `pulse_host_detect_receiver_status()`: the state the host's basic status gives.
    fn detect_receiver_status(&self, now_s: i64) -> u32 {
        use crate::pulse::host_status::*;
        use crate::status::{DbStatus, IngestStatus, IngestType};
        let s = self.status_basic(now_s);
        if s.db_status == DbStatus::Initializing || s.ingest_status == IngestStatus::Initializing {
            LOADING
        } else if s.ingest_type == IngestType::Localhost {
            LOCAL
        } else if s.ingest_type == IngestType::Virtual {
            VIRTUAL
        } else {
            match s.ingest_status {
                IngestStatus::Archived => ARCHIVED,
                IngestStatus::Replicating => RCV_REPLICATING,
                IngestStatus::Offline => RCV_OFFLINE,
                IngestStatus::Online => RCV_RUNNING,
                IngestStatus::Initializing => 0,
            }
        }
    }

    /// `host->stream.pulse_state`.
    pub fn pulse_state(&self) -> u32 {
        self.pulse_state.load(Ordering::Relaxed)
    }

    /// `host->stream.rcv.status.state_changed_s`.
    pub fn state_changed_s(&self) -> i64 {
        self.state_changed_s.load(Ordering::Relaxed)
    }

    /// Bytes a receiver of this host read from, and wrote to, its child.
    pub fn stream_bytes_received(&self, bytes: usize) {
        self.stream_bytes_in
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn stream_bytes_sent(&self, bytes: usize) {
        self.stream_bytes_out
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `host->stream.rcv.status.bytes_in` and `bytes_out`.
    pub fn stream_bytes(&self) -> (u64, u64) {
        (
            self.stream_bytes_in.load(Ordering::Relaxed),
            self.stream_bytes_out.load(Ordering::Relaxed),
        )
    }

    /// `rrdhost_receiver_replicating_charts()`.
    pub fn replicating_charts(&self) -> u32 {
        self.replicating_charts.load(Ordering::Relaxed)
    }

    /// `rrdhost_receiver_replicating_charts_plus_one()`: the new count.
    pub fn replicating_charts_plus_one(&self) -> u32 {
        self.replicating_charts
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    /// `rrdhost_receiver_replicating_charts_minus_one()`: the new count (it wraps below 0, as C's).
    pub fn replicating_charts_minus_one(&self) -> u32 {
        self.replicating_charts
            .fetch_sub(1, Ordering::Relaxed)
            .wrapping_sub(1)
    }

    /// `host->stream.rcv.status.connections`.
    pub fn receiver_connections(&self) -> u32 {
        self.receiver_connections.load(Ordering::Relaxed)
    }

    /// `backfill_pending`.
    pub fn backfill_pending(&self) -> u32 {
        self.backfill_pending.load(Ordering::Relaxed)
    }

    /// A chart's replication now waits for its backfill (`backfill_pending++`).
    pub fn backfill_requested(&self) {
        self.backfill_pending.fetch_add(1, Ordering::Relaxed);
    }

    /// `backfill_pending--` of an answer on the receiver's own thread.
    pub fn backfill_answered_inline(&self) {
        let _ = self
            .backfill_pending
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1));
    }

    /// `object_state_acquire()` and `backfill_pending--` of a backfill's answer: false when `receiver` is no longer
    /// the attached one (it went, or another came), which a reset already accounted for.
    pub fn backfill_answered(&self, receiver: &Weak<ReceiverSlot>) -> bool {
        let attached = lock(&self.receiver);
        if !attached
            .as_ref()
            .is_some_and(|r| std::ptr::eq(Arc::as_ptr(r), receiver.as_ptr()))
        {
            return false;
        }
        self.backfill_answered_inline();
        true
    }

    /// Whether `receiver` is the attached one (C's host state id, deactivated when the detach starts).
    pub fn is_receiver(&self, receiver: &Weak<ReceiverSlot>) -> bool {
        lock(&self.receiver).as_ref().is_some_and(|r| {
            std::ptr::eq(Arc::as_ptr(r), receiver.as_ptr()) && !r.detaching.load(Ordering::Acquire)
        })
    }

    /// `rrdhost_clear_receiver()`: detaches `slot` if it is still the attached one; then, the receiver lock released
    /// as C releases it, the host's sender is told the receiver left and its parents reset, with the receiver's
    /// `reason` (a `STREAM_HANDSHAKE` code); the slot empties last, as C sets `host->receiver = NULL` last
    /// (`stream-receiver.c:1507`), so whoever waits for it (a stale receiver's replacement, a free) waits for those
    /// steps too.
    pub fn clear_receiver(&self, slot: &Arc<ReceiverSlot>, reason: i32) {
        self.clear_receiver_then(slot, reason, || {});
    }

    /// [`Host::clear_receiver`], then `cleanup` under the receiver lock, attached or not: C cleans the receiver's
    /// parser up there (`stream-receiver.c:1510-1513`), so a new receiver collects into the host only after the old
    /// parser let its charts go.
    pub fn clear_receiver_then(&self, slot: &Arc<ReceiverSlot>, reason: i32, cleanup: impl FnOnce()) {
        let receiver = lock(&self.receiver);
        if !receiver.as_ref().is_some_and(|r| Arc::ptr_eq(r, slot)) || slot.detaching.swap(true, Ordering::AcqRel) {
            cleanup();
            return;
        }
        self.local.fetch_and(!local_flags::COLLECTOR_ONLINE, Ordering::AcqRel);
        // object_state_deactivate(): the child's functions unavailable, still registered
        self.functions.activate();
        self.receiver_last_connected_s.store(0, Ordering::Relaxed);
        self.receiver_last_disconnected_s
            .store(now_realtime_s(), Ordering::Relaxed);
        // health stays off until the child returns (rrdhost_update() sets it again): the stale path entry has no
        // HEALTH flag and a later metadata store writes it off
        self.info.write().unwrap_or_else(PoisonError::into_inner).health_enabled = false;
        self.stamp_health_iteration();
        self.orphan
            .store(true, std::sync::atomic::Ordering::Release);
        self.contexts.record_first_time_changes(Taker::Receiver, false);
        // stream_path_child_disconnected()
        self.replace_stream_path(Vec::new());
        self.replication_reset();
        drop(receiver);
        if let Some(up) = self.upstream() {
            up.receiver_left(reason);
        }
        self.contexts.child_disconnected();
        if let Some(up) = self.upstream() {
            up.parents_reset(reason);
        }
        let mut receiver = lock(&self.receiver);
        if receiver.as_ref().is_some_and(|r| Arc::ptr_eq(r, slot)) {
            *receiver = None;
            if let Some(count) = self.receivers_connected.get() {
                count.fetch_sub(1, Ordering::Relaxed);
            }
        }
        cleanup();
    }
}

/// Why `rrdhost_find_or_create()` returned NULL: the receiver answers busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotCreated {
    /// `rrdhost_create()`'s index add met a host under the same key (a GUID longer than the key, D126.7).
    IndexCollision,
    /// An archived host of another memory mode is being stored by the metadata writer (D95.7).
    MetadataBusy,
}

/// `GUID_LEN + 1`: how much of a machine GUID `rrdhost_create()` keeps.
const GUID_KEPT: usize = 37;

/// `strncpyz(host->machine_guid, guid, GUID_LEN + 1)`: the first 37 bytes of a machine GUID, at a character boundary
/// (C then writes the terminator past its array, which is not reproduced).
pub fn guid_key(guid: &str) -> &str {
    if guid.len() <= GUID_KEPT {
        return guid;
    }
    let mut end = GUID_KEPT;
    while !guid.is_char_boundary(end) {
        end -= 1;
    }
    &guid[..end]
}

/// The host index (`rrdhost_root_index` and the `localhost` list).
#[derive(Debug)]
pub struct Hosts {
    localhost: Arc<Host>,
    inner: RwLock<Index<Host>>,
    /// `dictionary_version(rrdhost_root_index)`: one per insert (and delete).
    version: std::sync::atomic::AtomicU32,
    /// `is_parent_label_cached_state` under its commit lock: whether localhost's `_is_parent` says a child is connected.
    is_parent: Mutex<bool>,
    /// `streaming_connected_receivers`, which every host of the index counts into.
    receivers: Arc<AtomicU32>,
    /// The storage every host it creates gets.
    storage: Arc<StorageLayout>,
    /// `rrdhost_load_rrdcontext_data()` over the daemon's databases, for the hosts it creates.
    context_loader: OnceLock<ContextLoader>,
}

/// `NETDATA_VIRTUAL_HOST`: the operating system of a virtual node.
pub const VIRTUAL_HOST_OS: &str = "Netdata Virtual Host 1.0";

/// The hosts' write lock held across a walk, as `rrd_wrlock()`.
pub struct HostsWrite<'a> {
    hosts: &'a Hosts,
    index: std::sync::RwLockWriteGuard<'a, Index<Host>>,
}

impl HostsWrite<'_> {
    /// Every host, localhost first, then in creation order.
    pub fn all(&self) -> Vec<Arc<Host>> {
        self.index.items().to_vec()
    }

    /// `rrdhost_free___while_having_rrd_wrlock()`: `host` leaves the index while the index still holds that very host
    /// (C's `host_check == host`; never localhost), then its data collection is cleaned up and it is marked deleted.
    /// Holders of its `Arc` keep it until they drop it.
    pub fn free(&mut self, host: &Host) -> Option<Arc<Host>> {
        let guid = host.machine_guid();
        if guid == self.hosts.localhost.machine_guid
            || !self.index.get(guid).is_some_and(|h| std::ptr::eq(&*h, host))
        {
            return None;
        }
        let host = self.index.remove(guid)?;
        self.hosts
            .version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // nothing inside takes the index again
        host.freed_unlinked();
        Some(host)
    }
}

/// Loads a new host's contexts on the creating thread.
struct ContextLoader(Box<dyn Fn(&Host) + Send + Sync>);

impl std::fmt::Debug for ContextLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContextLoader")
    }
}

impl Hosts {
    /// The index of hosts without the dbengine.
    pub fn new(localhost: Host) -> Self {
        Hosts::with_storage(localhost, Arc::default())
    }

    /// The index of hosts with this storage; `localhost` was created with it.
    pub fn with_storage(localhost: Host, storage: Arc<StorageLayout>) -> Self {
        let localhost = localhost.into_shared();
        let receivers = Arc::new(AtomicU32::new(0));
        let _ = localhost.receivers_connected.set(Arc::clone(&receivers));
        // creation order, localhost first
        let mut index = Index::default();
        index.insert(&localhost.machine_guid, Arc::clone(&localhost));
        localhost.log_created();
        localhost.created_connected();
        Hosts {
            localhost,
            inner: RwLock::new(index),
            version: std::sync::atomic::AtomicU32::new(1),
            is_parent: Mutex::new(false),
            receivers,
            storage,
            context_loader: OnceLock::new(),
        }
    }

    pub fn storage(&self) -> &Arc<StorageLayout> {
        &self.storage
    }

    /// The loader of the contexts of the hosts `find_or_create()` creates (set once, by the daemon).
    pub fn set_context_loader(&self, loader: impl Fn(&Host) + Send + Sync + 'static) {
        let _ = self.context_loader.set(ContextLoader(Box::new(loader)));
    }

    /// `rrdhost_load_rrdcontext_data()` through the daemon's loader, when one is set.
    pub fn load_contexts(&self, host: &Host) {
        if let Some(loader) = self.context_loader.get() {
            (loader.0)(host);
        }
    }

    /// `stream_receivers_currently_connected()`: the receivers attached to the index's hosts, without its lock (a
    /// receiver leaves while a walk that frees hosts holds it).
    pub fn receivers_connected(&self) -> usize {
        self.receivers.load(Ordering::Relaxed) as usize
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
    /// A label that changed asks health to recheck localhost's labels.
    pub fn update_is_parent_label(&self) {
        let mut cached = lock(&self.is_parent);
        let desired = self.receivers_connected() > 0;
        if *cached == desired {
            return;
        }
        *cached = desired;
        let value: &[u8] = if desired { b"true" } else { b"false" };
        let changed =
            self.localhost.update_labels(|labels| labels.add_changed(b"_is_parent", value, crate::labels::SRC_AUTO));
        // a rule's `host labels` may name it: localhost's alerts are matched again
        if changed.unwrap_or(false) {
            self.localhost.raise_label_recheck();
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
        self.inner.read().unwrap_or_else(PoisonError::into_inner).get(guid)
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
        index.items().iter().find(|h| h.hostname() == hostname).cloned()
    }

    /// `rrdhost_find_by_node_id()`: the first host whose node ID equals the parsed UUID. Unclaimed hosts have a
    /// zero node ID, so the nil UUID finds the first of them.
    pub fn find_by_node_id(&self, node_id: &[u8; 16]) -> Option<Arc<Host>> {
        let index = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        index.items().iter().find(|h| h.node_id() == *node_id).cloned()
    }

    /// Every host, localhost first, then in creation order.
    pub fn all(&self) -> Vec<Arc<Host>> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .items()
            .to_vec()
    }

    /// `rrd_wrlock()`: the hosts' write lock, for a walk that frees hosts; creating, discarding and freeing hosts, and
    /// every lookup, wait for it.
    pub fn write(&self) -> HostsWrite<'_> {
        HostsWrite {
            hosts: self,
            index: self.inner.write().unwrap_or_else(PoisonError::into_inner),
        }
    }

    /// [`HostsWrite::free`] under its own write lock.
    pub fn free(&self, host: &Host) -> Option<Arc<Host>> {
        self.write().free(host)
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
        if let Some(host) = index.get(guid) {
            return host;
        }
        let host = Host::with_storage(guid, false, info, &self.storage).into_shared();
        host.archived.store(true, Ordering::Release);
        // created archived: no function registry until it connects (rrdhost.c:636-640)
        host.functions.destroy();
        host.pending_context_load.store(true, Ordering::Release);
        host.orphan.store(true, Ordering::Release);
        before_record(&host);
        let _ = host.receivers_connected.set(Arc::clone(&self.receivers));
        index.insert(guid, Arc::clone(&host));
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
    /// The lookup takes the GUID whole, the new host keeps its first 37 characters ([`guid_key`]): a longer GUID
    /// never finds the host it made, and its next creation meets that host in the index, which C answers with NULL
    /// (D126.7). An archived host still loading its contexts is returned untouched (the receiver refuses it); one of
    /// another memory mode is discarded and created again, unless the metadata writer holds it (NULL too, D95.7).
    pub fn find_or_create(
        &self,
        guid: &str,
        mode: DbMode,
        create: impl FnOnce() -> HostInfo,
        update: impl FnOnce(&Host),
    ) -> Result<Arc<Host>, NotCreated> {
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        let found = index.get(guid);
        let found = match found {
            Some(host) if host.is_archived() && host.info().db_mode != mode => {
                if host.is_pending_context_load() {
                    return Ok(host);
                }
                // rw_spinlock_trywrite_lock(&host->metadata_lifetime_lock): a host being stored is not freed now
                let Some(mut freed) = host.metadata_try_write() else {
                    return Err(NotCreated::MetadataBusy);
                };
                nd_log!(
                    Source::Daemon,
                    Priority::Info,
                    "Archived host '{}' has memory mode '{}', but the wanted one is '{}'. Discarding archived state.",
                    host.hostname(),
                    host.info().db_mode.name(),
                    mode.name()
                );
                *freed = true;
                drop(freed);
                index.remove(guid);
                self.version
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // rrdhost_free___while_having_rrd_wrlock(): nothing inside takes the index again
                host.freed_unlinked();
                None
            }
            found => found,
        };
        if let Some(host) = found {
            drop(index);
            if !host.is_pending_context_load() {
                update(&host);
            }
            return Ok(host);
        }
        let key = guid_key(guid);
        let host = Host::with_storage(key, false, create(), &self.storage).into_shared();
        if let Some(existing) = index.get(key) {
            // rrdhost_index_add_by_guid() met it: C logs what it built so far, the collision, and frees the new host
            drop(index);
            host.log_created_before_index();
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "Host '{}': cannot add host with machine guid '{}' to index. It already exists as host '{}' with \
                 machine guid '{}'.",
                host.hostname(),
                host.machine_guid(),
                existing.hostname(),
                existing.machine_guid()
            );
            host.freed_unlinked();
            return Err(NotCreated::IndexCollision);
        }
        host.created_connected();
        let _ = host.receivers_connected.set(Arc::clone(&self.receivers));
        index.insert(key, Arc::clone(&host));
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        drop(index);
        host.log_created();
        // rrdhost_create() of a host that is not archived: its contexts load here (a dbengine host's from SQL)
        self.load_contexts(&host);
        Ok(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{backfill_dim, collected_chart, engine, info, store, tier_records};
    use nrpc::testing::inert;

    /// `stream_path_parent_disconnected()`: the entries after this agent's go, and only a cut reports one.
    #[test]
    fn the_stream_path_is_cut_after_an_agent() {
        let host = Host::with_storage("guid-p", false, info("p"), &Arc::default());
        let entry = |id: u8| crate::stream_path::PathEntry { host_id: [id; 16], ..Default::default() };
        host.replace_stream_path(vec![entry(1), entry(2), entry(3)]);
        assert!(host.cut_stream_path_after([2; 16]));
        assert_eq!(host.stream_path(), vec![entry(1), entry(2)]);
        assert!(!host.cut_stream_path_after([2; 16]), "nothing after it any more");
        assert!(!host.cut_stream_path_after([9; 16]), "not in the path");
        assert_eq!(host.stream_path(), vec![entry(1), entry(2)]);
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

    /// `rrdstats_metadata_collect()`: every host is a node, an offline one archived; the collected counts only of
    /// online hosts; a context id shared by two hosts is one unique context; a connected sender sends, whether its
    /// host is online or not (R55 M1).
    #[test]
    fn counts_the_hosts_as_c() {
        use crate::chart::Algorithm;
        use crate::metadata_stats::{Counts, MetadataStats};
        let storage = Arc::<StorageLayout>::default();
        let hosts = Hosts::with_storage(Host::with_storage("guid-l", true, info("l"), &storage), Arc::clone(&storage));
        let archived = hosts.add_archived("guid-x", info("x"), |_| {});
        for host in [hosts.localhost(), &archived] {
            let chart = collected_chart(host, DbMode::Ram);
            let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
            for t in [T0, T0 + 1] {
                store(&dim, t, 1.0);
                crate::contexts::collected_rrdset(&chart);
            }
            host.contexts().worker_cycle();
        }
        hosts.localhost().sender_flags_set(sender_flags::CONNECTED);
        archived.sender_flags_set(sender_flags::CONNECTED | sender_flags::ADDED);
        let one = Counts { collected: 1, available: 2 };
        assert_eq!(
            hosts.metadata_stats(),
            MetadataStats {
                nodes_total: 2,
                nodes_receiving: 0,
                nodes_sending: 2,
                nodes_archived: 1,
                metrics: one,
                instances: one,
                contexts: one,
                contexts_unique: 1,
            }
        );
    }

    const T0: i64 = 1_790_180_000;

    /// A dbengine dimension keeps a registry entry and a collection on every tier (N8, D68.6.1): the registry's
    /// update every is each tier's, stores go to tier 0, and the retention is the registry's. A new update every
    /// closes the page; a reset flushes it; finalize ends every tier's collection, and a re-add starts it again.
    #[test]
    fn dbengine_dims_collect_on_every_tier() {
        use crate::chart::Algorithm;
        let (_dirs, storage) = engine(3);
        let e = storage.dbengine().unwrap();
        let dbengine = HostInfo {
            db_mode: DbMode::Dbengine,
            ..info("d")
        };
        let host = Host::with_storage("guid-d", false, dbengine, &storage);
        let chart = collected_chart(&host, DbMode::Dbengine);
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        let ues = || -> Vec<u32> {
            (0..3)
                .map(|t| {
                    e.mrg
                        .get_and_acquire(dim.uuid(), t)
                        .unwrap()
                        .update_every_s()
                })
                .collect()
        };
        let collectors =
            || -> Vec<usize> { e.tiers.iter().map(|td| td.collectors_running()).collect() };
        assert_eq!((ues(), collectors()), (vec![1, 60, 3600], vec![1, 1, 1]));
        assert!(dim.ring().is_none());

        for i in 0..10 {
            store(&dim, T0 + i, i as f64);
        }
        assert_eq!((dim.first_entry_s(), dim.last_entry_s()), (T0, T0 + 9));
        assert_eq!(chart.tier0_retention(), (T0, T0 + 9));
        assert_eq!(
            (dim.tier_retention(1), dim.tier_retention(2)),
            ((0, 0), (0, 0))
        );
        assert_eq!(e.main.stats().hot_entries, 1);

        chart.set_update_every(2);
        assert_eq!(
            (e.main.stats().hot_entries, e.main.stats().dirty_entries),
            (0, 1)
        );
        assert_eq!(ues(), [2, 120, 7200]);
        store(&dim, T0 + 11, 11.0);
        assert_eq!(dim.last_entry_s(), T0 + 11);
        dim.store_flush();
        assert_eq!(e.main.stats().hot_entries, 0);
        assert_eq!(
            e.mrg
                .get_and_acquire(dim.uuid(), 0)
                .unwrap()
                .latest_clean_time_s(),
            T0 + 11
        );

        assert!(dim.finalize_collection(), "tier 0 has data");
        assert_eq!(collectors(), [0, 0, 0]);
        store(&dim, T0 + 13, 13.0);
        assert_eq!(dim.last_entry_s(), T0 + 11, "no collection after finalize");
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        assert_eq!(collectors(), [1, 1, 1]);

        let (fresh, _) = chart.dim_add("e", None, 1, 1, Algorithm::Absolute);
        assert!(!fresh.finalize_collection(), "no tier has data");
    }

    /// A ram host on the engine: the ring is tier 0, the tiers above hold registry entries; finalize counts the ring
    /// as retained, as C.
    #[test]
    fn ram_dims_on_the_engine_keep_their_ring_at_tier_0() {
        use crate::chart::Algorithm;
        let (_dirs, storage) = engine(3);
        let e = storage.dbengine().unwrap();
        let host = Host::with_storage("guid-a", false, info("a"), &storage);
        let chart = collected_chart(&host, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        let ring = dim.ring().expect("tier 0 is a ring");
        assert!(e.mrg.get_and_acquire(dim.uuid(), 0).is_none());
        assert!(e.mrg.get_and_acquire(dim.uuid(), 2).is_some());
        assert_eq!(e.tiers[0].collectors_running(), 0);
        store(&dim, T0, 1.0);
        assert_eq!(
            (dim.first_entry_s(), dim.last_entry_s()),
            (ring.oldest_time_s(), T0)
        );
        assert!(dim.finalize_collection());

        let plain = Host::new("guid-p", false, info("p"));
        let chart = collected_chart(&plain, DbMode::Ram);
        assert!(
            chart
                .dim_add("d", None, 1, 1, Algorithm::Absolute)
                .0
                .finalize_collection()
        );
        let orphan = collected_chart(&Host::new("guid-o", false, info("o")), DbMode::Dbengine);
        assert!(
            orphan
                .dim_add("d", None, 1, 1, Algorithm::Absolute)
                .0
                .finalize_collection()
        );
    }

    /// `rrdhost_finalize_collection()`: C's record, with the host's field, and every collection ended.
    #[test]
    fn a_host_finalizes_its_collection() {
        use crate::chart::Algorithm;
        let (_dirs, storage) = engine(2);
        let dbengine = HostInfo {
            db_mode: DbMode::Dbengine,
            ..info("h")
        };
        let host = Host::with_storage("guid-h", false, dbengine, &storage);
        let chart = collected_chart(&host, DbMode::Dbengine);
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        let ((), records) = netdata_agent_log::capture(|| host.finalize_collection());
        let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
        assert_eq!(messages, ["RRD: 'host:h' stopping data collection..."]);
        let e = storage.dbengine().unwrap();
        assert!(e.tiers.iter().all(|td| td.collectors_running() == 0));
    }

    /// The tiers above 0 aggregate tier 0's points into windows of `update every × grouping`: a completed window is
    /// parked and written at its chart's flush modulo (1 for the first chart), and finalize writes the parked one
    /// while the window being filled is lost (D72).
    #[test]
    fn tiers_aggregate_collected_windows() {
        use crate::chart::Algorithm;
        // multiples of 15
        const B: i64 = 1_790_179_995;
        let (_dirs, storage) = engine(3);
        let storage = Arc::new(
            Arc::try_unwrap(storage)
                .unwrap()
                .with_profile(vec![1, 5, 3], 1),
        );
        let e = Arc::clone(storage.dbengine().unwrap());
        let dbengine = HostInfo {
            db_mode: DbMode::Dbengine,
            ..info("t")
        };
        let host = Host::with_storage("guid-t", false, dbengine, &storage);
        let chart = collected_chart(&host, DbMode::Dbengine);
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        for t in B..=B + 41 {
            store(&dim, t, (t - B) as f64);
        }
        assert!(dim.finalize_collection());
        // the first window starts on a boundary and holds 6 points; B + 40's window was parked by B + 41
        let mut tier1 = vec![(B + 5, 15.0, 6)];
        tier1.extend((2..=8).map(|k| (B + 5 * k, (25 * k - 10) as f64, 5)));
        assert_eq!(tier_records(&e, &dim, 1), tier1);
        // the window being filled (from B + 31) is lost at finalize
        assert_eq!(
            tier_records(&e, &dim, 2),
            [(B + 15, 120.0, 16), (B + 30, 345.0, 15)]
        );
    }

    /// `backfill_tier_from_smaller_tiers()` after a restart (mode `new`): each tier takes the points after its newest
    /// record from the tier below first, then from tier 0, restoring the window the restart lost; the point just
    /// stored is read back and then stored again, as C does (D72.9).
    #[test]
    fn a_restarted_dimension_backfills_its_tiers() {
        const B: i64 = 1_790_179_995;
        let (_dirs, dim, e) = backfill_dim(crate::storage::Backfill::New, DbMode::Dbengine);
        for t in B..=B + 20 {
            store(&dim, t, (t - B) as f64);
        }
        dim.restarted();
        store(&dim, B + 40, 40.0);
        store(&dim, B + 41, 41.0);
        assert!(dim.finalize_collection());
        assert_eq!(
            tier_records(&e, &dim, 1),
            [
                (B + 5, 15.0, 6),
                (B + 10, 40.0, 5),
                (B + 15, 65.0, 5),
                (B + 20, 90.0, 5),
                (B + 40, 80.0, 2)
            ]
        );
        assert_eq!(
            tier_records(&e, &dim, 2),
            [(B + 15, 120.0, 16), (B + 30, 90.0, 5)]
        );
    }

    /// With the backfill off the restart's lost window stays lost; `new` does not backfill an empty tier; `full`
    /// does, reading back the first point just stored.
    #[test]
    fn backfill_modes() {
        use crate::storage::Backfill;
        const B: i64 = 1_790_179_995;
        let (_dirs, dim, e) = backfill_dim(Backfill::None, DbMode::Dbengine);
        for t in B..=B + 20 {
            store(&dim, t, (t - B) as f64);
        }
        dim.restarted();
        store(&dim, B + 40, 40.0);
        store(&dim, B + 41, 41.0);
        dim.finalize_collection();
        assert_eq!(
            tier_records(&e, &dim, 1),
            [(B + 5, 15.0, 6), (B + 10, 40.0, 5), (B + 15, 65.0, 5)]
        );
        // a ram host reads tier 0 from its ring, whose retention starts one interval before its first point
        // (rrddim_query_oldest_time_s()): the backfill's first point ends there, so the first window ends at B
        for (mode, db, want) in [
            (Backfill::New, DbMode::Dbengine, vec![(B + 5, 15.0, 6)]),
            (Backfill::Full, DbMode::Dbengine, vec![(B + 5, 15.0, 7)]),
            (
                Backfill::Full,
                DbMode::Ram,
                vec![(B, 0.0, 2), (B + 5, 15.0, 5)],
            ),
        ] {
            let (_dirs, dim, e) = backfill_dim(mode, db);
            for t in B..=B + 6 {
                store(&dim, t, (t - B) as f64);
            }
            store(&dim, B + 7, 7.0);
            assert_eq!(tier_records(&e, &dim, 1), want, "{mode:?} {db:?}");
        }
    }

    /// `send_to_child()`: before the move to running and after the removal there is no buffer, so 0 and nothing
    /// queued; with it, the length, POLLOUT only when the buffer was empty, nothing for no bytes, and an add of exactly
    /// the free space refused with -1 and BUFFER_OVERFLOW, the buffer as it was (D166).
    #[test]
    fn a_send_to_a_child_is_cs() {
        use crate::stream_buffer::INITIAL_MAX_SIZE;
        let slot = ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {}));
        assert_eq!(slot.send_to_child(b"early\n", Traffic::Metadata), 0);
        let ops = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ops);
        slot.open_buffer(Box::new(move |op| seen.lock().unwrap().push(op)));
        let queued = |slot: &ReceiverSlot| slot.buffer().as_ref().unwrap().stats().bytes_outstanding;
        assert_eq!(queued(&slot), 0, "the early line was not queued");
        let sends = [b"a\n" as &[u8], b"b\n", b""].map(|line| slot.send_to_child(line, Traffic::Functions));
        assert_eq!((sends, queued(&slot)), ([2, 2, 0], 4));
        assert_eq!(*ops.lock().unwrap(), [receiver_op::POLLOUT]);
        let free = INITIAL_MAX_SIZE - 4;
        assert_eq!(slot.send_to_child(&vec![b'x'; free], Traffic::Functions), -1);
        assert_eq!(queued(&slot), 4);
        assert_eq!(*ops.lock().unwrap(), [receiver_op::POLLOUT, receiver_op::BUFFER_OVERFLOW]);
        slot.buffer().as_mut().unwrap().del(4, 1);
        assert_eq!(slot.send_to_child(b"c\n", Traffic::Functions), 2);
        assert_eq!(ops.lock().unwrap().last(), Some(&receiver_op::POLLOUT), "drained, so empty again");
        slot.close_buffer();
        assert_eq!(slot.send_to_child(b"d\n", Traffic::Functions), 0);
    }

    /// A receiver's buffer autoscales (`send_to_child()` adds with autoscale, `stream-receiver.c:401`): data more than
    /// its free space, but less than the doubled maximum's, are taken and the maximum doubles (R70).
    #[test]
    fn a_send_larger_than_the_free_space_doubles_the_maximum() {
        use crate::stream_buffer::INITIAL_MAX_SIZE;
        let slot = ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {}));
        slot.open_buffer(Box::new(|_| {}));
        let fill = INITIAL_MAX_SIZE - 10;
        assert_eq!(slot.send_to_child(&vec![b'x'; fill], Traffic::Functions), fill as isize);
        assert_eq!(slot.send_to_child(&[b'y'; 11], Traffic::Functions), 11);
        assert_eq!(slot.buffer().as_ref().unwrap().stats().bytes_max_size, 2 * INITIAL_MAX_SIZE);
    }

    /// `rrdhost_clear_receiver()`: the receiver's end tells the host's sender and resets its parents, both with the
    /// receiver's reason and after the receiver lock is released, and turns the host's health off; a slot not attached
    /// does nothing.
    #[test]
    fn a_receivers_end_stops_the_hosts_sender() {
        let mut i = info("child");
        i.health_enabled = true;
        let host = Host::new("5a1e0000-0000-4000-8000-0000000000ca", false, i);
        let r = Arc::new(crate::testing::Recorder::default());
        host.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
        let slot = || Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        let (attached, other) = (slot(), slot());
        assert_eq!(host.set_receiver(Arc::clone(&attached)), Attach::Attached);
        host.clear_receiver(&other, -5);
        assert!(r.calls.lock().unwrap().is_empty());
        assert!(host.info().health_enabled);
        host.clear_receiver(&attached, -19);
        assert!(host.receiver().is_none());
        assert!(!host.info().health_enabled);
        assert_eq!(*r.calls.lock().unwrap(), vec![("receiver_left", -19), ("parents_reset", -19)]);
    }

    /// The receiver epochs (`stream-receiver.c:1400`, `:1472`): an attach retires what the host registered before it
    /// (`object_state_activate_if_not_activated()`), the attached receiver's detach retires what its child registered
    /// (`object_state_deactivate()`), and the detach of a receiver that is not the attached one changes nothing
    /// (R61-12); a retired method stays registered.
    #[test]
    fn a_receivers_attach_and_detach_retire_the_hosts_functions() {
        let host = Host::new("5a1e0000-0000-4000-8000-0000000000cb", false, info("child"));
        let register = |name: &'static [u8]| {
            let desc = MethodDesc {
                name,
                help: b"h",
                tags: b"",
                timeout_s: 10,
                priority: 0,
                version: 0,
                access: 0,
                sync: false,
                source: nrpc::Source::Stream,
                handler: inert(),
            };
            host.register_function(&desc).unwrap();
        };
        let available = |name: &[u8]| host.functions().available(name);
        let slot = || Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        register(b"before");
        assert!(available(b"before"));
        let attached = slot();
        assert_eq!(host.set_receiver(Arc::clone(&attached)), Attach::Attached);
        assert!(!available(b"before"), "the attach starts a new epoch");
        register(b"child");
        assert!(available(b"child"));
        host.clear_receiver(&slot(), 0);
        assert!(available(b"child"), "a receiver that is not the attached one");
        host.clear_receiver(&attached, 0);
        assert!(!available(b"child"), "the attached receiver's detach");
        assert!(host.functions().get(b"child").is_some() && host.functions().get(b"before").is_some());
    }

    /// A sender whose parents reset waits until released, as `Sender::parents_reset` waits for the parents lock an
    /// attempt holds across its connect.
    #[derive(Debug, Default)]
    struct SlowReset {
        entered: AtomicBool,
        release: AtomicBool,
    }

    impl Upstream for SlowReset {
        fn start(&self) {}
        fn disabled_capabilities(&self) -> u32 {
            0
        }
        fn capabilities(&self) -> u32 {
            0
        }
        fn commit(&self, _: &[u8], _: crate::upstream::Traffic) {}
        fn resync_iterations(&self) -> u16 {
            3
        }
        fn flush_ut(&self) -> u64 {
            0
        }
        fn commit_since(&self, _: &[u8], _: crate::upstream::Traffic, _: u64) -> bool {
            false
        }
        fn receiver_left(&self, _: i32) {}
        fn parents_reset(&self, _: i32) {
            self.entered.store(true, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        fn free(&self) {}
        fn reinit(&self, _: &StreamSend) {}
    }

    /// `rrdhost_clear_receiver()` empties the slot last (R55 M4): while the leaving receiver's parents reset waits,
    /// the host is offline and no longer its receiver's, yet still served, so a replacement waits as C's
    /// (`stream-receiver.c:1485`, `:1507`); the receivers' count drops with the slot.
    #[test]
    fn a_receivers_slot_empties_after_its_detach() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let host = hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).expect("created");
        let slow = Arc::new(SlowReset::default());
        host.set_upstream(Arc::clone(&slow) as Arc<dyn Upstream>);
        let slot = || Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        let old = slot();
        assert_eq!(host.set_receiver(Arc::clone(&old)), Attach::Attached);
        assert_eq!(hosts.receivers_connected(), 1);
        std::thread::scope(|s| {
            s.spawn(|| host.clear_receiver(&old, -10));
            while !slow.entered.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            let during = (
                host.receiver().is_some_and(|r| Arc::ptr_eq(&r, &old)),
                host.is_receiver(&Arc::downgrade(&old)),
                host.is_online(),
                hosts.receivers_connected(),
                host.set_receiver(slot()),
            );
            // a second detach of the same receiver does nothing
            host.clear_receiver(&old, -10);
            slow.release.store(true, Ordering::SeqCst);
            assert_eq!(during, (true, false, false, 1, Attach::AlreadyServed));
        });
        assert!(host.receiver().is_none());
        assert_eq!(hosts.receivers_connected(), 0);
        assert_eq!(host.set_receiver(slot()), Attach::Attached);
    }

    /// A vnode is created with localhost's settings under its own name and system info, the virtual OS and no cache
    /// directory.
    #[test]
    fn a_vnode_takes_localhosts_settings() {
        let localhost = HostInfo { cache_dir: Some("/var/cache/netdata".into()), ..info("l") };
        let system_info = SystemInfo { hops: 1, kernel_version: Some("6.1".into()), ..SystemInfo::default() };
        assert_eq!(
            HostInfo::for_vnode(&localhost, "v", system_info.clone()),
            HostInfo {
                hostname: "v".into(),
                registry_hostname: "v".into(),
                os: VIRTUAL_HOST_OS.into(),
                system_info,
                cache_dir: None,
                ..localhost
            }
        );
    }

    /// `pluginsd_host_claim_as_local_vnode()`: no receiver, a receiver that leaves when told, one that does not (C's
    /// record after 2 s).
    #[test]
    fn a_claim_evicts_a_receiver_as_c() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let host = hosts.find_or_create("guid-v", DbMode::Ram, || info("v"), |_| {}).expect("created");
        assert_eq!(host.claim_as_local_vnode(), Claim::Free);
        let leaving = Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(host.set_receiver(Arc::clone(&leaving)), Attach::Attached);
        let claim = std::thread::scope(|s| {
            s.spawn(|| {
                while !leaving.stop_requested.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                host.clear_receiver(&leaving, 0);
            });
            host.claim_as_local_vnode()
        });
        assert_eq!(claim, Claim::Evicted);
        let stuck = Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(host.set_receiver(Arc::clone(&stuck)), Attach::Attached);
        let (claim, records) = netdata_agent_log::capture(|| host.claim_as_local_vnode());
        assert_eq!(claim, Claim::Stuck);
        let records: Vec<_> = records.into_iter().map(|r| r.message.unwrap_or_default()).collect();
        assert_eq!(records.len(), 1, "{records:?}");
        assert!(records[0].ends_with("streaming thread takes too long to stop, giving up..."), "{records:?}");
    }

    /// C's local, online, ingest type and hops by host state (`rrdhost.h:455-470`, `rrdhost-status.c:103-105,
    /// 210-232`): localhost; a child attached, then gone; a vnode claimed, collected, then let go; a receiver refused
    /// while the host is a vnode.
    #[test]
    fn host_states_read_as_c() {
        use crate::status::IngestType;
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let state = |h: &Host| (h.is_local(), h.is_online(), h.collector_online(), h.status_basic(0).ingest_type, h.ingestion_hops());
        assert_eq!(state(hosts.localhost()), (true, true, true, IngestType::Localhost, 0));
        let slot = || Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        let child = hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).expect("created");
        let attached = slot();
        assert_eq!(child.set_receiver(Arc::clone(&attached)), Attach::Attached);
        assert_eq!(state(&child), (false, true, true, IngestType::Child, 0));
        child.clear_receiver(&attached, 0);
        assert_eq!(state(&child), (false, false, false, IngestType::Archived, 0));
        let vnode = hosts.find_or_create("guid-v", DbMode::Ram, || info("v"), |_| {}).expect("created");
        assert_eq!(state(&vnode), (false, false, false, IngestType::Archived, 0));
        vnode.set_virtual();
        assert_eq!(state(&vnode), (true, true, false, IngestType::Virtual, 1));
        assert_eq!(vnode.set_receiver(slot()), Attach::VnodeIsLocal);
        assert!(vnode.receiver().is_none());
        vnode.set_collector_online();
        assert_eq!(state(&vnode), (true, true, true, IngestType::Virtual, 1));
        vnode.virtual_offline();
        assert_eq!(state(&vnode), (false, false, false, IngestType::Archived, 0));
        assert_eq!(vnode.set_receiver(slot()), Attach::Attached);
    }

    /// `rrdhost_clear_receiver()` cleans the receiver's parser up last, under the receiver lock (R58-2,
    /// `stream-receiver.c:1510-1513`): a replacement attaches only after it, the slot already empty; a receiver that is
    /// not the host's has its cleanup run too.
    #[test]
    fn a_receivers_cleanup_runs_before_a_replacement_attaches() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let host = hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).expect("created");
        let slot = || Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        let old = slot();
        assert_eq!(host.set_receiver(Arc::clone(&old)), Attach::Attached);
        let (entered, release, attached) = (AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false));
        std::thread::scope(|s| {
            s.spawn(|| {
                host.clear_receiver_then(&old, -10, || {
                    entered.store(true, Ordering::SeqCst);
                    while !release.load(Ordering::SeqCst) {
                        std::thread::yield_now();
                    }
                })
            });
            while !entered.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            let replacement = s.spawn(|| {
                let attach = host.set_receiver(slot());
                attached.store(true, Ordering::SeqCst);
                attach
            });
            std::thread::sleep(std::time::Duration::from_millis(100));
            let during = attached.load(Ordering::SeqCst);
            release.store(true, Ordering::SeqCst);
            assert!(!during, "a replacement attached during the cleanup");
            assert_eq!(replacement.join().unwrap(), Attach::Attached);
        });
        let ran = AtomicBool::new(false);
        host.clear_receiver_then(&old, -10, || ran.store(true, Ordering::SeqCst));
        assert!(ran.load(Ordering::SeqCst), "the cleanup of a receiver that is not the host's");
    }

    /// A receiver leaves while a walk that frees hosts holds the index (R55 M3): the count it updates is not under
    /// the index's lock, as C's atomic `streaming_connected_receivers`.
    #[test]
    fn a_receiver_leaves_while_a_walk_holds_the_hosts() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let host = hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).expect("created");
        let slot = Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        hosts.update_is_parent_label();
        let walk = hosts.write();
        let finished = std::thread::scope(|s| {
            let leave = s.spawn(|| {
                host.clear_receiver(&slot, 0);
                hosts.update_is_parent_label();
                hosts.receivers_connected()
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !leave.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let finished = leave.is_finished();
            drop(walk);
            assert_eq!(leave.join().unwrap(), 0);
            finished
        });
        assert!(finished, "the receiver's leave waited for the hosts' write lock");
    }

    /// The function registry at C's three sites: none for a host created archived (`rrdhost.c:1020`'s destroy), one
    /// again when it connects (`:803`'s init), none once it is archived
    /// (`rrdhost_cleanup_data_collection_and_health()`).
    #[test]
    fn the_function_registry_follows_the_archive() {
        let storage = Arc::<StorageLayout>::default();
        let hosts = Hosts::with_storage(Host::with_storage("guid-l", true, info("l"), &storage), Arc::clone(&storage));
        assert!(hosts.localhost().functions().exists());
        let host = hosts.add_archived("guid-x", info("x"), |_| {});
        assert!(!host.functions().exists());
        host.update(&info("x"), 1, 3600, false, 0, 0);
        assert!(host.functions().exists());
        host.cleanup_data_collection();
        assert!(!host.functions().exists());
        host.update(&info("x"), 1, 3600, false, 0, 0);
        assert!(host.functions().exists());
    }

    /// `nrpc_owner_str()`: the registry's records name it by its host's handle, the one its creation record printed;
    /// the refusals name the host.
    #[test]
    fn a_reregistration_names_the_registry_by_its_handle() {
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000ce", false, info("child")));
        let ((), created) = netdata_agent_log::capture(|| host.log_registry_created("child"));
        let created = created[0].message.clone().unwrap();
        let key = created.strip_prefix("NRPC: function registry ").and_then(|r| r.split(' ').next()).unwrap();
        assert_eq!(created, format!("NRPC: function registry {key} created for host 'child'"));
        let desc = |timeout_s| MethodDesc {
            name: b"x",
            help: b"h",
            tags: b"",
            timeout_s,
            priority: 0,
            version: 0,
            access: 0,
            sync: false,
            source: nrpc::Source::Stream,
            handler: inert(),
        };
        host.register_function(&desc(10)).unwrap();
        let (_, records) = netdata_agent_log::capture(|| host.register_function(&desc(20)).unwrap());
        let texts: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
        assert_eq!(texts, [format!("NRPC: method 'x' of host {key} re-registered with changes")]);
        assert!(key.starts_with("0x") && key.len() == 18);
    }

    /// `stream_sender_structures_free()` at the host's cleanup: the sender freed once; the host no longer streams and
    /// its settings go with it; the revival's update brings the settings of that time back, and `init_upstream` sets
    /// the same sender up again, once.
    #[test]
    fn a_cleaned_up_host_frees_its_sender_and_a_revival_sets_it_up_again() {
        let mut i = info("child");
        i.stream_send = StreamSend::new(true, "grandparent:19999", "key", "*");
        let host = Host::new("5a1e0000-0000-4000-8000-0000000000cb", false, i.clone());
        let r = Arc::new(crate::testing::Recorder::default());
        host.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
        host.sender_flags_set(sender_flags::READY_4_METRICS);
        assert!(host.upstream().is_some());
        host.cleanup_data_collection();
        host.cleanup_data_collection();
        assert_eq!(*r.calls.lock().unwrap(), vec![("free", 0)]);
        assert!(host.upstream().is_none());
        assert!(!host.can_stream_metadata());
        assert!(host.info().stream_send.is_none());
        host.init_upstream(|_| unreachable!("no settings, no sender"));
        assert!(host.upstream().is_none());
        host.update(&i, 1, 3600, false, 0, 0);
        assert!(host.info().stream_send.is_some(), "the revival's settings");
        host.init_upstream(|_| unreachable!("set up again, not created"));
        assert!(host.upstream().is_some());
        host.init_upstream(|_| unreachable!("elected already"));
        assert_eq!(*r.calls.lock().unwrap(), vec![("free", 0), ("reinit", 0)]);
        // a host without a sender gets one created
        let other = Host::new("5a1e0000-0000-4000-8000-0000000000cc", false, i);
        let created = Arc::new(crate::testing::Recorder::default());
        other.init_upstream(|_| Arc::clone(&created) as Arc<dyn Upstream>);
        assert!(other.upstream().is_some() && created.calls.lock().unwrap().is_empty());
        // one setup is elected: a second caller, at once or while a free is under way, does nothing
        other.init_upstream(|_| unreachable!("elected already"));
        other.sender_flags_clear(sender_flags::ENABLED);
        other.init_upstream(|_| unreachable!("a free has not ended"));
        assert!(other.upstream().is_none());
    }

    /// `rrdhost_status_ingest()`: a host not online is archived until a receiver attached to it, offline after.
    #[test]
    fn hosts_are_archived_until_a_child_connects() {
        use crate::status::IngestStatus;
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let host = hosts.add_archived(
            "5a1e0000-0000-4000-8000-0000000000c9",
            info("child"),
            |_| {},
        );
        host.clear_pending_context_load();
        assert_eq!(host.status_basic(1).ingest_status, IngestStatus::Archived);
        let slot = Arc::new(ReceiverSlot::new(
            1,
            Default::default(),
            ReceiverLink::default(),
            Box::new(|| {}),
        ));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        host.clear_receiver(&slot, 0);
        assert_eq!(
            (
                host.receiver_connections(),
                host.status_basic(1).ingest_status
            ),
            (1, IngestStatus::Offline)
        );
    }

    /// `rrdhost_create()` of a host that is not archived loads its contexts, once, on the creating thread; finding it
    /// again does not.
    #[test]
    fn created_hosts_load_their_contexts() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let loaded = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&loaded);
        hosts.set_context_loader(move |host| seen.lock().unwrap().push(host.hostname()));
        for _ in 0..2 {
            hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).expect("created");
        }
        assert_eq!(*loaded.lock().unwrap(), ["c"]);
    }

    /// `rrdhost_update()` of a host that is not archived: the descriptive fields follow the child, while its update
    /// every, memory mode, history, replication and sender settings stay (C only warns about the first three, history
    /// as an `else if` after the memory mode, and re-applies the others only to an archived host).
    #[test]
    fn a_reconnecting_host_keeps_what_c_does_not_reapply() {
        let host = Host::new("guid-k", false, info("a"));
        let before = host.info();
        let wanted = HostInfo {
            hostname: "b".into(),
            registry_hostname: "b".into(),
            os: "other-os".into(),
            timezone: "Europe/Athens".into(),
            abbrev_timezone: "EEST".into(),
            utc_offset: 10800,
            program_name: "other".into(),
            program_version: "v9".into(),
            update_every: 2,
            db_mode: DbMode::Alloc,
            history_entries: 8192,
            replication_enabled: false,
            replication_period: 600,
            replication_step: 60,
            stream_send: StreamSend::new(true, "p:1", "key", "*"),
            ..before.clone()
        };
        let ((), records) = netdata_agent_log::capture(|| host.update(&wanted, 2, 8192, false, 600, 60));
        let warnings: Vec<_> = texts(&records)
            .into_iter()
            .filter(|(_, t)| t.contains("Restart netdata here"))
            .map(|(_, t)| t.split(" has ").nth(1).unwrap_or_default().split(',').next().unwrap_or_default().to_string())
            .collect();
        assert_eq!(warnings, ["an update frequency of 1 seconds", "memory mode 'ram'"]);
        let kept = HostInfo {
            update_every: before.update_every,
            db_mode: before.db_mode,
            history_entries: before.history_entries,
            replication_enabled: before.replication_enabled,
            replication_period: before.replication_period,
            replication_step: before.replication_step,
            stream_send: before.stream_send.clone(),
            ..wanted.clone()
        };
        assert_eq!(host.info(), kept);
    }

    /// `stream_receiver_replication_reset()` zeroes the child's replication replies (`counter_in`), the requests sent
    /// to it (`counter_out`) and the pending backfills, on attach and on detach.
    #[test]
    fn a_reset_zeroes_the_replication_traffic_counters() {
        let host = Host::new("guid-z", false, info("z"));
        let counters = |h: &Host| (h.replication_replies(), h.replication_requests(), h.backfill_pending());
        let bump = |h: &Host| {
            h.count_replication_reply();
            (0..2).for_each(|_| h.count_replication_request());
            (0..3).for_each(|_| h.backfill_requested());
        };
        let slot = Arc::new(ReceiverSlot::new(0, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        bump(&host);
        assert_eq!(counters(&host), (1, 2, 3));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        assert_eq!(counters(&host), (0, 0, 0), "attach");
        bump(&host);
        host.clear_receiver(&slot, 0);
        assert_eq!(counters(&host), (0, 0, 0), "detach");
    }

    /// `foreach_entry_in_connection_string()`: destinations split at commas and at C's white space, empty ones
    /// skipped, kept in order and duplicates kept; `:SSL` marks TLS; an empty destination makes no sender.
    #[test]
    fn destinations_split_as_c() {
        let parents = |destination: &str| {
            StreamSend::new(true, destination, "key", "*")
                .map(|s| s.parents().map(|(d, tls)| (d.to_string(), tls)).collect::<Vec<_>>())
        };
        let want = |list: &[(&str, bool)]| Some(list.iter().map(|&(d, t)| (d.to_string(), t)).collect::<Vec<_>>());
        assert_eq!(
            parents(" a:1, b:2\tc:3 ,,d \n\x0b\x0ce:5:SSL,a:1\r"),
            want(&[("a:1", false), ("b:2", false), ("c:3", false), ("d", false), ("e:5", true), ("a:1", false)])
        );
        assert_eq!(parents(" ,\t,\n"), want(&[]));
        assert_eq!(parents("a\u{a0}b;c|d"), want(&[("a\u{a0}b;c|d", false)]));
        assert!(StreamSend::new(true, "", "key", "*").is_none());
    }

    /// `stream_receiver_signal_to_stop_and_wait()`: the connection shut down once, a receiver its stream thread lets go
    /// waited for quietly, one still attached after 2000 waits of 1 ms given up on with C's record.
    #[test]
    fn a_receiver_still_attached_after_two_seconds_is_given_up_on() {
        let host = Host::new("guid-w", false, info("w"));
        let slot = |shutdowns: &Arc<AtomicU64>| {
            let shutdowns = Arc::clone(shutdowns);
            Arc::new(ReceiverSlot::new(
                1,
                ("127.0.0.1".into(), "4321".into()),
                ReceiverLink::default(),
                Box::new(move || {
                    shutdowns.fetch_add(1, Ordering::Relaxed);
                }),
            ))
        };
        let shutdowns = Arc::new(AtomicU64::new(0));
        let stuck = slot(&shutdowns);
        assert_eq!(host.set_receiver(Arc::clone(&stuck)), Attach::Attached);
        let started = std::time::Instant::now();
        let (stopped, records) = netdata_agent_log::capture(|| host.stop_receiver_and_wait(&stuck));
        assert!(started.elapsed() >= std::time::Duration::from_secs(2));
        assert_eq!(
            (stopped, texts(&records)),
            (
                false,
                vec![(
                    Priority::Err,
                    "STREAM RCV[x] 'w' [from [127.0.0.1]:4321]: streaming thread takes too long to stop, giving up..."
                        .to_string()
                )]
            )
        );
        stuck.stop();
        assert_eq!(shutdowns.load(Ordering::Relaxed), 1);

        host.clear_receiver(&stuck, 0);
        let released = slot(&shutdowns);
        assert_eq!(host.set_receiver(Arc::clone(&released)), Attach::Attached);
        let (stopped, records) = std::thread::scope(|s| {
            s.spawn(|| {
                while !released.stop_requested.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                host.clear_receiver(&released, 0);
            });
            netdata_agent_log::capture(|| host.stop_receiver_and_wait(&released))
        });
        assert_eq!((stopped, texts(&records)), (true, vec![]));
        assert_eq!(shutdowns.load(Ordering::Relaxed), 2);
    }

    /// `rrdhost_clear_receiver()`'s bookkeeping: the host orphaned, its replication counters zeroed, last connected 0
    /// and last disconnected now, and its contexts' retention recomputed for the disconnect on the next cycle.
    #[test]
    fn a_detach_orphans_the_host_and_resets_its_bookkeeping() {
        use crate::chart::Algorithm;
        let host = Host::new("guid-d", false, info("d"));
        let slot = Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        let chart = collected_chart(&host, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        store(&dim, T0, 1.0);
        store(&dim, T0 + 1, 1.0);
        crate::contexts::collected_rrdset(&chart);
        host.contexts().worker_cycle();
        let ctx = host.contexts().get("t.c").expect("the chart's context");
        assert!(ctx.flags.is_collected() && !host.is_orphan());
        host.count_replication_request();
        host.count_replication_reply();
        host.backfill_requested();
        let before = crate::clock::now_realtime_s();
        host.clear_receiver(&slot, 0);
        let after = crate::clock::now_realtime_s();
        assert!(host.is_orphan());
        assert_eq!((host.replication_requests(), host.replication_replies(), host.backfill_pending()), (0, 0, 0));
        assert_eq!(host.receiver_last_connected_s(), 0);
        assert!((before..=after).contains(&host.receiver_last_disconnected_s()));
        host.contexts().worker_cycle();
        assert!(ctx.flags.is_archived() && !ctx.flags.is_collected(), "{:#x}", ctx.flags.get());
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
        let times = |h: &Host| (h.receiver_last_connected_s() > 0, h.receiver_last_disconnected_s() > 0);
        assert_eq!(times(&host), (false, false));
        host.storage().next_health_iteration();
        host.storage().next_health_iteration();
        assert_eq!(host.set_receiver(Arc::clone(&first)), Attach::Attached);
        assert_eq!(times(&host), (true, false), "attached: connected, not disconnected");
        assert_eq!(host.health_last_iteration(), 2, "attached: stamped with the HEALTH pass");
        replicating(&chart);
        host.storage().next_health_iteration();
        host.clear_receiver(&first, 0);
        assert_eq!(times(&host), (false, true), "detached: disconnected, not connected");
        assert_eq!(host.health_last_iteration(), 3, "detached: stamped again");
        assert!(reset(&chart), "detach resets");
        replicating(&chart);
        assert_eq!(host.set_receiver(slot()), Attach::Attached);
        assert_eq!(times(&host), (true, false));
        assert!(reset(&chart), "attach resets");
    }

    /// `pulse_host_status()`'s transitions: a basic or receiver state takes the host's ephemerality and replaces the
    /// inbound state (a basic one the sender's too), a sender state only the sender's; the running latch holds a
    /// receiver that reached running through its charts' replication ripples, until it goes offline; the inbound
    /// state's age restarts only when that state changes; a deletion keeps its ephemerality bit, as C's.
    #[test]
    fn pulse_status_follows_cs_transitions() {
        use crate::pulse::host_status::*;
        let host = Host::new("guid-p", false, info("p"));
        // whether the age restarted since the last step
        let restarted = |host: &Host| {
            let changed = host.state_changed_s.swap(1, Ordering::Relaxed) != 1;
            (host.running_latched.load(Ordering::Relaxed), changed)
        };
        let steps = [
            (RCV_WAITING, RCV_WAITING | PERMANENT, (false, true)),
            (RCV_REPLICATING, RCV_REPLICATING | PERMANENT, (false, true)),
            (RCV_RUNNING, RCV_RUNNING | PERMANENT, (true, true)),
            (RCV_REPLICATING, RCV_RUNNING | PERMANENT, (true, false)),
            (
                SND_RUNNING,
                RCV_RUNNING | PERMANENT | SND_RUNNING,
                (true, false),
            ),
            (
                SND_OFFLINE,
                RCV_RUNNING | PERMANENT | SND_OFFLINE,
                (true, false),
            ),
            (
                RCV_OFFLINE,
                RCV_OFFLINE | PERMANENT | SND_OFFLINE,
                (false, true),
            ),
            (
                RCV_REPLICATING,
                RCV_REPLICATING | PERMANENT | SND_OFFLINE,
                (false, true),
            ),
            (ARCHIVED, ARCHIVED | PERMANENT, (false, true)),
        ];
        for (status, state, latch_and_age) in steps {
            host.pulse_status(status);
            assert_eq!(
                (host.pulse_state(), restarted(&host)),
                (state, latch_and_age),
                "after {status:#x}"
            );
        }
        host.set_ephemeral(true);
        host.pulse_status(RCV_RUNNING);
        assert_eq!(host.pulse_state(), RCV_RUNNING | EPHEMERAL);
        host.pulse_status(DELETED);
        assert_eq!(
            (host.pulse_state(), restarted(&host)),
            (DELETED | EPHEMERAL, (true, true))
        );
    }

    /// `pulse_host_detect_receiver_status()`: a host without data is loading, one with data and no receiver since it
    /// was created is archived; with a receiver it replicates while the counter says so and runs after, and it is
    /// offline once the receiver left; localhost is local.
    #[test]
    fn pulse_status_detects_the_receivers_state() {
        use crate::pulse::host_status::*;
        let host = Host::new("guid-p", false, info("p"));
        host.pulse_status(0);
        assert_eq!(host.pulse_state(), LOADING | PERMANENT);
        let chart = collected_chart(&host, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, crate::chart::Algorithm::Absolute);
        store(&dim, T0, 1.0);
        crate::contexts::collected_rrdset(&chart);
        host.contexts().worker_cycle();
        host.pulse_status(0);
        assert_eq!(host.pulse_state(), ARCHIVED | PERMANENT);
        let slot = Arc::new(ReceiverSlot::new(
            1,
            Default::default(),
            ReceiverLink::default(),
            Box::new(|| {}),
        ));
        assert_eq!(host.set_receiver(Arc::clone(&slot)), Attach::Attached);
        host.replicating_charts_plus_one();
        host.pulse_status(0);
        assert_eq!(host.pulse_state(), RCV_REPLICATING | PERMANENT);
        host.replicating_charts_minus_one();
        host.pulse_status(0);
        assert_eq!(host.pulse_state(), RCV_RUNNING | PERMANENT);
        host.clear_receiver(&slot, 0);
        host.pulse_status(0);
        assert_eq!(host.pulse_state(), RCV_OFFLINE | PERMANENT);
        let localhost = Host::new("guid-l", true, info("l"));
        let chart = collected_chart(&localhost, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, crate::chart::Algorithm::Absolute);
        store(&dim, T0, 1.0);
        crate::contexts::collected_rrdset(&chart);
        localhost.contexts().worker_cycle();
        localhost.pulse_status(0);
        assert_eq!(localhost.pulse_state(), LOCAL | PERMANENT);
    }

    /// `rrdhost_status_db()`: a host whose contexts are still loading is initializing, whatever its retention says.
    #[test]
    fn a_host_loading_its_contexts_is_initializing() {
        use crate::status::DbStatus;
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let host = hosts.add_archived("guid-a", info("a"), |_| {});
        let chart = collected_chart(&host, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, crate::chart::Algorithm::Absolute);
        store(&dim, T0, 1.0);
        crate::contexts::collected_rrdset(&chart);
        host.contexts().worker_cycle();
        let db_status = |host: &Host| host.status_basic(T0).db_status;
        assert_eq!(db_status(&host), DbStatus::Initializing);
        host.clear_pending_context_load();
        assert_eq!(db_status(&host), DbStatus::Queryable);
    }

    /// What health hears: a freed chart; at a host's cleanup the host first, then each of its charts; and a freed
    /// host after its cleanup.
    #[test]
    fn health_hears_of_freed_charts_and_hosts() {
        use crate::storage::HealthEvent;
        let storage = Arc::new(StorageLayout::default());
        let heard = Arc::new(Mutex::new(Vec::new()));
        storage.set_health_hook({
            let heard = Arc::clone(&heard);
            move |event| {
                lock(&heard).push(match event {
                    HealthEvent::ChartFreed(host, chart) => {
                        format!("chart {host} {} freed={}", chart.id(), chart.is_freed())
                    }
                    HealthEvent::HostCleanup(host) => format!("cleanup {host}"),
                    HealthEvent::HostFreed(host) => format!("freed {host}"),
                });
            }
        });
        let heard = || std::mem::take(&mut *lock(&heard));
        let hosts = Hosts::with_storage(Host::with_storage("guid-l", true, info("l"), &storage), Arc::clone(&storage));
        let child = hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).unwrap();
        let chart = |id| {
            let spec = crate::chart::ChartSpec { id, ..crate::testutil::chart_spec(DbMode::Ram) };
            child.charts().create(&spec).0
        };
        let (a, _b, _c) = (chart("a"), chart("b"), chart("c"));
        assert!(heard().is_empty());

        assert!(child.charts().free_if(&a, |_| true));
        assert_eq!(heard(), ["chart guid-c t.a freed=true"]);
        assert!(!child.charts().free_if(&a, |_| true), "freed already");
        assert!(heard().is_empty());

        child.cleanup_data_collection();
        assert_eq!(heard(), ["cleanup guid-c", "chart guid-c t.b freed=true", "chart guid-c t.c freed=true"]);

        assert!(hosts.free(&child).is_some());
        assert_eq!(heard(), ["cleanup guid-c", "freed guid-c"]);
    }

    /// `rrdhost_should_run_health()`: health enabled for the host, its collector online, no orphan, and its ingestion
    /// online, which a host without a stored metric is not.
    #[test]
    fn health_runs_for_an_enabled_host_that_is_collected() {
        let enabled = |name: &str| HostInfo { health_enabled: true, ..info(name) };
        let collect = |host: &Host| {
            let chart = collected_chart(host, DbMode::Ram);
            let (dim, _) = chart.dim_add("d", None, 1, 1, crate::chart::Algorithm::Absolute);
            store(&dim, T0, 1.0);
            crate::contexts::collected_rrdset(&chart);
            host.contexts().worker_cycle();
        };

        let localhost = Host::new("guid-l", true, enabled("l"));
        assert!(!localhost.should_run_health(T0), "an empty database is initializing");
        collect(&localhost);
        assert!(localhost.should_run_health(T0));

        let off = Host::new("guid-o", true, info("o"));
        collect(&off);
        assert!(!off.should_run_health(T0), "health is off for the host");

        // a vnode is local, so its ingestion is online whatever its two flags say: they are asked for themselves
        let vnode = Host::new("guid-v", false, enabled("v"));
        vnode.set_virtual();
        collect(&vnode);
        assert!(!vnode.should_run_health(T0), "its collector is not online");
        vnode.set_collector_online();
        assert!(vnode.should_run_health(T0));
        vnode.orphan.store(true, Ordering::Release);
        assert!(!vnode.should_run_health(T0), "an orphan");

        // a child whose collector is not online is not ingesting
        let child = Host::new("guid-c", false, enabled("c"));
        collect(&child);
        assert!(!child.should_run_health(T0));
    }

    /// `pulse_rrd_memory_size`: a ram dimension's ring counts from its creation until the dimension is dropped with its
    /// host.
    #[test]
    fn ram_rings_count_in_pulse() {
        let storage = Arc::new(StorageLayout::default());
        let memory = || storage.pulse().rrd_memory.read();
        let host = Host::with_storage("guid-m", false, info("m"), &storage);
        let chart = collected_chart(&host, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, crate::chart::Algorithm::Absolute);
        let ring = dim.ring().unwrap();
        let bytes = ring.entries() as i64 * 4;
        assert_eq!((ring.memsize() as i64, memory()), (bytes, bytes));
        chart.dim_add("e", None, 1, 1, crate::chart::Algorithm::Absolute);
        assert_eq!(memory(), 2 * bytes);
        chart.dim_add("e", None, 1, 1, crate::chart::Algorithm::Absolute);
        assert_eq!(
            memory(),
            2 * bytes,
            "an existing dimension is not counted again"
        );
        drop((dim, chart, host));
        assert_eq!(memory(), 0);
    }

    /// `labels_applied(_version)`: the pulse charts take a child's labels first, then again only when their version
    /// moves.
    #[test]
    fn pulse_charts_take_the_labels_when_they_change() {
        let host = Host::new("guid-p", false, info("p"));
        let version = host.labels_version();
        assert!(host.pulse_labels_refresh(version), "never taken");
        assert!(!host.pulse_labels_refresh(version));
        host.update_labels(|l| l.add(b"k", b"v", crate::labels::SRC_AUTO));
        assert!(host.pulse_labels_refresh(host.labels_version()));
        assert!(!host.pulse_labels_refresh(host.labels_version()));
    }

    /// `rrdhost_receiver_replicating_charts()`: the ingest counts each chart's replication once; a reset takes back
    /// the charts it finishes and, as C's, zeroes what is left with a warning.
    #[test]
    fn a_reset_takes_back_the_replicating_charts() {
        use crate::chart::flags;
        let host = Host::new("guid-r", false, info("r"));
        let chart = collected_chart(&host, DbMode::Ram);
        chart.update_meta(|m| {
            m.flags |= flags::RECEIVER_REPLICATION_IN_PROGRESS;
            m.flags &= !flags::RECEIVER_REPLICATION_FINISHED;
        });
        assert_eq!(host.replicating_charts_plus_one(), 1);
        let ((), records) = netdata_agent_log::capture(|| host.replication_reset());
        assert_eq!((host.replicating_charts(), texts(&records)), (0, vec![]));
        assert_eq!(host.replicating_charts_plus_one(), 1);
        let ((), records) = netdata_agent_log::capture(|| host.replication_reset());
        assert_eq!(
            (host.replicating_charts(), texts(&records)),
            (
                0,
                vec![(
                    Priority::Warning,
                    "STREAM REPLAY ERROR: receiver replication instances counter should be zero, but it is 1 - \
                     resetting it to zero"
                        .to_string()
                )]
            )
        );
        assert_eq!(host.replicating_charts_minus_one(), u32::MAX, "it wraps");
    }

    /// A longer machine GUID is cut to 37 characters at creation, so its next creation meets that host and fails, as
    /// C's NULL, with C's NOTICE (D126.7); the 37-character GUID itself finds it; the parent's own GUID plus a suffix
    /// is a child host.
    #[test]
    fn a_longer_guid_is_cut_at_creation_and_collides_as_c() {
        const G: &str = "5a1e0000-0000-4000-8000-0000000000c9";
        let hosts = Hosts::new(Host::new("5a1e0000-0000-4000-8000-0000000000aa", true, info("parent")));
        let long = format!("{G}xyz");
        let made = hosts.find_or_create(&long, DbMode::Ram, || info("c"), |_| panic!("new host")).expect("created");
        assert_eq!(made.machine_guid(), format!("{G}x"));
        let (again, records) = netdata_agent_log::capture(|| {
            hosts.find_or_create(&long, DbMode::Ram, || info("c"), |_| panic!("not found by the whole GUID"))
        });
        assert_eq!(again.err(), Some(NotCreated::IndexCollision));
        assert!(records.iter().any(|r| r.priority == Priority::Notice
            && r.message.as_deref()
                == Some(&*format!(
                    "Host 'c': cannot add host with machine guid '{G}x' to index. It already exists as host 'c' with \
                     machine guid '{G}x'."
                ))));
        assert_eq!(hosts.all().len(), 2);
        let found = hosts.find_or_create(&format!("{G}x"), DbMode::Ram, || panic!("exists"), |_| {}).expect("found");
        assert!(Arc::ptr_eq(&made, &found));
        let own = hosts
            .find_or_create("5a1e0000-0000-4000-8000-0000000000aaxyz", DbMode::Ram, || info("own"), |_| {})
            .expect("created");
        assert_eq!(own.machine_guid(), "5a1e0000-0000-4000-8000-0000000000aax");
        assert_eq!(hosts.all().len(), 3);
    }

    /// An archived host of another memory mode is not discarded while the metadata writer holds it: C's NULL (D95.7);
    /// once released, it is discarded and created again.
    #[test]
    fn a_discard_waits_for_the_metadata_writer() {
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let archived = hosts.add_archived("guid-a", info("a"), |_| {});
        archived.clear_pending_context_load();
        let other = if archived.info().db_mode == DbMode::Alloc { DbMode::Ram } else { DbMode::Alloc };
        let held = archived.metadata_try_read().unwrap();
        let busy = hosts.find_or_create("guid-a", other, || info("a"), |_| {});
        assert_eq!(busy.err(), Some(NotCreated::MetadataBusy));
        assert!(Arc::ptr_eq(&hosts.find_by_guid("guid-a").unwrap(), &archived));
        drop(held);
        let new = hosts.find_or_create("guid-a", other, || info("a"), |_| {}).expect("created");
        assert!(!Arc::ptr_eq(&new, &archived));
        assert!(archived.metadata_try_read().is_none(), "the discarded host reads freed");
    }

    #[test]
    fn index_keeps_creation_order_and_single_receivers() {
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let a = hosts.find_or_create("guid-a", DbMode::Ram, || info("a"), |_| panic!("new host")).expect("created");
        hosts.find_or_create("guid-b", DbMode::Ram, || info("b"), |_| panic!("new host")).expect("created");
        let again = hosts.find_or_create(
            "guid-a",
            DbMode::Ram,
            || panic!("exists"),
            |h| h.update_info(|i| i.hostname = "a2".into()),
        ).expect("created");
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
        assert_eq!(a.set_receiver(Arc::clone(&first)), Attach::Attached);
        assert_eq!(a.set_receiver(Arc::clone(&second)), Attach::AlreadyServed);
        a.clear_receiver(&second, 0);
        assert!(a.receiver().is_some());
        a.clear_receiver(&first, 0);
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
        local.stream_send = StreamSend::new(true, "127.0.0.1:29181:SSL, other:19999", "a-key", "*");
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
            hosts.find_or_create(guid, DbMode::Ram, || child, |_| panic!("new host")).expect("created")
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
            ).expect("created")
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
            hosts.find_or_create("guid-a", DbMode::Ram, || info("a"), |_| panic!("new host")).expect("created");
        let mut wanted = info("renamed");
        wanted.program_name = "other".into();
        wanted.program_version = "v1".into();
        let ((), records) =
            netdata_agent_log::capture(|| host.update(&wanted, 2, 8192, true, 86400, 3600));
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
        let ((), records) =
            netdata_agent_log::capture(|| host.update(&wanted, 1, 8192, true, 86400, 3600));
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
        ).expect("created");
        assert!(Arc::ptr_eq(&same, &host));
        host.clear_pending_context_load();
        // the same memory mode: updated and no longer archived
        let (again, records) = netdata_agent_log::capture(|| {
            hosts.find_or_create(
                "5a1e0000-0000-4000-8000-0000000000c1",
                DbMode::Alloc,
                || panic!("created"),
                |h| h.update(&alloc, 1, 3600, true, 86400, 3600),
            ).expect("created")
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
            ).expect("created")
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

    /// `rrdhost_set_replication_parameters()`: every mode but dbengine caps the period at history x update every;
    /// the switch and the step are kept as given.
    #[test]
    fn the_replication_period_is_capped_by_the_ring_as_c() {
        let cases = [
            (DbMode::Ram, 4096, 1, 86400, 4096),
            (DbMode::Ram, 4096, 2, 86400, 8192),
            (DbMode::Ram, 4096, 3, 3600, 3600),
            (DbMode::Alloc, 3600, 5, 86400, 18000),
            (DbMode::Alloc, 3600, 5, 18000, 18000),
            (DbMode::None, 5, 10, 86400, 50),
            (DbMode::Dbengine, 0, 1, 86400, 86400),
        ];
        for (db_mode, history_entries, update_every, period, want) in cases {
            let mut h = HostInfo { db_mode, history_entries, update_every, ..info("h") };
            h.set_replication(false, period, 600);
            assert_eq!(
                (h.replication_enabled, h.replication_period, h.replication_step),
                (false, want, 600),
                "{db_mode:?} {history_entries} x {update_every}, {period}"
            );
        }
    }

    /// An archived host is loaded without replication or a sender; when it connects again it takes the receiver's,
    /// as `rrdhost_update()` does: the configured period capped for the host's own ring (not the receiver's), its
    /// progress back at 100%.
    #[test]
    fn a_reconnected_archived_host_takes_the_receivers_settings() {
        let hosts = Hosts::new(Host::new("local-guid", true, info("parent")));
        let archived = HostInfo {
            db_mode: DbMode::Alloc,
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            ..info("child")
        };
        let guid = "5a1e0000-0000-4000-8000-0000000000c3";
        let host = hosts.add_archived(guid, archived.clone(), |_| {});
        host.clear_pending_context_load();
        host.set_replication_percent(42.0);
        // the receiver's ring is smaller than the archived host's
        let mut wanted = HostInfo {
            stream_send: StreamSend::new(true, "grandparent:19999", "a-key", "*"),
            history_entries: 3600,
            ..archived
        };
        wanted.set_replication(true, 86400, 3600);
        let (_, records) = netdata_agent_log::capture(|| {
            hosts.find_or_create(
                guid,
                DbMode::Alloc,
                || panic!("created"),
                |h| h.update(&wanted, 1, 3600, true, 86400, 3600),
            ).expect("created")
        });
        let info = host.info();
        assert_eq!(
            (
                info.replication_enabled,
                info.replication_period,
                info.replication_step,
                info.stream_send.is_some(),
                host.replication_percent()
            ),
            (true, 4096, 3600, true, 100.0)
        );
        assert!(records.iter().any(|r| r.message.as_deref()
            == Some(
                "STREAM PARENTS 'child': added streaming destination No 1: 'grandparent:19999'"
            )));
        // a host that is not archived keeps its settings
        host.update(&wanted, 1, 3600, false, 0, 0);
        assert!(host.info().replication_enabled);
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
            hosts.find_or_create("guid-a", DbMode::Ram, || info("a"), |_| panic!("new host")).expect("created");
        assert_eq!(child.meta_flags(), meta_flags::INFO | meta_flags::UPDATE);
        assert!(child.last_connected_s() > 0);
        assert!(
            child.take_meta_flags(meta_flags::INFO) && !child.take_meta_flags(meta_flags::LABELS)
        );
        assert_eq!(child.meta_flags(), meta_flags::UPDATE);
        child.update(&info("a"), 1, 3600, true, 86400, 3600);
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

    /// `rrdhost_set_receiver()` while the obsolete-all walk runs: refused as busy, before the already-served check;
    /// accepted once the walk ended.
    #[test]
    fn an_attach_during_the_obsolete_all_walk_is_busy() {
        let host = Host::new("guid-b", false, info("b"));
        let slot = || {
            Arc::new(ReceiverSlot::new(
                1,
                Default::default(),
                ReceiverLink::default(),
                Box::new(|| {}),
            ))
        };
        host.obsolete_all_busy.store(true, Ordering::Release);
        assert_eq!(host.set_receiver(slot()), Attach::CleanupBusy);
        assert!(host.receiver().is_none());
        host.obsolete_all_busy.store(false, Ordering::Release);
        assert_eq!(host.set_receiver(slot()), Attach::Attached);
        host.obsolete_all_busy.store(true, Ordering::Release);
        assert_eq!(host.set_receiver(slot()), Attach::CleanupBusy, "busy first");
        host.obsolete_all_busy.store(false, Ordering::Release);
        assert_eq!(host.set_receiver(slot()), Attach::AlreadyServed);
    }

    /// `rrdhost_set_receiver()`'s `rrdcontext_host_child_connected()`: the charts' and dimensions' collected caches
    /// clear, so the child's next collection reports the instance collected again.
    #[test]
    fn an_attach_clears_the_contexts_collected_caches() {
        use crate::chart::Algorithm;
        const T: i64 = 1_700_000_000;
        let host = Host::new("guid-cc", false, info("cc"));
        let chart = collected_chart(&host, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        // a collection without values sets the chart's cache: a later store collects the metric, not the instance
        contexts::collected_rrdset(&chart);
        store(&dim, T, 1.0);
        contexts::collected_rrdset(&chart);
        let ri = host.contexts().get("t.c").unwrap().instance("t.c").unwrap();
        assert!(!ri.flags.is_collected());
        let slot = Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(host.set_receiver(slot), Attach::Attached);
        store(&dim, T + 1, 1.0);
        contexts::collected_rrdset(&chart);
        assert!(ri.flags.is_collected());
    }

    /// `rrdhost_free___while_having_rrd_wrlock()` after C's `host_check == host`: a host the index no longer holds (a
    /// discard created another under its GUID) is not freed; localhost never is.
    #[test]
    fn a_free_takes_only_the_indexed_host() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let old = hosts.add_archived("guid-a", info("a"), |_| {});
        old.clear_pending_context_load();
        // the discard of an archived host of another memory mode
        let new = hosts.find_or_create("guid-a", DbMode::Alloc, || info("a"), |_| {}).expect("created");
        assert!(!Arc::ptr_eq(&old, &new));
        assert!(hosts.free(&old).is_none());
        assert!(hosts.find_by_guid("guid-a").is_some_and(|h| Arc::ptr_eq(&h, &new)));
        assert!(hosts.free(hosts.localhost()).is_none());
        assert!(hosts.free(&new).is_some_and(|h| Arc::ptr_eq(&h, &new)));
        assert!(hosts.find_by_guid("guid-a").is_none());
    }
}
