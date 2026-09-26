use super::*;
use crate::dbengine::engine::cache::{CachedPage, PageState, Search};
use crate::dbengine::engine::io::fault;
use crate::dbengine::engine::load::{TierConfig, load};
use crate::dbengine::engine::mrg::{Handle, Mrg};
use crate::dbengine::engine::query::{EngineConfig, Priority};
use crate::dbengine::engine::testutil::{NOW, points, same};
use crate::dbengine::format::descriptor::PAGE_TYPE_ARRAY_32BIT;
use crate::dbengine::format::extent::COMPRESSION_NONE;
use crate::dbengine::format::inspect::{failed_checks, for_each_page, tier_report};
use crate::dbengine::format::page::{DiskPage, PageBuilder};
use crate::storage_number::{SN_DEFAULT_FLAGS, pack};
use netdata_agent_evloop::work::WorkPool;
use serde_json::{Value, json};
use std::path::Path;

const T0: i64 = NOW - 1000;

/// A tier with array pages, uncompressed extents, and a 25 MiB quota: data files of 524,288 bytes.
fn tier_cfg(tier: usize, dir: &Path) -> TierConfig {
    TierConfig {
        max_disk_space: 25 << 20,
        page_type: PAGE_TYPE_ARRAY_32BIT,
        compression: COMPRESSION_NONE,
        ..TierConfig::new(tier, dir.to_path_buf())
    }
}

fn engine(dirs: &[&Path], main_cache_bytes: usize, pool: Option<WorkPool>) -> Arc<Dbengine> {
    let mrg = Mrg::new();
    let tiers = dirs
        .iter()
        .enumerate()
        .map(|(t, d)| load(tier_cfg(t, d), &mrg, NOW).unwrap())
        .collect();
    Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            main_cache_bytes,
            pool,
            ..EngineConfig::new(|| NOW)
        },
    )
}

fn uuid(n: usize) -> [u8; 16] {
    let mut u = [0u8; 16];
    u[..8].copy_from_slice(&(n as u64 + 1).to_be_bytes());
    u
}

/// A dirty page of `uuid` with these values one second apart from `start`; the metric registered with its retention.
fn dirty(e: &Dbengine, tier: usize, uuid: [u8; 16], start: i64, values: &[f64]) -> Handle {
    let mut builder = PageBuilder::new(PAGE_TYPE_ARRAY_32BIT, values.len()).unwrap();
    for &v in values {
        builder.append(v, v, v, 1, 0, SN_DEFAULT_FLAGS);
    }
    let end = start + values.len() as i64 - 1;
    let page = CachedPage::collected(start, 1, builder);
    page.hot_set_end_time_s(end);
    let page = e.main.add(tier, &uuid, page).unwrap();
    e.main.hot_to_dirty(tier, &page);
    let (metric, _) = e.mrg.add_and_acquire(&uuid, tier, start, end, 1);
    metric
}

fn values(n: usize, base: usize) -> Vec<f64> {
    (0..n).map(|i| (base + i) as f64).collect()
}

/// Every page a tier's files hold, in journal order: metric, start, storage numbers.
fn stored(dir: &Path) -> Vec<([u8; 16], i64, Vec<u32>)> {
    let mut out = Vec::new();
    for_each_page(dir, |d, bytes| {
        let numbers = DiskPage::load(d.page_type, bytes)
            .ok()
            .and_then(|p| p.storage_numbers())
            .unwrap_or_default();
        out.push((d.uuid, (d.start_time_ut / 1_000_000) as i64, numbers));
    })
    .unwrap();
    out
}

fn packed(values: &[f64]) -> Vec<u32> {
    values.iter().map(|&v| pack(v, SN_DEFAULT_FLAGS)).collect()
}

fn files(dir: &Path) -> Vec<Value> {
    tier_report(dir).unwrap()["files"]
        .as_array()
        .unwrap()
        .clone()
}

fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
    records.into_iter().filter_map(|r| r.message).collect()
}

