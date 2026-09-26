//! Turning a captured source into bytes the reader can open, once per
//! request, or into the reason it cannot be read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use super::super::sources::TraceSource;
use super::super::window::TimeWindow;
use crate::source::{Mapped, map_source};
use crate::status::{PartialReason, StatusBuilder};

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
    /// Bytes to read: a mapped sealed file, a WAL chunk image, or the live
    /// tail built into an image. A source of a live WAL is prepared even
    /// when it holds no row of the window (`in_window` false): the live pass
    /// derives over every row of its WAL.
    Open { mapped: Mapped, in_window: bool },
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
            let in_window = overlaps(&candidate.summary);
            if !in_window && candidate.coverage.is_none() {
                return Prepared::Outside;
            }
            match map_source(&candidate.source) {
                Ok(mapped) => Prepared::Open { mapped, in_window },
                Err(e) => Prepared::Failed(e.to_string()),
            }
        }
        TraceSource::Tail(tail) => {
            match ng_index::build_sfst_traces_range(&tail.path, tail.coverage.range) {
                Ok((summary, bytes)) => Prepared::Open {
                    mapped: Mapped::Memory(Arc::new(bytes)),
                    in_window: overlaps(&summary),
                },
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

/// Runs `lane` on up to `workers` threads (on the calling thread for one)
/// for `jobs` jobs, and collects what each lane returns.
fn on_lanes<T: Send>(workers: usize, jobs: usize, lane: impl Fn() -> T + Sync) -> Vec<T> {
    let lanes = workers.min(jobs);
    if lanes <= 1 {
        return vec![lane()];
    }
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
}

/// Prepare every source once, over up to `workers` threads, in `sources`
/// order. `None` when cancelled.
pub(super) fn prepare_all(
    sources: &[TraceSource],
    window: &TimeWindow,
    workers: usize,
    cancel: &CancellationToken,
) -> Option<Vec<Prepared>> {
    let next = AtomicUsize::new(0);
    let parts = on_lanes(workers, sources.len(), || {
        let mut part = Vec::new();
        while !cancel.is_cancelled() {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(source) = sources.get(index) else {
                break;
            };
            part.push((index, prepare(source, window)));
        }
        part
    });
    if cancel.is_cancelled() {
        return None;
    }
    let mut prepared: Vec<(usize, Prepared)> = parts.into_iter().flatten().collect();
    prepared.sort_by_key(|(index, _)| *index);
    Some(prepared.into_iter().map(|(_, prepared)| prepared).collect())
}

/// Hand each prepared source that holds rows of the window to `open` with its
/// index in `sources`, over up to `workers` threads, and count the rest. Each
/// thread folds into its own accumulator from `init` and its own tally; the
/// caller merges them. `progress` ticks once per source. `None` when
/// cancelled: a partial answer would depend on which sources were read.
pub(super) fn evaluate_prepared<A: Send>(
    sources: &[TraceSource],
    prepared: &[Prepared],
    workers: usize,
    cancel: &CancellationToken,
    progress: &AtomicUsize,
    init: impl Fn() -> A + Sync,
    open: impl Fn(&mut A, &mut SourceTally, usize, Mapped) + Sync,
) -> Option<Vec<(SourceTally, A)>> {
    let next = AtomicUsize::new(0);
    let folded = on_lanes(workers, sources.len(), || {
        let mut tally = SourceTally::default();
        let mut acc = init();
        while !cancel.is_cancelled() {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let (Some(source), Some(prepared)) = (sources.get(index), prepared.get(index)) else {
                break;
            };
            match prepared {
                Prepared::Outside
                | Prepared::Open {
                    in_window: false, ..
                } => {}
                Prepared::Unavailable => {
                    tally.candidates += 1;
                    tally.unavailable += 1;
                }
                Prepared::Failed(error) => {
                    tally.candidates += 1;
                    tally.failed += 1;
                    tracing::warn!("sfsq traces: source {} failed: {error}", source.source_id());
                }
                Prepared::Open {
                    mapped,
                    in_window: true,
                } => {
                    tally.candidates += 1;
                    open(&mut acc, &mut tally, index, mapped.clone());
                }
            }
            progress.fetch_add(1, Ordering::Relaxed);
        }
        (tally, acc)
    });
    if cancel.is_cancelled() {
        return None;
    }
    Some(folded)
}

/// [`prepare_all`] then [`evaluate_prepared`], for requests without a live
/// pass.
pub(super) fn evaluate_sources<A: Send>(
    sources: &[TraceSource],
    window: &TimeWindow,
    workers: usize,
    cancel: &CancellationToken,
    progress: &AtomicUsize,
    init: impl Fn() -> A + Sync,
    open: impl Fn(&mut A, &mut SourceTally, usize, Mapped) + Sync,
) -> Option<Vec<(SourceTally, A)>> {
    let prepared = prepare_all(sources, window, workers, cancel)?;
    evaluate_prepared(sources, &prepared, workers, cancel, progress, init, open)
}
