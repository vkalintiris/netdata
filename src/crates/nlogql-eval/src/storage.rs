//! `Backend` trait + supporting data types.
//!
//! The trait is the line of separation between the evaluator and
//! the data source. Two concrete impls land in Phase E:
//! [`mem::MemBackend`] (in-memory, test-only) in SOW-E2 and
//! [`sfst::SfstBackend`] (one `.sfst` file) in SOW-E3.

use std::hash::Hasher;

use nlogql::ast::{Matcher, MatcherOp};
use regex::Regex;
use twox_hash::XxHash64;

/// A label set identifying one log stream, plus a precomputed
/// signature for cheap equality / dedup / join keys.
///
/// `labels` is stored in canonical (sorted-by-name) order so that
/// `signature` is order-independent and any two `StreamMeta`s with
/// the same labels compare bit-for-bit equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamMeta {
    pub labels: Vec<(String, String)>,
    pub signature: u64,
}

impl StreamMeta {
    /// Build a `StreamMeta` from a list of `(name, value)` label
    /// pairs. Sorts the labels and computes the signature.
    pub fn new(mut labels: Vec<(String, String)>) -> Self {
        labels.sort();
        let signature = label_signature(&labels);
        Self { labels, signature }
    }
}

/// One log entry yielded by a backend. The backend produces only
/// the stream-level information (timestamp + raw text); pipeline
/// stages (parsers, label_format) add per-line labels in the
/// evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLine {
    /// Wall-clock timestamp in nanoseconds since the Unix epoch.
    pub timestamp_ns: i64,
    pub line: String,
}

/// A half-open time window `[start_ns, end_ns)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeRange {
    pub start_ns: i64,
    pub end_ns: i64,
}

impl TimeRange {
    /// Empty range that matches no lines.
    pub const EMPTY: Self = Self {
        start_ns: 0,
        end_ns: 0,
    };

    pub fn contains(&self, ts: i64) -> bool {
        ts >= self.start_ns && ts < self.end_ns
    }
}

/// Failure surfaced by a backend.
///
/// `Eq` is preserved because none of the variants carry non-Eq
/// payloads (we stringify [`std::io::Error`] for the `Io` arm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// Underlying I/O failure (file open, read, etc.). The string
    /// is the Display rendering of the original error.
    Io(String),
    /// A `=~` / `!~` matcher carries a regex that wouldn't compile.
    InvalidRegex { pattern: String, reason: String },
    /// Backend-specific failure not covered by the other variants.
    Other(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Io(msg) => write!(f, "backend i/o: {msg}"),
            BackendError::InvalidRegex { pattern, reason } => {
                write!(f, "backend invalid regex `{pattern}`: {reason}")
            }
            BackendError::Other(msg) => write!(f, "backend: {msg}"),
        }
    }
}

impl std::error::Error for BackendError {}

/// The evaluator's window into a data source.
///
/// Two operations:
///
/// - [`Backend::matching_streams`] resolves a stream selector
///   (list of matchers) to all label sets that satisfy it.
/// - [`Backend::lines_for`] yields the lines of one stream within
///   a time range, lazily.
///
/// Backends are responsible for compiling regex matchers (`=~`,
/// `!~`) — they may cache compilations or skip them entirely if
/// the matcher is unreachable given indexed data.
pub trait Backend {
    /// Resolve a selector to its matching streams. Returns the
    /// streams in a backend-defined order; callers that need
    /// determinism should sort by `signature` themselves.
    fn matching_streams(&self, matchers: &[Matcher]) -> Result<Vec<StreamMeta>, BackendError>;

    /// Yield lines from `stream` whose timestamps fall in `range`.
    /// The order within a stream is the backend's natural one —
    /// typically time-ascending.
    fn lines_for<'a>(
        &'a self,
        stream: &StreamMeta,
        range: TimeRange,
    ) -> Result<Box<dyn Iterator<Item = Result<RawLine, BackendError>> + 'a>, BackendError>;
}

// ===========================================================
// MemBackend (SOW-E2)
// ===========================================================

/// In-memory test backend. Build with [`MemBackend::builder`].
///
/// Streams and their lines are stored in insertion order; the
/// matcher engine scans them linearly. Fine for unit tests, not
/// intended for production data sizes.
#[derive(Debug, Default, Clone)]
pub struct MemBackend {
    streams: Vec<(StreamMeta, Vec<RawLine>)>,
}

impl MemBackend {
    /// Start a fluent builder for an empty backend.
    pub fn builder() -> MemBackendBuilder {
        MemBackendBuilder::default()
    }
}

