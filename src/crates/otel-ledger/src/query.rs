//! Per-tenant query candidate selection.
//!
//! Combines the SFST and WAL registries into a single unified list of
//! files that may contain data matching a query. The within-file scan
//! and result merge are downstream concerns; this layer only decides
//! *which files* to look at.

use std::collections::HashMap;

use file_registry::Query;

use crate::registry::Registry;

/// One file the query planner has decided is a candidate for a read.
///
/// References point into the registries, so the result is bounded by
/// the lifetime of the `Registry` it was produced from. Downstream code
/// matches on the variant to choose the right reader (`sfst::Reader`
/// vs `wal::Reader`) and access pattern.
#[derive(Debug, Clone)]
pub enum CandidateSource<'a> {
    /// A sealed, indexed file. Open with `sfst::Reader::open`.
    Sfst(&'a sfst::File),
    /// A WAL file (active or archived) whose data has not yet been
    /// reflected in an SFST. Open with `wal::Reader::open`.
    Wal(&'a wal::File),
}

impl Registry {
    /// Identify the set of files needed to satisfy `q`, deduplicated
    /// across the SFST and WAL registries.
    ///
    /// During the brief window between an SFST being tracked and its
    /// originating WAL being deleted, a single `seq` lives in both
    /// registries. The planner returns only the SFST candidate in that
    /// case — the SFST is sealed, has a query-friendly layout, and
    /// carries the authoritative summary. Reading the WAL would be
    /// strictly more work for the same data.
    ///
    /// Output is sorted by `FileId.seq` for determinism. Seq is
    /// monotonic at allocation time, so the order correlates with file
    /// creation order, but it is not chronological by log-data
    /// timestamp — a downstream merger should sort by `min_timestamp`
    /// if it needs that.
    pub fn plan_candidates<'a>(&'a self, q: &Query) -> Vec<CandidateSource<'a>> {
        let mut by_seq: HashMap<u64, CandidateSource<'a>> = HashMap::new();

        // WAL first so SFST overwrites on the same seq.
        for f in self.wal.candidates(q) {
            by_seq.insert(f.id.seq, CandidateSource::Wal(f));
        }
        for f in self.sfst.candidates(q) {
            by_seq.insert(f.id.seq, CandidateSource::Sfst(f));
        }

        let mut out: Vec<_> = by_seq.into_values().collect();
        out.sort_by_key(|c| match c {
            CandidateSource::Sfst(f) => f.id.seq,
            CandidateSource::Wal(f) => f.id.seq,
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_registry::{ByteSize, FileId, StreamEntry, TenantId, TimestampNs};
    use uuid::Uuid;
    use wal::FileEvent;

    fn machine() -> Uuid {
        Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
    }
    fn boot() -> Uuid {
        Uuid::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111)
    }
    fn fid(seq: u64, ns_hash: u64) -> FileId {
        FileId::new(machine(), boot(), seq, ns_hash)
    }

    fn make_registry() -> Registry {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let catalog_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::Registry::new(sfst_dir.path());
        let catalog_files =
            otel_catalog::Registry::new(catalog_dir.path(), TenantId::from("tenant1"));
        std::mem::forget((wal_dir, sfst_dir, catalog_dir));
        Registry::new(wal, sfst, catalog_files)
    }

    /// Track a WAL file via the event flow with the given range and
    /// `Archived` status (post-Closed).
    fn track_wal(reg: &mut Registry, seq: u64, ns_hash: u64, min_s: u32, max_s: u32) {
        const NS: u64 = 1_000_000_000;
        let id = fid(seq, ns_hash);
        reg.wal
            .apply_event(&FileEvent::Created {
                file_id: id,
                created_at_ns: TimestampNs(0),
            })
            .unwrap();
        reg.wal
            .apply_event(&FileEvent::Closed {
                file_id: id,
                frame_count: 0,
                min_timestamp_ns: TimestampNs(min_s as u64 * NS),
                max_timestamp_ns: TimestampNs(max_s as u64 * NS),
                size: ByteSize(0),
            })
            .unwrap();
    }

    /// Track an SFST file with the given range and stream.
    fn track_sfst(reg: &mut Registry, seq: u64, ns_hash: u64, min_s: u32, max_s: u32) {
        let id = fid(seq, ns_hash);
        reg.sfst.track(
            id,
            ByteSize(1),
            sfst::FileSummary {
                min_timestamp_s: min_s,
                max_timestamp_s: max_s,
                total_logs: 1,
                stream: StreamEntry::new("ns", "a"),
            },
        );
    }

    fn full_range_query() -> Query {
        Query {
            time_range: 0..u32::MAX,
            stream: None,
        }
    }

    #[test]
    fn sfst_only_candidates() {
        let mut reg = make_registry();
        track_sfst(&mut reg, 1, 7, 100, 200);
        track_sfst(&mut reg, 2, 7, 300, 400);

        let plan = reg.plan_candidates(&full_range_query());
        assert_eq!(plan.len(), 2);
        for c in &plan {
            assert!(matches!(c, CandidateSource::Sfst(_)));
        }
    }

    #[test]
    fn wal_only_candidates() {
        let mut reg = make_registry();
        track_wal(&mut reg, 1, 7, 100, 200);
        track_wal(&mut reg, 2, 7, 300, 400);

        let plan = reg.plan_candidates(&full_range_query());
        assert_eq!(plan.len(), 2);
        for c in &plan {
            assert!(matches!(c, CandidateSource::Wal(_)));
        }
    }

    #[test]
    fn disjoint_seqs_keep_both_sources() {
        let mut reg = make_registry();
        track_sfst(&mut reg, 1, 7, 100, 200);
        track_wal(&mut reg, 2, 7, 300, 400);

        let plan = reg.plan_candidates(&full_range_query());
        assert_eq!(plan.len(), 2);
        // Sorted by seq: SFST seq=1, then WAL seq=2.
        assert!(matches!(plan[0], CandidateSource::Sfst(f) if f.id.seq == 1));
        assert!(matches!(plan[1], CandidateSource::Wal(f) if f.id.seq == 2));
    }

    #[test]
    fn overlap_resolves_to_sfst() {
        let mut reg = make_registry();
        // Same seq=1 in both registries — the post-index, pre-WAL-delete
        // window. Planner must return only the SFST.
        track_sfst(&mut reg, 1, 7, 100, 200);
        track_wal(&mut reg, 1, 7, 100, 200);

        let plan = reg.plan_candidates(&full_range_query());
        assert_eq!(plan.len(), 1);
        assert!(matches!(plan[0], CandidateSource::Sfst(f) if f.id.seq == 1));
    }

    #[test]
    fn empty_registry_returns_empty() {
        let reg = make_registry();
        assert!(reg.plan_candidates(&full_range_query()).is_empty());
    }

    #[test]
    fn query_excludes_out_of_range_files() {
        let mut reg = make_registry();
        track_sfst(&mut reg, 1, 7, 100, 200);
        track_wal(&mut reg, 2, 7, 1000, 2000);

        // Window that misses both files.
        let q = Query {
            time_range: 500..600,
            stream: None,
        };
        assert!(reg.plan_candidates(&q).is_empty());

        // Window that hits only the SFST.
        let q = Query {
            time_range: 50..250,
            stream: None,
        };
        let plan = reg.plan_candidates(&q);
        assert_eq!(plan.len(), 1);
        assert!(matches!(plan[0], CandidateSource::Sfst(f) if f.id.seq == 1));
    }

    #[test]
    fn stream_filter_applies_to_both_sources() {
        let mut reg = make_registry();
        let api_hash = file_registry::compute_ns_hash(Some("ns"), Some("a"));
        let other_hash = file_registry::compute_ns_hash(Some("ns"), Some("b"));
        track_sfst(&mut reg, 1, api_hash, 100, 200);
        track_wal(&mut reg, 2, other_hash, 100, 200);

        let q = Query {
            time_range: 0..u32::MAX,
            stream: Some(StreamEntry::new("ns", "a")),
        };
        let plan = reg.plan_candidates(&q);
        assert_eq!(plan.len(), 1);
        // The WAL on a different ns_hash is excluded; only the SFST
        // (whose summary stream matches "ns/a") survives.
        assert!(matches!(plan[0], CandidateSource::Sfst(f) if f.id.seq == 1));
    }
}
