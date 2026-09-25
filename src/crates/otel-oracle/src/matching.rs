//! Which captured spans the store holds, and which windows can be judged.
//!
//! The capture says what the agent was sent and how it answered; the ingest
//! replay says which spans it kept; membership says which rows the store holds
//! and in which unit. Matching pairs the two as multisets of row keys:
//!
//! - a span the agent certainly kept claims a stored row; with none left it
//!   is lost;
//! - a span it may have rejected (an error answer, or a rejected count the
//!   replay cannot settle) only takes a row nobody claimed, and is then
//!   reported as stored despite the doubt;
//! - a stored row nobody claims is unmatched, and its unit is not fully known;
//! - one key with two different contents is a collision; the units holding it
//!   are not fully known.
//!
//! A window can be judged when nothing was lost or left unmatched inside it,
//! every unit it overlaps is fully known, and no set-aside WAL overlaps it.
//! The calculator's input for a judged window is every row of the units it
//! overlaps.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::capture::{Record, Signal};
use crate::ingest::{IngestWindow, replay_record};
use crate::membership::{RowKey, StaleWal, Unit};
use crate::model::OracleSpan;

/// A span the capture expects in the store, and the record that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub record: usize,
    pub span: OracleSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestNote {
    /// A traces record whose request does not decode.
    Undecodable { record: usize },
    /// An acknowledged record whose rejected count the replay cannot explain.
    RejectedCount {
        record: usize,
        reported: i64,
        certain: u64,
        undecided: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Expected {
    pub kept: Vec<Claim>,
    pub doubtful: Vec<Claim>,
    /// Spans without a start: stored at a start only the agent knows, so they
    /// can never match.
    pub synthesized: u64,
    pub notes: Vec<IngestNote>,
}

fn claims(record: usize, spans: Vec<OracleSpan>) -> impl Iterator<Item = Claim> {
    spans.into_iter().map(move |span| Claim { record, span })
}

/// The rows the capture's traces records should have left in the store.
pub fn expected_rows(records: &[Record], window: &IngestWindow) -> Expected {
    let mut out = Expected::default();
    for (index, record) in records.iter().enumerate() {
        if record.signal != Signal::Traces {
            continue;
        }
        let Ok(replay) = replay_record(record, window) else {
            out.notes.push(IngestNote::Undecodable { record: index });
            continue;
        };
        out.synthesized += replay.synthesized;

        let stored = if record.acknowledged() {
            replay.stored(record.rejected)
        } else {
            None
        };
        match stored {
            Some(rows) => out.kept.extend(claims(index, rows)),
            None => {
                if record.acknowledged() {
                    out.notes.push(IngestNote::RejectedCount {
                        record: index,
                        reported: record.rejected,
                        certain: replay.rejected,
                        undecided: replay.undecided.len(),
                    });
                }
                out.doubtful.extend(claims(index, replay.kept));
                out.doubtful.extend(claims(index, replay.undecided));
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Matched {
    /// Every matched row, its `unit` set to the index of the unit holding it.
    pub spans: Vec<OracleSpan>,
    pub lost: Vec<Claim>,
    /// Stored rows no span claimed: (unit index, key).
    pub unmatched: Vec<(usize, RowKey)>,
    /// Doubtful spans that did find a stored row (also in `spans`).
    pub despite_error: Vec<Claim>,
    pub collisions: Vec<RowKey>,
    /// Per unit: every row claimed and no collision.
    pub known: Vec<bool>,
}

fn same_content(a: &OracleSpan, b: &OracleSpan) -> bool {
    a.fields == b.fields && a.parent_span_id == b.parent_span_id
}

pub fn match_rows(expected: &Expected, units: &[Unit]) -> Matched {
    let mut stored: BTreeMap<RowKey, VecDeque<usize>> = BTreeMap::new();
    for (index, unit) in units.iter().enumerate() {
        for key in &unit.rows {
            stored.entry(*key).or_default().push_back(index);
        }
    }

    let mut out = Matched {
        known: vec![true; units.len()],
        ..Matched::default()
    };
    let mut content: BTreeMap<RowKey, &OracleSpan> = BTreeMap::new();
    let mut collided: BTreeSet<RowKey> = BTreeSet::new();

    for (claims, certain) in [(&expected.kept, true), (&expected.doubtful, false)] {
        for claim in claims {
            let key = RowKey::of(&claim.span);
            match content.get(&key) {
                Some(first) if !same_content(first, &claim.span) => {
                    collided.insert(key);
                }
                Some(_) => {}
                None => {
                    content.insert(key, &claim.span);
                }
            }

            match stored.get_mut(&key).and_then(|units| units.pop_front()) {
                Some(unit) => {
                    let mut span = claim.span.clone();
                    span.unit = unit;
                    out.spans.push(span);
                    if !certain {
                        out.despite_error.push(claim.clone());
                    }
                }
                None if certain => out.lost.push(claim.clone()),
                None => {}
            }
        }
    }

    for (key, left) in stored {
        for unit in left {
            out.unmatched.push((unit, key));
            out.known[unit] = false;
        }
    }
    for key in &collided {
        for (index, unit) in units.iter().enumerate() {
            if unit.rows.contains(key) {
                out.known[index] = false;
            }
        }
    }
    out.collisions = collided.into_iter().collect();
    out
}

/// Whether a unit with these start seconds holds rows of `[after_s, before_s)`.
pub fn overlaps(seconds: Option<(u32, u32)>, after_s: u32, before_s: u32) -> bool {
    seconds.is_some_and(|(min, max)| max >= after_s && min < before_s)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCheck {
    /// Indexes of the units the window overlaps.
    pub units: Vec<usize>,
    /// Certainly-kept spans starting inside the window that were not stored.
    pub lost: usize,
    /// Stored rows no span claimed, in the units the window overlaps.
    pub unmatched: usize,
    /// Overlapping units that are not fully known.
    pub unknown: Vec<usize>,
    /// Whether a set-aside WAL may hold rows of the window.
    pub stale: bool,
}

impl WindowCheck {
    /// ORC-INGEST for the window: nothing sent went missing, nothing stored is
    /// unexplained.
    pub fn ingest_ok(&self) -> bool {
        self.lost == 0 && self.unmatched == 0
    }

    /// Whether the calculator can judge the window at all.
    pub fn judged(&self) -> bool {
        self.ingest_ok() && self.unknown.is_empty() && !self.stale
    }
}

fn start_second(span: &OracleSpan) -> i64 {
    span.start_ns.div_euclid(1_000_000_000)
}

/// Checks the window `[after_s, before_s)`; for explore answers pass the
/// window aligned to its grid, which is what the agent reads.
pub fn check_window(
    units: &[Unit],
    matched: &Matched,
    stale: &[StaleWal],
    after_s: u32,
    before_s: u32,
) -> WindowCheck {
    let overlapping: Vec<usize> = (0..units.len())
        .filter(|&index| overlaps(units[index].seconds, after_s, before_s))
        .collect();
    let in_window = |second: i64| second >= i64::from(after_s) && second < i64::from(before_s);

    WindowCheck {
        lost: matched
            .lost
            .iter()
            .filter(|claim| in_window(start_second(&claim.span)))
            .count(),
        unmatched: matched
            .unmatched
            .iter()
            .filter(|(unit, _)| overlapping.contains(unit))
            .count(),
        unknown: overlapping
            .iter()
            .copied()
            .filter(|&unit| !matched.known[unit])
            .collect(),
        stale: stale.iter().any(|wal| wal.overlaps(after_s, before_s)),
        units: overlapping,
    }
}

/// The calculator's input for a window: every matched row of the units it
/// overlaps.
pub fn window_spans(matched: &Matched, check: &WindowCheck) -> Vec<OracleSpan> {
    matched
        .spans
        .iter()
        .filter(|span| check.units.contains(&span.unit))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use opentelemetry_proto::tonic::{
        collector::trace::v1::ExportTraceServiceRequest,
        trace::v1::{ResourceSpans, ScopeSpans, Span},
    };
    use prost::Message;

    use super::*;
    use crate::membership::{Stem, UnitKind};
    use crate::model::spans_of_request;

    const NOW: u64 = 1_790_000_000_000_000_000;
    const NOW_S: u32 = 1_790_000_000;
    const OPEN: IngestWindow = IngestWindow {
        max_age_ns: 1_000_000_000_000,
        future_skew_ns: 1_000_000_000_000,
        edge_ns: 0,
    };
    const STEM: &str = "baf93e178ba34a37bdf778e397883163-d5439cb5a64e4ceb89c297cbcd8ee6f2-00001-0000000079-0000000000000000";

    fn span(id: u8, second: u64, name: &str) -> Span {
        let start = NOW + second * 1_000_000_000;
        Span {
            trace_id: vec![1; 16],
            span_id: vec![id; 8],
            start_time_unix_nano: start,
            end_time_unix_nano: start + 1_000,
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn request(spans: &[Span]) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: spans.to_vec(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    fn record(spans: &[Span], grpc_code: i32, rejected: i64) -> Record {
        Record {
            received_unix_ns: i64::try_from(NOW).unwrap(),
            signal: Signal::Traces,
            grpc_code,
            rejected,
            request: request(spans).encode_to_vec(),
        }
    }

    fn keys(spans: &[Span]) -> Vec<RowKey> {
        spans_of_request(&request(spans), 0)
            .iter()
            .map(RowKey::of)
            .collect()
    }

    fn unit(kind: UnitKind, rows: Vec<RowKey>) -> Unit {
        let seconds = rows.iter().map(|row| row.start_ns / 1_000_000_000);
        let (min, max) = (seconds.clone().min(), seconds.max());
        Unit {
            path: PathBuf::from(format!("{STEM}.wal")),
            stem: Stem::parse(STEM).unwrap(),
            kind,
            seconds: min
                .zip(max)
                .map(|(min, max)| (u32::try_from(min).unwrap(), u32::try_from(max).unwrap())),
            rows,
        }
    }

    fn ids(spans: &[OracleSpan]) -> Vec<(u8, usize)> {
        spans
            .iter()
            .map(|span| (span.span_id.unwrap()[0], span.unit))
            .collect()
    }

    fn claim_ids(claims: &[Claim]) -> Vec<u8> {
        claims
            .iter()
            .map(|claim| claim.span.span_id.unwrap()[0])
            .collect()
    }

    #[test]
    fn a_resent_span_matches_one_row_in_each_unit() {
        let a = span(1, 0, "a");
        let expected = expected_rows(
            &[
                record(std::slice::from_ref(&a), 0, 0),
                record(std::slice::from_ref(&a), 0, 0),
            ],
            &OPEN,
        );
        let units = [
            unit(UnitKind::Sealed, keys(std::slice::from_ref(&a))),
            unit(UnitKind::Tail(0), keys(&[a])),
        ];

        let matched = match_rows(&expected, &units);

        assert_eq!(ids(&matched.spans), vec![(1, 0), (1, 1)]);
        assert!(matched.lost.is_empty() && matched.unmatched.is_empty());
        assert_eq!(matched.known, vec![true, true]);
    }

    #[test]
    fn finds_lost_unmatched_and_stored_despite_an_error() {
        let (a, b, c, d) = (
            span(1, 0, "a"),
            span(2, 1, "b"),
            span(3, 2, "c"),
            span(4, 3, "d"),
        );
        let refused = span(5, 4, "refused");
        let expected = expected_rows(
            &[
                record(&[a.clone(), b], 0, 0),
                record(std::slice::from_ref(&c), 14, 0),
                record(&[refused], 14, 0),
            ],
            &OPEN,
        );
        let units = [unit(UnitKind::Sealed, keys(&[a, c, d.clone()]))];

        let matched = match_rows(&expected, &units);

        assert_eq!(ids(&matched.spans), vec![(1, 0), (3, 0)]);
        assert_eq!(claim_ids(&matched.lost), vec![2]);
        assert_eq!(claim_ids(&matched.despite_error), vec![3]);
        assert_eq!(matched.unmatched, vec![(0, keys(&[d])[0])]);
        assert_eq!(matched.known, vec![false]);
    }

    #[test]
    fn a_key_stored_with_two_contents_is_a_collision() {
        let (x, y) = (span(1, 0, "x"), span(1, 0, "y"));
        let expected = expected_rows(
            &[record(std::slice::from_ref(&x), 0, 0), record(&[y], 0, 0)],
            &OPEN,
        );
        let key = keys(&[x])[0];
        let units = [
            unit(UnitKind::Sealed, vec![key]),
            unit(UnitKind::Tail(0), vec![key]),
            unit(UnitKind::Chunk(0), keys(&[span(9, 5, "z")])),
        ];

        let matched = match_rows(&expected, &units);

        assert_eq!(matched.collisions, vec![key]);
        assert_eq!(matched.known, vec![false, false, false]);
    }

    #[test]
    fn a_span_without_a_start_is_counted_and_its_row_left_unmatched() {
        let mut startless = span(1, 0, "a");
        startless.start_time_unix_nano = 0;
        let expected = expected_rows(&[record(&[startless], 0, 0)], &OPEN);
        let stored_by_the_agent = keys(&[span(1, 0, "a")]);

        let matched = match_rows(&expected, &[unit(UnitKind::Tail(0), stored_by_the_agent)]);

        assert_eq!((expected.synthesized, expected.kept.len()), (1, 0));
        assert_eq!(matched.unmatched.len(), 1);
    }

    #[test]
    fn notes_records_it_cannot_explain_and_skips_logs() {
        let mut garbage = record(&[], 0, 0);
        garbage.request = vec![0xff, 0xff, 0xff];
        let mut logs = record(&[span(7, 0, "log")], 0, 0);
        logs.signal = Signal::Logs;
        let records = [
            garbage,
            record(&[span(1, 0, "a")], 0, 5),
            logs,
            record(&[span(2, 0, "b")], 13, 0),
        ];

        let expected = expected_rows(&records, &OPEN);

        assert_eq!(
            expected.notes,
            vec![
                IngestNote::Undecodable { record: 0 },
                IngestNote::RejectedCount {
                    record: 1,
                    reported: 5,
                    certain: 0,
                    undecided: 0
                },
            ]
        );
        assert!(expected.kept.is_empty());
        assert_eq!(claim_ids(&expected.doubtful), vec![1, 2]);
    }

    #[test]
    fn overlap_includes_a_unit_ending_at_the_start_and_excludes_one_starting_at_the_end() {
        let cases = [
            ("ends where the window starts", Some((10, 20)), 20, 30, true),
            ("starts where the window ends", Some((10, 20)), 5, 10, false),
            ("inside", Some((12, 14)), 10, 20, true),
            ("after", Some((21, 25)), 10, 21, false),
            ("no rows", None, 0, u32::MAX, false),
        ];
        for (name, seconds, after, before, expected) in cases {
            assert_eq!(overlaps(seconds, after, before), expected, "{name}");
        }
    }

    #[test]
    fn a_window_is_judged_only_when_nothing_in_it_is_unexplained() {
        let (a, b, c) = (span(1, 0, "a"), span(2, 100, "b"), span(3, 101, "c"));
        let stray = span(4, 1, "stray");
        let expected = expected_rows(&[record(&[a.clone(), b.clone(), c], 0, 0)], &OPEN);
        let units = [
            unit(UnitKind::Sealed, keys(&[a, stray])),
            unit(UnitKind::Tail(0), keys(&[b])),
        ];
        let matched = match_rows(&expected, &units);
        let stale_elsewhere = [StaleWal {
            path: PathBuf::from("old.wal"),
            seconds: Some((NOW_S + 500, NOW_S + 600)),
            readable: true,
        }];

        let early = check_window(&units, &matched, &stale_elsewhere, NOW_S, NOW_S + 10);
        assert_eq!(
            early,
            WindowCheck {
                units: vec![0],
                lost: 0,
                unmatched: 1,
                unknown: vec![0],
                stale: false
            }
        );
        assert!(!early.ingest_ok());

        let late = check_window(&units, &matched, &stale_elsewhere, NOW_S + 100, NOW_S + 102);
        assert_eq!((late.units.clone(), late.lost), (vec![1], 1));
        assert!(!late.judged());

        let clean = check_window(&units, &matched, &stale_elsewhere, NOW_S + 100, NOW_S + 101);
        assert!(clean.judged());
        assert_eq!(ids(&window_spans(&matched, &clean)), vec![(2, 1)]);

        let with_stale = check_window(&units, &matched, &stale_elsewhere, NOW_S + 100, NOW_S + 501);
        assert!(with_stale.stale && !with_stale.judged());
    }

    #[test]
    fn one_dropped_record_leaves_exactly_its_rows_unmatched() {
        let batches = [
            vec![span(1, 0, "a"), span(2, 1, "b")],
            vec![span(3, 2, "c"), span(4, 3, "d")],
            vec![span(5, 4, "e")],
        ];
        let all: Vec<Span> = batches.iter().flatten().cloned().collect();
        let units = [unit(UnitKind::Sealed, keys(&all))];
        let records = [record(&batches[0], 0, 0), record(&batches[2], 0, 0)];

        let matched = match_rows(&expected_rows(&records, &OPEN), &units);

        let unmatched: Vec<RowKey> = matched.unmatched.iter().map(|(_, key)| *key).collect();
        assert_eq!(unmatched, keys(&batches[1]));
        assert!(matched.lost.is_empty());
    }
}
