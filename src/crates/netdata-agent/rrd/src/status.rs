//! A host's status (`rrdhost_status()`, `src/database/rrdhost-status.c`): whether its database is queryable, whether
//! it is live, what feeds it and what it offers. [`Host::status_basic`] is C's `RRDHOST_STATUS_BASIC`, the six fields
//! health and pulse ask on their own paths; [`Host::status`] is `RRDHOST_STATUS_ALL` without two parts: health's,
//! which the daemon computes (health sits above this crate), and `stream`, the sender's, which is not ported yet.

use std::sync::Arc;

use crate::host::{Host, ReceiverSlot, local_flags, netdata_start_time};
use crate::mode::DbMode;

/// `RRDHOST_DB_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbStatus {
    Initializing,
    Queryable,
}

/// `RRDHOST_DB_LIVENESS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbLiveness {
    Stale,
    Live,
}

/// `RRDHOST_INGEST_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestType {
    Localhost,
    /// A vnode a plugin of this agent collects.
    Virtual,
    Child,
    Archived,
}

/// `RRDHOST_INGEST_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestStatus {
    Archived,
    Initializing,
    Replicating,
    Online,
    Offline,
}

/// `RRDHOST_ML_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlStatus {
    Disabled,
    Offline,
    Running,
}

/// `RRDHOST_ML_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlType {
    Disabled,
    /// `RRDHOST_ML_TYPE_SELF`: the host's models are trained by this agent.
    Own,
    Received,
}

/// `RRDHOST_DYNCFG_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DyncfgStatus {
    Unavailable,
    Available,
}

impl MlStatus {
    /// `rrdhost_ml_status_to_string()`.
    pub fn name(self) -> &'static str {
        match self {
            MlStatus::Disabled => "disabled",
            MlStatus::Offline => "offline",
            MlStatus::Running => "online",
        }
    }
}

impl MlType {
    /// `rrdhost_ml_type_to_string()`.
    pub fn name(self) -> &'static str {
        match self {
            MlType::Disabled => "disabled",
            MlType::Own => "self",
            MlType::Received => "received",
        }
    }
}

impl DyncfgStatus {
    /// `rrdhost_dyncfg_status_to_string()`.
    pub fn name(self) -> &'static str {
        match self {
            DyncfgStatus::Unavailable => "unavailable",
            DyncfgStatus::Available => "online",
        }
    }
}

impl DbStatus {
    /// `rrdhost_db_status_to_string()`.
    pub fn name(self) -> &'static str {
        match self {
            DbStatus::Initializing => "initializing",
            DbStatus::Queryable => "online",
        }
    }
}

/// `ENUM_STR_DEFINE_FUNCTIONS`' `_2id()`: the variant whose name is exactly `text`, else `default`.
fn from_name<T: Copy>(all: &[T], name: impl Fn(T) -> &'static str, text: &[u8], default: T) -> T {
    all.iter().copied().find(|&v| name(v).as_bytes() == text).unwrap_or(default)
}

impl DbStatus {
    /// `RRDHOST_DB_STATUS_2id()`.
    pub fn from_name(text: &[u8]) -> Self {
        from_name(&[DbStatus::Initializing, DbStatus::Queryable], DbStatus::name, text, DbStatus::Initializing)
    }
}

impl DbLiveness {
    /// `RRDHOST_DB_LIVENESS_2id()`.
    pub fn from_name(text: &[u8]) -> Self {
        from_name(&[DbLiveness::Stale, DbLiveness::Live], DbLiveness::name, text, DbLiveness::Stale)
    }
}

impl IngestType {
    /// `RRDHOST_INGEST_TYPE_2id()`.
    pub fn from_name(text: &[u8]) -> Self {
        let all = [IngestType::Localhost, IngestType::Virtual, IngestType::Child, IngestType::Archived];
        from_name(&all, IngestType::name, text, IngestType::Archived)
    }
}

impl IngestStatus {
    /// `RRDHOST_INGEST_STATUS_2id()`.
    pub fn from_name(text: &[u8]) -> Self {
        let all = [
            IngestStatus::Archived,
            IngestStatus::Initializing,
            IngestStatus::Replicating,
            IngestStatus::Online,
            IngestStatus::Offline,
        ];
        from_name(&all, IngestStatus::name, text, IngestStatus::Offline)
    }
}