/// Fluent builder for [`MemBackend`].
///
/// Usage:
///
/// ```ignore
/// let backend = MemBackend::builder()
///     .stream([("app", "foo"), ("env", "prod")])
///         .line(1_000, "first")
///         .line(2_000, "second")
///     .stream([("app", "bar")])
///         .line(1_500, "lone")
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct MemBackendBuilder {
    streams: Vec<(StreamMeta, Vec<RawLine>)>,
}

impl MemBackendBuilder {
    /// Begin a new stream identified by the given labels. Lines
    /// added after this call attach to this stream until `stream`
    /// is called again.
    pub fn stream<L, K, V>(mut self, labels: L) -> Self
    where
        L: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let owned: Vec<(String, String)> = labels
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self.streams.push((StreamMeta::new(owned), Vec::new()));
        self
    }

    /// Append a line to the most recently started stream.
    /// Panics in debug if no stream has been started yet — that's
    /// a builder misuse, not a runtime error worth surfacing.
    pub fn line<S: Into<String>>(mut self, timestamp_ns: i64, line: S) -> Self {
        let last = self
            .streams
            .last_mut()
            .expect("MemBackendBuilder::line called before stream()");
        last.1.push(RawLine {
            timestamp_ns,
            line: line.into(),
        });
        self
    }

    pub fn build(self) -> MemBackend {
        MemBackend {
            streams: self.streams,
        }
    }
}

/// A matcher with its regex (if any) pre-compiled.
#[derive(Debug)]
enum CompiledMatcher {
    Eq { name: String, value: String },
    NotEq { name: String, value: String },
    Match { name: String, regex: Regex },
    NotMatch { name: String, regex: Regex },
}

fn compile_matcher(m: &Matcher) -> Result<CompiledMatcher, BackendError> {
    match m.op {
        MatcherOp::Eq => Ok(CompiledMatcher::Eq {
            name: m.name.clone(),
            value: m.value.clone(),
        }),
        MatcherOp::NotEq => Ok(CompiledMatcher::NotEq {
            name: m.name.clone(),
            value: m.value.clone(),
        }),
        MatcherOp::Match => Ok(CompiledMatcher::Match {
            name: m.name.clone(),
            regex: compile_anchored(&m.value)?,
        }),
        MatcherOp::NotMatch => Ok(CompiledMatcher::NotMatch {
            name: m.name.clone(),
            regex: compile_anchored(&m.value)?,
        }),
    }
}

/// Wrap a user-supplied pattern in `^(?:…)$` so the regex matches
/// the entire label value, matching Loki / Prometheus semantics.
fn compile_anchored(pattern: &str) -> Result<Regex, BackendError> {
    Regex::new(&format!("^(?:{pattern})$")).map_err(|e| BackendError::InvalidRegex {
        pattern: pattern.to_string(),
        reason: e.to_string(),
    })
}

fn lookup_label<'a>(meta: &'a StreamMeta, name: &str) -> Option<&'a str> {
    meta.labels
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn matcher_satisfied(m: &CompiledMatcher, meta: &StreamMeta) -> bool {
    // Absent label is treated as `""` (Prometheus / Loki semantics).
    match m {
        CompiledMatcher::Eq { name, value } => lookup_label(meta, name).unwrap_or("") == value,
        CompiledMatcher::NotEq { name, value } => lookup_label(meta, name).unwrap_or("") != value,
        CompiledMatcher::Match { name, regex } => {
            regex.is_match(lookup_label(meta, name).unwrap_or(""))
        }
        CompiledMatcher::NotMatch { name, regex } => {
            !regex.is_match(lookup_label(meta, name).unwrap_or(""))
        }
    }
}

impl Backend for MemBackend {
    fn matching_streams(&self, matchers: &[Matcher]) -> Result<Vec<StreamMeta>, BackendError> {
        let compiled: Vec<CompiledMatcher> = matchers
            .iter()
            .map(compile_matcher)
            .collect::<Result<_, _>>()?;
        let mut out = Vec::new();
        for (meta, _) in &self.streams {
            if compiled.iter().all(|m| matcher_satisfied(m, meta)) {
                out.push(meta.clone());
            }
        }
        Ok(out)
    }

    fn lines_for<'a>(
        &'a self,
        stream: &StreamMeta,
        range: TimeRange,
    ) -> Result<Box<dyn Iterator<Item = Result<RawLine, BackendError>> + 'a>, BackendError> {
        let lines: Box<dyn Iterator<Item = Result<RawLine, BackendError>> + 'a> = match self
            .streams
            .iter()
            .find(|(m, _)| m.signature == stream.signature)
        {
            Some((_, lines)) => Box::new(
                lines
                    .iter()
                    .filter(move |l| range.contains(l.timestamp_ns))
                    .cloned()
                    .map(Ok),
            ),
            None => Box::new(std::iter::empty()),
        };
        Ok(lines)
    }
}