/// A full batch becomes one extent in the last pair, its transaction after it: the files check, the pages turn clean
/// and join the open cache, and the tier counts the bytes.
#[test]
fn a_full_batch_becomes_one_extent() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let _metrics: Vec<Handle> = (0..109)
        .map(|i| dirty(&e, 0, uuid(i), T0, &values(10, i)))
        .collect();
    assert!(!e.flush_pages(0, None, true, false));

    let report = &files(dir.path())[0];
    assert_eq!(failed_checks(report), Vec::<String>::new());
    assert_eq!(
        (&report["ndf_size"], &report["njf_size"], &report["tx_ids"]),
        (&json!(16384), &json!(8192), &json!([1, "...", 1]))
    );
    assert_eq!(report["pages_per_extent"], json!({"109": 1}));
    let pages = stored(dir.path());
    assert_eq!(pages.len(), 109);
    for (i, (u, start, numbers)) in pages.into_iter().enumerate() {
        assert_eq!((u, start, numbers), (uuid(i), T0, packed(&values(10, i))));
    }
    let td = &e.tiers[0];
    assert_eq!(td.current_disk_space(), 8192 + 12288 + 4096);
    assert_eq!((td.last_flush_fileno(), td.needs_indexing(), td.extents_in_flight()), (1, false, 0));
    assert_eq!(td.last_file().writers_running(), (0, 0));
    let page = e.main.search(0, &uuid(0), T0, Search::Exact).unwrap();
    assert_eq!(page.state(), PageState::Clean);
    let open = td.open();
    let op = open.pages(&uuid(0)).unwrap()[&T0];
    assert_eq!((op.fileno, op.block, op.bytes, op.end_time_s), (1, 1, 8403, T0 + 9));
}

/// Without `all`, fewer pages than an extent takes stay dirty; a timer flush writes whole batches in the order the
/// pages closed and leaves the rest; `all` writes the rest too.
#[test]
fn batches_are_whole_unless_all() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let _m: Vec<Handle> = (0..108)
        .map(|i| dirty(&e, 0, uuid(i), T0, &values(2, i)))
        .collect();
    assert!(!e.flush_pages(0, None, true, false));
    assert_eq!(e.main.stats().dirty_entries, 108);
    assert!(stored(dir.path()).is_empty());

    let _more: Vec<Handle> = (108..250)
        .map(|i| dirty(&e, 0, uuid(i), T0, &values(2, i)))
        .collect();
    e.flush_pages(0, None, true, false);
    let written: Vec<[u8; 16]> = stored(dir.path()).into_iter().map(|p| p.0).collect();
    assert_eq!(written, (0..218).map(uuid).collect::<Vec<_>>());
    assert_eq!(e.main.stats().dirty_entries, 32);

    e.flush_pages(0, None, true, true);
    let report = &files(dir.path())[0];
    assert_eq!(report["pages_per_extent"], json!({"32": 1, "109": 2}));
    assert_eq!(report["tx_ids"], json!([1, 2, 3, "...", 1, 2, 3]));
    assert_eq!(e.main.stats().dirty_entries, 0);
}

/// Each tier writes its own batches, tiers in order.
#[test]
fn tiers_flush_their_own_batches() {
    let (d0, d1) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let e = engine(&[d0.path(), d1.path()], 64 << 20, None);
    let mut m = Vec::new();
    for i in 0..109 {
        m.push(dirty(&e, 0, uuid(i), T0, &values(2, i)));
        m.push(dirty(&e, 1, uuid(i), T0, &values(2, i)));
    }
    e.flush_pages(0, None, true, false);
    assert_eq!(stored(d0.path()).len(), 109);
    assert_eq!(stored(d1.path()).len(), 109);
    assert_eq!(e.main.stats().dirty_entries, 0);
}

