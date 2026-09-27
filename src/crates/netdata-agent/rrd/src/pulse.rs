//! The agent's counters for its own pulse charts (`daemon/pulse/`): what the web server, the stream receiver and the
//! query engine count as they work, read once a second by the PULSE thread. They live in the storage layout, with
//! C's other process globals (D77.2, D80.5).

use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;

/// `PULSE_HOST_STATUS`: a host's streaming state as the pulse charts count it (`pulse-parents.h`).
pub mod host_status {
    pub const LOCAL: u32 = 1 << 0;
    pub const VIRTUAL: u32 = 1 << 1;
    pub const LOADING: u32 = 1 << 2;
    pub const ARCHIVED: u32 = 1 << 3;
    pub const RCV_OFFLINE: u32 = 1 << 4;
    pub const RCV_WAITING: u32 = 1 << 5;
    pub const RCV_REPLICATING: u32 = 1 << 6;
    pub const RCV_REPLICATION_WAIT: u32 = 1 << 7;
    pub const RCV_RUNNING: u32 = 1 << 8;
    pub const SND_OFFLINE: u32 = 1 << 9;
    pub const SND_PENDING: u32 = 1 << 10;
    pub const SND_CONNECTING: u32 = 1 << 11;
    pub const SND_NO_DST: u32 = 1 << 12;
    pub const SND_NO_DST_FAILED: u32 = 1 << 13;
    pub const SND_WAITING: u32 = 1 << 14;
    pub const SND_REPLICATING: u32 = 1 << 15;
    pub const SND_RUNNING: u32 = 1 << 16;
    pub const DELETED: u32 = 1 << 17;
    pub const EPHEMERAL: u32 = 1 << 18;
    pub const PERMANENT: u32 = 1 << 19;

    pub const EPHEMERALITY: u32 = EPHEMERAL | PERMANENT;
    pub const BASIC: u32 = LOCAL | VIRTUAL | LOADING | ARCHIVED | DELETED;
    pub const RECEIVER: u32 =
        RCV_OFFLINE | RCV_WAITING | RCV_REPLICATING | RCV_REPLICATION_WAIT | RCV_RUNNING;
    pub const SENDER: u32 = SND_OFFLINE
        | SND_PENDING
        | SND_CONNECTING
        | SND_WAITING
        | SND_REPLICATING
        | SND_RUNNING
        | SND_NO_DST
        | SND_NO_DST_FAILED;
}

/// Every counter group.
#[derive(Debug, Default)]
pub struct Pulse {
    pub web: Web,
    pub network: Network,
    pub ingestion: Ingestion,
    pub queries: Queries,
}

/// `web_statistics` (`pulse-http-api.c`): the web server's clients and completed requests.
#[derive(Debug, Default)]
pub struct Web {
    connected_clients: AtomicI64,
    requests: AtomicU64,
    usec: AtomicU64,
    usec_max: AtomicU64,
    content_size_uncompressed: AtomicU64,
    content_size_compressed: AtomicU64,
}

/// A read of [`Web`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebStats {
    pub connected_clients: i64,
    pub requests: u64,
    pub usec: u64,
    pub usec_max: u64,
    pub content_size_uncompressed: u64,
    pub content_size_compressed: u64,
}

impl Web {
    /// `pulse_web_client_connected()`.
    pub fn client_connected(&self) {
        self.connected_clients.fetch_add(1, Ordering::Relaxed);
    }

    /// `pulse_web_client_disconnected()`.
    pub fn client_disconnected(&self) {
        self.connected_clients.fetch_sub(1, Ordering::Relaxed);
    }

    /// `pulse_web_request_completed()`: a request that took `dt_usec`, with its content size before and after
    /// compression.
    pub fn request_completed(&self, dt_usec: u64, content_size: u64, compressed_content_size: u64) {
        self.usec_max.fetch_max(dt_usec, Ordering::Relaxed);
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.usec.fetch_add(dt_usec, Ordering::Relaxed);
        self.content_size_uncompressed
            .fetch_add(content_size, Ordering::Relaxed);
        self.content_size_compressed
            .fetch_add(compressed_content_size, Ordering::Relaxed);
    }

