//! Which captured spans the agent kept, replayed from the documented ingestion
//! window (not from the plugin's code):
//!
//! - the window is `[now − max_age, now + future_skew]`, `now` being when the
//!   request arrived; the capture's receive time stands in for the agent's
//!   clock;
//! - a span is kept when its start is inside the lower bound and its effective
//!   end (the end, or the start when the end is unset or before it) is inside
//!   the upper bound, both edges inclusive;
//! - a span without a start gets one from the agent's clock, so it is exempt
//!   from the lower bound and only its raw end faces the upper bound; the
//!   calculator cannot know that start, so such spans are counted apart;
//! - the agent answers with how many spans it rejected.
//!
//! The two clocks differ by the relay's latency, so a span closer than
//! `edge_ns` to either bound is left undecided; the agent's rejected count
//! settles those.

use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest, trace::v1::Span,
};

use crate::capture::Record;
use crate::model::{OracleSpan, spans_of_request};

/// The calculator's own copy of the ingestion limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestWindow {
    pub max_age_ns: u64,
    pub future_skew_ns: u64,
    /// A span closer than this to a bound stays undecided; 0 decides every
    /// span exactly.
    pub edge_ns: u64,
}

impl IngestWindow {
    /// The agent's defaults (24 h back, 10 min ahead) with a 5 s margin.
    pub const LAB: IngestWindow = IngestWindow {
        max_age_ns: 24 * 3_600 * 1_000_000_000,
        future_skew_ns: 10 * 60 * 1_000_000_000,
        edge_ns: 5 * 1_000_000_000,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Kept,
    Rejected,
    /// Within `edge_ns` of a bound: kept or rejected depending on the clocks.
    Undecided,
    /// No start: kept, at a start only the agent knows.
    Synthesized,
}

pub fn verdict(span: &Span, now_ns: u64, window: &IngestWindow) -> Verdict {
    let min = now_ns.saturating_sub(window.max_age_ns);
    let max = now_ns.saturating_add(window.future_skew_ns);
    let near = |value: u64, bound: u64| value.abs_diff(bound) < window.edge_ns;

    if span.start_time_unix_nano == 0 {
        let end = span.end_time_unix_nano;
        if near(end, max) {
            return Verdict::Undecided;
        }
        return if end > max {
            Verdict::Rejected
        } else {
            Verdict::Synthesized
        };
    }

    let start = span.start_time_unix_nano;
    let end = span.end_time_unix_nano.max(start);
    if near(start, min) || near(end, max) {
        return Verdict::Undecided;
    }
    if start < min || end > max {
        Verdict::Rejected
    } else {
        Verdict::Kept
    }
}

/// One traces request replayed against the window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Replay {
    pub kept: Vec<OracleSpan>,
    pub undecided: Vec<OracleSpan>,
    pub rejected: u64,
    pub synthesized: u64,
}

impl Replay {
    /// The rows the agent stored, once its rejected count settles the
    /// undecided spans: all kept when it matches the certain rejections, all
    /// rejected when it also covers every undecided span; otherwise `None`,
    /// for the caller to report (a mixed or impossible count).
    pub fn stored(&self, reported_rejected: i64) -> Option<Vec<OracleSpan>> {
        let certain = i64::try_from(self.rejected).ok()?;
        let undecided = i64::try_from(self.undecided.len()).ok()?;

        if reported_rejected == certain {
            let mut stored = self.kept.clone();
            stored.extend(self.undecided.iter().cloned());
            return Some(stored);
        }
        if undecided > 0 && reported_rejected == certain + undecided {
            return Some(self.kept.clone());
        }
        None
    }
}

/// Replays `request`, received at `now_ns`; the rows carry unit 0 until the
/// membership assigns them.
pub fn replay(request: &ExportTraceServiceRequest, now_ns: u64, window: &IngestWindow) -> Replay {
    let mut verdicts = Vec::new();
    for rs in &request.resource_spans {
        for ss in &rs.scope_spans {
            for span in &ss.spans {
                verdicts.push(verdict(span, now_ns, window));
            }
        }
    }

    let mut out = Replay::default();
    for (row, verdict) in spans_of_request(request, 0).into_iter().zip(verdicts) {
        match verdict {
            Verdict::Kept => out.kept.push(row),
            Verdict::Undecided => out.undecided.push(row),
            Verdict::Rejected => out.rejected += 1,
            Verdict::Synthesized => out.synthesized += 1,
        }
    }
    out
}

