//! A host's stream path (`host->stream.path`, `src/streaming/stream-path.c`): the agents between a node and the
//! place its data is stored, as the child last reported them. The protocol that fills and sends it lives in `ingest`.

/// `STREAM_PATH_FLAG_ACLK`.
pub const FLAG_ACLK: u8 = 1 << 0;
/// `STREAM_PATH_FLAG_HEALTH`.
pub const FLAG_HEALTH: u8 = 1 << 1;
/// `STREAM_PATH_FLAG_ML`.
pub const FLAG_ML: u8 = 1 << 2;
/// `STREAM_PATH_FLAG_EPHEMERAL`.
pub const FLAG_EPHEMERAL: u8 = 1 << 3;
/// `STREAM_PATH_FLAG_VIRTUAL`.
pub const FLAG_VIRTUAL: u8 = 1 << 4;

/// `STREAM_PATH`: one agent on the path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathEntry {
    pub hostname: String,
    /// The agent's machine GUID.
    pub host_id: [u8; 16],
    /// Its Netdata Cloud node id, zero when unknown.
    pub node_id: [u8; 16],
    /// Its claim id, zero when unclaimed.
    pub claim_id: [u8; 16],
    /// -1 for a stale node, 0 for the node itself, above 0 the hops from it.
    pub hops: i16,
    /// When the entry was last updated (the agent's start or the connection's acceptance).
    pub since: i64,
    /// The oldest timestamp in the agent's database.
    pub first_time_t: i64,
    pub flags: u8,
    pub capabilities: u32,
    /// Median milliseconds the agent needs to start and to shut down.
    pub start_time_ms: u32,
    pub shutdown_time_ms: u32,
}
