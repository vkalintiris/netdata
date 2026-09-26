//! The running engine over the C-written snapshots of the private fixtures (check `storage.dbengine-engine`):
//! `NETDATA_DBENGINE_FIXTURES` points at them, and without it each test says it skipped. Brief
//! `knowledge/brief-dbengine-s2.md` §5.2 in the status repository.

use std::path::{Path, PathBuf};

use netdata_agent_log::Priority;
use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
use netdata_agent_storage::dbengine::engine::mrg::Mrg;

fn fixtures() -> Option<PathBuf> {
    let dir = std::env::var_os("NETDATA_DBENGINE_FIXTURES").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: NETDATA_DBENGINE_FIXTURES unset");
    }
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        std::fs::copy(entry.path(), &target).unwrap();
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o644);
        std::fs::set_permissions(&target, perms).unwrap();
    }
}

/// runR's start (2026-09-25T15:56:32Z) on run1's files: the Rust agent's startup decides and records what C's did
/// (runR's `daemon.log`, paths aside), and the v2 it builds for tier 0's file 3 is C's, byte for byte.
#[test]
fn a_start_on_run1_decides_as_runr() {
    let Some(fx) = fixtures() else {
        return;
    };
    const RUNR_START: i64 = 1_790_351_792;
    let work = tempfile::tempdir().unwrap();
    let mrg = Mrg::new();
    let mut ours = Vec::new();
    for tier in 0..3 {
        let name = if tier == 0 {
            "dbengine".to_string()
        } else {
            format!("dbengine-tier{tier}")
        };
        let dir = work.path().join(&name);
        copy_dir(&fx.join("run1/cache").join(&name), &dir);
        let cfg = TierConfig {
            tier,
            path: dir.clone(),
            direct_io: false,
            max_disk_space: 25 * 1024 * 1024,
            journal_check: false,
        };
        let (loaded, records) = netdata_agent_log::capture(|| load(cfg, &mrg, RUNR_START));
        loaded.unwrap();
        for r in records {
            if r.priority != Priority::Debug {
                let message = r.message.unwrap_or_default();
                ours.push(message.replace(&dir.display().to_string(), &format!("<{name}>")));
            }
        }
    }
    let log = std::fs::read_to_string(fx.join("runR/log/daemon.log")).unwrap();
    let staged = "/home/dv/repos/nd/rust/.local/staging/d4-prep/work/runR/cache/";
    let theirs: Vec<String> = log
        .lines()
        .filter(|l| l.contains("thread=DBENGINIT[") && !l.contains("populating retention"))
        .filter(|l| !l.contains("MRG: Loaded"))
        .filter_map(|l| {
            l.split_once("msg=\"")
                .map(|(_, m)| m.trim_end_matches('"').to_string())
        })
        .map(|m| {
            ["dbengine-tier2", "dbengine-tier1", "dbengine"]
                .iter()
                .fold(m, |m, name| {
                    m.replace(&format!("{staged}{name}"), &format!("<{name}>"))
                })
        })
        .collect();
    // C loads the tiers on parallel DBENGINIT threads: its records interleave
    let (mut ours, mut theirs) = (ours, theirs);
    ours.sort();
    theirs.sort();
    assert_eq!(ours, theirs);
    let built = std::fs::read(work.path().join("dbengine/journalfile-1-0000000003.njfv2")).unwrap();
    let c = std::fs::read(fx.join("runR/cache/dbengine/journalfile-1-0000000003.njfv2")).unwrap();
    assert!(built == c, "the rebuilt v2 differs from C's");
}
