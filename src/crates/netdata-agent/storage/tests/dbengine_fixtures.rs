//! dbengine readers and writers against tier directories the C agent wrote. The snapshots are not committed (D50.2):
//! `NETDATA_DBENGINE_FIXTURES` points at them, and without it each test says it skipped. Brief
//! `knowledge/brief-dbengine-s0.md` §7.2 L3 in the status repository.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use netdata_agent_storage::dbengine::format::journal_v2::{self, Retention, Verdict};
use netdata_agent_storage::dbengine::format::{ReadAt, journal_v1};

const SNAPSHOTS: [&str; 6] = [
    "run1",
    "runR",
    "run2",
    "orig-20260924/run1",
    "orig-20260924/runR",
    "orig-20260924/run2",
];
const TIERS: [&str; 3] = [
    "cache/dbengine",
    "cache/dbengine-tier1",
    "cache/dbengine-tier2",
];

fn fixtures() -> Option<PathBuf> {
    let dir = std::env::var_os("NETDATA_DBENGINE_FIXTURES").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: NETDATA_DBENGINE_FIXTURES unset");
    }
    dir
}

/// Every `.njfv2` of the snapshots with its `.njf`.
fn v2_files(fx: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for snapshot in SNAPSHOTS {
        for tier in TIERS {
            let Ok(entries) = fs::read_dir(fx.join(snapshot).join(tier)) else {
                continue;
            };
            for e in entries {
                let v2 = e.unwrap().path();
                if v2.extension().is_some_and(|x| x == "njfv2") {
                    out.push((v2.with_extension("njf"), v2));
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
fn v2_files_validate_and_rebuild_byte_identical() {
    let Some(fx) = fixtures() else {
        return;
    };
    let files = v2_files(&fx);
    assert_eq!(files.len(), 14);
    for (v1, v2) in files {
        let expected = fs::read(&v2).unwrap();
        let v1_file = File::open(&v1).unwrap();
        let v1_size = v1_file.size().unwrap();
        assert_eq!(
            journal_v2::validate(&expected[..], v1_size as u32, true).unwrap(),
            Verdict::Ok,
            "{}",
            v2.display()
        );
        let replay = journal_v1::replay(&v1_file, v1_size).unwrap();
        let built = journal_v2::from_v1(&replay, v1_size, 0, &mut Retention::default()).unwrap();
        assert!(
            built == expected,
            "{} differs from its rebuild",
            v2.display()
        );
    }
}
