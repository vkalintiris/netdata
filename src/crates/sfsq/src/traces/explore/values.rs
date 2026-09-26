//! Value suggestions for the search box: the distinct stored values of one
//! field that start with a prefix.
//!
//! Each file that may hold rows for the window contributes from its dictionary
//! for the field (never its rows), so a file overlapping the window contributes
//! values of rows outside the window too. A file gives at most the first
//! `limit + 1` values with the prefix, so memory stays at the limit however
//! many values a field has. The live tail is read through its image, like
//! everywhere in the explorer; the error origins of a live WAL exist only
//! after the live pass, which runs when they are asked for.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use tokio_util::sync::CancellationToken;

use super::live::live_pass;
use super::query::ExploreRequestError;
use super::shard::{is_legacy, open_source};
use super::source::{ExploreOptions, SourceTally, evaluate_prepared, is_sealed, prepare_all};
use crate::traces::{
    PartialReason, QueryStatus, StatusBuilder, TimeWindow, TraceSource, validate_sources,
};
use sfst::ERR_ORIGIN_FIELD;

/// Most values one request may ask for.
pub const VALUES_LIMIT_MAX: usize = 1000;

/// A value-suggestion request.
pub struct ValuesQuery {
    /// Unix nanoseconds, `[start, end)`.
    pub window: std::ops::Range<i64>,
    /// A storage field name.
    pub field: String,
    /// Keep the values starting with these bytes (case-sensitive); empty
    /// keeps every value.
    pub prefix: String,
    /// 1 to [`VALUES_LIMIT_MAX`].
    pub limit: usize,
}

/// The first values in byte order, and whether more matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuesData {
    pub status: QueryStatus,
    pub values: Vec<String>,
    pub truncated: bool,
}

impl ValuesData {
    fn cancelled() -> Self {
        let mut status = StatusBuilder::new();
        status.add(PartialReason::Cancelled);
        ValuesData {
            status: status.finish(),
            values: Vec::new(),
            truncated: false,
        }
    }
}

/// Suggest values of `query.field` over `sources`.
///
/// Pure sync, like [`explore`](super::explore), over up to `options.workers`
/// threads. Cancellation is all-or-empty: a partial union would depend on the
/// order the sources were read.
pub fn field_values(
    sources: Vec<TraceSource>,
    query: ValuesQuery,
    options: ExploreOptions,
    cancel: CancellationToken,
    progress: Arc<AtomicUsize>,
) -> Result<ValuesData, ExploreRequestError> {
    validate_sources(&sources)?;
    if query.field.is_empty() || query.limit == 0 || query.limit > VALUES_LIMIT_MAX {
        return Err(ExploreRequestError::Invalid(format!(
            "values need a field and a limit of 1 to {VALUES_LIMIT_MAX}"
        )));
    }
    let window = TimeWindow::new(query.window.start, query.window.end)?;

    // The smallest `limit + 1` values: one more than returned tells whether
    // the answer is truncated.
    let cap = query.limit + 1;
    let Some(prepared) = prepare_all(&sources, &window, options.workers, &cancel) else {
        return Ok(ValuesData::cancelled());
    };
    let live = (query.field == ERR_ORIGIN_FIELD).then(|| live_pass(&sources, &prepared));
    let Some(lanes) = evaluate_prepared(
        &sources,
        &prepared,
        options.workers,
        &cancel,
        &progress,
        BTreeSet::new,
        |kept, tally, index, mapped| match source_values(
            mapped.bytes(),
            live.as_ref().and_then(|live| live.derived[index].as_ref()),
            &query,
            cap,
            is_sealed(&sources[index]),
        ) {
            Ok(None) => tally.legacy += 1,
            Ok(Some(values)) => {
                for value in values {
                    keep(kept, value, cap);
                }
            }
            Err(e) => {
                tally.failed += 1;
                tracing::warn!(
                    "sfsq traces: source {} failed to list {}: {e}",
                    sources[index].source_id(),
                    query.field
                );
            }
        },
    ) else {
        return Ok(ValuesData::cancelled());
    };
    let mut tally = SourceTally::default();
    let mut kept = BTreeSet::new();
    for (lane_tally, lane_kept) in lanes {
        tally.add(&lane_tally);
        for value in lane_kept {
            keep(&mut kept, value, cap);
        }
    }

    let truncated = kept.len() > query.limit;
    let mut values = Vec::with_capacity(kept.len());
    for value in kept.into_iter().take(query.limit) {
        values.push(value);
    }
    let mut status = tally.status();
    if let Some(live) = live {
        status.add_n(PartialReason::LivePassFailed, live.failed);
        status.of(PartialReason::LivePassFailed, live.wals);
    }
    Ok(ValuesData {
        status: status.finish(),
        values,
        truncated,
    })
}

/// One file's first `cap` values of the field with the prefix, in byte order;
/// `None` for a legacy file.
fn source_values(
    bytes: &[u8],
    derived: Option<&Arc<sfst::DerivedValues>>,
    query: &ValuesQuery,
    cap: usize,
    sealed: bool,
) -> Result<Option<Vec<String>>, sfst::Error> {
    let reader = open_source(bytes, derived)?;
    if is_legacy(&reader, sealed) {
        return Ok(None);
    }
    if !reader.field_table().contains(&query.field) {
        return Ok(Some(Vec::new()));
    }
    let values = reader.field_values_with_prefix(&query.field, &query.prefix, cap)?;
    Ok(Some(values))
}

/// Insert `value` into `kept`, which holds at most `cap` of the smallest values.
fn keep(kept: &mut BTreeSet<String>, value: String, cap: usize) {
    if kept.len() == cap && kept.last().is_some_and(|last| value >= *last) {
        return;
    }
    kept.insert(value);
    if kept.len() > cap {
        kept.pop_last();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_holds_the_smallest_values() {
        let mut kept = BTreeSet::new();
        for value in ["d", "b", "e", "a", "b", "c"] {
            keep(&mut kept, value.to_string(), 3);
        }
        assert_eq!(kept.into_iter().collect::<Vec<_>>(), ["a", "b", "c"]);
    }
}
