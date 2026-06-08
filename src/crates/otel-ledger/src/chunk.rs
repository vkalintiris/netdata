//! Query-time chunk indexing of active WAL files.
//!
//! When a query needs an active WAL — one still being written, not yet
//! rotated into an SFST — the ledger indexes its durable prefix in
//! fixed-entry **chunks** and serves those plus a row-scanned tail (see
//! `docs/wal-query-design.md`, milestone 3). This module owns the two
//! pieces that make that affordable:
//!
//! - [`chunk_boundaries`] — the pure policy that folds a frame-header
//!   scan ([`wal::scan_frame_boundaries`]) into chunk boundaries at a
//!   `min_entries` threshold. Chunk boundaries are **append-only and
//!   immutable**: a boundary is fixed by the frame entry counts up to
//!   it, so `valid_up_to` advancing only ever appends new higher-index
//!   chunks — it never moves or invalidates an existing one. That is
//!   what lets the cache below be a write-once memo.
//!
//! - [`ChunkCache`] — a process-wide memo of built chunk SFST byte
//!   images, keyed `(wal_seq, chunk_index)`, with build singleflight and
//!   a byte-budget LRU (both from `moka`). Concurrent queries that need
//!   the same chunk build it once; a chunk, once built, is reused until
//!   the WAL rotates ([`ChunkCache::drop_seq`]) or the budget evicts it.
//!
//! The per-query orchestration that uses these — capturing the
//! durable-prefix snapshot under the registry lock, building the missing
//! chunks, and handing the chunk images + tail range to the engine — is
//! the query-path wiring (milestone 4), not here.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;

use moka::future::Cache;
use wal::FrameBoundary;

/// One complete chunk of a WAL's durable prefix: a contiguous,
/// frame-aligned byte range carrying at least the threshold's worth of
/// log records (the last frame can push it over).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkBoundary {
    /// 0-based index within the WAL, dense and stable across queries.
    pub index: u32,
    /// Byte offset of the chunk's first frame (a frame boundary).
    pub start: u64,
    /// Byte offset just past the chunk's last frame.
    pub end: u64,
    /// Log records in the chunk (>= `min_entries`).
    pub entry_count: u64,
}

/// Fold a frame-boundary scan into complete chunks of at least
/// `min_entries` records each.
///
/// `frames` are the boundaries from [`wal::scan_frame_boundaries`] over
/// `[start, valid_up_to)`, in file order; `start` is the offset of the
/// first frame (`wal::HEADER_SIZE` for a whole-prefix scan, or a prior
/// chunk's `end`). Each chunk extends to the first frame boundary at or
/// past the running `min_entries`, then a new chunk begins. Frames after
/// the last complete chunk (cumulative `< min_entries`) are **not**
/// returned — they are the *tail*, beginning at
/// `chunks.last().map_or(start, |c| c.end)`, and are evaluated per query
/// by the row scan rather than indexed.
///
/// Boundaries are a deterministic function of the entry counts, so a
/// longer prefix yields the same chunks plus possibly more — never a
/// different split of the same data.
pub fn chunk_boundaries(frames: &[FrameBoundary], start: u64, min_entries: u64) -> Vec<ChunkBoundary> {
    // `min_entries == 0` would make every frame its own chunk —
    // degenerate, and contrary to the >=16K design intent. The caller's
    // threshold is a config knob, so guard it in debug rather than at
    // runtime cost.
    debug_assert!(min_entries > 0, "min_entries must be positive");

    let mut chunks = Vec::new();
    let mut chunk_start = start;
    let mut acc: u64 = 0;
    for f in frames {
        acc += u64::from(f.entry_count);
        if acc >= min_entries {
            chunks.push(ChunkBoundary {
                index: chunks.len() as u32,
                start: chunk_start,
                end: f.end_offset,
                entry_count: acc,
            });
            chunk_start = f.end_offset;
            acc = 0;
        }
    }
    chunks
}

/// The byte offset where the tail begins for a chunk list produced by
/// [`chunk_boundaries`] over the same `start`: the end of the last
/// complete chunk, or `start` when there are none.
pub fn tail_start(chunks: &[ChunkBoundary], start: u64) -> u64 {
    chunks.last().map_or(start, |c| c.end)
}

