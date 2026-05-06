use std::collections::BTreeMap;
use std::ops::Range;

use chrono::NaiveDate;
use file_registry::{FileId, Query, TenantId, TimestampNs};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entry::CatalogEntry;
use crate::{Error, FORMAT_VERSION};

/// Per-tenant, per-date, per-machine, per-boot record of uploaded SFSTs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub tenant_id: TenantId,
    pub date: NaiveDate,
    pub machine_id: Uuid,
    pub boot_id: Uuid,
    pub entries: BTreeMap<FileId, CatalogEntry>,
    pub created_at_ns: TimestampNs,
    pub updated_at_ns: TimestampNs,
}

impl Catalog {
    pub fn new(
        tenant_id: TenantId,
        date: NaiveDate,
        machine_id: Uuid,
        boot_id: Uuid,
        now_ns: TimestampNs,
    ) -> Self {
        Self {
            tenant_id,
            date,
            machine_id,
            boot_id,
            entries: BTreeMap::new(),
            created_at_ns: now_ns,
            updated_at_ns: now_ns,
        }
    }

    pub fn add(&mut self, entry: CatalogEntry, now_ns: TimestampNs) {
        self.entries.insert(entry.id, entry);
        if now_ns > self.updated_at_ns {
            self.updated_at_ns = now_ns;
        }
    }

    pub fn remove(&mut self, id: &FileId, now_ns: TimestampNs) -> Option<CatalogEntry> {
        let removed = self.entries.remove(id);
        if removed.is_some() && now_ns > self.updated_at_ns {
            self.updated_at_ns = now_ns;
        }
        removed
    }

    // TODO: O(n) scan. The BTreeMap is keyed by FileId, not time, so range
    // filtering touches every entry. Fine at current scales (~hundreds of
    // entries per scope); revisit with an interval index or date-bucketed
    // key if query planner workloads show it matters.
    /// Iterate entries whose `[min_timestamp_s, max_timestamp_s]` range
    /// (inclusive on both ends) intersects the query's `[start, end)`
    /// range (half-open) — matching the convention used by
    /// `sfst::Registry::candidates` and `wal::Registry::candidates`.
    pub fn find<'a>(&'a self, q: &Query) -> impl Iterator<Item = &'a CatalogEntry> + 'a {
        // Extract q's contents upfront so the filter closures don't borrow
        // q. Decouples the iterator's lifetime from q's, letting callers
        // pass a temporary `Query`.
        let q_range = q.time_range.clone();
        let q_stream = q.stream.clone();
        self.entries
            .values()
            .filter(move |e| range_overlaps(e, &q_range))
            .filter(move |e| q_stream.as_ref().is_none_or(|s| e.stream == *s))
    }

    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        let env = Envelope {
            version: FORMAT_VERSION,
            tenant_id: self.tenant_id.clone(),
            date: self.date,
            machine_id: self.machine_id,
            boot_id: self.boot_id,
            created_at_ns: self.created_at_ns,
            updated_at_ns: self.updated_at_ns,
            entries: self.entries.values().cloned().collect(),
        };
        Ok(serde_json::to_vec(&env)?)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let env: Envelope = serde_json::from_slice(bytes)?;
        if env.version != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion(env.version));
        }
        let mut entries = BTreeMap::new();
        for entry in env.entries {
            entries.insert(entry.id, entry);
        }
        Ok(Self {
            tenant_id: env.tenant_id,
            date: env.date,
            machine_id: env.machine_id,
            boot_id: env.boot_id,
            entries,
            created_at_ns: env.created_at_ns,
            updated_at_ns: env.updated_at_ns,
        })
    }
}

