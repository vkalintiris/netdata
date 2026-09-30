//! What a host sends its parent from the collectors' side (`src/streaming/protocol/`): the per-collection gate, the
//! chart definitions with their claim of a replication, the obsolete push, the v2 and v1 data of a collection, the
//! reset at each connection and the host's metadata (D104, D108). The connection itself is the streaming crate's,
//! reached through [`Upstream`]. Map: `knowledge/map-m7-commit5-metadata.md` in the status repository.

use std::cell::RefCell;
use std::sync::Arc;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_pluginsd_proto::caps;
use netdata_agent_pluginsd_proto::emit::stream::{self as emit, Baseline, ChartDef, DimDef, Enc, V1Value, V2Block};
use netdata_agent_storage::storage_number::flags_text;
use netdata_agent_text::c::double_to_i64;
use netdata_agent_text::simple_pattern::SimplePatternResult;

use crate::chart::{Chart, ChartCollection, Dim, dim_flags, flags};
use crate::clock::now_realtime_s;
use crate::host::{Host, StreamSend, sender_flags};
use crate::labels::FLAG_INTERNAL;
use crate::pulse::host_status;

/// `STREAM_TRAFFIC_TYPE`, the per-type byte counters' index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Traffic {
    Replication = 0,
    Functions,
    Metadata,
    Data,
}

/// A host's sender (`host->sender`) as the host and its collectors see it, set once when the host streams
/// (`stream_sender_structures_init()`, D103.1): the transport, which the renderers here commit to (D108).
pub trait Upstream: Send + Sync + std::fmt::Debug {
    /// `stream_sender_start_host()`: queue the host for its parents.
    fn start(&self);
    /// `sender->disabled_capabilities`, which `stream_our_capabilities(host, true)` leaves out.
    fn disabled_capabilities(&self) -> u32;
    /// `sender->capabilities`: negotiated with the parent of the last connection.
    fn capabilities(&self) -> u32;
    /// `sender_commit()`: one message, dropped while no connection is dispatched.
    fn commit(&self, bytes: &[u8], traffic: Traffic);
    /// `stream_send.initial_clock_resync_iterations`.
    fn resync_iterations(&self) -> u16;
    /// `stream_circular_buffer_last_flush_ut()`: when the sender's buffer was last emptied (each connection and
    /// disconnection).
    fn flush_ut(&self) -> u64;
    /// `sender_buffer_commit()` of a message rendered since the buffer flush at `flush_ut`: dropped when the buffer
    /// was flushed again meanwhile (D106.11) or no connection is dispatched; whether a session took it.
    fn commit_since(&self, bytes: &[u8], traffic: Traffic, flush_ut: u64) -> bool;
    /// `stream_sender_signal_to_stop_and_wait(host, reason, false)` when the host's receiver left: a queued or
    /// dispatched sender stops (it starts again when the child returns), `reason` (a `STREAM_HANDSHAKE` code) the
    /// receiver's.
    fn receiver_left(&self, reason: i32);
    /// `stream_parents_host_reset()`: every parent waits one reconnect delay, `reason` recorded on each.
    fn parents_reset(&self, reason: i32);
    /// `stream_sender_structures_free()`'s sender side: stopped and emptied; a sender still live after the bounded
    /// wait (D118.2) is left as it is.
    fn free(&self);
    /// `stream_sender_structures_init()` of a freed sender: set up again as a new one, with `send`'s settings.
    fn reinit(&self, send: &StreamSend);
}