impl DbLiveness {
    /// `rrdhost_db_liveness_to_string()`.
    pub fn name(self) -> &'static str {
        match self {
            DbLiveness::Stale => "stale",
            DbLiveness::Live => "live",
        }
    }
}

impl IngestType {
    /// `rrdhost_ingest_type_to_string()`.
    pub fn name(self) -> &'static str {
        match self {
            IngestType::Localhost => "localhost",
            IngestType::Virtual => "virtual",
            IngestType::Child => "child",
            IngestType::Archived => "archived",
        }
    }
}

impl IngestStatus {
    /// `rrdhost_ingest_status_to_string()`.
    pub fn name(self) -> &'static str {
        match self {
            IngestStatus::Archived => "archived",
            IngestStatus::Initializing => "initializing",
            IngestStatus::Replicating => "replicating",
            IngestStatus::Online => "online",
            IngestStatus::Offline => "offline",
        }
    }
}

/// `RRDHOST_STATUS` with `RRDHOST_STATUS_BASIC`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostStatus {
    pub db_status: DbStatus,
    pub db_liveness: DbLiveness,
    pub ingest_type: IngestType,
    pub ingest_status: IngestStatus,
    pub first_time_s: i64,
    pub last_time_s: i64,
}

/// `RRDHOST_STATUS`' `db`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Db {
    pub status: DbStatus,
    pub liveness: DbLiveness,
    /// `host->rrd_memory_mode`.
    pub mode: DbMode,
    pub first_time_s: i64,
    pub last_time_s: i64,
    /// The items of the host's contexts tree (`host->rrdctx.*_count`), collected or not.
    pub metrics: u64,
    pub instances: u64,
    pub contexts: u64,
}

/// `SOCKET_PEERS`: the two ends of a socket as texts. The default is C's zeroed struct, what a host without an
/// attached receiver has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SocketPeers {
    pub local_ip: String,
    pub local_port: u16,
    pub peer_ip: String,
    pub peer_port: u16,
}

/// `RRDHOST_STATUS`' `replication`, of the receiver here.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Replication {
    pub in_progress: bool,
    /// Percent, as stored; nothing clamps it.
    pub completion: f64,
    /// The charts that replicate.
    pub instances: u64,
}

/// `RRDHOST_STATUS`' `ingest`. Its last four fields are a child's: filled for a host that is not local and has a
/// receiver while its collector is online, C's zeroes for any other.
#[derive(Debug, Clone, PartialEq)]
pub struct Ingest {
    /// `host->stream.rcv.status.connections`: the receivers that attached to the host since the agent started.
    pub id: u32,
    pub hops: i16,
    pub kind: IngestType,
    pub status: IngestStatus,
    /// Since when the host is in this state: its last attach or detach; the agent's start for a local host that
    /// ingests and for a host no receiver touched; the end of its data for an archived one.
    pub since_s: i64,
    /// `host->stream.rcv.status.reason`, a `STREAM_HANDSHAKE` code: the attached receiver's capabilities (positive,
    /// which reads `CONNECTED`), or why the last receiver ended; 0 for a host none attached to.
    pub reason: i32,
    /// The items of the tree that are collected now (`host->collected.*_count`).
    pub metrics: u64,
    pub instances: u64,
    pub contexts: u64,
    pub replication: Replication,
    /// The attached receiver's negotiated capabilities.
    pub capabilities: u32,
    /// The two ends of its socket, as they were when it attached (C asks the socket at each status; D241 F4).
    pub peers: SocketPeers,
    /// Its connection has TLS.
    pub tls: bool,
}

/// `RRDHOST_STATUS`' `ml`, without the counts of a host whose models run (no host's do: ML is not ported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ml {
    pub status: MlStatus,
    pub kind: MlType,
}

/// `RRDHOST_STATUS` with `RRDHOST_STATUS_ALL`, without the parts the module's note names.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// The clock the status was asked with: an online host's data ends there, and ages are counted from it.
    pub now: i64,
    pub db: Db,
    pub ingest: Ingest,
    pub ml: Ml,
    pub dyncfg: DyncfgStatus,
}