/// True iff the entry's `[min_timestamp_s, max_timestamp_s]` range
/// (inclusive on both ends) shares any second with the query's
/// `[start, end)` range (half-open).
///
/// An empty query (`start >= end`) matches no entry.
fn range_overlaps(entry: &CatalogEntry, q: &Range<u32>) -> bool {
    if q.start >= q.end {
        return false;
    }
    entry.max_timestamp_s >= q.start && entry.min_timestamp_s < q.end
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    tenant_id: TenantId,
    date: NaiveDate,
    machine_id: Uuid,
    boot_id: Uuid,
    created_at_ns: TimestampNs,
    updated_at_ns: TimestampNs,
    entries: Vec<CatalogEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::StreamEntry;
    use file_registry::ByteSize;

    fn test_catalog() -> Catalog {
        Catalog::new(
            TenantId::from("tenant1"),
            NaiveDate::from_ymd_opt(2026, 4, 17).unwrap(),
            Uuid::nil(),
            Uuid::from_u128(1),
            TimestampNs(1_000_000_000),
        )
    }

    fn entry_at(seq: u64, min_s: u32, max_s: u32, stream: StreamEntry) -> CatalogEntry {
        CatalogEntry {
            id: FileId::new(Uuid::nil(), Uuid::from_u128(1), seq, 0),
            remote_key: format!("tenant1/sfst/2026-04-17/{seq}.sfst"),
            min_timestamp_s: min_s,
            max_timestamp_s: max_s,
            total_logs: 10,
            stream,
            size: ByteSize(1024),
            uploaded_at_ns: TimestampNs(2_000_000_000),
        }
    }

    #[test]
    fn new_has_empty_entries_and_matching_timestamps() {
        let c = test_catalog();
        assert!(c.entries.is_empty());
        assert_eq!(c.created_at_ns, c.updated_at_ns);
    }

    #[test]
    fn add_then_remove_returns_to_empty() {
        let mut c = test_catalog();
        let e = entry_at(1, 100, 200, StreamEntry::new("", ""));
        c.add(e.clone(), TimestampNs(3_000_000_000));
        assert_eq!(c.entries.len(), 1);

        let removed = c.remove(&e.id, TimestampNs(4_000_000_000)).unwrap();
        assert_eq!(removed, e);
        assert!(c.entries.is_empty());
        assert_eq!(c.updated_at_ns, TimestampNs(4_000_000_000));
    }

    #[test]
    fn remove_missing_returns_none_without_advancing_updated_at() {
        let mut c = test_catalog();
        let before = c.updated_at_ns;
        let fake = FileId::new(Uuid::nil(), Uuid::from_u128(1), 42, 0);
        assert!(c.remove(&fake, TimestampNs(9_999_999_999)).is_none());
        assert_eq!(c.updated_at_ns, before);
    }

    #[test]
    fn add_older_now_does_not_regress_updated_at() {
        let mut c = test_catalog();
        let high = TimestampNs(5_000_000_000);
        c.add(entry_at(1, 100, 200, StreamEntry::new("", "")), high);
        c.add(
            entry_at(2, 300, 400, StreamEntry::new("", "")),
            TimestampNs(1_000),
        );
        assert_eq!(c.updated_at_ns, high);
    }

    #[test]
    fn roundtrip_json_preserves_entries_and_metadata() {
        let mut c = test_catalog();
        c.add(
            entry_at(1, 100, 200, StreamEntry::new("prod", "api")),
            TimestampNs(3_000_000_000),
        );
        c.add(
            entry_at(2, 300, 500, StreamEntry::new("", "")),
            TimestampNs(4_000_000_000),
        );

        let bytes = c.to_json().unwrap();
        let parsed = Catalog::from_json(&bytes).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn find_range_overlap_semantics() {
        let mut c = test_catalog();
        let now = TimestampNs(3_000_000_000);
        c.add(entry_at(1, 100, 200, StreamEntry::new("", "")), now);
        c.add(entry_at(2, 300, 400, StreamEntry::new("", "")), now);
        c.add(entry_at(3, 150, 350, StreamEntry::new("", "")), now);

        // Window [50, 250) — file 1's max=200 is in range, file 3's
        // min=150 is in range, file 2's min=300 is past the upper bound.
        let q = Query {
            time_range: 50..250,
            stream: None,
        };
        let hits: Vec<u64> = c.find(&q).map(|e| e.id.seq).collect();
        assert_eq!(hits, vec![1, 3]);

        // Inclusive lower / exclusive upper edges. Window [200, 300):
        //  - file 1: max=200 ≥ 200 ✓ and min=100 < 300 ✓ → in
        //  - file 2: max=400 ≥ 200 ✓ and min=300 < 300 ✗ → out
        //  - file 3: max=350 ≥ 200 ✓ and min=150 < 300 ✓ → in
        let q = Query {
            time_range: 200..300,
            stream: None,
        };
        let hits: Vec<u64> = c.find(&q).map(|e| e.id.seq).collect();
        assert_eq!(hits, vec![1, 3]);

        let q = Query {
            time_range: 500..600,
            stream: None,
        };
        assert_eq!(c.find(&q).count(), 0);

        // Single-second range [200, 201) hits file 1 (max=200) and
        // file 3 (min=150, max=350); file 2's min=300 is out.
        let q = Query {
            time_range: 200..201,
            stream: None,
        };
        let hits: Vec<u64> = c.find(&q).map(|e| e.id.seq).collect();
        assert_eq!(hits, vec![1, 3]);
    }

    #[test]
    fn find_with_stream_filter_matches_by_exact_equality() {
        let mut c = test_catalog();
        let now = TimestampNs(3_000_000_000);
        // Two entries on the "api" stream, one on "worker".
        c.add(entry_at(1, 100, 200, StreamEntry::new("prod", "api")), now);
        c.add(
            entry_at(2, 100, 200, StreamEntry::new("prod", "worker")),
            now,
        );
        c.add(entry_at(3, 100, 200, StreamEntry::new("prod", "api")), now);

        let q = Query {
            time_range: 0..1000,
            stream: Some(StreamEntry::new("prod", "api")),
        };
        let hits: Vec<u64> = c.find(&q).map(|e| e.id.seq).collect();
        assert_eq!(hits, vec![1, 3]);
    }

    #[test]
    fn find_empty_string_stream_matches_only_empty_string_entry() {
        let mut c = test_catalog();
        let now = TimestampNs(3_000_000_000);
        c.add(entry_at(1, 100, 200, StreamEntry::new("", "")), now);
        c.add(entry_at(2, 100, 200, StreamEntry::new("prod", "api")), now);

        let q = Query {
            time_range: 0..1000,
            stream: Some(StreamEntry::new("", "")),
        };
        let hits: Vec<u64> = c.find(&q).map(|e| e.id.seq).collect();
        assert_eq!(hits, vec![1]);
    }

    #[test]
    fn find_empty_query_matches_nothing() {
        let mut c = test_catalog();
        let now = TimestampNs(3_000_000_000);
        c.add(entry_at(1, 100, 200, StreamEntry::new("", "")), now);

        // start == end → empty window.
        let q = Query {
            time_range: 200..200,
            stream: None,
        };
        assert_eq!(c.find(&q).count(), 0);
    }

    #[test]
    fn from_json_rejects_unsupported_version() {
        let json = br#"{
            "version": 999,
            "tenant_id": "t",
            "date": "2026-04-17",
            "machine_id": "00000000-0000-0000-0000-000000000000",
            "boot_id": "00000000-0000-0000-0000-000000000000",
            "created_at_ns": 0,
            "updated_at_ns": 0,
            "entries": []
        }"#;
        match Catalog::from_json(json) {
            Err(Error::UnsupportedVersion(999)) => {}
            other => panic!("expected UnsupportedVersion(999), got {other:?}"),
        }
    }

    #[test]
    fn from_json_rejects_truncated_json() {
        let truncated = b"{\"version\": 1, \"tenant_id\": \"t";
        match Catalog::from_json(truncated) {
            Err(Error::Json(_)) => {}
            other => panic!("expected Json error, got {other:?}"),
        }
    }
}
