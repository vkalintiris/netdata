use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::dbengine::engine::load::load;
use crate::dbengine::engine::mrg::{Handle, Mrg};
use crate::dbengine::engine::query::{EngineConfig, Priority};
use crate::dbengine::engine::testutil::{
    NOW, dirty_page, messages, nth, points, same, seq_values, write_cfg, write_engine,
};
use crate::dbengine::format::journal_v1;
use crate::dbengine::format::journal_v2::{Retention, from_v1};

const T0: i64 = NOW - 10_000;

/// `n` one-page extents of metrics `from..from + n`, 1024 points each (4096 bytes, the most a page holds): 8 KiB
/// extents, 63 to a data file.
fn fill(e: &Arc<Dbengine>, from: usize, n: usize) -> Vec<Handle> {
    (from..from + n)
        .map(|i| {
            let metric = dirty_page(e, 0, nth(i), T0, &seq_values(1024, i));
            e.flush_pages(0, None, true, true);
            metric
        })
        .collect()
}

/// The v2 file C builds at startup from the file's journal.
fn startup_build(dir: &Path, fileno: u32) -> Vec<u8> {
    let njf = std::fs::read(dir.join(file_name(FileKind::Journal, 1, fileno))).unwrap();
    let replay = journal_v1::replay(&njf[..], njf.len() as u64).unwrap();
    from_v1(&replay, njf.len() as u64, 0, &mut Retention::default()).unwrap()
}

fn read_v2(dir: &Path, fileno: u32) -> Vec<u8> {
    std::fs::read(dir.join(file_name(FileKind::JournalV2, 1, fileno))).unwrap()
}

/// A rotated file is indexed with C's records: its v2 file is the one C builds at startup, it serves the file's
/// pages in place of the open cache, and the tier counts it.
#[test]
fn a_rotated_file_is_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    let metrics = fill(&e, 0, 64);
    let query = |m: &Handle| points(&mut e.query(m, T0, T0 + 1023, Priority::Normal));
    let before = query(&metrics[0]);
    assert!(before.len() == 1024 && before.iter().all(|p| !p.1.is_nan()));
    let td = &e.tiers[0];
    let (df, disk) = (td.file(1).unwrap(), td.current_disk_space());

    let (count, records) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!(count, 1);
    let v2 = read_v2(dir.path(), 1);
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: datafile-1-0000000001 is ready to be indexed".to_string(),
            "DBENGINE: tier 0: indexing journalfile-1-0000000001.njfv2: extents 63, metrics 63, pages 63".to_string(),
            format!(
                "DBENGINE: tier 0: migrated journalfile-1-0000000001.njfv2, {}",
                size_to_string(v2.len() as u64, "B", false).unwrap()
            ),
            "DBENGINE: tier 0: journal indexing done; 1 files processed".to_string(),
        ]
    );
    assert!(v2 == startup_build(dir.path(), 1));
    assert!(df.v2_available());
    assert!(td.open().file_pages(1).is_empty() && td.open().pages(&nth(0)).is_none());
    assert_eq!(td.current_disk_space(), disk + v2.len() as u64);
    assert_eq!((df.first_time_s(), df.last_time_s()), (T0, T0 + 1023));
    assert!(same(&query(&metrics[0]), &before));
    assert_eq!(journal_index(&e, 0), 0);
}

/// The last file is never indexed; a file with writers on it is skipped for now.
#[test]
fn busy_and_last_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 64 << 20, None);
    assert_eq!(journal_index(&e, 0), 0);
    let _m = fill(&e, 0, 64);
    let df = e.tiers[0].file(1).unwrap();
    df.writer_started();
    let (count, records) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!(count, 0);
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: datafile-1-0000000001 needs to be indexed, but it has writers working on it - \
             skipping it for now"
        ]
    );
    df.writer_finished();
    assert_eq!(journal_index(&e, 0), 1);
    assert!(!e.tiers[0].file(2).unwrap().v2_available());
}

/// Over its quota a tier indexes one file, then stops and leaves the rest to index.
#[test]
fn a_tier_over_its_quota_indexes_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let mrg = Mrg::new();
    let cfg = TierConfig {
        max_retention_s: 3600,
        ..write_cfg(0, dir.path())
    };
    let tier = load(cfg, &mrg, NOW).unwrap();
    let e = Dbengine::new(mrg, vec![tier], EngineConfig::new(|| NOW));
    let _m = fill(&e, 0, 3 * 63 + 1);
    let (count, records) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!(count, 1);
    let records = messages(records);
    assert_eq!(
        records[3..],
        [
            "DBENGINE: tier 0: reached quota limit, stopping journal indexing",
            "DBENGINE: tier 0: journal indexing done; 1 files processed"
        ]
    );
    let td = &e.tiers[0];
    assert!(td.needs_indexing());
    assert!(td.file(1).unwrap().v2_available() && !td.file(2).unwrap().v2_available());
}

/// A shutting-down tier indexes nothing.
#[test]
fn a_quiesced_tier_indexes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 64 << 20, None);
    let _m = fill(&e, 0, 64);
    e.tiers[0].quiesce();
    let (count, records) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!((count, messages(records)), (0, Vec::<String>::new()));
}

/// A reused last file holds replayed pages and pages written after the restart: its v2 file, once it rotates, is the
/// one C builds from its journal.
#[test]
fn a_reused_file_indexes_replayed_and_written_pages() {
    let dir = tempfile::tempdir().unwrap();
    drop(fill(&write_engine(&[dir.path()], 64 << 20, None), 0, 3));
    let e = write_engine(&[dir.path()], 64 << 20, None);
    let _m = fill(&e, 3, 61);
    assert_eq!(e.tiers[0].last_fileno(), 2);
    assert_eq!(journal_index(&e, 0), 1);
    assert!(read_v2(dir.path(), 1) == startup_build(dir.path(), 1));
}