thread_local! {
    /// `sender_thread_buffer()`: each collector thread's data buffer, reused across collections.
    static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn take_buffer() -> Vec<u8> {
    let mut b = BUFFER.with(|b| std::mem::take(&mut *b.borrow_mut()));
    b.clear();
    b
}

/// Kept for the next collection, unless a large one (a long gap) grew it: C recreates an oversized thread buffer too.
fn give_back(b: Vec<u8>) {
    const KEEP: usize = 256 * 1024;
    if b.capacity() <= KEEP {
        BUFFER.with(|slot| *slot.borrow_mut() = b);
    }
}

/// The DATA commits a child's receiver holds before the next one sends them (`sender_thread_commit()`'s `reused`).
pub const BATCH_HELD: u32 = 100;
/// The pending bytes that send a receiver's DATA at once (`COMPRESSION_MAX_MSG_SIZE * 2 / 3`).
pub const BATCH_BYTES: usize = 16_255 * 2 / 3;

/// The host buffer a child's receiver forwards through (`host->stream.snd.commit`), owned by its parser and passed
/// down (D106.1): no lock, no thread identity. DATA commits are held until the 101st or until the pending bytes reach
/// [`BATCH_BYTES`]; any other commit sends what is held ahead of itself. What was held when the sender's buffer was
/// flushed is dropped, as C frees the host buffer at each disconnection.
#[derive(Debug, Default)]
pub struct ForwardBuffer {
    buf: Vec<u8>,
    /// `commit->reused`: the DATA commits held.
    reused: u32,
    /// The sender's buffer flush the bytes belong to.
    flush_ut: u64,
}

impl ForwardBuffer {
    /// `sender_commit_start()`: the bytes to render into, which still hold the batch while DATA is held.
    pub fn start(&mut self, up: &dyn Upstream) -> &mut Vec<u8> {
        let flush_ut = up.flush_ut();
        if flush_ut != self.flush_ut {
            self.flush_ut = flush_ut;
            self.reused = 0;
        }
        if self.reused == 0 {
            self.buf.clear();
        }
        &mut self.buf
    }

    /// The bytes of the block [`ForwardBuffer::start`] began.
    pub fn bytes(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    /// `sender_thread_commit()` on the receiver's thread: a DATA commit is held while fewer than [`BATCH_HELD`] are
    /// and the bytes stay under [`BATCH_BYTES`]; any other sends the whole buffer as its own traffic.
    pub fn commit(&mut self, up: &dyn Upstream, traffic: Traffic) {
        if traffic == Traffic::Data && self.reused < BATCH_HELD && self.buf.len() < BATCH_BYTES {
            self.reused += 1;
            return;
        }
        up.commit_since(&self.buf, traffic, self.flush_ut);
        self.buf.clear();
        self.reused = 0;
    }
}

/// `preferred_sender_buffer()`: a collection's lines go through the collecting thread's own buffer, each commit sent
/// at once, or, on a child's receiver, through its forward buffer.
#[derive(Debug)]
pub enum BufferSource<'f> {
    Thread,
    Forward(&'f mut ForwardBuffer),
}

/// A gate's metadata commit: at once from the thread's buffer, or through the forward buffer, sending the DATA held
/// there ahead of it.
fn commit_metadata(up: &dyn Upstream, src: &mut BufferSource<'_>, render: impl FnOnce(&mut Vec<u8>)) {
    match src {
        BufferSource::Thread => {
            let mut out = take_buffer();
            render(&mut out);
            up.commit(&out, Traffic::Metadata);
            give_back(out);
        }
        BufferSource::Forward(fwd) => {
            render(fwd.start(up));
            fwd.commit(up, Traffic::Metadata);
        }
    }
}

/// Where a collection's data is rendered.
#[derive(Debug)]
enum Out<'f> {
    Thread(Vec<u8>),
    Forward(&'f mut ForwardBuffer),
}

impl Out<'_> {
    fn bytes(&mut self) -> &mut Vec<u8> {
        match self {
            Out::Thread(out) => out,
            Out::Forward(fwd) => fwd.bytes(),
        }
    }
}

/// `RRDSET_STREAM_BUFFER`: a chart collection's data for the parent, committed by [`StreamBuffer::finish`].
#[derive(Debug)]
pub struct StreamBuffer<'a> {
    up: &'a dyn Upstream,
    capabilities: u32,
    /// INTERPOLATED: stored points in BEGIN2/SET2/END2; else the collected values in BEGIN/SET/END.
    v2: bool,
    /// The chart's flags at the gate.
    chart_flags: u32,
    out: Out<'a>,
    block: V2Block<'a>,
}

/// `RRDSET_STREAM_BUFFER` of a block a child's receiver forwards: what its gate saw, kept by the parser from BEGIN2
/// to END2.
#[derive(Debug, Clone, Copy)]
pub struct ProxyBlock {
    /// The parent's, at the gate.
    pub capabilities: u32,
    /// INTERPOLATED: the child's BEGIN2 and SET2 go on; else END2 sends the chart's collected values in v1.
    pub v2: bool,
    /// The chart's, at the gate.
    pub chart_flags: u32,
    /// `begin_v2_added`: a forwarded BEGIN2 is open.
    pub begin_added: bool,
}

