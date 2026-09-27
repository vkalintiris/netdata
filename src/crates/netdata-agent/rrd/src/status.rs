//! A host's status (`rrdhost_status()` with `RRDHOST_STATUS_BASIC`, `src/database/rrdhost-status.c`): whether its
//! database is queryable, whether it is live, and what feeds it. Only the fields this agent reports so far.

use crate::host::Host;

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
    /// `rrdhost_status(host, now, &s, RRDHOST_STATUS_BASIC)`, without vnodes: a host that is not online is archived
    /// when no receiver attached to it since the agent started (one loaded from the metadata database), else offline.
    pub fn status_basic(&self, now: i64) -> HostStatus {
        let attached = self.receiver().is_some();
        let online = self.is_online();
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
        } else if self.is_localhost() {
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
            } else {
                IngestType::Archived
            },
            ingest_status,
            first_time_s,
            last_time_s,
        }
    }
}