impl Host {
    /// `rrdhost_should_run_health()`: health is enabled for the host, its collector is online, it is no orphan and
    /// it ingests data now. A host without a metric is not online in that sense, so an empty one has no pass.
    pub fn should_run_health(&self, now: i64) -> bool {
        self.info().health_enabled
            && self.collector_online()
            && !self.is_orphan()
            && self.status_basic(now).ingest_status == IngestStatus::Online
    }

    /// `rrdhost_status(host, now, &s, RRDHOST_STATUS_BASIC)`: a host that is not online is archived when no receiver
    /// attached to it since the agent started (one loaded from the metadata database), else offline.
    pub fn status_basic(&self, now: i64) -> HostStatus {
        let flags = self.local_flags();
        let contexts = self.contexts();
        self.status_decided(
            flags,
            now,
            || self.receiver_connections(),
            || contexts.any_metric(),
            || contexts.any_metric_collected(),
        )
        .basic
    }

    /// `rrdhost_status(host, now, &s, RRDHOST_STATUS_ALL)`, without the parts the module's note names. The host's
    /// contexts tree is walked once: its counts are the status's, and the questions the basic status asks the tree
    /// are answered from them.
    pub fn status(&self, now: i64) -> Status {
        self.status_started(now, netdata_start_time())
    }

    /// [`Host::status`] of an agent that started at `start_s` (C's `netdata_start_time`, one value per process).
    fn status_started(&self, now: i64, start_s: i64) -> Status {
        // C's order (`rrdhost_status()`): the flags, the database's counts, then the receiver's values in one hold.
        // An attach writes its count and its time before it sets the collector online, all in one hold: with the
        // flags read first, a child that reads online never has the count and the time of the connection before
        let flags = self.local_flags();
        let counts = self.contexts().counts(|_| {});
        let receiver = self.receiver_status();
        let Decided { basic, slot, replicating_charts } = self.status_decided(
            flags,
            now,
            || receiver.connections,
            || counts.metrics.available > 0,
            || counts.metrics.collected > 0,
        );
        // rrdhost_status_ingest()'s second hold of the receiver lock: what an attached receiver adds, C's zeroes
        // without one. The replicating charts are the decision's own load when it made one, as in C
        let child = slot.map(|slot| {
            let instances = u64::from(replicating_charts.unwrap_or_else(|| self.replicating_charts()));
            let replication =
                Replication { in_progress: instances > 0, completion: self.replication_percent(), instances };
            (replication, slot.link.capabilities, slot.peers().clone(), slot.tls())
        });
        let (replication, capabilities, peers, tls) = child.unwrap_or_default();
        // rrdhost_status_ingest()'s `since`. A host is local when it is localhost or a vnode, and its type says so:
        // a vnode takes no receiver, so it is never a child
        let local = matches!(basic.ingest_type, IngestType::Localhost | IngestType::Virtual);
        let since_s = if basic.ingest_status == IngestStatus::Archived {
            basic.last_time_s
        } else if local && basic.ingest_status == IngestStatus::Online {
            start_s
        } else {
            receiver.last_connected_s.max(receiver.last_disconnected_s)
        };
        Status {
            now,
            db: Db {
                status: basic.db_status,
                liveness: basic.db_liveness,
                mode: self.info().db_mode,
                first_time_s: basic.first_time_s,
                last_time_s: basic.last_time_s,
                metrics: counts.metrics.available,
                instances: counts.instances.available,
                contexts: counts.contexts.available,
            },
            ingest: Ingest {
                id: receiver.connections,
                hops: self.ingestion_hops(),
                kind: basic.ingest_type,
                status: basic.ingest_status,
                since_s: if since_s == 0 { start_s } else { since_s },
                reason: receiver.reason,
                metrics: counts.metrics.collected,
                instances: counts.instances.collected,
                contexts: counts.contexts.collected,
                replication,
                capabilities,
                peers,
                tls,
            },
            // rrdhost_status_ml_internal() of a host without an ML host: no host has one here
            ml: Ml {
                status: MlStatus::Disabled,
                kind: MlType::Disabled,
            },
            dyncfg: if self.dyncfg_available() {
                DyncfgStatus::Available
            } else {
                DyncfgStatus::Unavailable
            },
        }
    }

