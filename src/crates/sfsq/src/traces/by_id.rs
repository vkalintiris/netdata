//! Cross-source trace-by-id — the phase-4a operation.
//!
//! [`trace_by_id`] assembles ONE trace from a validated set of
//! [`TraceSource`]s (sealed SFSTs, in-memory chunk SFSTs, WAL tails)
//! through the shared combiner (`sfst::trace_combine`): per-file sessions
//! decode the TBLM bloom once and test the id against it, surviving
//! sources feed lightweight span refs into the comparator-ordered k-way
//! merge, and payloads materialize only as the merge accepts them.
//!
//! Status honesty (the design-record contract): a source that fails to
//! map, open, or decode surfaces as a
//! [`SourceFailure`](PartialReason::SourceFailure) reason on the
//! query-level [`QueryStatus`], and an unavailable source as
//! [`RemoteUnavailable`](PartialReason::RemoteUnavailable) — never a
//! silent skip. Cancellation before
//! all source heads are resolved returns an EMPTY result with
//! [`Cancelled`](PartialReason::Cancelled); cancellation during the merge
//! returns the deterministic merged prefix. The span cap yields
//! [`SizeCap`](PartialReason::SizeCap) with the globally earliest spans
//! kept (cap+1 detection — an exactly-cap trace is `Complete`).
//!
//! The result carries a cross-source coalesced field→schema-kind map for
//! typed reconstruction (the phase-5 consumer parses by schema kind,
//! never inferring from rendered strings): kinds merge — via the
//! [`sfst::join_value_kinds`] lattice — from exactly the sources that
//! contributed retained canonical spans.
//!
//! Every returned span also gets its error origin and child time, derived
//! over the assembled trace (all its spans, across files) with the seal's
//! own function, so a live span reads like a sealed one.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use sfst::trace_combine::{SpanSource, combine};

use super::sources::{SourceSetError, TraceSource, validate_sources};
use super::status::{PartialReason, QueryStatus, StatusBuilder};
use super::wal_scan::TraceWalScan;
use crate::source::map_source;

/// The field whose first value says whether a span is an error.
const STATUS_FIELD: &str = "status_code";

/// The library default span cap ([`TraceQuery::new`] applies it): far
/// above any honest trace, low enough that a runaway merge stays bounded.
pub const DEFAULT_SPAN_CAP: usize = 65_536;

/// A trace-by-id request in engine terms. [`new`](Self::new) applies the
/// default span cap; [`span_cap`](Self::span_cap) overrides it and
/// [`unbounded`](Self::unbounded) removes it.
#[derive(Debug, Clone)]
pub struct TraceQuery {
    trace_id: sfst::TraceId,
    span_cap: Option<usize>,
}

impl TraceQuery {
    pub fn new(trace_id: sfst::TraceId) -> Self {
        Self {
            trace_id,
            span_cap: Some(DEFAULT_SPAN_CAP),
        }
    }

    /// Override the span cap. Zero is rejected at [`trace_by_id`]'s
    /// request validation.
    pub fn span_cap(mut self, cap: usize) -> Self {
        self.span_cap = Some(cap);
        self
    }

    /// Remove the span cap entirely.
    pub fn unbounded(mut self) -> Self {
        self.span_cap = None;
        self
    }
}

