//! Opaque pagination cursor for the log-row table.
//!
//! Encodes the global total order over log rows — `(timestamp_ns,
//! file_seq, sub_id, position)` — as a compact colon-delimited string.
//! It rides in the response's hidden cursor column and the consumer
//! echoes it back verbatim as the `anchor` request param. The `:`
//! separators keep it non-numeric, which the consuming UI relies on: it
//! NaN-guards a numeric parse of the pagination column, so a purely
//! numeric anchor would instead coerce to a wrong value.

/// Nanoseconds per second. The cursor orders by `timestamp_ns` (nanoseconds);
/// code holding second-granular values (e.g. `beyond_boundary` comparing SFST
/// summary bounds) multiplies by it to convert seconds → nanoseconds.
pub(super) const NS_PER_S: i64 = 1_000_000_000;

/// A decoded pagination cursor.
///
/// Ordering is lexicographic over `(timestamp_ns, file_seq, sub_id,
/// position)` — the total order the multi-file merge and the exclusive
/// anchor comparison rely on. `file_seq` is the SFST/WAL file's monotonic
/// `seq` (globally unique). `sub_id` distinguishes the parts of one
/// active WAL that share a `seq`: `0` for a sealed on-disk SFST, the
/// chunk index for an in-memory chunk, and [`Cursor::TAIL_SUB_ID`] for
/// the row-scanned tail (which sorts after every chunk). It only breaks
/// ties at equal `(timestamp_ns, file_seq)`, exactly as `position` breaks
/// ties within one chunk/file. `position` is time-sorted within an SFST,
/// or the insertion index within the tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cursor {
    pub timestamp_ns: i64,
    pub file_seq: u64,
    pub sub_id: u32,
    pub position: u32,
}

impl Cursor {
    /// `sub_id` for an on-disk SFST (the steady-state, single-part case).
    pub const SFST_SUB_ID: u32 = 0;
    /// `sub_id` for an active WAL's row-scanned tail — sorts after every
    /// chunk of the same `seq` (chunk indices are `0..n`).
    pub const TAIL_SUB_ID: u32 = u32::MAX;

    /// Encode as `"{timestamp_ns}:{file_seq}:{sub_id}:{position}"`.
    pub fn encode(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.timestamp_ns, self.file_seq, self.sub_id, self.position
        )
    }

    /// Decode the string form. Returns `None` for any malformed input
    /// (wrong field count, non-integer field, trailing garbage) so the
    /// handler can treat a bad anchor as "no anchor" rather than error.
    /// A legacy 3-field cursor (pre-`sub_id`) is therefore treated as no
    /// anchor — a one-time reset to the page edge across the upgrade.
    pub fn decode(s: &str) -> Option<Cursor> {
        let mut parts = s.split(':');
        let timestamp_ns: i64 = parts.next()?.parse().ok()?;
        let file_seq: u64 = parts.next()?.parse().ok()?;
        let sub_id: u32 = parts.next()?.parse().ok()?;
        let position: u32 = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Cursor {
            timestamp_ns,
            file_seq,
            sub_id,
            position,
        })
    }
}

#[cfg(test)]
mod tests;
