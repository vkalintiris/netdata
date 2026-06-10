use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal::FrameBoundary;

use super::{ChunkCache, chunk_boundaries, tail_start};

// ── chunk_boundaries (pure) ───────────────────────────────────────

fn fb(end_offset: u64, entry_count: u32) -> FrameBoundary {
    FrameBoundary {
        end_offset,
        entry_count,
    }
}

#[test]
fn groups_frames_into_threshold_chunks() {
    // Threshold 10: frame counts 8,8,8,8 → chunk0 = frames[0,1] (16),
    // chunk1 = frames[2,3] (16). Offsets are the frames' ends.
    let frames = [fb(100, 8), fb(200, 8), fb(300, 8), fb(400, 8)];
    let chunks = chunk_boundaries(&frames, 50, 10);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].index, 0);
    assert_eq!((chunks[0].range.start(), chunks[0].range.end(), chunks[0].entry_count), (50, 200, 16));
    assert_eq!(chunks[1].index, 1);
    assert_eq!((chunks[1].range.start(), chunks[1].range.end(), chunks[1].entry_count), (200, 400, 16));
    // No leftover: the tail starts at the last chunk's end.
    assert_eq!(tail_start(&chunks, 50), 400);
}

#[test]
fn last_frame_pushes_a_chunk_over_the_threshold() {
    // 5,3,7 with threshold 10: cumulative crosses 10 only at frame 2.
    let frames = [fb(100, 5), fb(150, 3), fb(230, 7)];
    let chunks = chunk_boundaries(&frames, 0, 10);
    assert_eq!(chunks.len(), 1);
    assert_eq!((chunks[0].range.start(), chunks[0].range.end(), chunks[0].entry_count), (0, 230, 15));
}

#[test]
fn frames_below_threshold_are_all_tail() {
    let frames = [fb(100, 5), fb(150, 3)];
    let chunks = chunk_boundaries(&frames, 64, 10);
    assert!(chunks.is_empty());
    // The whole range is tail, starting where the scan started.
    assert_eq!(tail_start(&chunks, 64), 64);
}

#[test]
fn empty_scan_yields_no_chunks() {
    let chunks = chunk_boundaries(&[], 64, 10);
    assert!(chunks.is_empty());
    assert_eq!(tail_start(&chunks, 64), 64);
}

#[test]
fn longer_prefix_only_appends_chunks() {
    // The first two frames must produce the same chunk whether or not
    // more frames follow — boundaries are append-only.
    let short = [fb(100, 6), fb(200, 6)];
    let long = [fb(100, 6), fb(200, 6), fb(300, 6), fb(400, 6)];
    let c_short = chunk_boundaries(&short, 0, 10);
    let c_long = chunk_boundaries(&long, 0, 10);
    assert_eq!(c_short.len(), 1);
    assert_eq!(c_long.len(), 2);
    assert_eq!(c_short[0], c_long[0]); // identical first chunk
}

// ── ChunkCache (async, fake builder) ──────────────────────────────

#[derive(Debug, PartialEq, Eq)]
struct BuildErr;

/// An init future that records each time it actually runs, sleeps to
/// widen the singleflight window, then yields some bytes.
fn counting_build(
    counter: Arc<AtomicUsize>,
    bytes: Vec<u8>,
) -> impl std::future::Future<Output = Result<Arc<Vec<u8>>, BuildErr>> {
    async move {
        counter.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok::<Arc<Vec<u8>>, BuildErr>(Arc::new(bytes))
    }
}

fn cache() -> Arc<ChunkCache> {
    Arc::new(ChunkCache::new(64 * 1024 * 1024))
}

#[tokio::test]
async fn concurrent_requests_build_a_chunk_once() {
    let cache = cache();
    let builds = Arc::new(AtomicUsize::new(0));

    // 8 tasks race for the same (seq, index); exactly one build runs.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let c = cache.clone();
        let b = builds.clone();
        handles.push(tokio::spawn(async move {
            c.get_or_build(7, 0, counting_build(b, vec![1, 2, 3])).await
        }));
    }
    for h in handles {
        let bytes = h.await.unwrap().unwrap();
        assert_eq!(*bytes, vec![1, 2, 3]);
    }
    assert_eq!(builds.load(Ordering::SeqCst), 1, "singleflight: one build");
}

#[tokio::test]
async fn distinct_keys_build_independently() {
    let cache = cache();
    let builds = Arc::new(AtomicUsize::new(0));

    cache
        .get_or_build(7, 0, counting_build(builds.clone(), vec![0]))
        .await
        .unwrap();
    cache
        .get_or_build(7, 1, counting_build(builds.clone(), vec![1]))
        .await
        .unwrap();
    // Different WAL, same chunk index: still distinct.
    cache
        .get_or_build(8, 0, counting_build(builds.clone(), vec![2]))
        .await
        .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 3);

    // A repeat of an existing key is served from cache — no rebuild.
    let bytes = cache
        .get_or_build(7, 0, counting_build(builds.clone(), vec![9]))
        .await
        .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 3, "cache hit, no rebuild");
    assert_eq!(*bytes, vec![0], "served the originally built bytes");
}

#[tokio::test]
async fn build_errors_are_not_cached() {
    let cache = cache();
    let attempts = Arc::new(AtomicUsize::new(0));

    let failing = |attempts: Arc<AtomicUsize>| async move {
        attempts.fetch_add(1, Ordering::SeqCst);
        Err::<Arc<Vec<u8>>, BuildErr>(BuildErr)
    };

    let r1 = cache.get_or_build(7, 0, failing(attempts.clone())).await;
    assert!(r1.is_err());
    // A second request rebuilds rather than returning a cached error.
    let r2 = cache.get_or_build(7, 0, failing(attempts.clone())).await;
    assert!(r2.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 2, "error not cached");

    // And a later success populates the entry normally.
    let builds = Arc::new(AtomicUsize::new(0));
    let ok = cache
        .get_or_build(7, 0, counting_build(builds.clone(), vec![5]))
        .await
        .unwrap();
    assert_eq!(*ok, vec![5]);
}

#[tokio::test]
async fn drop_seq_unknown_is_a_noop() {
    let cache = cache();
    // Never built anything for seq 99 — dropping it must not panic.
    cache.drop_seq(99).await;
}

#[tokio::test]
async fn drop_seq_invalidates_every_chunk_of_a_wal() {
    let cache = cache();
    let builds = Arc::new(AtomicUsize::new(0));

    cache
        .get_or_build(7, 0, counting_build(builds.clone(), vec![0]))
        .await
        .unwrap();
    cache
        .get_or_build(7, 1, counting_build(builds.clone(), vec![1]))
        .await
        .unwrap();
    // A different WAL's chunk must survive the drop.
    cache
        .get_or_build(8, 0, counting_build(builds.clone(), vec![8]))
        .await
        .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 3);

    cache.drop_seq(7).await;

    // seq 7's chunks rebuild (immediately consistent invalidation)...
    cache
        .get_or_build(7, 0, counting_build(builds.clone(), vec![0]))
        .await
        .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 4, "dropped chunk rebuilt");
    // ...while seq 8's chunk is still cached.
    cache
        .get_or_build(8, 0, counting_build(builds.clone(), vec![8]))
        .await
        .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 4, "other WAL untouched");
}