/// A request error — the caller built an invalid request or source set.
/// Distinct from a [`Partial`](QueryStatus::Partial) result: nothing was
/// queried.
#[derive(Debug, thiserror::Error)]
pub enum TraceRequestError {
    /// The all-zero trace id is the OTLP "unset/invalid" sentinel, and
    /// TIDX deliberately omits it while a tail scan could serve it — a
    /// lookup would be layout-dependent, so it is rejected outright.
    #[error("the all-zero (unset) trace id cannot be looked up")]
    UnsetTraceId,
    #[error("a zero span cap would return nothing; omit the cap or raise it")]
    ZeroSpanCap,
    #[error(transparent)]
    SourceSet(#[from] SourceSetError),
}

/// The coalesced field→schema-kind maps for typed reconstruction,
/// SECTIONED to mirror how the trace exposes names: span-level `fields`
/// keep their storage names; event/link attribute keys are the
/// prefix-stripped names [`sfst::TraceEvent`]/[`sfst::TraceLink`] expose
/// (a flat map would collide an event attr `foo` with a link attr `foo`
/// whose kinds differ). Every section is FILTERED to exactly the names
/// the returned trace exposes — the map describes the returned data, not
/// whatever else lives in the contributing files. Sorted by name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FieldKinds {
    pub fields: Vec<(String, sfst::ValueKind)>,
    pub event_attributes: Vec<(String, sfst::ValueKind)>,
    pub link_attributes: Vec<(String, sfst::ValueKind)>,
}

/// One assembled trace plus everything the consumer needs to interpret
/// it honestly.
#[derive(Debug)]
pub struct TraceData {
    /// The merged trace (combiner total order, node-index graph). Empty
    /// when the id is absent everywhere — that is a `Complete` empty,
    /// not an error.
    pub trace: sfst::Trace,
    /// Query-level completeness.
    pub status: QueryStatus,
    /// Field → coalesced scalar kind (see [`FieldKinds`]), merged — via
    /// the [`sfst::join_value_kinds`] lattice — from exactly the sources
    /// that contributed retained spans.
    pub field_kinds: FieldKinds,
    /// Index-parallel to `trace.spans`: each span's error origin and the
    /// time its children cover, over the returned spans (a size-capped
    /// trace derives over the kept ones).
    pub family: sfst::SpanFamily,
}

