//! A replication request's answer (`src/streaming/stream-replication-sender.c`: `replication_execute_request()` with
//! `[db] replication prefetch = 1`, D105.3): the window normalized against the chart's retention, the exposed
//! dimensions' tier 0 walked step by step into `RBEGIN`/`RSET` lines up to a quarter of the sender's buffer, the
//! collection state when the answer starts streaming, `REND`, and the flip that ends the chart's replication. The
//! queue, the threads and the transport are the streaming crate's (D111.5). Map:
//! `knowledge/map-m7-commit6-replication.md` §4-§6 in the status repository.

use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log, nd_log_limit};
use netdata_agent_pluginsd_proto::caps;
use netdata_agent_pluginsd_proto::emit::stream::{self as emit, Enc};
use netdata_agent_storage::query::{Priority as QueryPriority, StorageQuery};
use netdata_agent_storage::storage_number::flags_text;
use netdata_agent_storage::storage_point::StoragePoint;

use super::LastCollected;
use crate::chart::{Chart, Dim, dim_flags, flags};
use crate::clock::now_realtime_s;
use crate::host::Host;
use crate::pulse::host_status;

/// `max_skip`: how many points a dimension may return behind the step before it is left out.
const MAX_SKIP: i32 = 1000;
/// Promoted to streaming when the request ends within this many update intervals of now.
const STREAMING_HORIZON_ITERATIONS: i64 = 100;

static SKIP_RECORD: ErrorLimit = ErrorLimit::new(1, 0);

/// A parent's request for one chart (`REPLAY_CHART`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub chart_id: String,
    pub after: i64,
    pub before: i64,
    pub start_streaming: bool,
}

/// What answering did, for the replication's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    /// The chart is not the host's: the empty answer that unblocks the parent (`error_not_found`).
    NotFound,
    /// Answered (`executed`).
    Executed,
}

/// The window the child answers for (`replication_response_prepare()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Window {
    pub(super) after: i64,
    pub(super) before: i64,
    pub(super) streaming: bool,
}

/// `replication_response_prepare()`'s rules: swapped when reversed, empty and streaming when either end is 0 or it
/// starts in the future, streaming up to now when it ends within 100 intervals of now, then clamped to the retention
/// and streaming up to its end when it reaches it.
pub(super) fn normalize(request: &Request, wall_s: i64, update_every: i64, db: (i64, i64)) -> Window {
    let (mut after, mut before, mut streaming) = (request.after, request.before, request.start_streaming);
    if after > before {
        std::mem::swap(&mut after, &mut before);
    }
    if after == 0 || before == 0 || after > wall_s {
        (after, before, streaming) = (0, 0, true);
    } else if before >= wall_s - update_every * STREAMING_HORIZON_ITERATIONS {
        (before, streaming) = (wall_s, true);
    }
    let (db_first, db_last) = db;
    if after != 0 && before != 0 {
        after = after.max(db_first);
        before = before.min(db_last);
        if after > before {
            std::mem::swap(&mut after, &mut before);
        }
        if streaming || before >= db_last {
            (before, streaming) = (db_last, true);
        }
    }
    Window { after, before, streaming }
}

/// What the walk reads of a dimension's query: the storage engines' in the agent, crafted points in the units.
pub(super) trait Points {
    fn next_metric(&mut self) -> StoragePoint;
    fn is_finished(&self) -> bool;
}

impl Points for StorageQuery<'_> {
    fn next_metric(&mut self) -> StoragePoint {
        StorageQuery::next_metric(self)
    }
    fn is_finished(&self) -> bool {
        StorageQuery::is_finished(self)
    }
}

/// One exposed dimension's tier 0 query (`struct replication_dimension`).
pub(super) struct DimQuery<'a, Q = StorageQuery<'a>> {
    pub(super) dim: &'a Dim,
    pub(super) query: Q,
    pub(super) sp: StoragePoint,
    pub(super) skip: bool,
}

/// What the walk did.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Walk {
    pub(super) finished_with_gap: bool,
    pub(super) points_read: usize,
    pub(super) points_generated: usize,
}