    /// What both statuses decide alike (`rrdhost_status_db()` and `rrdhost_status_ingest()`): the retention, the
    /// database's status and liveness, the ingestion's type and status. `flags` is the caller's one load of the
    /// local flags, taken before anything else it reads of the host (C's `flags` snapshot). `connections` is the
    /// host's count of attached receivers; it and the two questions about the contexts tree are asked at most once
    /// each, where C loads its counters, so the basic status answers without walking the tree whole.
    fn status_decided(
        &self,
        flags: u8,
        now: i64,
        connections: impl Fn() -> u32,
        has_metric: impl Fn() -> bool,
        has_collected_metric: impl Fn() -> bool,
    ) -> Decided {
        // with one load the type and the online state agree while a vnode's run ends (ORPHAN, a word of its own
        // here, is read apart)
        let is_virtual = flags & local_flags::VIRTUAL != 0;
        let is_local = self.is_localhost() || is_virtual;
        let collector_online = flags & local_flags::COLLECTOR_ONLINE != 0;
        // has_receiver: a host that is not local, with a receiver while its collector is online
        let slot = if !is_local && collector_online { self.receiver() } else { None };
        let online = is_local || (collector_online && !self.is_orphan());
        let (first_time_s, mut last_time_s) = self.contexts().retention();
        if online {
            last_time_s = now;
        }
        let db_status = if first_time_s == 0 || last_time_s == 0 || self.is_pending_context_load() || !has_metric() {
            DbStatus::Initializing
        } else {
            DbStatus::Queryable
        };
        let mut replicating_charts = None;
        let ingest_status = if !online {
            if connections() == 0 {
                IngestStatus::Archived
            } else {
                IngestStatus::Offline
            }
        } else if db_status == DbStatus::Initializing {
            IngestStatus::Initializing
        } else if is_local {
            IngestStatus::Online
        } else {
            let charts = self.replicating_charts();
            replicating_charts = Some(charts);
            if charts > 0 || !has_collected_metric() {
                IngestStatus::Replicating
            } else {
                IngestStatus::Online
            }
        };
        let basic = HostStatus {
            db_status,
            db_liveness: if ingest_status == IngestStatus::Online {
                DbLiveness::Live
            } else {
                DbLiveness::Stale
            },
            ingest_type: if self.is_localhost() {
                IngestType::Localhost
            } else if slot.is_some() {
                IngestType::Child
            } else if is_virtual {
                IngestType::Virtual
            } else {
                IngestType::Archived
            },
            ingest_status,
            first_time_s,
            last_time_s,
        };
        Decided { basic, slot, replicating_charts }
    }
}

/// What [`Host::status_decided`] found on its way that the full status reads too.
struct Decided {
    basic: HostStatus,
    /// The attached receiver's slot: of a host that is not local, while its collector is online.
    slot: Option<Arc<ReceiverSlot>>,
    /// The receiver's replicating charts, when the decision loaded them.
    replicating_charts: Option<u32>,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::chart::Algorithm;
    use crate::host::{Attach, Hosts, ReceiverLink, ReceiverSlot};
    use crate::testutil::{collected_chart, info, store};

    const T0: i64 = 1_790_180_000;
    /// The agent's start in these units; no test here reads the process's own.
    const START: i64 = 1_790_170_000;

    /// A chart of the host with one dimension that stored at `T0` and was collected, its contexts processed.
    fn collect(host: &Host) {
        let chart = collected_chart(host, DbMode::Ram);
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        store(&dim, T0, 1.0);
        crate::contexts::collected_rrdset(&chart);
        host.contexts().worker_cycle();
    }

    fn slot() -> Arc<ReceiverSlot> {
        slot_with(0)
    }

    /// A receiver's slot whose connection negotiated `capabilities`.
    fn slot_with(capabilities: u32) -> Arc<ReceiverSlot> {
        let link = ReceiverLink { capabilities, ..ReceiverLink::default() };
        Arc::new(ReceiverSlot::new(1, Default::default(), link, Box::new(|| {})))
    }

    /// The six fields of the basic status, taken from the full one.
    fn basic_of(s: &Status) -> HostStatus {
        HostStatus {
            db_status: s.db.status,
            db_liveness: s.db.liveness,
            ingest_type: s.ingest.kind,
            ingest_status: s.ingest.status,
            first_time_s: s.db.first_time_s,
            last_time_s: s.db.last_time_s,
        }
    }

