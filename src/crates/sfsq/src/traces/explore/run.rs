//! Orchestration: prepare every source once, evaluate the readable ones,
//! count the rest, and assemble the sections.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use super::query::{ExploreQuery, ExploreRequestError};
use super::shard::{Evaluated, evaluate};
use super::source::{Prepared, prepare};
use super::{ExploreData, HistogramData, StackBucket, Totals};
use crate::logs::merge::merge_timelines;
use crate::traces::{PartialReason, StatusBuilder, TimeWindow, TraceSource, validate_sources};

/// Answer an explorer request over `sources`.
///
/// Pure sync: reads and decompresses files and builds the live tail's image;
/// run it off any async runtime thread. `progress` ticks once per source.
/// Cancellation is all-or-empty: a cancelled call returns no sections and
/// the `cancelled` reason.
pub fn explore(
    sources: Vec<TraceSource>,
    query: ExploreQuery,
    cancel: CancellationToken,
    progress: Arc<AtomicUsize>,
) -> Result<ExploreData, ExploreRequestError> {
    validate_sources(&sources)?;
    query.validate()?;
    let window_ns = query.grid.range_ns();
    let window = TimeWindow::new(window_ns.start, window_ns.end)?;
    let buckets = query.grid.num_buckets;

    let mut candidates = 0u64;
    let mut failed = 0u64;
    let mut legacy = 0u64;
    let mut unavailable = 0u64;
    let mut stack_high = 0u64;
    let mut matched = 0u64;
    let mut errors = 0u64;
    let mut timelines = Vec::new();
    let mut other = vec![0u64; buckets];

    for source in &sources {
        if cancel.is_cancelled() {
            return Ok(ExploreData::cancelled());
        }
        match prepare(source, &window) {
            Prepared::Outside => {}
            Prepared::Unavailable => {
                candidates += 1;
                unavailable += 1;
            }
            Prepared::Failed(error) => {
                candidates += 1;
                failed += 1;
                tracing::warn!("sfsq traces: source {} failed: {error}", source.source_id());
            }
            Prepared::Open(mapped) => {
                candidates += 1;
                match evaluate(mapped.bytes(), &query) {
                    Ok(Evaluated::Legacy) => legacy += 1,
                    Ok(Evaluated::Shard(shard)) => {
                        matched += shard.matched;
                        errors += shard.errors;
                        if let Some(timeline) = shard.timeline {
                            timelines.push(timeline);
                        }
                        if shard.stack_high {
                            stack_high += 1;
                            for (sum, n) in other.iter_mut().zip(&shard.other) {
                                *sum += n;
                            }
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        tracing::warn!(
                            "sfsq traces: source {} failed to evaluate: {e}",
                            source.source_id()
                        );
                    }
                }
            }
        }
        progress.fetch_add(1, Ordering::Relaxed);
    }
    if cancel.is_cancelled() {
        return Ok(ExploreData::cancelled());
    }

    let mut status = StatusBuilder::new();
    for (reason, count) in [
        (PartialReason::SourceFailure, failed),
        (PartialReason::RemoteUnavailable, unavailable),
        (PartialReason::LegacyFile, legacy),
        (PartialReason::StackFieldHighCard, stack_high),
    ] {
        status.add_n(reason, count);
        status.of(reason, candidates);
    }

    let histogram = query.sections.histogram.map(|spec| {
        if stack_high > 0 {
            status.detail(PartialReason::StackFieldHighCard, spec.stack.clone());
        }
        let (dimensions, stacked) = match merge_timelines(timelines) {
            Some(timeline) => (timeline.dimensions, timeline.buckets),
            None => (Vec::new(), Vec::new()),
        };
        let mut out = Vec::with_capacity(buckets);
        for (index, other) in other.into_iter().enumerate() {
            let (counts, unset) = match stacked.get(index) {
                Some(bucket) => (bucket.counts.clone(), bucket.unset),
                None => (vec![0; dimensions.len()], 0),
            };
            out.push(StackBucket {
                counts,
                unset,
                other,
            });
        }
        HistogramData {
            stack: spec.stack,
            dimensions,
            buckets: out,
            totals: Totals {
                count: matched,
                errors,
            },
        }
    });

    Ok(ExploreData {
        status: status.finish(),
        sources: candidates,
        histogram,
    })
}
