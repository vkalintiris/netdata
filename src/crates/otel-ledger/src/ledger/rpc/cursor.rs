//! Opaque pagination cursor for the otel-logs row table.
//!
//! Encodes the global total order over log rows — `(timestamp_ns,
//! file_seq, position)` — as a compact colon-delimited string. It rides
//! in the response's hidden cursor column and the UI echoes it back
//! verbatim as the `anchor` request param. The `:` separators keep it
//! non-numeric, which the cloud-frontend histogram-hover handler
//! tolerates (it NaN-guards a numeric parse of the pagination column;
//! a purely numeric anchor would instead coerce to a wrong value).
//!
//! Wired into the pagination engine in a following commit.
#![allow(dead_code)]

/// A decoded pagination cursor.
///
/// Ordering is lexicographic over `(timestamp_ns, file_seq, position)`
/// — the total order the multi-file merge and the exclusive anchor
/// comparison rely on. `file_seq` is the SFST file's monotonic `seq`, a
/// stable per-file identifier (unique within a tenant; cross-tenant
/// disambiguation is deferred with the rest of tenant scoping).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Cursor {
    pub timestamp_ns: i64,
    pub file_seq: u64,
    pub position: u32,
}

impl Cursor {
    /// Encode as `"{timestamp_ns}:{file_seq}:{position}"`.
    pub(super) fn encode(&self) -> String {
        format!("{}:{}:{}", self.timestamp_ns, self.file_seq, self.position)
    }

    /// Decode the string form. Returns `None` for any malformed input
    /// (wrong field count, non-integer field, trailing garbage) so the
    /// handler can treat a bad anchor as "no anchor" rather than error.
    pub(super) fn decode(s: &str) -> Option<Cursor> {
        let mut parts = s.split(':');
        let timestamp_ns: i64 = parts.next()?.parse().ok()?;
        let file_seq: u64 = parts.next()?.parse().ok()?;
        let position: u32 = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Cursor {
            timestamp_ns,
            file_seq,
            position,
        })
    }
}

#[cfg(test)]
mod tests;