/// `stream_send_metrics_init()`, at the top of `rrdset_timed_done()`: a buffer for this collection's data from `src`
/// when the gate lets it go.
pub fn metrics_init<'a>(
    host: &'a Host,
    chart: &'a Chart,
    wall_clock_s: i64,
    mut src: BufferSource<'a>,
) -> Option<StreamBuffer<'a>> {
    let (up, chart_flags) = gate(host, chart, &mut src)?;
    // the flush stamp before the capabilities: a block never carries an older session's encodings under a newer stamp
    let out = match src {
        BufferSource::Thread => Out::Thread(take_buffer()),
        BufferSource::Forward(fwd) => {
            fwd.start(up);
            Out::Forward(fwd)
        }
    };
    let capabilities = up.capabilities();
    Some(StreamBuffer {
        up,
        capabilities,
        v2: capabilities & caps::INTERPOLATED != 0,
        chart_flags,
        out,
        block: V2Block::new(
            Enc::live(capabilities),
            u64::from(chart.chart_slot()),
            chart.id(),
            chart.update_every() as u64,
            wall_clock_s,
        ),
    })
}

/// `stream_send_metrics_init()` on a child's receiver, at BEGIN2: the chart's forwarded block, started in `fwd`, when
/// the gate lets it go.
pub fn forward_gate(host: &Host, chart: &Chart, fwd: &mut ForwardBuffer) -> Option<ProxyBlock> {
    let (up, chart_flags) = gate(host, chart, &mut BufferSource::Forward(&mut *fwd))?;
    // the flush stamp first, as metrics_init
    fwd.start(up);
    let capabilities = up.capabilities();
    Some(ProxyBlock { capabilities, v2: capabilities & caps::INTERPOLATED != 0, chart_flags, begin_added: false })
}

/// The gate both share: nothing until the sender is ready (queued for its parents meanwhile, with one record per
/// transition, D104.7); the host's functions again when they changed; the chart's definition when the parent lacks
/// it, both committed through `src`; then the sender and the chart's flags, unless the chart is filtered out or
/// waits for its replication.
fn gate<'a>(host: &'a Host, chart: &Chart, src: &mut BufferSource<'_>) -> Option<(&'a dyn Upstream, u32)> {
    let up = host.upstream()?.as_ref();
    let host_flags = host.sender_flags();
    if host_flags & sender_flags::READY_4_METRICS == 0 {
        // RRDHOST_FLAG_COLLECTOR_ONLINE
        if host.is_online() && host_flags & sender_flags::ADDED == 0 {
            up.start();
        }
        // the snapshot first, as C: no shared write at every collection while the sender waits
        if host_flags & sender_flags::LOGGED_STATUS == 0
            && host.sender_flags_set(sender_flags::LOGGED_STATUS) & sender_flags::LOGGED_STATUS == 0
        {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "STREAM SND '{}': streaming is not ready, not sending data to a parent...",
                host.hostname()
            );
        }
        return None;
    }
    if host_flags & sender_flags::LOGGED_STATUS != 0
        && host.sender_flags_clear(sender_flags::LOGGED_STATUS) & sender_flags::LOGGED_STATUS != 0
    {
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "STREAM SND '{}': streaming is ready, sending metrics to parent...",
            host.hostname()
        );
    }
    if host_flags & sender_flags::GLOBAL_FUNCTIONS_UPDATED != 0 {
        render_global_functions(host, up, src);
    }
    let exposed = chart.is_exposed_upstream();
    let chart_flags = chart.flags();
    let mut replicating = chart_flags & flags::SENDER_REPLICATION_FINISHED == 0;
    if (exposed && replicating) || !should_send(host, chart, chart_flags) {
        return None;
    }
    if !exposed {
        commit_metadata(up, src, |out| replicating = send_definition(host, up, chart, out));
    }
    if replicating {
        return None;
    }
    Some((up, chart_flags))
}

