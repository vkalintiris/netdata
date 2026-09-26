use super::*;
use crate::dbengine::format::descriptor::{PAGE_TYPE_ARRAY_32BIT, PageDescriptor};
use crate::dbengine::format::journal_v1::{StoreData, encode_transaction};

const NOW: i64 = 1_800_000_000;
const A: [u8; 16] = [0xaa; 16];
const B: [u8; 16] = [0xbb; 16];

fn cfg(dir: &Path) -> TierConfig {
    TierConfig {
        tier: 0,
        path: dir.to_path_buf(),
        direct_io: false,
        max_disk_space: 256 * 1024 * 1024,
        journal_check: false,
    }
}

/// An array page of `entries` points one second apart, its first point at `start_s`.
fn page(uuid: [u8; 16], start_s: i64, entries: u64) -> PageDescriptor {
    PageDescriptor::array(
        PAGE_TYPE_ARRAY_32BIT,
        uuid,
        (entries * 4) as u32,
        start_s as u64 * 1_000_000,
        (start_s as u64 + entries - 1) * 1_000_000,
    )
}

/// A pair: a data file of `blocks` blocks after its superblock, and a journal with one transaction of these pages.
fn pair(dir: &Path, fileno: u32, blocks: usize, pages: Vec<PageDescriptor>) {
    let mut data = superblock::encode_datafile().to_vec();
    data.resize(BLOCK_SIZE * (1 + blocks), 0);
    std::fs::write(dir.join(file_name(FileKind::Datafile, 1, fileno)), data).unwrap();
    let mut journal = superblock::encode_journal().to_vec();
    if !pages.is_empty() {
        let store = StoreData {
            extent_offset: BLOCK_SIZE as u64,
            extent_size: BLOCK_SIZE as u32,
            descriptors: pages,
        };
        journal.extend_from_slice(&encode_transaction(u64::from(fileno) * 10, &store));
    }
    std::fs::write(dir.join(file_name(FileKind::Journal, 1, fileno)), journal).unwrap();
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
    records
        .into_iter()
        .filter(|r| r.priority != Priority::Debug)
        .filter_map(|r| r.message)
        .collect()
}

fn load_logged(dir: &Path, mrg: &Mrg, now_s: i64) -> (io::Result<Tier>, Vec<String>) {
    let (tier, records) = netdata_agent_log::capture(|| load(cfg(dir), mrg, now_s));
    (tier, messages(records))
}

/// `init_data_files()` on an empty directory: C's records and a first pair of superblocks.
#[test]
fn an_empty_tier_gets_its_first_pair() {
    let dir = tempfile::tempdir().unwrap();
    let (tier, records) = load_logged(dir.path(), &Mrg::new(), NOW);
    let tier = tier.unwrap();
    let path = dir.path().display();
    assert_eq!(
        records,
        [
            format!("DBENGINE: tier 0: found 0 files in path {path}"),
            format!("DBENGINE: data files not found, creating in path \"{path}\"."),
            "DBENGINE: tier 0: created datafile-1-0000000001 (.ndf, .njf).".to_string(),
        ]
    );
    assert_eq!(
        (tier.last_fileno, tier.files.len(), tier.files[0].pos),
        (1, 1, 4096)
    );
    assert_eq!(
        names(dir.path()),
        ["datafile-1-0000000001.ndf", "journalfile-1-0000000001.njf"]
    );
    let data = std::fs::read(dir.path().join("datafile-1-0000000001.ndf")).unwrap();
    assert_eq!(data, superblock::encode_datafile());
}