/// A data file takes extents up to its target exactly; the next extent opens a new pair, which leaves the old file to
/// index.
#[test]
fn a_full_data_file_rotates_to_a_new_pair() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let mut m = Vec::new();
    // 1000 points: 4047 bytes, one block per extent
    for i in 0..127 {
        m.push(dirty(&e, 0, uuid(i), T0, &values(1000, 0)));
        e.flush_pages(0, None, true, true);
    }
    assert_eq!(e.tiers[0].last_file().pos(), 524_288);
    m.push(dirty(&e, 0, uuid(127), T0, &values(1000, 0)));
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    assert_eq!(
        messages(records),
        [
            format!(
                "DBENGINE: creating new data and journal files in path \"{}\"",
                dir.path().display()
            ),
            "DBENGINE: tier 0: created datafile-1-0000000002 (.ndf, .njf).".to_string(),
        ]
    );
    let reports = files(dir.path());
    assert_eq!(failed_checks(&reports[0]), Vec::<String>::new());
    assert_eq!(reports[0]["tx_ids"], json!([1, 2, 3, "...", 125, 126, 127]));
    assert_eq!(reports[1]["tx_ids"], json!([128, "...", 128]));
    assert_eq!(reports[1]["last_extent_end"], json!(8192));
    let td = &e.tiers[0];
    assert_eq!((td.last_fileno(), td.needs_indexing()), (2, true));
    assert_eq!(td.current_disk_space(), 8192 + 128 * 8192 + 8192);
}

/// A restart reuses the last pair with its journal: the next transaction id follows, and the next extent appends to
/// the same files.
#[test]
fn a_restart_appends_to_the_reused_pair() {
    let dir = tempfile::tempdir().unwrap();
    {
        let e = engine(&[dir.path()], 64 << 20, None);
        let _m: Vec<Handle> = (0..3)
            .map(|i| {
                let m = dirty(&e, 0, uuid(i), T0 + i as i64 * 10, &values(10, i));
                e.flush_pages(0, None, true, true);
                m
            })
            .collect();
    }
    let e = engine(&[dir.path()], 64 << 20, None);
    let _m = dirty(&e, 0, uuid(3), T0 + 30, &values(10, 3));
    e.flush_pages(0, None, true, true);
    let reports = files(dir.path());
    assert_eq!(reports.len(), 1);
    assert_eq!(failed_checks(&reports[0]), Vec::<String>::new());
    assert_eq!(reports[0]["tx_ids"], json!([1, 2, 3, "...", 2, 3, 4]));
    assert_eq!(reports[0]["njf_size"], json!(5 * 4096));
}

/// A last file with a v2 index takes no extent (D65.6): the next pair follows the newest file number, the deleted
/// pair's included.
#[test]
fn an_indexed_last_file_gets_a_new_pair() {
    let dir = tempfile::tempdir().unwrap();
    {
        let e = engine(&[dir.path()], 64 << 20, None);
        let mut m = Vec::new();
        for i in 0..128 {
            m.push(dirty(&e, 0, uuid(i), T0, &values(1000, 0)));
            e.flush_pages(0, None, true, true);
        }
    }
    // pair 2's data file loses its superblock: the pair is deleted at the next start, and file 1 is indexed
    let ndf2 = dir.path().join("datafile-1-0000000002.ndf");
    std::fs::write(&ndf2, vec![0u8; 8192]).unwrap();
    let before = std::fs::read(dir.path().join("datafile-1-0000000001.ndf")).unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    assert_eq!(e.tiers[0].last_fileno(), 2);
    let _m = dirty(&e, 0, uuid(200), T0, &values(10, 0));
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    assert!(
        messages(records).contains(&"DBENGINE: tier 0: created datafile-1-0000000003 (.ndf, .njf).".to_string())
    );
    assert_eq!(std::fs::read(dir.path().join("datafile-1-0000000001.ndf")).unwrap(), before);
    assert_eq!(e.tiers[0].last_file().fileno, 3);
}

