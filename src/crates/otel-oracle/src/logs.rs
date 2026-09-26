//! Logs of a trace: the records a trace- or span-filtered `otel-logs` request
//! selects, rebuilt from OTLP and the documented storage rules (not from the
//! plugin's code):
//!
//! - a record's time is `time_unix_nano`, else `observed_time_unix_nano`; a
//!   record with neither is stamped by the agent's clock at ingest, which the
//!   calculator cannot know, so it is never judged;
//! - trace and span ids of the wrong length, or all zero, are unset;
//! - a record's stream is its resource's string `service.namespace` and
//!   `service.name` attributes (the last of each wins; a missing or non-string
//!   one is empty).
//!
//! Only what such a request judges is kept: time, ids and stream.

use std::collections::BTreeSet;
use std::ops::Range;

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::resource::v1::Resource;

use crate::model::id;

/// A logs stream: the service a record's resource names.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stream {
    pub namespace: String,
    pub name: String,
}

/// One stored log record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleLog {
    /// `None`: stamped at ingest (see the module rules).
    pub ts_ns: Option<i64>,
    pub trace_id: Option<[u8; 16]>,
    pub span_id: Option<[u8; 8]>,
    pub stream: Stream,
}

/// The records a request produces, in request order.
pub fn logs_of_request(request: &ExportLogsServiceRequest) -> Vec<OracleLog> {
    let mut out = Vec::new();
    for rl in &request.resource_logs {
        let stream = stream_of(rl.resource.as_ref());
        for sl in &rl.scope_logs {
            for record in &sl.log_records {
                let ts = if record.time_unix_nano != 0 {
                    record.time_unix_nano
                } else {
                    record.observed_time_unix_nano
                };
                out.push(OracleLog {
                    ts_ns: (ts != 0).then(|| i64::try_from(ts).unwrap_or(i64::MAX)),
                    trace_id: id::<16>(&record.trace_id),
                    span_id: id::<8>(&record.span_id),
                    stream: stream.clone(),
                });
            }
        }
    }
    out
}

fn stream_of(resource: Option<&Resource>) -> Stream {
    let mut stream = Stream::default();
    let Some(resource) = resource else {
        return stream;
    };
    for kv in &resource.attributes {
        let Some(any_value::Value::StringValue(text)) =
            kv.value.as_ref().and_then(|v| v.value.as_ref())
        else {
            continue;
        };
        match kv.key.as_str() {
            "service.namespace" => stream.namespace = text.clone(),
            "service.name" => stream.name = text.clone(),
            _ => {}
        }
    }
    stream
}

/// What a trace- or span-filtered request asks for.
#[derive(Debug, Clone)]
pub struct LogsWanted {
    /// `[start, end)`, unix nanoseconds.
    pub window: Range<i64>,
    /// Records of any of these traces; empty: no trace term.
    pub trace_ids: BTreeSet<[u8; 16]>,
    /// Records of any of these spans; empty: no span term.
    pub span_ids: BTreeSet<[u8; 8]>,
    /// `None`: every stream.
    pub streams: Option<BTreeSet<Stream>>,
}

impl LogsWanted {
    fn selects(&self, ts_ns: i64, log: &OracleLog) -> bool {
        let trace = self.trace_ids.is_empty()
            || log.trace_id.is_some_and(|id| self.trace_ids.contains(&id));
        let span =
            self.span_ids.is_empty() || log.span_id.is_some_and(|id| self.span_ids.contains(&id));
        let stream = self
            .streams
            .as_ref()
            .is_none_or(|streams| streams.contains(&log.stream));
        self.window.contains(&ts_ns) && trace && span && stream
    }
}

/// A record as a comparison sees it: ordered by time first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogKey {
    pub ts_ns: i64,
    pub trace_id: Option<[u8; 16]>,
    pub span_id: Option<[u8; 8]>,
    pub stream: Stream,
}

/// The records `wanted` selects, oldest first. Records of equal time are in
/// key order: the store breaks those ties its own way, so a comparison treats
/// each time's records as a multiset ([`page_diff`]).
pub fn logs_of(logs: &[OracleLog], wanted: &LogsWanted) -> Vec<LogKey> {
    let mut out = Vec::new();
    for log in logs {
        let Some(ts_ns) = log.ts_ns else {
            continue;
        };
        if !wanted.selects(ts_ns, log) {
            continue;
        }
        out.push(LogKey {
            ts_ns,
            trace_id: log.trace_id,
            span_id: log.span_id,
            stream: log.stream.clone(),
        });
    }
    out.sort();
    out
}

/// How `page` (the store's rows, in any order) differs from the `last`
/// records of `expected` closest to the page's start (`expected` in that
/// order: oldest first for a forward page, newest first for a backward one);
/// `None` when it does not. Every record closer than the page's farthest time
/// must be on it; at that time, any of the records there may be.
pub fn page_diff(expected: &[LogKey], page: &[LogKey], last: usize) -> Option<String> {
    let take = last.min(expected.len());
    if page.len() != take {
        return Some(format!("{} rows, expected {take}", page.len()));
    }
    if take == 0 {
        return None;
    }
    let edge = expected[take - 1].ts_ns;
    let mut want_inside = Vec::new();
    for key in &expected[..take] {
        if key.ts_ns != edge {
            want_inside.push(key.clone());
        }
    }
    let mut got_inside = Vec::new();
    let mut got_edge = Vec::new();
    for key in page {
        if key.ts_ns == edge {
            got_edge.push(key.clone());
        } else {
            got_inside.push(key.clone());
        }
    }
    want_inside.sort();
    got_inside.sort();
    if want_inside != got_inside {
        return Some(format!(
            "rows before time {edge} differ: expected {want_inside:?}, got {got_inside:?}"
        ));
    }
    let mut at_edge = Vec::new();
    for key in expected {
        if key.ts_ns == edge {
            at_edge.push(key.clone());
        }
    }
    for key in got_edge {
        match at_edge.iter().position(|candidate| *candidate == key) {
            Some(at) => {
                at_edge.swap_remove(at);
            }
            None => return Some(format!("row {key:?} at time {edge} is not expected there")),
        }
    }
    None
}

#[cfg(test)]
mod tests;
