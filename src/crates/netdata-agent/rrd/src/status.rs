//! A host's status (`rrdhost_status()` with `RRDHOST_STATUS_BASIC`, `src/database/rrdhost-status.c`): whether its
//! database is queryable, whether it is live, and what feeds it. Only the fields this agent reports so far.

use crate::host::{Host, local_flags};

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
    /// A vnode's (none exists here yet); parents report it in `stream_info`.
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

impl Host {
    /// `rrdhost_status(host, now, &s, RRDHOST_STATUS_BASIC)`: a host that is not online is archived when no receiver
    /// attached to it since the agent started (one loaded from the metadata database), else offline.
    pub fn status_basic(&self, now: i64) -> HostStatus {
        // C's one `flags` load: the type and the online state agree while a vnode's run ends
        let flags = self.local_flags();
        let is_virtual = flags & local_flags::VIRTUAL != 0;
        let is_local = self.is_localhost() || is_virtual;
        let collector_online = flags & local_flags::COLLECTOR_ONLINE != 0;
        // has_receiver: a host that is not local, with a receiver while its collector is online
        let attached = !is_local && collector_online && self.receiver().is_some();
        let online = is_local || (collector_online && !self.is_orphan());
        let (first_time_s, mut last_time_s) = self.contexts().retention();
        if online {
            last_time_s = now;
        }
        let db_status = if first_time_s == 0
            || last_time_s == 0
            || self.is_pending_context_load()
            || !self.contexts().any_metric()
        {
            DbStatus::Initializing
        } else {
            DbStatus::Queryable
        };
        let ingest_status = if !online {
            if self.receiver_connections() == 0 {
                IngestStatus::Archived
            } else {
                IngestStatus::Offline
            }
        } else if db_status == DbStatus::Initializing {
            IngestStatus::Initializing
        } else if is_local {
            IngestStatus::Online
        } else if self.replicating_charts() > 0 || !self.contexts().any_metric_collected() {
            IngestStatus::Replicating
        } else {
            IngestStatus::Online
        };
        HostStatus {
            db_status,
            db_liveness: if ingest_status == IngestStatus::Online {
                DbLiveness::Live
            } else {
                DbLiveness::Stale
            },
            ingest_type: if self.is_localhost() {
                IngestType::Localhost
            } else if attached {
                IngestType::Child
            } else if is_virtual {
                IngestType::Virtual
            } else {
                IngestType::Archived
            },
            ingest_status,
            first_time_s,
            last_time_s,
        }
    }
}