/// The scan: unknown names reported, lax data file names taken as C's `sscanf()` takes them (and opened by their
/// canonical path), journals without a data file deleted.
#[test]
fn the_scan_follows_cs_name_rules() {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), 1, 1, vec![page(A, NOW - 100, 10)]);
    std::fs::write(dir.path().join("journalfile-1-0000000009.njf"), b"").unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"").unwrap();
    std::fs::write(dir.path().join("datafile-2-0000000003.ndf"), b"").unwrap();
    std::fs::write(dir.path().join("datafile-1-7.ndf.bak"), b"").unwrap();
    let (tier, records) = load_logged(dir.path(), &Mrg::new(), NOW);
    let path = dir.path().display();
    assert_eq!(
        records[..4],
        [
            format!("DBENGINE: tier 0: found 6 files in path {path}"),
            format!("Unknown file detected : \"{path}/datafile-2-0000000003.ndf\""),
            format!("Unknown file detected : \"{path}/notes.txt\""),
            format!(
                "DBENGINE: deleting journal file without matching data file: {path}/journalfile-1-0000000009.njf"
            ),
        ]
    );
    assert!(records.contains(&format!(
        "DBENGINE: uv_fs_unlink(\"{path}/journalfile-1-0000000009.njfv2\"): no such file or directory"
    )));
    assert!(
        records
            .contains(&"DBENGINE: deleted 1 journal files without matching data files".to_string())
    );
    // datafile-1-7 has no canonical file: its pair cannot load and is deleted
    assert!(records.contains(&format!(
        "Failed to open file \"{path}/datafile-1-0000000007.ndf\"."
    )));
    assert!(
        records.contains(&"DBENGINE: deleting invalid data and journal file pair.".to_string())
    );
    let tier = tier.unwrap();
    assert_eq!(tier.files.iter().map(|f| f.fileno).collect::<Vec<_>>(), [1]);
    assert_eq!(
        tier.last_fileno, 7,
        "the last number stays, as C's last_fileno (D62.9)"
    );
}

/// A pair whose data file has a bad superblock is deleted without its journal being replayed (D29); its v2 stays.
#[test]
fn an_invalid_data_file_deletes_its_pair() {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), 1, 1, vec![page(A, NOW - 100, 10)]);
    pair(dir.path(), 2, 1, vec![page(B, NOW - 50, 10)]);
    std::fs::write(dir.path().join("journalfile-1-0000000001.njfv2"), b"stale").unwrap();
    let mut data = std::fs::read(dir.path().join("datafile-1-0000000001.ndf")).unwrap();
    data[0] ^= 0xff;
    std::fs::write(dir.path().join("datafile-1-0000000001.ndf"), data).unwrap();
    let mrg = Mrg::new();
    let (tier, records) = load_logged(dir.path(), &mrg, NOW);
    let path = dir.path().display();
    assert_eq!(
        records[2..],
        [
            "DBENGINE: file has invalid superblock.".to_string(),
            "DBENGINE: deleting invalid data and journal file pair.".to_string(),
            format!("DBENGINE: deleted journal file \"{path}/journalfile-1-0000000001.njf\"."),
            format!("DBENGINE: deleted data file \"{path}/datafile-1-0000000001.ndf\"."),
        ]
    );
    assert!(
        mrg.get_and_acquire(&A, 0).is_none(),
        "the deleted pair's journal was not replayed"
    );
    assert!(mrg.get_and_acquire(&B, 0).is_some());
    assert!(names(dir.path()).contains(&"journalfile-1-0000000001.njfv2".to_string()));
    assert_eq!(tier.unwrap().files.len(), 1);
}

/// The last file stays open when small and recent: its pages are hot, no v2 is built and no pair is added. On tier
/// 0 a newest page more than a day old gets it indexed and a new pair created.
#[test]
fn the_last_file_is_reused_or_indexed_as_c() {
    for (age, reused) in [(OLD_DATA_S, true), (OLD_DATA_S + 1, false)] {
        let dir = tempfile::tempdir().unwrap();
        pair(
            dir.path(),
            1,
            1,
            vec![page(A, NOW - age - 9, 10), page(B, NOW - age - 9, 10)],
        );
        let mrg = Mrg::new();
        let (tier, records) = load_logged(dir.path(), &mrg, NOW);
        let tier = tier.unwrap();
        assert_eq!(
            mrg.get_and_acquire(&A, 0)
                .map(|m| m.retention().last_time_s),
            Some(NOW - age),
            "the replay expands the registry"
        );
        if reused {
            assert_eq!(records.len(), 2, "{records:?}");
            assert_eq!(
                (tier.open_pages.len(), tier.files.len(), tier.last_fileno),
                (2, 1, 1)
            );
            assert!(tier.files[0].v2.is_none());
        } else {
            assert_eq!(
                records[2..],
                [
                    "DBENGINE: tier 0: indexing journalfile-1-0000000001.njfv2: extents 1, metrics 2, pages 2"
                        .to_string(),
                    "DBENGINE: tier 0: migrated journalfile-1-0000000001.njfv2, 4.2KiB".to_string(),
                    "DBENGINE: tier 0: created datafile-1-0000000002 (.ndf, .njf).".to_string(),
                ]
            );
            assert!(tier.open_pages.is_empty() && tier.files[0].v2.is_some());
            assert_eq!((tier.files.len(), tier.last_fileno), (2, 2));
            let v2 = File::open(dir.path().join("journalfile-1-0000000001.njfv2")).unwrap();
            assert_eq!(journal_v2::validate(&v2, 8192, true).unwrap(), Verdict::Ok);
        }
    }
}

