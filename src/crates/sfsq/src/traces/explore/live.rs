//! The live pass. A sealed file stores the values the seal derives over the
//! rows of its WAL (error origins, the time children cover); a live WAL's
//! chunk images and tail store neither. The pass derives them at query time,
//! over every row of each live WAL the request captured, with the seal's own
//! derivation, and hands each image its share to attach: the images then
//! read like the sealed file those frames will become.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::query::STATUS_FIELD;
use super::source::Prepared;
use crate::traces::TraceSource;

/// The live pass over a request's sources.
pub(super) struct LivePass {
    /// Per source: the values to attach, for the images of a WAL whose pass
    /// ran.
    pub derived: Vec<Option<Arc<sfst::DerivedValues>>>,
    /// Live WALs whose pass could not run, out of all the live WALs captured.
    pub failed: u64,
    pub wals: u64,
}

/// Runs the pass for every live WAL among `sources`. A WAL fails as a whole:
/// its captured ranges must run contiguously from the WAL's first frame, and
/// every image must be readable and store no derived value.
pub(super) fn live_pass(sources: &[TraceSource], prepared: &[Prepared]) -> LivePass {
    let mut wals: BTreeMap<&str, Vec<(u64, u64, usize)>> = BTreeMap::new();
    for (index, source) in sources.iter().enumerate() {
        if let Some(coverage) = source.coverage() {
            let range = (coverage.range.start(), coverage.range.end(), index);
            wals.entry(&coverage.wal_id).or_default().push(range);
        }
    }
    let mut pass = LivePass {
        derived: vec![None; sources.len()],
        failed: 0,
        wals: wals.len() as u64,
    };
    for (wal, mut members) in wals {
        members.sort_unstable();
        match derive_wal(&members, prepared) {
            Ok(values) => {
                for (&(_, _, index), values) in members.iter().zip(values) {
                    pass.derived[index] = Some(Arc::new(values));
                }
            }
            Err(why) => {
                pass.failed += 1;
                tracing::warn!("sfsq traces: the live pass over {wal} failed: {why}");
            }
        }
    }
    pass
}

/// The derived values of one WAL's images (`(start, end, source)` in byte
/// order), one per image in that order.
fn derive_wal(
    members: &[(u64, u64, usize)],
    prepared: &[Prepared],
) -> Result<Vec<sfst::DerivedValues>, String> {
    let mut expected = wal::HEADER_SIZE as u64;
    for &(start, end, _) in members {
        if start != expected {
            return Err(format!(
                "the captured ranges leave a gap at byte {expected}"
            ));
        }
        expected = end;
    }

    let mut trace_ids = sfst::TraceIds::default();
    let mut span_ids = sfst::SpanIds::default();
    let mut parent_span_ids = sfst::ParentSpanIds::default();
    let mut starts: Vec<i64> = Vec::new();
    let mut durations: Vec<i64> = Vec::new();
    let mut errors: Vec<bool> = Vec::new();
    let mut lengths = Vec::with_capacity(members.len());
    let errors_only = sfst::Filter::new().select(STATUS_FIELD, "ERROR");
    for &(_, _, index) in members {
        let Some(Prepared::Open { mapped, .. }) = prepared.get(index) else {
            return Err("one of its images could not be read".to_string());
        };
        let reader = sfst::IndexReader::open(mapped.bytes()).map_err(|e| e.to_string())?;
        if reader.stores_derived() {
            return Err("an image already stores derived values".to_string());
        }
        let rows = reader.summary().record_count as usize;
        let read = |e: sfst::Error| e.to_string();
        for id in reader.trace_ids().map_err(read)?.iter() {
            trace_ids.push(id);
        }
        for id in reader.span_ids().map_err(read)?.iter() {
            span_ids.push(id);
        }
        for id in reader.parent_span_ids().map_err(read)?.iter() {
            parent_span_ids.push(id);
        }
        starts.extend_from_slice(reader.load_timestamps().map_err(read)?.as_slice());
        durations.extend_from_slice(&reader.durations().map_err(read)?.0);
        let first = errors.len();
        errors.resize(first + rows, false);
        let filter = reader.compile_filter(&errors_only, None).map_err(read)?;
        for position in reader
            .matched_positions(&filter, i64::MIN..i64::MAX)
            .map_err(read)?
        {
            errors[first + position as usize] = true;
        }
        lengths.push(rows);
    }

    let family = sfst::derive_span_family(&sfst::SpanRows {
        trace_ids: &trace_ids,
        span_ids: &span_ids,
        parent_span_ids: &parent_span_ids,
        start_ns: &starts,
        duration_ns: &durations,
        is_error: &errors,
    })
    .map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(lengths.len());
    let mut offset = 0usize;
    for rows in lengths {
        let mut err_origin = Vec::new();
        for (position, origin) in family.error_origin[offset..offset + rows]
            .iter()
            .enumerate()
        {
            if *origin {
                err_origin.push(position as u32);
            }
        }
        out.push(sfst::DerivedValues {
            err_origin,
            child_duration: sfst::ChildDurations(family.child_ns[offset..offset + rows].to_vec()),
        });
        offset += rows;
    }
    Ok(out)
}
