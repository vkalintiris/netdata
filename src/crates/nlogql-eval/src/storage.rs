//! `Backend` trait + supporting data types.
//!
//! The trait is the line of separation between the evaluator and
//! the data source. Two concrete impls land in Phase E:
//! [`mem::MemBackend`] (in-memory, test-only) in SOW-E2 and
//! [`sfst::SfstBackend`] (one `.sfst` file) in SOW-E3.

use std::hash::Hasher;

use nlogql::ast::Matcher;
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
    fn matching_streams(
        &self,
        matchers: &[Matcher],
    ) -> Result<Vec<StreamMeta>, BackendError>;

    /// Yield lines from `stream` whose timestamps fall in `range`.
    /// The order within a stream is the backend's natural one —
    /// typically time-ascending.
    fn lines_for<'a>(
        &'a self,
        stream: &StreamMeta,
        range: TimeRange,
    ) -> Result<
        Box<dyn Iterator<Item = Result<RawLine, BackendError>> + 'a>,
        BackendError,
    >;
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
        let r = TimeRange { start_ns: 100, end_ns: 200 };
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