    /// `pulse_web_copy()`: the counters; with `reset_max` the maximum starts again from 0 unless a request raised it
    /// since it was read.
    pub fn read(&self, reset_max: bool) -> WebStats {
        let stats = WebStats {
            connected_clients: self.connected_clients.load(Ordering::Relaxed),
            requests: self.requests.load(Ordering::Relaxed),
            usec: self.usec.load(Ordering::Relaxed),
            usec_max: self.usec_max.load(Ordering::Relaxed),
            content_size_uncompressed: self.content_size_uncompressed.load(Ordering::Relaxed),
            content_size_compressed: self.content_size_compressed.load(Ordering::Relaxed),
        };
        if reset_max {
            let _ = self.usec_max.compare_exchange(
                stats.usec_max,
                0,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        stats
    }
}

/// `network_statistics` (`pulse-network.c`): the bytes of the web API and of streaming, both directions.
#[derive(Debug, Default)]
pub struct Network {
    api_received: AtomicU64,
    api_sent: AtomicU64,
    stream_received: AtomicU64,
    stream_sent: AtomicU64,
}

/// A read of [`Network`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkStats {
    pub api_received: u64,
    pub api_sent: u64,
    pub stream_received: u64,
    pub stream_sent: u64,
}

impl Network {
    /// `pulse_web_server_received_bytes()`.
    pub fn api_received(&self, bytes: usize) {
        self.api_received.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_web_server_sent_bytes()`.
    pub fn api_sent(&self, bytes: usize) {
        self.api_sent.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_stream_received_bytes()`.
    pub fn stream_received(&self, bytes: usize) {
        self.stream_received
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_stream_sent_bytes()`.
    pub fn stream_sent(&self, bytes: usize) {
        self.stream_sent.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_network_copy()`.
    pub fn read(&self) -> NetworkStats {
        NetworkStats {
            api_received: self.api_received.load(Ordering::Relaxed),
            api_sent: self.api_sent.load(Ordering::Relaxed),
            stream_received: self.stream_received.load(Ordering::Relaxed),
            stream_sent: self.stream_sent.load(Ordering::Relaxed),
        }
    }
}

/// `ingest_statistics` (`pulse-ingestion.c`): the points stored per tier.
#[derive(Debug, Default)]
pub struct Ingestion {
    stored: [AtomicU64; RRD_STORAGE_TIERS],
}

thread_local! {
    /// `rrdset_done_statistics_points_stored_per_tier`: this thread's points stored since it last flushed them, kept
    /// per thread since every stream thread stores points.
    static STORED: Cell<[u64; RRD_STORAGE_TIERS]> = const { Cell::new([0; RRD_STORAGE_TIERS]) };
}

/// A point stored on `tier` by this thread.
pub fn point_stored(tier: usize) {
    STORED.with(|stored| {
        let mut counts = stored.get();
        if let Some(count) = counts.get_mut(tier) {
            *count += 1;
        }
        stored.set(counts);
    });
}

impl Ingestion {
    /// `store_metric_collection_completed()`: this thread's points stored on the `tiers` in use since its last flush
    /// move to the counters (C flushes at the end of a collection, of a backfill query, and at END2 and REND).
    pub fn collection_completed(&self, tiers: usize) {
        STORED.with(|stored| {
            let mut counts = stored.get();
            for (count, total) in counts.iter_mut().zip(&self.stored).take(tiers) {
                total.fetch_add(*count, Ordering::Relaxed);
                *count = 0;
            }
            stored.set(counts);
        });
    }

    /// `pulse_ingestion_copy()`.
    pub fn read(&self) -> [u64; RRD_STORAGE_TIERS] {
        std::array::from_fn(|t| self.stored[t].load(Ordering::Relaxed))
    }
}

/// `QUERY_SOURCE`: who asked for a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuerySource {
    ApiData,
    Ml,
    ApiWeights,
    ApiBadge,
    Health,
    UnitTest,
    Unknown,
}

/// The queries of one source, the points they read and the points they generated.
#[derive(Debug, Default)]
struct SourceCounters {
    queries: AtomicU64,
    points_read: AtomicU64,
    points_generated: AtomicU64,
}

/// A read of one source's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceStats {
    pub queries: u64,
    pub points_read: u64,
    pub points_generated: u64,
}

/// `query_statistics` (`pulse-queries.c`): the queries made and the points they read, per source.
#[derive(Debug, Default)]
pub struct Queries {
    /// `/api/vX/data`, ML, weights, badges, health.
    sources: [SourceCounters; 5],
    backfill_queries: AtomicU64,
    backfill_points_read: AtomicU64,
}

impl QuerySource {
    /// The counters a source's queries add to; none for unit tests and unknown sources.
    fn index(self) -> Option<usize> {
        match self {
            QuerySource::ApiData => Some(0),
            QuerySource::Ml => Some(1),
            QuerySource::ApiWeights => Some(2),
            QuerySource::ApiBadge => Some(3),
            QuerySource::Health => Some(4),
            QuerySource::UnitTest | QuerySource::Unknown => None,
        }
    }
}

impl Queries {
    /// `pulse_queries_rrdr_query_completed()`: queries of `source` and the points they read and generated.
    pub fn rrdr_query_completed(
        &self,
        queries: u64,
        points_read: u64,
        points_generated: u64,
        source: QuerySource,
    ) {
        if let Some(c) = source.index().map(|i| &self.sources[i]) {
            c.queries.fetch_add(queries, Ordering::Relaxed);
            c.points_read.fetch_add(points_read, Ordering::Relaxed);
            c.points_generated
                .fetch_add(points_generated, Ordering::Relaxed);
        }
    }