/// A process-wide memo of built chunk SFST byte images.
///
/// Keyed `(wal_seq, chunk_index)`. Values are `Arc<Vec<u8>>` — a
/// self-contained SFST parseable by `sfst::IndexReader::open`. The cache
/// owns build singleflight (one build per key under contention) and a
/// byte-budget LRU; it does **not** know how to build a chunk — the
/// caller passes the build future, so the same cache serves production
/// (`sfst::index_range` on a blocking thread) and tests (a canned
/// builder).
pub struct ChunkCache {
    cache: Cache<ChunkKey, Arc<Vec<u8>>>,
    /// `wal_seq -> number of chunk indices ever built for it`, so
    /// [`drop_seq`](Self::drop_seq) can invalidate each key by hand
    /// (per-key `invalidate` is immediately consistent, unlike the
    /// predicate-based bulk invalidation).
    ///
    /// May **overcount** relative to moka's live contents — moka can
    /// evict a chunk under byte pressure that this still tracks — but
    /// never undercounts: every successful build is recorded. Overcount
    /// is benign: [`drop_seq`](Self::drop_seq) invalidating an
    /// already-evicted key is a no-op. Entries are removed only by
    /// `drop_seq`; a `wal_seq` that rotates without one (an M4 contract
    /// violation) leaks a single `u64 -> u32` until restart.
    built: Mutex<HashMap<u64, u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChunkKey {
    wal_seq: u64,
    chunk_index: u32,
}

impl ChunkCache {
    /// Create a cache bounded to roughly `max_bytes` of chunk images
    /// (LRU eviction by serialized size). Eviction is safe at any time:
    /// a chunk is a pure function of immutable WAL bytes, so an evicted
    /// chunk simply rebuilds on its next request.
    pub fn new(max_bytes: u64) -> Self {
        // A budget below one chunk would thrash (rebuild every query).
        // The caller should size it well above a single chunk (the M4/M5
        // config picks the value); guard the obviously-broken zero.
        debug_assert!(max_bytes > 0, "ChunkCache budget must be positive");
        let cache = Cache::builder()
            .max_capacity(max_bytes)
            .weigher(|_k: &ChunkKey, v: &Arc<Vec<u8>>| v.len().min(u32::MAX as usize) as u32)
            .build();
        Self {
            cache,
            built: Mutex::new(HashMap::new()),
        }
    }

    /// Return chunk `(wal_seq, chunk_index)`'s bytes, building them with
    /// `init` only if absent. Under contention for the same key exactly
    /// one `init` runs and the rest await its result; a build error is
    /// **not** cached (a later request retries). The error is returned
    /// as `Arc<E>` because `moka` shares one error across all waiters.
    pub async fn get_or_build<E>(
        &self,
        wal_seq: u64,
        chunk_index: u32,
        init: impl Future<Output = Result<Arc<Vec<u8>>, E>>,
    ) -> Result<Arc<Vec<u8>>, Arc<E>>
    where
        E: Send + Sync + 'static,
    {
        let key = ChunkKey {
            wal_seq,
            chunk_index,
        };
        let bytes = self.cache.try_get_with(key, init).await?;
        // Record the index so drop_seq can find it later. Idempotent —
        // a cache hit re-records the same max.
        //
        // RACE: a query that began before rotation can resolve its
        // try_get_with after drop_seq(wal_seq) already ran, re-inserting
        // the seq here. Benign: the chunk is correct (immutable WAL
        // bytes, and the file still exists — SFST registration precedes
        // WAL deletion), and the re-acquired `built` entry is bounded to
        // one per affected rotation (LRU reclaims the chunk memory; no
        // further drop_seq occurs for a rotated seq). M4's contract that
        // no new query targets a rotated seq keeps this window unreached
        // in steady state.
        {
            let mut built = self.built.lock().unwrap();
            let n = built.entry(wal_seq).or_insert(0);
            *n = (*n).max(chunk_index + 1);
        }
        Ok(bytes)
    }

    /// Drop every chunk of `wal_seq` — called when the WAL rotates and
    /// its authoritative SFST is registered, so the chunks are
    /// superseded. Per-key invalidation is immediately consistent, so a
    /// query starting after this never sees a stale chunk; an in-flight
    /// query keeps the chunk bytes alive through its own `Arc` clone.
    pub async fn drop_seq(&self, wal_seq: u64) {
        let count = self.built.lock().unwrap().remove(&wal_seq);
        if let Some(count) = count {
            for chunk_index in 0..count {
                self.cache
                    .invalidate(&ChunkKey {
                        wal_seq,
                        chunk_index,
                    })
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