    /// The full status of a host, which must agree with the basic one in the six fields both have.
    fn full(host: &Host, now: i64) -> Status {
        let s = host.status_started(now, START);
        assert_eq!(basic_of(&s), host.status_basic(now));
        s
    }

    /// The texts of the three enums the full status adds (`rrdhost-status.c:44-69`): a host whose models run says
    /// `online`, one trained here `self`, and DynCfg that is available `online`.
    #[test]
    fn the_added_status_texts_are_cs() {
        let ml = [MlStatus::Disabled, MlStatus::Offline, MlStatus::Running].map(MlStatus::name);
        assert_eq!(ml, ["disabled", "offline", "online"]);
        let kind = [MlType::Disabled, MlType::Own, MlType::Received].map(MlType::name);
        assert_eq!(kind, ["disabled", "self", "received"]);
        let dyncfg = [DyncfgStatus::Unavailable, DyncfgStatus::Available].map(DyncfgStatus::name);
        assert_eq!(dyncfg, ["unavailable", "online"]);
    }

    /// Localhost (`rrdhost-status.c:119-145`, `:169-234`): with nothing collected its database is initializing and
    /// stale, with no first time and its last time the clock, and its ingestion initializing since the agent's
    /// start; with a collected metric both are online and live, since the start again. Its id and hops are 0, ML is
    /// off, and its DynCfg is always there.
    #[test]
    fn a_local_host_s_status() {
        let host = Host::new("guid-l", true, info("l"));
        let nothing = Status {
            now: T0 + 50,
            db: Db {
                status: DbStatus::Initializing,
                liveness: DbLiveness::Stale,
                mode: DbMode::Ram,
                first_time_s: 0,
                last_time_s: T0 + 50,
                metrics: 0,
                instances: 0,
                contexts: 0,
            },
            ingest: Ingest {
                id: 0,
                hops: 0,
                kind: IngestType::Localhost,
                status: IngestStatus::Initializing,
                since_s: START,
                reason: 0,
                metrics: 0,
                instances: 0,
                contexts: 0,
                replication: Replication::default(),
                capabilities: 0,
                peers: SocketPeers::default(),
                tls: false,
            },
            ml: Ml {
                status: MlStatus::Disabled,
                kind: MlType::Disabled,
            },
            dyncfg: DyncfgStatus::Available,
        };
        assert_eq!(full(&host, T0 + 50), nothing);

        collect(&host);
        let s = full(&host, T0 + 50);
        assert_eq!((s.db.status, s.db.liveness), (DbStatus::Queryable, DbLiveness::Live));
        // the stored point is the second that ends at T0
        assert_eq!((s.db.first_time_s, s.db.last_time_s), (T0 - 1, T0 + 50));
        assert_eq!((s.db.metrics, s.db.instances, s.db.contexts), (1, 1, 1));
        let collected = Ingest {
            status: IngestStatus::Online,
            metrics: 1,
            instances: 1,
            contexts: 1,
            ..nothing.ingest.clone()
        };
        assert_eq!(s.ingest, collected);
        assert_eq!((s.ml, s.dyncfg), (nothing.ml, DyncfgStatus::Available));
    }