    /// The counters of `source` (zero for the ones C does not count).
    pub fn source(&self, source: QuerySource) -> SourceStats {
        source.index().map_or_else(SourceStats::default, |i| {
            let c = &self.sources[i];
            SourceStats {
                queries: c.queries.load(Ordering::Relaxed),
                points_read: c.points_read.load(Ordering::Relaxed),
                points_generated: c.points_generated.load(Ordering::Relaxed),
            }
        })
    }

    /// `pulse_queries_backfill_query_completed()`: one query of a lower tier for a backfill, and its points.
    pub fn backfill_query_completed(&self, points_read: u64) {
        self.backfill_queries.fetch_add(1, Ordering::Relaxed);
        self.backfill_points_read
            .fetch_add(points_read, Ordering::Relaxed);
    }

    /// The backfill queries and their points.
    pub fn backfill(&self) -> (u64, u64) {
        (
            self.backfill_queries.load(Ordering::Relaxed),
            self.backfill_points_read.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web counters as C keeps them: clients up and down, requests with their time and sizes, the maximum reset
    /// at a read unless a request raised it in between.
    #[test]
    fn web_counters_follow_c() {
        let web = Web::default();
        web.client_connected();
        web.client_connected();
        web.client_disconnected();
        web.request_completed(300, 1000, 400);
        web.request_completed(100, 50, 50);
        assert_eq!(
            web.read(true),
            WebStats {
                connected_clients: 1,
                requests: 2,
                usec: 400,
                usec_max: 300,
                content_size_uncompressed: 1050,
                content_size_compressed: 450,
            }
        );
        assert_eq!(web.read(false).usec_max, 0, "reset by the read");
        web.request_completed(20, 0, 0);
        assert_eq!(web.read(false).usec_max, 20);
    }

    /// Stored points count per thread until a flush moves the tiers in use to the totals; another thread's points
    /// wait for that thread's flush.
    #[test]
    fn stored_points_flush_per_thread() {
        let ingestion = Ingestion::default();
        point_stored(0);
        point_stored(0);
        point_stored(1);
        point_stored(2);
        point_stored(RRD_STORAGE_TIERS);
        ingestion.collection_completed(2);
        assert_eq!(ingestion.read()[..3], [2, 1, 0]);
        std::thread::scope(|s| {
            s.spawn(|| point_stored(0));
        });
        ingestion.collection_completed(3);
        assert_eq!(
            ingestion.read()[..3],
            [2, 1, 1],
            "tier 2 waited here; the other thread's point not"
        );
    }

    /// Each source counts apart; unit tests and unknown sources count nowhere.
    #[test]
    fn queries_count_per_source() {
        let q = Queries::default();
        q.rrdr_query_completed(1, 10, 5, QuerySource::ApiData);
        q.rrdr_query_completed(1, 2, 1, QuerySource::ApiData);
        q.rrdr_query_completed(3, 4, 5, QuerySource::Health);
        q.rrdr_query_completed(9, 9, 9, QuerySource::UnitTest);
        q.rrdr_query_completed(9, 9, 9, QuerySource::Unknown);
        let stats = |queries, points_read, points_generated| SourceStats {
            queries,
            points_read,
            points_generated,
        };
        assert_eq!(q.source(QuerySource::ApiData), stats(2, 12, 6));
        assert_eq!(q.source(QuerySource::Health), stats(3, 4, 5));
        assert_eq!(q.source(QuerySource::Ml), stats(0, 0, 0));
        assert_eq!(q.source(QuerySource::UnitTest), stats(0, 0, 0));
    }

    #[test]
    fn network_counters_add_up() {
        let net = Network::default();
        net.api_received(10);
        net.api_sent(20);
        net.stream_received(30);
        net.stream_sent(40);
        net.api_sent(1);
        assert_eq!(
            net.read(),
            NetworkStats {
                api_received: 10,
                api_sent: 21,
                stream_received: 30,
                stream_sent: 40,
            }
        );
    }
}
