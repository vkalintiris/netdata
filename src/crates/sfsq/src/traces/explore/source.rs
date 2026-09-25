//! Turning a captured source into bytes the reader can open, once per
//! request, or into the reason it cannot be read.

use std::sync::Arc;

use super::super::sources::TraceSource;
use super::super::window::TimeWindow;
use crate::source::{Mapped, map_source};

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