/// `replication_query_execute()`: from `after + 1`, one step at a time over the dimensions' shortest point, each
/// step an `RBEGIN ''` line and an `RSET` for every dimension that covers it; cut before a step once the answer
/// passes `max_msg_size`, which turns it into a partial one (`window.before` lowered, not streaming).
#[allow(clippy::too_many_arguments)]
pub(super) fn walk<Q: Points>(
    out: &mut Vec<u8>,
    enc: &Enc,
    host: &Host,
    chart: &Chart,
    dims: &mut [DimQuery<'_, Q>],
    window: &mut Window,
    wall_s: i64,
    interpolated: bool,
    max_msg_size: usize,
) -> Walk {
    let chart_update_every = i64::from(chart.update_every());
    let chart_slot = u64::from(chart.chart_slot());
    // the end the walk runs to: a gap before it lowers it, a cut only lowers the answer's
    let mut before = window.before;
    let mut w = Walk { finished_with_gap: false, points_read: 0, points_generated: 0 };
    let mut now = window.after + 1;
    let mut last_end_in_buffer = 0;
    while now <= before {
        let (mut min_start, mut max_start, mut min_end) = (0, 0, 0);
        let (mut min_ue, mut max_ue) = (0, 0);
        for d in dims.iter_mut().filter(|d| !d.skip) {
            // C's `max_skip-- >= 0`: up to 1001 reads, and exactly 1000 that end on a good point trip it too
            let mut max_skip = MAX_SKIP;
            while d.sp.end_time_s < now && !d.query.is_finished() && max_skip >= 0 {
                max_skip -= 1;
                d.sp = d.query.next_metric();
                w.points_read += 1;
            }
            if max_skip <= 0 {
                d.skip = true;
                nd_log_limit!(
                    &SKIP_RECORD,
                    Source::Daemon,
                    Priority::Err,
                    "STREAM SND REPLAY: 'host:{}/chart:{}/dim:{}': db does not advance the query beyond time {} \
                     (tried 1000 times to get the next point and always got back a point in the past)",
                    host.hostname(),
                    chart.id(),
                    d.dim.id(),
                    now
                );
                continue;
            }
            if d.sp.end_time_s < now || d.sp.end_time_s < d.sp.start_time_s {
                continue;
            }
            let mut ue = d.sp.end_time_s - d.sp.start_time_s;
            if ue == 0 {
                ue = chart_update_every;
            }
            if min_ue == 0 {
                min_ue = ue;
            }
            if min_start == 0 {
                min_start = d.sp.start_time_s;
            }
            if min_end == 0 {
                min_end = d.sp.end_time_s;
            }
            min_ue = min_ue.min(ue);
            max_ue = max_ue.max(ue);
            min_start = min_start.min(d.sp.start_time_s);
            max_start = max_start.max(d.sp.start_time_s);
            min_end = min_end.min(d.sp.end_time_s);
        }
        if min_ue != max_ue || min_start != max_start {
            // misaligned dimensions: the step starts where the last one ended, when that is among their starts
            min_start = if last_end_in_buffer != 0 && (min_start..=max_start).contains(&last_end_in_buffer) {
                last_end_in_buffer
            } else {
                min_end - min_ue
            };
        }
        if min_start <= now && min_end >= now {
            if min_end == min_start {
                min_start = min_end - chart_update_every;
            }
            if out.len() > max_msg_size && last_end_in_buffer != 0 {
                window.before = last_end_in_buffer;
                window.streaming = false;
                break;
            }
            last_end_in_buffer = min_end;
            emit::rbegin_point(out, enc, chart_slot, min_start, min_end, wall_s);
            for d in dims.iter() {
                if d.sp.start_time_s <= min_end && d.sp.end_time_s >= min_end && !d.sp.is_unset() && !d.sp.is_gap() {
                    emit::rset(
                        out,
                        enc,
                        u64::from(d.dim.slot()),
                        d.dim.id(),
                        d.sp.sum,
                        flags_text(d.sp.flags, interpolated),
                    );
                    w.points_generated += 1;
                }
            }
            now = min_end + 1;
        } else if min_end < now {
            break;
        } else {
            now = min_start;
            if min_start > before && w.points_generated == 0 {
                before = min_start - 1;
                window.before = before;
                w.finished_with_gap = true;
                break;
            }
        }
    }
    if last_end_in_buffer < before - chart_update_every {
        w.finished_with_gap = true;
    }
    w
}

/// `replication_query_align_to_optimal_before()`: a query that does not start streaming ends where the pages it
/// reads end, when that is later but within 1024 intervals, before the chart's last update and before now. C's
/// minimum restarts at a dimension that answers 0.
fn align_to_optimal_before(
    dims: &mut [DimQuery<'_>],
    window: &mut Window,
    chart: &Chart,
    last_updated_s: i64,
    wall_s: i64,
) {
    let mut expanded = 0;
    for d in dims.iter_mut() {
        let new_before = d.query.align_to_optimal_before();
        if expanded == 0 || new_before < expanded {
            expanded = new_before;
        }
    }
    let update_every = i64::from(chart.update_every());
    if expanded > window.before
        && (expanded - window.before) / update_every < 1024
        && expanded < last_updated_s
        && expanded < wall_s
    {
        window.before = expanded;
    }
}

/// `replication_send_chart_collection_state()`: each exposed dimension's collection state, then the chart's.
fn collection_state(out: &mut Vec<u8>, enc: &Enc, chart: &Chart, capabilities: u32) {
    for dim in chart.dims().iter().filter(|d| d.is_sent_upstream()) {
        let d = dim.collection();
        let last = LastCollected {
            int: d.last_collected_value,
            float: d.last_collected_value_float,
            is_float: dim.meta().flags & dim_flags::FLOAT != 0,
        };
        emit::rdstate(
            out,
            enc,
            u64::from(dim.slot()),
            dim.id(),
            micros(d.last_collected_time),
            last.baseline(capabilities),
            d.last_calculated_value,
            d.last_stored_value,
        );
    }
    let c = chart.collection();
    emit::rsstate(out, enc, micros(c.last_collected), micros(c.last_updated));
}

/// A `(seconds, microseconds)` time as C's `usec_t`.
fn micros((s, us): (i64, i64)) -> u64 {
    (s as u64).wrapping_mul(1_000_000).wrapping_add(us as u64)
}

/// The end of a chart's replication once an answer that starts streaming went into the request's session: the
/// host's claim given back (the last one marks it running) and, unless the answer ended with a gap, no more resync.
/// Only when the chart was not finished already.
fn finish(host: &Host, chart: &Chart, finished_with_gap: bool) {
    let old = chart.flags_set_and_clear(flags::SENDER_REPLICATION_FINISHED, flags::SENDER_REPLICATION_IN_PROGRESS);
    if old & flags::SENDER_REPLICATION_FINISHED != 0 {
        return;
    }
    if host.sender_replicating_charts_minus_one() == 0 {
        host.pulse_status(host_status::SND_RUNNING);
    }
    if !finished_with_gap {
        chart.set_resync_time_s(0);
    }
}

/// `replication_execute_request()`: answers `request` for the host in `out` (cleared first) with `capabilities`, a
/// partial answer once it passes `max_msg_size`; `commit` sends it and tells whether it went into the request's
/// session (the sender's buffer not flushed since the request came). An answer that starts streaming holds the
/// chart's collection lock from its query to the flip that ends the replication.
pub fn answer(
    host: &Host,
    request: &Request,
    capabilities: u32,
    max_msg_size: usize,
    out: &mut Vec<u8>,
    commit: impl FnOnce(&[u8]) -> bool,
) -> Answered {
    out.clear();
    let enc = Enc::replication(capabilities);
    let Some(chart) = host.charts().find(&request.chart_id, true) else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM SND REPLAY ERROR: 'host:{}/chart:{}' not found, sending empty response to unblock parent",
            host.hostname(),
            request.chart_id
        );
        emit::rbegin_chart(out, &enc, 0, &request.chart_id);
        emit::rend(out, &enc, 0, 0, 0, true, 0, 0, now_realtime_s());
        commit(out);
        return Answered::NotFound;
    };
    let wall_s = now_realtime_s();
    let mut window =
        normalize(request, wall_s, i64::from(chart.update_every()), chart.retention_for_collected(wall_s));
    let mut all = chart.dims();
    let mut guard = None;
    let mut dims = Vec::new();
    if !all.is_empty() && window.after != 0 && window.before != 0 {
        if window.streaming {
            guard = Some(Chart::lock_collection(chart.as_ref()));
            // the dimensions as the lock leaves them, as C walks them under it
            all = chart.dims();
            let last_updated_s = chart.collection().last_updated.0;
            if last_updated_s > window.before {
                window.before = last_updated_s.min(wall_s);
            }
        }
        for dim in all.iter().filter(|d| d.is_sent_upstream()) {
            if let Some(query) = dim.tier_query(0, window.after, window.before, QueryPriority::SynchronousFirst) {
                dims.push(DimQuery { dim, query, sp: StoragePoint::default(), skip: false });
            }
        }
        if dims.is_empty() {
            guard = None;
        }
    }
    emit::rbegin_chart(out, &enc, u64::from(chart.chart_slot()), chart.id());
    let mut finished_with_gap = false;
    if !dims.is_empty() {
        if !window.streaming {
            let last_updated_s = chart.collection().last_updated.0;
            align_to_optimal_before(&mut dims, &mut window, &chart, last_updated_s, wall_s);
        }
        let interpolated = capabilities & caps::INTERPOLATED != 0;
        let walked = walk(out, &enc, host, &chart, &mut dims, &mut window, wall_s, interpolated, max_msg_size);
        finished_with_gap = walked.finished_with_gap;
        chart.storage().pulse().queries.replication_query_completed(
            dims.len() as u64,
            walked.points_read as u64,
            walked.points_generated as u64,
        );
    }
    if window.streaming {
        collection_state(out, &enc, &chart, capabilities);
    }
    // replication_query_finalize(): the dimension queries end before the answer's last line
    drop(dims);
    let end_wall_s = now_realtime_s();
    let (db_first, db_last) = chart.retention_for_collected(end_wall_s);
    // C reads it as it writes REND
    let update_every = i64::from(chart.update_every());
    emit::rend(out, &enc, update_every, db_first, db_last, window.streaming, window.after, window.before, end_wall_s);
    if commit(out) && window.streaming {
        finish(host, &chart, finished_with_gap);
    }
    drop(guard);
    Answered::Executed
}