/// A last file past a third of the target size is indexed even when recent, and a new pair follows; a file that is
/// not the last one is indexed after its replay, and at the next start its v2 loads without a replay.
#[test]
fn v2_files_are_loaded_or_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    pair(dir.path(), 1, 1, vec![page(A, NOW - 100, 10)]);
    // target 2,684,354 bytes: a third is 894,784; 219 blocks and the superblock pass it
    pair(dir.path(), 2, 219, vec![page(B, NOW - 50, 10)]);
    let mrg = Mrg::new();
    let (first, records) = load_logged(dir.path(), &mrg, NOW);
    let first = first.unwrap();
    assert!(
        first.files[0].v2.is_some() && first.files[1].v2.is_some(),
        "{records:?}"
    );
    assert_eq!(
        first.last_fileno, 3,
        "the big last file was indexed and a new pair created"
    );
    assert!(mrg.get_and_acquire(&A, 0).is_some() && mrg.get_and_acquire(&B, 0).is_some());
    drop(first);

    // again: both v2 files load, so a fresh registry learns nothing from them yet; file 3 is empty and reused
    let mrg = Mrg::new();
    let (second, records) = load_logged(dir.path(), &mrg, NOW);
    let second = second.unwrap();
    assert!(mrg.get_and_acquire(&A, 0).is_none() && mrg.get_and_acquire(&B, 0).is_none());
    assert!(second.files[0].v2.is_some() && second.files[1].v2.is_some());
    assert!(
        !records.iter().any(|r| r.contains("indexing")),
        "{records:?}"
    );
    assert_eq!(second.last_fileno, 3);

    // a corrupt v2 is rebuilt from its journal
    let v2 = dir.path().join("journalfile-1-0000000001.njfv2");
    let mut bytes = std::fs::read(&v2).unwrap();
    bytes[10] ^= 0xff;
    std::fs::write(&v2, bytes).unwrap();
    let (_, records) = load_logged(dir.path(), &Mrg::new(), NOW);
    assert!(records.contains(&"DBENGINE: file CRC32 check: FAILED".to_string()));
    assert!(records.contains(&format!(
        "File \"{}\" is invalid and it will be rebuilt",
        v2.display()
    )));
    assert!(
        records
            .iter()
            .any(|r| r.contains("indexing journalfile-1-0000000001.njfv2"))
    );

    // a missing journal with a valid v2 keeps the pair, with C's record
    std::fs::remove_file(dir.path().join("journalfile-1-0000000001.njf")).unwrap();
    let (tier, records) = load_logged(dir.path(), &Mrg::new(), NOW);
    assert!(records.contains(&format!(
        "Failed to open file \"{}/journalfile-1-0000000001.njf\".",
        dir.path().display()
    )));
    let tier = tier.unwrap();
    assert_eq!(tier.files[0].journal_pos, 0);
    assert!(tier.files[0].v2.is_some());
}

/// `sscanf()`'s view of names.
#[test]
fn names_scan_as_sscanf() {
    assert_eq!(
        scan_numbers("datafile-1-0000000012.ndf", "datafile-"),
        Some((1, 12))
    );
    assert_eq!(
        scan_numbers("datafile-1-12.ndf.bak", "datafile-"),
        Some((1, 12))
    );
    assert_eq!(scan_numbers("datafile-1- 12", "datafile-"), Some((1, 12)));
    assert_eq!(scan_numbers("datafile-12-1.ndf", "datafile-"), None);
    assert_eq!(scan_numbers("datafile-1-x.ndf", "datafile-"), None);
    assert_eq!(
        scan_numbers("datafile-1-99999999999", "datafile-"),
        Some((1, 9999999999u64 as u32))
    );
}
