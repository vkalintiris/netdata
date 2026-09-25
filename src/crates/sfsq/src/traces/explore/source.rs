//! Turning a captured source into bytes the reader can open, once per
//! request, or into the reason it cannot be read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use super::super::sources::TraceSource;
use super::super::status::{PartialReason, StatusBuilder};
use super::super::window::TimeWindow;
use crate::source::{Mapped, map_source};

/// Sources that may hold rows for the window, and how many of them could not
/// be counted in, by reason.
#[derive(Debug, Default)]
pub(super) struct SourceTally {
    pub candidates: u64,
    pub failed: u64,
    pub unavailable: u64,
    pub legacy: u64,
}

impl SourceTally {
    pub fn add(&mut self, other: &SourceTally) {
        self.candidates += other.candidates;
        self.failed += other.failed;
        self.unavailable += other.unavailable;
        self.legacy += other.legacy;
    }

    /// The reasons that hold for every section: each count out of the
    /// candidates.
    pub fn status(&self) -> StatusBuilder {
        let mut status = StatusBuilder::new();
        for (reason, count) in [
            (PartialReason::SourceFailure, self.failed),
            (PartialReason::RemoteUnavailable, self.unavailable),
            (PartialReason::LegacyFile, self.legacy),
        ] {
            status.add_n(reason, count);
            status.of(reason, self.candidates);
        }
        status
    }
}

/// What one captured source contributes to a request.
pub(super) enum Prepared {
    /// Bytes to evaluate: a mapped sealed file, a WAL chunk image, or the
    /// live tail built into an image.
    Open(Mapped),
    /// Holds data for the window but cannot be read.
    Failed(String),
    /// A remote file whose bytes could not be obtained.
    Unavailable,
    /// Holds no data for the window.
    Outside,
}

/// Prepare `source` for `window`. The live tail is built into an SFST image
/// with the seal's own builder, so it is evaluated exactly like a sealed
/// file: same tokens (events and links included), same tiers, same indexes.
pub(super) fn prepare(source: &TraceSource, window: &TimeWindow) -> Prepared {
    let overlaps = |summary: &sfst::Summary| {
        window.overlaps_summary(summary.min_timestamp_s, summary.max_timestamp_s)
    };
    match source {
        TraceSource::Sfst(candidate) => {
            if !overlaps(&candidate.summary) {
                return Prepared::Outside;
            }
            match map_source(&candidate.source) {
                Ok(mapped) => Prepared::Open(mapped),
                Err(e) => Prepared::Failed(e.to_string()),
            }
        }
        TraceSource::Tail(tail) => {
            match ng_index::build_sfst_traces_range(&tail.path, tail.coverage.range) {
                Ok((summary, bytes)) if overlaps(&summary) => {
                    Prepared::Open(Mapped::Memory(Arc::new(bytes)))
                }
                Ok(_) => Prepared::Outside,
                Err(e) => Prepared::Failed(format!("building the tail image: {e}")),
            }
        }
        TraceSource::Unavailable(unavailable) => {
            if overlaps(&unavailable.summary) {
                Prepared::Unavailable
            } else {
                Prepared::Outside
            }
        }
        TraceSource::Failed(failed) => Prepared::Failed(failed.error.clone()),
    }
}

/// Whether `source` is a sealed file (not a chunk image or tail of a live WAL).
pub(super) fn is_sealed(source: &TraceSource) -> bool {
    matches!(source, TraceSource::Sfst(candidate) if candidate.coverage.is_none())
}

/// How a request spreads its sources over threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExploreOptions {
    /// Sources evaluated at once; 1 evaluates them on the calling thread.
    pub workers: usize,
}

impl Default for ExploreOptions {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        ExploreOptions {
            workers: cores.min(4),
        }
    }
}

/// Prepare every source and hand each readable one to `open` with its index
/// in `sources`, over up to `workers` threads. Each thread folds into its own
/// accumulator from `init` and its own tally; the caller merges them. `None`
/// when cancelled: a partial answer would depend on which sources were read.
pub(super) fn evaluate_sources<A: Send>(
    sources: &[TraceSource],
    window: &TimeWindow,
    workers: usize,
    cancel: &CancellationToken,
    progress: &AtomicUsize,
    init: impl Fn() -> A + Sync,
    open: impl Fn(&mut A, &mut SourceTally, usize, Mapped) + Sync,
) -> Option<Vec<(SourceTally, A)>> {
    let next = AtomicUsize::new(0);
    let lane = || {
        let mut tally = SourceTally::default();
        let mut acc = init();
        while !cancel.is_cancelled() {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(source) = sources.get(index) else {
                break;
            };
            match prepare(source, window) {
                Prepared::Outside => {}
                Prepared::Unavailable => {
                    tally.candidates += 1;
                    tally.unavailable += 1;
                }
                Prepared::Failed(error) => {
                    tally.candidates += 1;
                    tally.failed += 1;
                    tracing::warn!("sfsq traces: source {} failed: {error}", source.source_id());
                }
                Prepared::Open(mapped) => {
                    tally.candidates += 1;
                    open(&mut acc, &mut tally, index, mapped);
                }
            }
            progress.fetch_add(1, Ordering::Relaxed);
        }
        (tally, acc)
    };

    let lanes = workers.min(sources.len());
    let folded = if lanes <= 1 {
        vec![lane()]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..lanes).map(|_| scope.spawn(&lane)).collect();
            let mut folded = Vec::with_capacity(handles.len());
            for handle in handles {
                match handle.join() {
                    Ok(result) => folded.push(result),
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }
            folded
        })
    };
    if cancel.is_cancelled() {
        return None;
    }
    Some(folded)
}