/// Replays a captured traces record at its receive time.
pub fn replay_record(record: &Record, window: &IngestWindow) -> Result<Replay, prost::DecodeError> {
    let request = record.traces()?;
    let now_ns = u64::try_from(record.received_unix_ns).unwrap_or(0);
    Ok(replay(&request, now_ns, window))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Signal;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans};
    use prost::Message;

    const NOW: u64 = 1_790_000_000_000_000_000;
    const EXACT: IngestWindow = IngestWindow {
        max_age_ns: 1_000,
        future_skew_ns: 100,
        edge_ns: 0,
    };

    fn span(id: u8, start: u64, end: u64) -> Span {
        Span {
            trace_id: vec![1; 16],
            span_id: vec![id; 8],
            start_time_unix_nano: start,
            end_time_unix_nano: end,
            ..Default::default()
        }
    }

    fn request(spans: Vec<Span>) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    #[test]
    fn judges_each_edge_to_the_nanosecond() {
        let min = NOW - 1_000;
        let max = NOW + 100;
        let cases = [
            ("start on the lower bound", span(1, min, min), Verdict::Kept),
            (
                "start 1 ns before it",
                span(1, min - 1, NOW),
                Verdict::Rejected,
            ),
            ("end on the upper bound", span(1, NOW, max), Verdict::Kept),
            ("end 1 ns past it", span(1, NOW, max + 1), Verdict::Rejected),
            (
                "an unset end judged at the start",
                span(1, NOW, 0),
                Verdict::Kept,
            ),
            (
                "an end before the start judged at the start",
                span(1, max, NOW),
                Verdict::Kept,
            ),
            (
                "a start past the upper bound despite an early end",
                span(1, max + 1, NOW),
                Verdict::Rejected,
            ),
            (
                "no start, an end inside",
                span(1, 0, max),
                Verdict::Synthesized,
            ),
            (
                "no start, an end past the upper bound",
                span(1, 0, max + 1),
                Verdict::Rejected,
            ),
            ("no start and no end", span(1, 0, 0), Verdict::Synthesized),
        ];
        for (name, span, expected) in cases {
            assert_eq!(verdict(&span, NOW, &EXACT), expected, "{name}");
        }
    }

    #[test]
    fn leaves_spans_near_a_bound_undecided() {
        let window = IngestWindow {
            edge_ns: 10,
            ..EXACT
        };
        let min = NOW - 1_000;
        let max = NOW + 100;
        let cases = [
            (
                "start just inside the lower bound",
                span(1, min + 9, NOW),
                Verdict::Undecided,
            ),
            (
                "start just outside it",
                span(1, min - 9, NOW),
                Verdict::Undecided,
            ),
            ("start clear of it", span(1, min + 10, NOW), Verdict::Kept),
            (
                "end just past the upper bound",
                span(1, NOW, max + 9),
                Verdict::Undecided,
            ),
            (
                "end clear past it",
                span(1, NOW, max + 10),
                Verdict::Rejected,
            ),
            (
                "no start, an end near the upper bound",
                span(1, 0, max - 9),
                Verdict::Undecided,
            ),
        ];
        for (name, span, expected) in cases {
            assert_eq!(verdict(&span, NOW, &window), expected, "{name}");
        }
    }

    #[test]
    fn the_rejected_count_settles_the_undecided_spans() {
        let window = IngestWindow {
            edge_ns: 10,
            ..EXACT
        };
        let replayed = replay(
            &request(vec![
                span(1, NOW, NOW + 50),
                span(2, NOW - 5_000, NOW),
                span(3, NOW, NOW + 105),
                span(4, 0, NOW),
            ]),
            NOW,
            &window,
        );

        assert_eq!(
            (
                replayed.kept.len(),
                replayed.undecided.len(),
                replayed.rejected,
                replayed.synthesized
            ),
            (1, 1, 1, 1)
        );
        let ids = |rows: Option<Vec<OracleSpan>>| {
            rows.map(|rows| {
                rows.iter()
                    .map(|row| row.span_id.unwrap()[0])
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(ids(replayed.stored(1)), Some(vec![1, 3]));
        assert_eq!(ids(replayed.stored(2)), Some(vec![1]));
        assert_eq!(ids(replayed.stored(0)), None);
        assert_eq!(ids(replayed.stored(3)), None);
    }

    #[test]
    fn a_rejected_count_that_disagrees_is_reported() {
        let replayed = replay(&request(vec![span(1, NOW, NOW)]), NOW, &EXACT);

        assert_eq!(replayed.stored(0).map(|rows| rows.len()), Some(1));
        assert_eq!(replayed.stored(1), None);
    }

    #[test]
    fn replays_a_record_at_its_receive_time() {
        let record = Record {
            received_unix_ns: i64::try_from(NOW).unwrap(),
            signal: Signal::Traces,
            grpc_code: 0,
            rejected: 1,
            request: request(vec![span(1, NOW, NOW), span(2, NOW - 2_000, NOW)]).encode_to_vec(),
        };

        let replayed = replay_record(&record, &EXACT).unwrap();

        assert_eq!((replayed.kept.len(), replayed.rejected), (1, 1));
        assert_eq!(
            replayed.stored(record.rejected).map(|rows| rows.len()),
            Some(1)
        );
    }
}