/// A dimension's last collected value, as the collection loop holds it before this collection's carry.
#[derive(Debug, Clone, Copy)]
pub struct LastCollected {
    pub int: i64,
    pub float: f64,
    pub is_float: bool,
}

impl LastCollected {
    /// As a baseline: a float one as a double with FLOAT_BASELINE, else as C's cast to an integer
    /// (`rrddim_last_collected_as_double()`, `rrddim_last_collected_raw_int()`).
    fn baseline(&self, capabilities: u32) -> Baseline {
        if self.is_float && capabilities & caps::FLOAT_BASELINE != 0 {
            Baseline::Float(self.float)
        } else if self.is_float {
            Baseline::Int(double_to_i64(self.float))
        } else {
            Baseline::Int(self.int)
        }
    }
}

impl StreamBuffer<'_> {
    /// `stream_send_rrddim_metrics_v2()` with INTERPOLATED: a stored point, with the dimension's previous collected value as baseline
    /// (a float one as a double with FLOAT_BASELINE, else as C's cast to an integer).
    pub fn set2(&mut self, dim: &Dim, point_end_ut: u64, value: f64, sn_flags: u32, last: LastCollected) {
        if !self.v2 {
            return;
        }
        let baseline = last.baseline(self.capabilities);
        self.block.set2(
            self.out.bytes(),
            (point_end_ut / 1_000_000) as i64,
            u64::from(dim.slot()),
            dim.id(),
            baseline,
            value,
            flags_text(sn_flags, true),
        );
    }

    /// `stream_send_rrdset_metrics_v1()` without INTERPOLATED, after the collection counted.
    pub fn v1(&mut self, chart: &Chart, dims: &[Arc<Dim>], c: &ChartCollection) {
        if !self.v2 {
            write_v1(self.out.bytes(), chart, dims, c, self.capabilities, self.chart_flags);
        }
    }

    /// `stream_send_rrdset_metrics_finished()`: the open block closed, then the collection's data committed.
    pub fn finish(mut self, chart: &Chart) {
        close_block(chart, self.v2 && self.block.is_open(), self.chart_flags, self.out.bytes());
        match self.out {
            Out::Thread(out) => {
                self.up.commit(&out, Traffic::Data);
                give_back(out);
            }
            Out::Forward(fwd) => fwd.commit(self.up, Traffic::Data),
        }
    }
}

/// `stream_send_rrdset_metrics_v1()` of a forwarded block, at END2 for a parent without INTERPOLATED: the chart's
/// collected values, which SET2 never sets, so zeros as C (D106.3).
pub fn forward_v1(chart: &Chart, b: &ProxyBlock, fwd: &mut ForwardBuffer) {
    if !b.v2 {
        write_v1(fwd.bytes(), chart, &chart.dims(), &chart.collection(), b.capabilities, b.chart_flags);
    }
}

/// `stream_send_rrdset_metrics_finished()` of a forwarded block: closed, then committed as DATA into the batch.
pub fn forward_finish(host: &Host, chart: &Chart, b: &ProxyBlock, fwd: &mut ForwardBuffer) {
    let Some(up) = host.upstream() else {
        return;
    };
    close_block(chart, b.v2 && b.begin_added, b.chart_flags, fwd.bytes());
    fwd.commit(up.as_ref(), Traffic::Data);
}

/// The v1 block: the values collected, `0` for the time until the chart resynced; a dimension collected but not yet
/// sent with the chart's definition is left for its next definition.
fn write_v1(
    out: &mut Vec<u8>,
    chart: &Chart,
    dims: &[Arc<Dim>],
    c: &ChartCollection,
    capabilities: u32,
    chart_flags: u32,
) {
    let usec = if c.last_collected.0 > chart.resync_time_s() { c.usec_since_last_update } else { 0 };
    emit::v1_begin(out, chart.id(), usec);
    for dim in dims {
        let m = dim.meta();
        if m.flags & dim_flags::UPDATED == 0 {
            continue;
        }
        // read per dimension, as C's `rrddim_check_upstream_exposed_collector()`: an unexposed one moves it
        if !dim.is_exposed_upstream(chart.version()) {
            chart.dim_metadata_updated(dim);
            continue;
        }
        let d = dim.collection();
        let value = if m.flags & dim_flags::FLOAT == 0 {
            V1Value::Int(d.collected_value)
        } else if capabilities & caps::FLOAT_BASELINE != 0 {
            V1Value::Float(d.collected_value_float)
        } else {
            V1Value::Int(double_to_i64(d.collected_value_float))
        };
        emit::v1_set(out, dim.id(), value);
    }
    if chart_flags & flags::UPSTREAM_SEND_VARIABLES != 0 {
        chart_variables(chart, out);
    }
    emit::v1_end(out);
}