/// Deterministic hash of a sorted label set.
///
/// Computed over the canonical byte sequence
/// `(name \0 value \0)*` — order-independent only when the input
/// is already sorted (which [`StreamMeta::new`] guarantees). Uses
/// xxhash64 with a fixed seed so signatures are stable across
/// process invocations.
pub fn label_signature(sorted_labels: &[(String, String)]) -> u64 {
    let mut h = XxHash64::with_seed(0);
    for (k, v) in sorted_labels {
        h.write(k.as_bytes());
        h.write(&[0]);
        h.write(v.as_bytes());
        h.write(&[0]);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_order_independent_when_sorted_first() {
        let a = StreamMeta::new(vec![
            ("app".into(), "foo".into()),
            ("env".into(), "prod".into()),
        ]);
        let b = StreamMeta::new(vec![
            ("env".into(), "prod".into()),
            ("app".into(), "foo".into()),
        ]);
        assert_eq!(a.signature, b.signature);
        assert_eq!(a, b);
    }

    #[test]
    fn signature_differs_for_different_labels() {
        let a = StreamMeta::new(vec![("app".into(), "foo".into())]);
        let b = StreamMeta::new(vec![("app".into(), "bar".into())]);
        assert_ne!(a.signature, b.signature);
    }

    #[test]
    fn signature_avoids_concatenation_collision() {
        // Without a separator byte, `("ab", "cd")` and `("a", "bcd")`
        // would hash identically. The `\0` separator prevents that.
        let a = StreamMeta::new(vec![("ab".into(), "cd".into())]);
        let b = StreamMeta::new(vec![("a".into(), "bcd".into())]);
        assert_ne!(a.signature, b.signature);
    }

    #[test]
    fn signature_empty_label_set_is_stable() {
        let a = StreamMeta::new(Vec::new());
        let b = StreamMeta::new(Vec::new());
        assert_eq!(a.signature, b.signature);
    }

    #[test]
    fn time_range_contains_half_open() {
        let r = TimeRange {
            start_ns: 100,
            end_ns: 200,
        };
        assert!(r.contains(100));
        assert!(r.contains(199));
        assert!(!r.contains(200));
        assert!(!r.contains(99));
    }

    #[test]
    fn empty_time_range_contains_nothing() {
        assert!(!TimeRange::EMPTY.contains(0));
        assert!(!TimeRange::EMPTY.contains(-1));
        assert!(!TimeRange::EMPTY.contains(i64::MAX));
    }

    // -- MemBackend ------------------------------------------------

    use nlogql::span::Span;

    fn matcher(name: &str, op: MatcherOp, value: &str) -> Matcher {
        Matcher {
            name: name.to_string(),
            op,
            value: value.to_string(),
            span: Span::new(0, 0),
        }
    }

    fn three_stream_backend() -> MemBackend {
        MemBackend::builder()
            .stream([("app", "foo"), ("env", "prod")])
            .line(1_000, "alpha")
            .line(2_000, "bravo")
            .line(3_000, "charlie")
            .stream([("app", "foo"), ("env", "dev")])
            .line(1_500, "delta")
            .stream([("app", "bar")])
            .line(1_000, "echo")
            .line(4_000, "foxtrot")
            .build()
    }

    #[test]
    fn matching_streams_eq() {
        let b = three_stream_backend();
        let m = matcher("app", MatcherOp::Eq, "foo");
        let streams = b.matching_streams(&[m]).unwrap();
        assert_eq!(streams.len(), 2);
    }

    #[test]
    fn matching_streams_not_eq() {
        let b = three_stream_backend();
        let m = matcher("env", MatcherOp::NotEq, "prod");
        let streams = b.matching_streams(&[m]).unwrap();
        // Two streams match: app=foo/env=dev (env != prod) AND
        // app=bar (env absent → "" != "prod").
        assert_eq!(streams.len(), 2);
    }

    #[test]
    fn matching_streams_regex() {
        let b = three_stream_backend();
        let m = matcher("app", MatcherOp::Match, "foo|baz");
        let streams = b.matching_streams(&[m]).unwrap();
        assert_eq!(streams.len(), 2); // both foo streams; baz absent
    }

    #[test]
    fn matching_streams_not_regex() {
        let b = three_stream_backend();
        let m = matcher("app", MatcherOp::NotMatch, "foo");
        let streams = b.matching_streams(&[m]).unwrap();
        // app=foo (×2) excluded; app=bar matches.
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].labels[0], ("app".into(), "bar".into()));
    }

    #[test]
    fn matching_streams_anchored_regex() {
        // `^(?:.)$` anchoring: `app =~ "f"` shouldn't match "foo".
        let b = three_stream_backend();
        let m = matcher("app", MatcherOp::Match, "f");
        let streams = b.matching_streams(&[m]).unwrap();
        assert_eq!(streams.len(), 0, "anchored regex must not substring-match");
    }

    #[test]
    fn matching_streams_absent_label_as_empty() {
        // `env = ""` should match streams where env is absent.
        let b = three_stream_backend();
        let m = matcher("env", MatcherOp::Eq, "");
        let streams = b.matching_streams(&[m]).unwrap();
        // Only the app=bar stream has no env.
        assert_eq!(streams.len(), 1);
        assert!(
            streams[0]
                .labels
                .iter()
                .any(|(k, v)| k == "app" && v == "bar"),
        );
    }

    #[test]
    fn matching_streams_present_label_via_not_empty() {
        // `env != ""` matches every stream that HAS env.
        let b = three_stream_backend();
        let m = matcher("env", MatcherOp::NotEq, "");
        let streams = b.matching_streams(&[m]).unwrap();
        assert_eq!(streams.len(), 2);
    }

    #[test]
    fn matching_streams_and_composition() {
        // Multiple matchers AND together.
        let b = three_stream_backend();
        let ms = vec![
            matcher("app", MatcherOp::Eq, "foo"),
            matcher("env", MatcherOp::Eq, "prod"),
        ];
        let streams = b.matching_streams(&ms).unwrap();
        assert_eq!(streams.len(), 1);
    }

    #[test]
    fn matching_streams_invalid_regex_errors() {
        let b = three_stream_backend();
        let m = matcher("app", MatcherOp::Match, "[unclosed");
        let err = b.matching_streams(&[m]).unwrap_err();
        match err {
            BackendError::InvalidRegex { pattern, .. } => {
                assert_eq!(pattern, "[unclosed");
            }
            other => panic!("expected InvalidRegex, got {other:?}"),
        }
    }

    #[test]
    fn lines_for_yields_within_time_range() {
        let b = three_stream_backend();
        let stream = StreamMeta::new(vec![
            ("app".into(), "foo".into()),
            ("env".into(), "prod".into()),
        ]);
        let lines: Vec<RawLine> = b
            .lines_for(
                &stream,
                TimeRange {
                    start_ns: 1_500,
                    end_ns: 3_000,
                },
            )
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        // bravo at 2000 only; alpha (1000) below, charlie (3000) is end-exclusive.
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].line, "bravo");
    }

    #[test]
    fn lines_for_unknown_stream_is_empty() {
        let b = three_stream_backend();
        let phantom = StreamMeta::new(vec![("does".into(), "not_exist".into())]);
        let lines: Vec<_> = b
            .lines_for(
                &phantom,
                TimeRange {
                    start_ns: 0,
                    end_ns: i64::MAX,
                },
            )
            .unwrap()
            .collect();
        assert!(lines.is_empty());
    }

    #[test]
    fn lines_for_all_in_range() {
        let b = three_stream_backend();
        let stream = StreamMeta::new(vec![("app".into(), "bar".into())]);
        let lines: Vec<RawLine> = b
            .lines_for(
                &stream,
                TimeRange {
                    start_ns: 0,
                    end_ns: i64::MAX,
                },
            )
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line, "echo");
        assert_eq!(lines[1].line, "foxtrot");
    }

    #[test]
    fn lines_for_empty_range() {
        let b = three_stream_backend();
        let stream = StreamMeta::new(vec![("app".into(), "bar".into())]);
        let lines: Vec<_> = b.lines_for(&stream, TimeRange::EMPTY).unwrap().collect();
        assert!(lines.is_empty());
    }

    #[test]
    fn builder_handles_empty_backend() {
        let b = MemBackend::builder().build();
        let streams = b.matching_streams(&[]).unwrap();
        assert!(streams.is_empty());
    }

    #[test]
    fn empty_matcher_list_returns_all_streams() {
        // No matchers = match everything.
        let b = three_stream_backend();
        let streams = b.matching_streams(&[]).unwrap();
        assert_eq!(streams.len(), 3);
    }

    #[test]
    fn backend_error_display() {
        let e = BackendError::Io("connection refused".into());
        assert_eq!(e.to_string(), "backend i/o: connection refused");
        let e = BackendError::InvalidRegex {
            pattern: "[".into(),
            reason: "unclosed character class".into(),
        };
        assert!(e.to_string().contains("`[`"));
    }
}