/// A data file write that fails for good marks the pair failed and moves the extent to a new pair, under a new
/// transaction id; later extents skip the failed file.
#[test]
fn a_failed_write_moves_the_extent_to_a_new_pair() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let _m = dirty(&e, 0, uuid(0), T0, &values(10, 0));
    fault::script([Some(libc::EBADF)]);
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    let messages = messages(records);
    assert_eq!(
        messages[0],
        "DBENGINE: tier 0 datafile 1 write failed (bad file descriptor) - rotating to a new datafile and retrying \
         the extent, to prevent data loss"
    );
    assert_eq!(messages[2], "DBENGINE: tier 0: created datafile-1-0000000002 (.ndf, .njf).");
    let reports = files(dir.path());
    assert_eq!((&reports[0]["ndf_size"], &reports[0]["tx_count"]), (&json!(4096), &json!(0)));
    assert_eq!(reports[1]["tx_ids"], json!([2, "...", 2]));
    let td = &e.tiers[0];
    assert_eq!(td.open().pages(&uuid(0)).unwrap()[&T0].fileno, 2);
    assert_eq!(td.current_disk_space(), 8192 + 8192 + 4096 + 4096);
    let _m2 = dirty(&e, 0, uuid(1), T0, &values(10, 0));
    e.flush_pages(0, None, true, true);
    assert_eq!(files(dir.path())[1]["tx_ids"], json!([2, 3, "...", 2, 3]));
}

/// A journal write that fails leaves the extent in the data file (counted) without its transaction, and the retry
/// in a new pair carries the journal's error.
#[test]
fn a_failed_journal_write_retries_in_a_new_pair() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let _m = dirty(&e, 0, uuid(0), T0, &values(10, 0));
    fault::script([None, Some(libc::EROFS)]);
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    assert!(messages(records)[0].contains("datafile 1 write failed (read-only file system) - rotating"));
    let reports = files(dir.path());
    assert_eq!((&reports[0]["ndf_size"], &reports[0]["njf_size"]), (&json!(8192), &json!(4096)));
    assert_eq!(reports[1]["tx_ids"], json!([2, "...", 2]));
    assert_eq!(e.tiers[0].current_disk_space(), 8192 + 4096 + 8192 + 4096 + 4096);
}

/// Retries: eight passing failures and a success write without a record; nine fail; an error no retry passes stops
/// at the first attempt.
#[test]
fn writes_retry_as_c() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let _m = dirty(&e, 0, uuid(0), T0, &values(10, 0));
    fault::script([Some(libc::EIO); 8]);
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    assert_eq!(messages(records), Vec::<String>::new());
    assert_eq!(files(dir.path())[0]["tx_ids"], json!([1, "...", 1]));

    let _m = dirty(&e, 0, uuid(1), T0, &values(10, 0));
    fault::script([Some(libc::EIO); 9]);
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    assert!(messages(records)[0].contains("datafile 1 write failed (i/o error) - rotating"));

    let _m = dirty(&e, 0, uuid(2), T0, &values(10, 0));
    fault::script([Some(libc::ENOSPC), None]);
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    assert!(messages(records)[0].contains("datafile 2 write failed (no space left on device) - rotating"));
}

/// When the move finds no new pair the extent is lost: its pages stay readable while cached, and join no open
/// cache; the next write creates the pair.
#[test]
fn an_extent_with_no_pair_to_move_to_is_lost() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let metric = dirty(&e, 0, uuid(0), T0, &values(10, 0));
    fault::script([Some(libc::EBADF), Some(libc::EACCES)]);
    let (_, records) = netdata_agent_log::capture(|| e.flush_pages(0, None, true, true));
    let ndf2 = dir.path().join("datafile-1-0000000002.ndf");
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0 datafile 1 write failed (bad file descriptor) - rotating to a new datafile and \
             retrying the extent, to prevent data loss"
                .to_string(),
            format!(
                "DBENGINE: creating new data and journal files in path \"{}\"",
                dir.path().display()
            ),
            format!("DBENGINE: Failed to create datafile {}", ndf2.display()),
            "DBENGINE: tier 0 datafile 1 write failed (bad file descriptor) - the extent is lost".to_string(),
        ]
    );
    assert!(!ndf2.exists());
    let td = &e.tiers[0];
    assert_eq!(td.last_fileno(), 1);
    assert!(td.open().pages(&uuid(0)).is_none());
    let got = points(&mut e.query(&metric, T0, T0 + 9, Priority::Normal));
    let want: Vec<(i64, f64)> = (0..10).map(|i| (T0 + i, i as f64)).collect();
    assert!(same(&got, &want), "{got:?}");

    let _m = dirty(&e, 0, uuid(1), T0, &values(10, 0));
    e.flush_pages(0, None, true, true);
    assert_eq!(files(dir.path())[1]["tx_ids"], json!([2, "...", 2]));
}