/// An open v2 block's end: the chart's variables when they changed before the gate, then END2.
fn close_block(chart: &Chart, open: bool, chart_flags: u32, out: &mut Vec<u8>) {
    if open {
        if chart_flags & flags::UPSTREAM_SEND_VARIABLES != 0 {
            chart_variables(chart, out);
        }
        emit::end2(out);
    }
}

/// `should_send_rrdset_matching()`: only a chart whose replication from its child finished; the verdict of the host's
/// `send charts matching` on the chart's context, name and id, in that order, is kept on the chart (ML's anomaly
/// charts come with ML, D100.11).
pub fn should_send(host: &Host, chart: &Chart, chart_flags: u32) -> bool {
    if chart_flags & flags::RECEIVER_REPLICATION_FINISHED == 0 {
        return false;
    }
    let mut chart_flags = chart_flags;
    if chart_flags & (flags::UPSTREAM_SEND | flags::UPSTREAM_IGNORE) == 0 {
        let (context, name) = chart.with_meta(|m| (m.context.clone(), m.name.clone()));
        let (mut positive, mut negative) = (0, 0);
        host.with_charts_matching(|pattern| {
            let mut try_match = |text: Option<&str>| {
                if negative != 0 {
                    return;
                }
                let r = text.map_or(SimplePatternResult::NotMatched, |t| {
                    pattern.map_or(SimplePatternResult::NotMatched, |p| p.matches_extract(t.as_bytes(), 0).0)
                });
                match r {
                    SimplePatternResult::MatchedPositive => positive += 1,
                    SimplePatternResult::MatchedNegative => negative += 1,
                    SimplePatternResult::NotMatched => {}
                }
            };
            try_match(Some(&context));
            try_match(name.as_deref());
            try_match(Some(chart.id()));
        });
        let verdict = if negative == 0 && positive != 0 { flags::UPSTREAM_SEND } else { flags::UPSTREAM_IGNORE };
        chart.update_meta(|m| m.flags |= verdict);
        chart_flags = chart.flags();
    }
    chart_flags & flags::UPSTREAM_SEND != 0
}

/// `rrdvar_print_to_streaming_custom_chart_variables()`.
fn chart_variables(chart: &Chart, out: &mut Vec<u8>) {
    chart.update_meta(|m| m.flags &= !flags::UPSTREAM_SEND_VARIABLES);
    for (name, value) in chart.variables() {
        emit::variable(out, emit::VarScope::Chart, &name, value);
    }
}