/// Run a cross-source trace-by-id. See the module docs for the status
/// and cancellation contract. `progress` ticks once per source during
/// setup, success or failure (the caller advertises `sources.len()` out
/// of band);
/// callers that don't report pass a fresh counter, callers that don't
/// cancel pass `CancellationToken::new()`.
///
/// Pure sync — reads and decompresses files; invoke off any async
/// runtime thread (the logs-engine contract).
pub fn trace_by_id(
    sources: Vec<TraceSource>,
    query: TraceQuery,
    cancel: CancellationToken,
    progress: Arc<AtomicUsize>,
) -> Result<TraceData, TraceRequestError> {
    if query.trace_id.is_unset() {
        return Err(TraceRequestError::UnsetTraceId);
    }
    if query.span_cap == Some(0) {
        return Err(TraceRequestError::ZeroSpanCap);
    }
    validate_sources(&sources)?;

    let mut status = StatusBuilder::new();
    let cancelled_empty = |mut status: StatusBuilder| {
        status.add(PartialReason::Cancelled);
        TraceData {
            trace: sfst::Trace {
                spans: Vec::new(),
                roots: Vec::new(),
                children: Vec::new(),
            },
            status: status.finish(),
            field_kinds: FieldKinds::default(),
            family: span_family(sfst::TraceId::default(), &[]),
        }
    };
    // Pre-heads cancellation returns EMPTY + Cancelled — polled up front
    // so a zero-source or already-cancelled call can never report
    // Complete.
    if cancel.is_cancelled() {
        return Ok(cancelled_empty(status));
    }

    // ── Setup: resolve every source head ─────────────────────────────
    // Map SFST bytes and decode tail frames. A failure is reported
    // (SourceFailure) and the source dropped; cancellation HERE returns
    // the empty result — no deterministic prefix exists before every
    // head is known.
    let mut mapped_sfsts: Vec<(crate::source::Mapped, &TraceSource)> = Vec::new();
    let mut tails: Vec<(super::sources::SourceId, TraceWalScan)> = Vec::new();
    for source in &sources {
        if cancel.is_cancelled() {
            return Ok(cancelled_empty(status));
        }
        match source {
            TraceSource::Sfst(c) => match map_source(&c.source) {
                Ok(mapped) => mapped_sfsts.push((mapped, source)),
                Err(e) => {
                    tracing::warn!("sfsq traces: source {} failed to map: {e}", c.source_id);
                    status.add(PartialReason::SourceFailure);
                }
            },
            // The scan reads the VALIDATED coverage range — the one field
            // the overlap check saw (no second range to diverge).
            TraceSource::Tail(t) => match TraceWalScan::scan_range(&t.path, t.coverage.range) {
                Ok(scan) => tails.push((t.source_id.clone(), scan)),
                Err(e) => {
                    tracing::warn!("sfsq traces: tail {} failed to scan: {e}", t.source_id);
                    status.add(PartialReason::SourceFailure);
                }
            },
            // No summary pruning here (the bloom prunes a by-id probe),
            // so any unavailable source may hold the trace's spans.
            TraceSource::Unavailable(_) => status.add(PartialReason::RemoteUnavailable),
            TraceSource::Failed(f) => {
                tracing::warn!("sfsq traces: source {} failed: {}", f.source_id, f.error);
                status.add(PartialReason::SourceFailure);
            }
        }
        progress.fetch_add(1, Ordering::Relaxed);
    }

    // Readers borrow the mappings (which stay put from here on); an
    // unparseable SFST is a SourceFailure. Sessions then borrow the
    // readers; the merge borrows the sessions — strictly sequential
    // borrows, no self-reference.
    let mut readers: Vec<(sfst::IndexReader<'_>, &TraceSource)> = Vec::new();
    for (mapped, source) in &mapped_sfsts {
        if cancel.is_cancelled() {
            return Ok(cancelled_empty(status));
        }
        if bloom_excludes(mapped.bytes(), query.trace_id) {
            continue;
        }
        match sfst::IndexReader::open(mapped.bytes()) {
            Ok(reader) => readers.push((reader, source)),
            Err(e) => {
                tracing::warn!(
                    "sfsq traces: source {} failed to parse: {e}",
                    source.source_id()
                );
                status.add(PartialReason::SourceFailure);
            }
        }
    }
    let mut sessions: Vec<sfst::TraceFileSession<'_, '_>> = readers
        .iter()
        .map(|(reader, _)| sfst::TraceFileSession::open(reader))
        .collect();

    // The combiner's input slice: sessions first, tails after; `origin`
    // maps a merge index back to a diagnostic label.
    let session_count = sessions.len();
    let mut origin: Vec<String> = readers
        .iter()
        .map(|(_, source)| source.source_id().to_string())
        .collect();
    let mut merged: Vec<&mut dyn SpanSource> = Vec::new();
    for session in sessions.iter_mut() {
        merged.push(session);
    }
    for (source_id, tail) in tails.iter_mut() {
        merged.push(tail);
        origin.push(source_id.to_string());
    }

    let outcome = combine(&mut merged, query.trace_id, query.span_cap, &|| {
        cancel.is_cancelled()
    });
    drop(merged);

    if outcome.truncated {
        status.add(PartialReason::SizeCap);
    }
    if outcome.cancelled {
        status.add(PartialReason::Cancelled);
    }
    for (idx, e) in &outcome.failures {
        let who = origin.get(*idx).map(String::as_str).unwrap_or("?");
        tracing::warn!("sfsq traces: source {who} failed during merge: {e}");
        status.add(PartialReason::SourceFailure);
    }

    // ── Field kinds: from exactly the contributing sources, projected
    // onto exactly the names the result exposes ───────────────────────
    let mut kinds: BTreeMap<String, sfst::ValueKind> = BTreeMap::new();
    let mut fold = |pairs: Vec<(String, sfst::ValueKind)>| {
        for (field, kind) in pairs {
            kinds
                .entry(field)
                .and_modify(|k| *k = sfst::join_value_kinds(*k, kind))
                .or_insert(kind);
        }
    };
    for &idx in &outcome.contributing_sources {
        if idx < session_count {
            fold(readers[idx].0.metadata().tree.derive_scalar_kinds());
        } else {
            fold(tails[idx - session_count].1.field_kinds().to_vec());
        }
    }
    // Exposed names per section. Span fields keep storage names; event/
    // link attribute names are prefix-stripped in the result, so their
    // kinds resolve through the storage prefix.
    use std::collections::BTreeSet;
    let mut field_names: BTreeSet<&str> = BTreeSet::new();
    let mut event_attr_names: BTreeSet<&str> = BTreeSet::new();
    let mut link_attr_names: BTreeSet<&str> = BTreeSet::new();
    for span in &outcome.trace.spans {
        field_names.extend(span.fields.iter().map(|(k, _)| k.as_str()));
        for ev in &span.events {
            event_attr_names.extend(ev.attributes.iter().map(|(k, _)| k.as_str()));
        }
        for lk in &span.links {
            link_attr_names.extend(lk.attributes.iter().map(|(k, _)| k.as_str()));
        }
    }
    let project = |names: &BTreeSet<&str>, prefix: &str| -> Vec<(String, sfst::ValueKind)> {
        names
            .iter()
            .filter_map(|name| {
                kinds
                    .get(&format!("{prefix}{name}"))
                    .map(|k| (name.to_string(), *k))
            })
            .collect()
    };
    let field_kinds = FieldKinds {
        fields: project(&field_names, ""),
        event_attributes: project(&event_attr_names, "events.attributes."),
        link_attributes: project(&link_attr_names, "links.attributes."),
    };

    let family = span_family(query.trace_id, &outcome.trace.spans);
    Ok(TraceData {
        trace: outcome.trace,
        status: status.finish(),
        field_kinds,
        family,
    })
}

/// Whether a file's trace-id bloom rules `trace_id` out, read from its table
/// of contents and bloom chunk alone, before anything else is decoded. A
/// missing or unreadable bloom rules nothing out: the full open reports a
/// broken file, and the session falls back to exact lookups.
fn bloom_excludes(bytes: &[u8], trace_id: sfst::TraceId) -> bool {
    let Ok(chunks) = sfst::ChunkReader::open(bytes) else {
        return false;
    };
    if !chunks.has_trace_id_bloom() {
        return false;
    }
    match chunks.trace_id_bloom() {
        Ok(bloom) => !bloom.might_contain(trace_id),
        Err(_) => false,
    }
}

/// The seal's derivation over one assembled trace. A span is an error when
/// its first `status_code` is `ERROR`, as the seal reads it.
fn span_family(trace_id: sfst::TraceId, spans: &[sfst::TraceSpan]) -> sfst::SpanFamily {
    let mut trace_ids = sfst::TraceIds::with_capacity(spans.len());
    let mut span_ids = sfst::SpanIds::with_capacity(spans.len());
    let mut parent_span_ids = sfst::ParentSpanIds::with_capacity(spans.len());
    let mut start_ns = Vec::with_capacity(spans.len());
    let mut duration_ns = Vec::with_capacity(spans.len());
    let mut is_error = Vec::with_capacity(spans.len());
    for span in spans {
        trace_ids.push(trace_id);
        span_ids.push(span.span_id);
        parent_span_ids.push(span.parent_span_id);
        start_ns.push(span.start_ns);
        duration_ns.push(span.duration_ns);
        let mut status = None;
        for (field, value) in &span.fields {
            if field == STATUS_FIELD {
                status = Some(value.as_str());
                break;
            }
        }
        is_error.push(status == Some("ERROR"));
    }
    sfst::derive_span_family(&sfst::SpanRows {
        trace_ids: &trace_ids,
        span_ids: &span_ids,
        parent_span_ids: &parent_span_ids,
        start_ns: &start_ns,
        duration_ns: &duration_ns,
        is_error: &is_error,
    })
    .expect("the rows are built index-parallel")
}