    /// A vnode a plugin of this agent collects is `virtual`, one hop away, with no receiver ever (id 0); it is
    /// local, so once it has data it is online since the agent's start. It is not localhost: its DynCfg needs a
    /// `config` method.
    #[test]
    fn a_vnode_s_status() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let vnode = hosts.find_or_create("guid-v", DbMode::Ram, || info("v"), |_| {}).expect("created");
        vnode.set_virtual();
        vnode.set_collector_online();
        let s = full(&vnode, T0 + 50);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Virtual, IngestStatus::Initializing));
        assert_eq!((s.ingest.id, s.ingest.hops, s.ingest.since_s), (0, 1, START));
        collect(&vnode);
        let s = full(&vnode, T0 + 50);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Virtual, IngestStatus::Online));
        assert_eq!((s.ingest.id, s.ingest.hops, s.ingest.since_s), (0, 1, START));
        assert_eq!((s.db.liveness, s.dyncfg), (DbLiveness::Live, DyncfgStatus::Unavailable));

        // a host that was a child and is a vnode now is local too: since the agent's start, whatever its receiver
        // left behind (`rrdhost-status.c:175-178`), and the count of its connections stays
        let former = hosts.find_or_create("guid-f", DbMode::Ram, || info("f"), |_| {}).expect("created");
        let gone = slot();
        assert_eq!(former.set_receiver(Arc::clone(&gone)), Attach::Attached);
        former.clear_receiver(&gone, 0);
        let left = former.receiver_last_disconnected_s();
        assert!(left > START);
        former.set_virtual();
        former.set_collector_online();
        // while its database initializes it is not yet "online", and C leaves it since its receiver's times
        let s = full(&former, T0 + 50);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Virtual, IngestStatus::Initializing));
        assert_eq!((s.ingest.id, s.ingest.since_s), (1, left));
        collect(&former);
        let s = full(&former, T0 + 50);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Virtual, IngestStatus::Online));
        assert_eq!((s.ingest.id, s.ingest.since_s), (1, START));

        // a vnode its plugin stopped collecting is no vnode any more: archived, since the end of its data
        vnode.virtual_offline();
        let s = full(&vnode, T0 + 50);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Archived, IngestStatus::Archived));
        assert!(s.db.last_time_s >= T0 && s.db.last_time_s != T0 + 50, "{s:?}");
        assert_eq!(s.ingest.since_s, s.db.last_time_s);
    }

    /// A host loaded from the metadata database that no receiver attached to is archived, since the end of its
    /// data (`rrdhost-status.c:197-198`); without data, since the agent's start (`:202`). Its database reads as
    /// stored: the last time is not the clock.
    #[test]
    fn an_archived_host_s_since_is_its_data_s_end() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let empty = hosts.add_archived("guid-e", info("e"), |_| {});
        empty.clear_pending_context_load();
        let s = full(&empty, T0 + 50);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Archived, IngestStatus::Archived));
        assert_eq!((s.db.status, s.db.last_time_s), (DbStatus::Initializing, 0));
        assert_eq!((s.ingest.since_s, s.ingest.id), (START, 0));
        // an ephemeral host counts as disconnected at its load; archived, it still reads since its data's end
        empty.set_receiver_last_disconnected_s(T0 + 7);
        let s = full(&empty, T0 + 50);
        assert_eq!((s.ingest.status, s.ingest.since_s, s.ingest.reason), (IngestStatus::Archived, START, 0));

        let stored = hosts.add_archived("guid-a", info("a"), |_| {});
        collect(&stored);
        stored.clear_pending_context_load();
        let s = full(&stored, T0 + 50);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Archived, IngestStatus::Archived));
        assert_eq!((s.db.status, s.db.liveness), (DbStatus::Queryable, DbLiveness::Stale));
        assert!(s.db.last_time_s >= T0 && s.db.last_time_s != T0 + 50, "{s:?}");
        assert_eq!(s.ingest.since_s, s.db.last_time_s);
    }

    /// A child (`rrdhost-status.c:162-169`, `:187-234`): attached, it is a `child` since its connection, with the
    /// count of its connections as its id; after it left it is archived by type and offline by status, since its
    /// disconnection, and its id stays. The database's counts are the tree's items and the ingestion's the collected
    /// ones: a child that left has its metrics and collects none.
    #[test]
    fn a_child_s_status_follows_its_receiver() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let child = hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).expect("created");
        assert_eq!(full(&child, T0).ingest.reason, 0);
        let peers = SocketPeers {
            local_ip: "10.0.0.1".into(),
            local_port: 19999,
            peer_ip: "10.0.0.2".into(),
            peer_port: 40000,
        };
        let link = ReceiverLink { capabilities: 0x41, ..ReceiverLink::default() };
        let attached = ReceiverSlot::new(1, Default::default(), link, Box::new(|| {})).with_socket(peers.clone(), true);
        let attached = Arc::new(attached);
        assert_eq!(child.set_receiver(Arc::clone(&attached)), Attach::Attached);
        let connected = child.receiver_last_connected_s();
        assert!(connected > START);
        let s = full(&child, connected + 5);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Child, IngestStatus::Initializing));
        assert_eq!((s.ingest.id, s.ingest.since_s, s.dyncfg), (1, connected, DyncfgStatus::Unavailable));
        // the stored reason of an attached receiver is its capabilities
        assert_eq!(s.ingest.reason, 0x41);
        // a child's own part: its receiver's capabilities, the two ends of its socket, its TLS flag, and its
        // replication, of which no chart is in progress
        let idle = Replication { in_progress: false, completion: child.replication_percent(), instances: 0 };
        assert_eq!((s.ingest.capabilities, &s.ingest.peers, s.ingest.tls), (0x41, &peers, true));
        assert_eq!(s.ingest.replication, idle);

        collect(&child);
        let s = full(&child, connected + 5);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Child, IngestStatus::Online));
        assert_eq!((s.db.liveness, s.db.last_time_s, s.ingest.since_s), (DbLiveness::Live, connected + 5, connected));
        assert_eq!((s.db.metrics, s.ingest.metrics), (1, 1));

        // a chart of it replicates: the status says so, with the count, in the basic status too
        child.replicating_charts_plus_one();
        let s = full(&child, connected + 5);
        assert_eq!(s.ingest.status, IngestStatus::Replicating);
        assert_eq!((s.ingest.replication.in_progress, s.ingest.replication.instances), (true, 1));
        child.replicating_charts_minus_one();
        assert_eq!(full(&child, connected + 5).ingest.status, IngestStatus::Online);

        // the receiver says why it ends; the detach's own argument is not what the host keeps
        attached.set_exit_reason(-6, false);
        child.clear_receiver(&attached, 0);
        child.contexts().worker_cycle();
        let disconnected = child.receiver_last_disconnected_s();
        assert!(disconnected >= connected);
        let s = full(&child, disconnected + 5);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Archived, IngestStatus::Offline));
        assert_eq!(s.ingest.reason, -6);
        // no receiver: C's zeroes
        assert_eq!((s.ingest.capabilities, s.ingest.tls), (0, false));
        assert_eq!((&s.ingest.peers, s.ingest.replication), (&SocketPeers::default(), Replication::default()));
        assert_eq!((s.ingest.id, s.ingest.since_s, s.db.liveness), (1, disconnected, DbLiveness::Stale));
        assert_eq!((s.db.metrics, s.db.instances, s.db.contexts), (1, 1, 1));
        assert_eq!((s.ingest.metrics, s.ingest.instances, s.ingest.contexts), (0, 0, 0));

        // it returns: its metrics are there and none is collected yet, so it replicates (`:179-182`), in the full
        // status as in the basic one, with a second connection to its count and its new capabilities as the reason
        assert_eq!(child.set_receiver(slot_with(0x43)), Attach::Attached);
        let s = full(&child, disconnected + 5);
        assert_eq!((s.ingest.kind, s.ingest.status, s.ingest.id), (IngestType::Child, IngestStatus::Replicating, 2));
        assert_eq!((s.db.status, s.db.liveness), (DbStatus::Queryable, DbLiveness::Stale));
        assert_eq!((s.db.metrics, s.ingest.metrics, s.ingest.reason), (1, 0, 0x43));
    }

    /// A child that left and was then cleaned up to archive (`rrdhost_cleanup_data_collection_and_health()`,
    /// `rrdhost.c:975-1031`): archived by type and offline by status still (the count of its connections is never
    /// reset), since its disconnection, with the reason it left with. Its function registry is gone, so an
    /// instance of it prints no `functions`, and so is its sender.
    #[test]
    fn a_child_cleaned_up_to_archive_keeps_its_count() {
        let hosts = Hosts::new(Host::new("guid-l", true, info("l")));
        let child = hosts.find_or_create("guid-c", DbMode::Ram, || info("c"), |_| {}).expect("created");
        let gone = slot_with(0x41);
        assert_eq!(child.set_receiver(Arc::clone(&gone)), Attach::Attached);
        gone.set_exit_reason(-6, false);
        child.clear_receiver(&gone, 0);
        let disconnected = child.receiver_last_disconnected_s();
        assert!(child.functions().exists());
        child.cleanup_data_collection();
        let s = full(&child, disconnected + 5);
        assert_eq!((s.ingest.kind, s.ingest.status), (IngestType::Archived, IngestStatus::Offline));
        assert_eq!((s.ingest.id, s.ingest.since_s, s.ingest.reason), (1, disconnected, -6));
        assert!(!child.functions().exists() && child.upstream().is_none());
    }
}