/// `stream_sender_send_rrdset_definition()`: CHART, its labels, every dimension, its variables and, with
/// REPLICATION, CHART_DEFINITION_END and the claim of the chart's replication (the host counter up before the flag
/// is published, undone when the chart turned obsolete or the sender stopped meanwhile). The chart and its
/// dimensions are then sent at the version read first; whether the chart now waits for its replication.
fn send_definition(host: &Host, up: &dyn Upstream, chart: &Chart, out: &mut Vec<u8>) -> bool {
    let version = chart.version();
    let capabilities = up.capabilities();
    let enc = Enc::live(capabilities);
    let meta = chart.meta();
    emit::chart(
        out,
        &enc,
        &ChartDef {
            slot: u64::from(chart.chart_slot()),
            id: chart.id(),
            name: emit::chart_name(chart.id(), meta.name.as_deref()),
            title: &meta.title,
            units: &meta.units,
            family: &meta.family,
            context: &meta.context,
            chart_type: meta.chart_type.name(),
            // C's int32_t
            priority: meta.priority as i32,
            update_every: meta.update_every,
            obsolete: meta.flags & flags::OBSOLETE != 0,
            store_first: meta.flags & flags::STORE_FIRST != 0,
            hidden: meta.flags & flags::HIDDEN != 0,
            plugin: &meta.plugin,
            module: &meta.module,
        },
    );
    if capabilities & caps::CLABELS != 0 && !meta.labels.is_empty() {
        for label in meta.labels.iter() {
            emit::clabel(out, &label.name, &label.value, (label.flags & !FLAG_INTERNAL) as i32);
        }
        emit::clabel_commit(out);
    }
    let dims = chart.dims();
    for dim in &dims {
        let m = dim.meta();
        emit::dimension(
            out,
            &enc,
            &DimDef {
                slot: u64::from(dim.slot()),
                id: dim.id(),
                name: &m.name,
                algorithm: m.algorithm.name(),
                multiplier: m.multiplier,
                divisor: m.divisor,
                obsolete: m.flags & dim_flags::OBSOLETE != 0,
                hidden: m.flags & dim_flags::HIDDEN != 0,
                noreset: m.flags & dim_flags::DONT_DETECT_RESETS_OR_OVERFLOWS != 0,
                float: m.flags & dim_flags::FLOAT != 0,
            },
        );
    }
    chart_variables(chart, out);
    let mut replication_progress = false;
    if capabilities & caps::REPLICATION != 0 {
        let now = now_realtime_s();
        let (first, last) = chart.retention_for_collected(now);
        emit::chart_definition_end(out, first, last, now);
        // the parent does not replicate an obsolete chart, so nothing is claimed for it
        if chart.flags() & flags::OBSOLETE == 0 {
            let first_claim = host.sender_replicating_charts_plus_one() == 1;
            let old = chart.flags_set_and_clear(
                flags::SENDER_REPLICATION_IN_PROGRESS,
                flags::SENDER_REPLICATION_FINISHED,
            );
            let we_caused_transition = old & flags::SENDER_REPLICATION_IN_PROGRESS == 0;
            if we_caused_transition {
                if first_claim {
                    host.pulse_status(host_status::SND_REPLICATING);
                }
            } else if host.sender_replicating_charts_minus_one() == 0 {
                host.pulse_status(host_status::SND_RUNNING);
            }
            if chart.flags() & flags::OBSOLETE != 0 || !host.can_stream_metadata() {
                if we_caused_transition {
                    give_back_claim(host, chart);
                }
            } else {
                replication_progress = true;
            }
        }
    }
    for dim in &dims {
        dim.set_exposed_upstream(version);
    }
    chart.set_exposed_upstream(version);
    let iterations = i64::from(up.resync_iterations());
    chart.set_resync_time_s(chart.collection().last_collected.0 + iterations * i64::from(meta.update_every));
    replication_progress
}

/// `stream_sender_send_rrdset_definition_now()`: a chart that will not be collected again (obsolete) is defined at
/// once, in its own message; whether it was.
pub fn send_definition_now(host: &Host, chart: &Chart) -> bool {
    let Some(up) = host.upstream() else {
        return false;
    };
    if !host.can_stream_metadata() || !should_send(host, chart, chart.flags()) {
        return false;
    }
    let mut out = Vec::new();
    send_definition(host, up.as_ref(), chart, &mut out);
    up.commit(&out, Traffic::Metadata);
    true
}

/// A claim given back (the chart finished, as at the parent's end of its replication): the last one marks the host
/// running. Only the thread that finds the claim takes it back.
fn give_back_claim(host: &Host, chart: &Chart) {
    let old = chart.flags_set_and_clear(flags::SENDER_REPLICATION_FINISHED, flags::SENDER_REPLICATION_IN_PROGRESS);
    if old & flags::SENDER_REPLICATION_IN_PROGRESS != 0 && host.sender_replicating_charts_minus_one() == 0 {
        host.pulse_status(host_status::SND_RUNNING);
    }
}

/// The release of `rrdset_is_obsolete___safe_from_collector_thread()`: the parent does not replicate an obsolete
/// chart, so a claim it held is given back.
pub(crate) fn release_on_obsolete(host: &Host, chart: &Chart) {
    give_back_claim(host, chart);
}