/// With no cache budget the flushed pages are evicted: queries read them through the open cache and the data file,
/// and after a restart through the replayed journal.
#[test]
fn flushed_pages_read_back_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let want: Vec<(i64, f64)> = (0..10).map(|i| (T0 + i, i as f64)).collect();
    {
        let e = engine(&[dir.path()], 0, None);
        let metric = dirty(&e, 0, uuid(0), T0, &values(10, 0));
        e.flush_pages(0, None, true, true);
        assert!(e.main.is_empty());
        let got = points(&mut e.query(&metric, T0, T0 + 9, Priority::Normal));
        assert!(same(&got, &want), "{got:?}");
    }
    let e = engine(&[dir.path()], 0, None);
    let metric = e.mrg.get_and_acquire(&uuid(0), 0).unwrap();
    let got = points(&mut e.query(&metric, T0, T0 + 9, Priority::Normal));
    assert!(same(&got, &want), "{got:?}");
}

/// A quiescing tier still writes, but adds nothing to the open cache and leaves nothing to index.
#[test]
fn a_quiesced_tier_writes_without_the_open_cache() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let _m = dirty(&e, 0, uuid(0), T0, &values(10, 0));
    e.tiers[0].quiesce();
    e.flush_pages(0, None, true, true);
    assert_eq!(files(dir.path())[0]["tx_ids"], json!([1, "...", 1]));
    assert!(e.tiers[0].open().pages(&uuid(0)).is_none());
    assert!(!e.tiers[0].needs_indexing());
}

/// With a pool the extent is written by a pool thread while the flusher waits.
#[test]
fn a_pool_writes_the_extent() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, Some(WorkPool::new(2, 256 * 1024)));
    let _m = dirty(&e, 0, uuid(0), T0, &values(10, 0));
    e.flush_pages(0, None, true, true);
    assert_eq!(files(dir.path())[0]["tx_ids"], json!([1, "...", 1]));
    assert_eq!(e.tiers[0].extents_in_flight(), 0);
}

/// N2: pages validate against the engine's clock as each extent loads, whatever window the query asks for.
#[test]
fn pages_validate_against_the_engine_clock() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 0, None);
    let valid = dirty(&e, 0, uuid(0), NOW - 8, &values(10, 0));
    let future = dirty(&e, 0, uuid(1), NOW - 7, &values(10, 0));
    e.flush_pages(0, None, true, true);
    let got = points(&mut e.query(&valid, NOW - 8, NOW + 1, Priority::Normal));
    let want: Vec<(i64, f64)> = (0..10).map(|i| (NOW - 8 + i, i as f64)).collect();
    assert!(same(&got, &want), "{got:?}");
    let got = points(&mut e.query(&future, NOW - 7, NOW + 2, Priority::Normal));
    assert!(got.iter().all(|p| p.1.is_nan()), "{got:?}");
}

/// Two threads flushing one tier: every extent lands whole, the extents tile the data file, and every page reads
/// back.
#[test]
fn concurrent_flushes_tile_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(&[dir.path()], 64 << 20, None);
    let _m: Vec<Handle> = (0..8 * 109)
        .map(|i| dirty(&e, 0, uuid(i), T0, &values(10, i)))
        .collect();
    std::thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| while e.flush_pages(1, None, true, false) {});
        }
    });
    e.flush_pages(0, None, true, true);
    let report = &files(dir.path())[0];
    for check in ["extent_crc_all_ok", "extent_descr_eq_journal", "tx_crc_all_ok"] {
        assert_eq!(report[check], json!(true), "{check}");
    }
    let mut pages = stored(dir.path());
    pages.sort_by_key(|p| p.0);
    assert_eq!(pages.len(), 8 * 109);
    for (i, (u, _, numbers)) in pages.into_iter().enumerate() {
        assert_eq!((u, numbers), (uuid(i), packed(&values(10, i))));
    }
}
