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

/// Over its quota a tier indexes one file, then stops and asks for another run, which goes on from the next file; a
/// run that indexed nothing (its v2 could not be written) asks for none, where C would spin (D75.2).
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
    // as DBEV does when it starts the run
    e.tiers[0].clear_needs_indexing();
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
    // rrdeng_get_used_disk_space(): the files, plus a file's target, less what the last file holds
    let last = td.filenos().into_iter().max().unwrap();
    assert_eq!(
        td.used_disk_space(),
        td.current_disk_space() + td.config.target_datafile_size() - td.file(last).unwrap().pos()
    );
    // the next run goes on from the next file
    td.clear_needs_indexing();
    assert_eq!(journal_index(&e, 0), 1);
    assert!(td.file(2).unwrap().v2_available() && !td.file(3).unwrap().v2_available());
    // a directory the v2 cannot be written to: the runs index nothing and ask for no more
    use std::os::unix::fs::PermissionsExt;
    let dir_path = dir.path().to_path_buf();
    std::fs::set_permissions(&dir_path, std::fs::Permissions::from_mode(0o555)).unwrap();
    td.clear_needs_indexing();
    let (count, _) = netdata_agent_log::capture(|| journal_index(&e, 0));
    std::fs::set_permissions(&dir_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(count, 1, "one file tried, then the quota stop");
    assert!(!td.needs_indexing());
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

/// `datafile_acquire_for_deletion()`'s records and answers: a file whose hot pages the open cache holds is marked
/// pending and blocked (phase 2) but not deletable, and the indexer can no longer take it; a file C's open cache
/// holds clean pages of (indexed in this run) is blocked, its clean pages evicted, and deletable; a file nothing
/// holds (its v2 loaded at start) is deletable at once.
#[test]
fn deletions_wait_for_the_users_of_a_file() {
    use crate::dbengine::engine::tier::Reason;
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    fill(&e, 0, 64 + 63);
    let td = &e.tiers[0];
    // the indexer takes file 1 first, then indexes files 1 and 2 (clean pages in C's open cache)
    let ((), _) = netdata_agent_log::capture(|| {
        let use_ = td.next_for_indexing(None).unwrap();
        assert_eq!(use_.fileno, 1);
        drop(use_);
    });
    assert_eq!(journal_index(&e, 0), 2, "files 1 and 2");
    fill(&e, 200, 1);
    let (f1, f3) = (td.file(1).unwrap(), td.file(3).unwrap());
    let (deletable, records) = netdata_agent_log::capture(|| td.acquire_for_deletion(&f1));
    assert!(deletable);
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: datafile-1-0000000001 is pending deletion",
            "DBENGINE: tier 0: datafile-1-0000000001 entered deletion phase-2 (new users blocked)"
        ]
    );
    for reason in [
        Reason::OpenCache,
        Reason::PageDetails,
        Reason::Retention,
        Reason::Indexing,
    ] {
        assert!(f1.acquire(reason).is_none(), "{reason:?}");
    }
    // file 3 holds hot pages: pending and blocked, not deletable, and the indexer passes it over
    let (deletable, records) = netdata_agent_log::capture(|| td.acquire_for_deletion(&f3));
    assert!(!deletable);
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: datafile-1-0000000003 is pending deletion",
            "DBENGINE: tier 0: datafile-1-0000000003 entered deletion phase-2 (new users blocked)"
        ]
    );
    let (again, records) = netdata_agent_log::capture(|| td.acquire_for_deletion(&f3));
    assert!(!again && messages(records).is_empty(), "recorded once");

    // after a restart file 2's v2 is loaded: nothing holds it
    drop(e);
    let e = write_engine(&[dir.path()], 0, None);
    let td = &e.tiers[0];
    let f2 = td.file(2).unwrap();
    let (deletable, records) = netdata_agent_log::capture(|| td.acquire_for_deletion(&f2));
    assert!(deletable);
    assert_eq!(
        messages(records),
        ["DBENGINE: tier 0: datafile-1-0000000002 is pending deletion"]
    );
}

/// While a deletion is pending, only the open cache takes the file, and only while an extent is being written to it.
#[test]
fn a_pending_file_takes_the_open_cache_while_written() {
    use crate::dbengine::engine::tier::Reason;
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    let td = &e.tiers[0];
    let df = td.last_file();
    df.writer_started();
    let ((), _) = netdata_agent_log::capture(|| assert!(!td.acquire_for_deletion(&df)));
    let open = df.acquire(Reason::OpenCache);
    assert!(open.is_some());
    assert!(df.acquire(Reason::PageDetails).is_none());
    df.writer_finished();
    assert!(df.acquire(Reason::OpenCache).is_none());
    assert_eq!(df.lockers(Some(Reason::OpenCache)), 1);
    drop(open);
    assert_eq!(df.lockers(None), 0);
}

/// A query holds its page details' file until it is dropped: the file cannot be deleted meanwhile, and a query that
/// starts once it is pending finds none of its pages.
#[test]
fn a_query_holds_the_files_of_its_pages() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    let metrics = fill(&e, 0, 64);
    assert_eq!(journal_index(&e, 0), 1);
    let td = &e.tiers[0];
    let f1 = td.file(1).unwrap();
    let held = e.query(&metrics[0], T0, T0 + 1023, Priority::Normal);
    assert!(f1.lockers(None) > 0);
    let ((), _) = netdata_agent_log::capture(|| assert!(!td.acquire_for_deletion(&f1)));
    let after = points(&mut e.query(&metrics[1], T0, T0 + 1023, Priority::Normal));
    assert!(
        after.iter().all(|p| p.1.is_nan()),
        "the pending file's pages are skipped"
    );
    drop(held);
    assert_eq!(f1.lockers(None), 0);
    let ((), _) = netdata_agent_log::capture(|| assert!(td.acquire_for_deletion(&f1)));
}

/// The indexer takes a file in five attempts and passes over one it cannot take, with C's record.
#[test]
fn the_indexer_passes_over_a_file_it_cannot_take() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    fill(&e, 0, 64 + 63 + 1);
    let td = &e.tiers[0];
    let ((), _) =
        netdata_agent_log::capture(|| assert!(!td.acquire_for_deletion(&td.file(1).unwrap())));
    let (count, records) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!(count, 1);
    let records = messages(records);
    assert_eq!(
        records[0],
        "DBENGINE: tier 0: datafile-1-0000000001 cannot be locked for indexing after retries; skipping"
    );
    assert_eq!(
        records[1],
        "DBENGINE: tier 0: datafile-1-0000000002 is ready to be indexed"
    );
}

/// A page whose metric left the registry is not indexed (`pgc_open_cache_to_journal_v2()`'s rejected page, D76.3).
#[test]
fn a_page_of_a_metric_gone_is_not_indexed() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    let gone = dirty_page(&e, 0, nth(1 << 20), T0, &seq_values(1024, 0));
    e.flush_pages(0, None, true, true);
    gone.clear_retention();
    assert!(gone.release(), "it leaves the registry");
    let _metrics = fill(&e, 0, 63);
    let (count, _) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!(count, 1);
    let index = e.tiers[0].v2_from(0).into_iter().next().unwrap();
    assert!(index.find(&nth(1 << 20)).unwrap().is_none());
    assert!(index.find(&nth(0)).unwrap().is_some());
}