/// `stream_sender_charts_and_replication_reset()`'s charts, at every connection and disconnection: no replication
/// claimed, no resync horizon, nothing sent, so each chart is defined again at its next collection. The host counter
/// is only taken back for the claims found, since a claim may be in flight.
pub fn reset_charts(host: &Host) {
    for chart in host.charts().all() {
        let old =
            chart.flags_set_and_clear(flags::SENDER_REPLICATION_FINISHED, flags::SENDER_REPLICATION_IN_PROGRESS);
        if old & flags::SENDER_REPLICATION_IN_PROGRESS != 0 {
            host.sender_replicating_charts_minus_one();
        }
        chart.set_resync_time_s(0);
        for dim in chart.dims() {
            dim.set_exposed_upstream(0);
        }
        chart.metadata_updated();
    }
    let residual = host.sender_replicating_charts();
    if residual != 0 {
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "STREAM REPLAY: sender replicating-charts counter is {residual} after reset (expected 0); leaving it \
             untouched to preserve any concurrent claim-before-publish in flight"
        );
    }
}

/// `stream_sender_send_custom_host_variables()`: every host variable.
pub fn send_host_variables(host: &Host) {
    let Some(up) = host.upstream() else {
        return;
    };
    if !host.can_stream_metadata() {
        return;
    }
    let mut out = Vec::new();
    for (name, value) in host.variables() {
        emit::variable(&mut out, emit::VarScope::Host, &name, value);
    }
    up.commit(&out, Traffic::Metadata);
}

/// `stream_sender_send_this_host_variable_now()`: a host variable whose value changed.
pub(crate) fn send_host_variable(host: &Host, name: &str, value: f64) {
    let Some(up) = host.upstream() else {
        return;
    };
    if !host.can_stream_metadata() {
        return;
    }
    let mut out = Vec::new();
    emit::variable(&mut out, emit::VarScope::Host, name, value);
    up.commit(&out, Traffic::Metadata);
}

/// `stream_sender_send_claimed_id()`.
pub fn send_claimed_id(host: &Host) {
    let Some(up) = host.upstream() else {
        return;
    };
    if up.capabilities() & caps::CLAIM == 0 || !host.can_stream_metadata() {
        return;
    }
    let mut out = Vec::new();
    emit::claimed_id(&mut out, host.machine_guid(), host.claim_id().as_ref());
    up.commit(&out, Traffic::Metadata);
}

/// `stream_send_host_labels()`: every label with its source, then `OVERWRITE labels` even with none.
pub fn send_host_labels(host: &Host) {
    let Some(up) = host.upstream() else {
        return;
    };
    if !host.can_stream_metadata() || up.capabilities() & caps::HLABELS == 0 {
        return;
    }
    let mut out = Vec::new();
    for label in host.labels().iter() {
        emit::label(&mut out, &label.name, label.flags, &label.value);
    }
    emit::overwrite_labels(&mut out);
    up.commit(&out, Traffic::Metadata);
}

/// `stream_send_global_functions()`: the host's functions, as the ready hook sends them.
pub fn send_global_functions(host: &Host) {
    let Some(up) = host.upstream() else {
        return;
    };
    if up.capabilities() & caps::FUNCTIONS == 0 || !host.can_stream_metadata() {
        return;
    }
    render_global_functions(host, up.as_ref(), &mut BufferSource::Thread);
}

/// The render and commit both call sites share, under the host's lock (`global_functions_spinlock`) so neither
/// interleaves with the other: the flag is cleared first, so a change after the render is sent again. Dynamic
/// configuration methods are left out (`NRPC_CATALOG_FILTER_STREAM_GLOBAL`); the FUNCTION_DEL queue and DynCfg's own
/// line, which stands for them, come with the functions milestone, M8 (D100.9).
fn render_global_functions(host: &Host, up: &dyn Upstream, src: &mut BufferSource<'_>) {
    let _serialized = host.lock_global_functions();
    host.sender_flags_clear(sender_flags::GLOBAL_FUNCTIONS_UPDATED);
    commit_metadata(up, src, |out| {
        for (name, m) in host.functions().all() {
            if m.flags & netdata_agent_nrpc::FLAG_DYNCFG != 0 {
                continue;
            }
            emit::function_global(out, &name, m.timeout_s, &m.help, &m.tags, m.access, m.priority, m.version);
        }
    });
}

pub mod replay;

#[cfg(test)]
mod tests;
