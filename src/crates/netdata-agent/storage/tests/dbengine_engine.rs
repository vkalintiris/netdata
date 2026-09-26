//! The running engine over the C-written snapshots of the private fixtures (check `storage.dbengine-engine`):
//! `NETDATA_DBENGINE_FIXTURES` points at them, and without it each test says it skipped. Brief
//! `knowledge/brief-dbengine-s2.md` §5.2 in the status repository.

use std::path::{Path, PathBuf};

use std::collections::HashMap;

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::Priority;
use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
use netdata_agent_storage::dbengine::engine::mrg::Mrg;
use netdata_agent_storage::dbengine::engine::query::{
    Dbengine, EngineConfig, Priority as QueryPriority,
};
use netdata_agent_storage::dbengine::engine::v2index::{populate, readiness};
use netdata_agent_storage::dbengine::format::descriptor::PAGE_TYPE_GORILLA_32BIT;
use netdata_agent_storage::dbengine::format::inspect::for_each_page;

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
            max_disk_space: 25 * 1024 * 1024,
            ..TierConfig::new(tier, dir.clone())
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

/// A start on runR's files an hour after runR's start: after the population, every metric's retention in the
/// registry, per tier, is the one its pages on disk span (the first page's start, the last page's end), whether it
/// came from a v2 index or from the replayed last journal.
#[test]
fn population_matches_the_pages_on_disk() {
    let Some(fx) = fixtures() else {
        return;
    };
    const NOW: i64 = 1_790_351_792 + 3600;
    let work = tempfile::tempdir().unwrap();
    let mrg = Mrg::new();
    let pool = WorkPool::new(4, 256 * 1024);
    for tier in 0..3 {
        let name = if tier == 0 {
            "dbengine".to_string()
        } else {
            format!("dbengine-tier{tier}")
        };
        let dir = work.path().join(&name);
        copy_dir(&fx.join("runR/cache").join(&name), &dir);
        let mut pages: HashMap<[u8; 16], (i64, i64)> = HashMap::new();
        for_each_page(&dir, |d, _| {
            let start = (d.start_time_ut / 1_000_000) as i64;
            let end = if d.page_type == PAGE_TYPE_GORILLA_32BIT {
                start + i64::from(d.gorilla_delta_s())
            } else {
                (d.end_time_ut() / 1_000_000) as i64
            };
            let e = pages.entry(d.uuid).or_insert((start, end));
            e.0 = e.0.min(start);
            e.1 = e.1.max(end);
        })
        .unwrap();
        let cfg = TierConfig {
            max_disk_space: 25 * 1024 * 1024,
            ..TierConfig::new(tier, dir)
        };
        let mut loaded = load(cfg, &mrg, NOW).unwrap();
        populate(&mut loaded, &mrg, &pool, 4, NOW);
        readiness(&mut loaded, NOW);
        assert!(!pages.is_empty());
        for (uuid, &(first, last)) in &pages {
            let r = mrg.get_and_acquire(uuid, tier).map(|m| m.retention());
            assert_eq!(
                r.map(|r| (r.first_time_s, r.last_time_s)),
                Some((first, last)),
                "tier {tier} metric {uuid:02x?}"
            );
        }
        assert!(mrg.entries() >= pages.len());
    }
}

/// The metric `chart.dim` of runR's child, from the checkers' map.
fn uuid_of(fx: &Path, chart: &str, dim: &str) -> [u8; 16] {
    let map = std::fs::read_to_string(fx.join("results/runR/metric-map.tsv")).unwrap();
    let line = map
        .lines()
        .find(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            f.len() > 2 && f[1] == chart && f[2] == dim
        })
        .unwrap();
    let hex = line.split('\t').next().unwrap();
    let mut u = [0u8; 16];
    for (i, b) in u.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
    }
    u
}

/// Queries on runR read what the fixture generator sent: tier 0 point by point (`(t/10 % 1000) + c/2 + d/8`) over
/// every metric's retention, with c3's gap jumped over and c1.d4's empty samples, tier 1 as one-minute aggregates of
/// 60 points.
#[test]
fn queries_read_the_generated_values() {
    let Some(fx) = fixtures() else {
        return;
    };
    const START: i64 = 1_789_980_541;
    const NOW: i64 = 1_790_351_792 + 3600;
    let work = tempfile::tempdir().unwrap();
    let mrg = Mrg::new();
    let pool = WorkPool::new(4, 256 * 1024);
    let mut tiers = Vec::new();
    for tier in 0..3 {
        let name = if tier == 0 {
            "dbengine".to_string()
        } else {
            format!("dbengine-tier{tier}")
        };
        let dir = work.path().join(&name);
        copy_dir(&fx.join("runR/cache").join(&name), &dir);
        let cfg = TierConfig {
            max_disk_space: 25 * 1024 * 1024,
            ..TierConfig::new(tier, dir)
        };
        let mut loaded = load(cfg, &mrg, NOW).unwrap();
        populate(&mut loaded, &mrg, &pool, 4, NOW);
        readiness(&mut loaded, NOW);
        tiers.push(loaded);
    }
    let engine = Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            main_cache_bytes: 64 * 1024 * 1024,
            extent_cache_bytes: 16 * 1024 * 1024,
            pool: Some(pool),
            ..EngineConfig::new(|| NOW)
        },
    );
    let value = |t: i64, c: f64, d: f64| (t / 10 % 1000) as f64 + c * 0.5 + d * 0.125;

    let metric = engine
        .mrg
        .get_and_acquire(&uuid_of(&fx, "b6.c0", "d2"), 0)
        .unwrap();
    let (from, to) = (START + 1000, START + 1100);
    let mut q = engine.query(&metric, from, to, QueryPriority::Normal, NOW);
    let mut n = 0;
    while !q.is_finished() {
        let p = q.next_metric();
        assert_eq!(p.count, 1, "at {}", p.end_time_s);
        let want = value(p.end_time_s, 0.0, 2.0);
        assert!(
            (p.sum - want).abs() < 1e-6 * want.max(1.0),
            "at {}: {} != {want}",
            p.end_time_s,
            p.sum
        );
        n += 1;
    }
    assert_eq!(n, to - from + 1);

    // c3 has no data in [start + 100000, start + 105000): the points jump over the gap
    let metric = engine
        .mrg
        .get_and_acquire(&uuid_of(&fx, "b6.c3", "d0"), 0)
        .unwrap();
    let mut q = engine.query(
        &metric,
        START + 99_990,
        START + 105_010,
        QueryPriority::Normal,
        NOW,
    );
    let mut times = Vec::new();
    while !q.is_finished() {
        let p = q.next_metric();
        assert_eq!(p.count, 1, "at {}", p.end_time_s);
        let want = value(p.end_time_s, 3.0, 0.0);
        assert!(
            (p.sum - want).abs() < 1e-6 * want.max(1.0),
            "at {}",
            p.end_time_s
        );
        times.push(p.end_time_s - START);
    }
    let want: Vec<i64> = (99_990..100_000).chain(105_000..=105_010).collect();
    assert_eq!(times, want);

    // every point of every metric over tier 0's retention
    for c in 0..4 {
        for d in 0..5 {
            let metric = engine
                .mrg
                .get_and_acquire(&uuid_of(&fx, &format!("b6.c{c}"), &format!("d{d}")), 0)
                .unwrap();
            let r = metric.retention();
            let mut q = engine.query(
                &metric,
                r.first_time_s,
                r.last_time_s,
                QueryPriority::Normal,
                NOW,
            );
            let mut t = r.first_time_s;
            while !q.is_finished() {
                if c == 3 && t == START + 100_000 {
                    t = START + 105_000;
                }
                let p = q.next_metric();
                assert_eq!((p.end_time_s, p.count), (t, 1), "c{c}.d{d}");
                if c == 1 && d == 4 && t % 1000 == 500 {
                    assert!(p.sum.is_nan(), "c{c}.d{d} at {t}: sent empty");
                } else {
                    let want = value(t, f64::from(c), f64::from(d));
                    assert!(
                        (p.sum - want).abs() < 1e-6 * want.max(1.0),
                        "c{c}.d{d} at {t}: {} != {want}",
                        p.sum
                    );
                }
                t += 1;
            }
            assert_eq!(t, r.last_time_s + 1, "c{c}.d{d}");
        }
    }

    // tier 1: minute aggregates of the points in (end - 60, end]
    let metric = engine
        .mrg
        .get_and_acquire(&uuid_of(&fx, "b6.c0", "d0"), 1)
        .unwrap();
    let mut q = engine.query(
        &metric,
        START + 3600,
        START + 7200,
        QueryPriority::Normal,
        NOW,
    );
    let mut minutes = 0;
    while !q.is_finished() {
        let p = q.next_metric();
        if p.count > 0 {
            let e = p.end_time_s;
            assert_eq!((p.count, e % 60), (60, 0), "at {e}");
            let values: Vec<f64> = (e - 59..=e).map(|t| value(t, 0.0, 0.0)).collect();
            let sum: f64 = values.iter().sum();
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            assert_eq!((p.sum, p.min, p.max), (sum, min, max), "at {e}");
            minutes += 1;
        }
    }
    assert!(minutes >= 59, "{minutes}");
}

/// runR's tier 0 at its next start: the last file (4, 12 KiB) is reused while its newest page is at most a day old,
/// and indexed with a new pair 5 after (C: `extents 1, metrics 94, pages 94`).
#[test]
fn runr_last_file_follows_the_one_day_rule() {
    use netdata_agent_storage::dbengine::format::journal_v1;
    use netdata_agent_storage::dbengine::format::journal_v2::{Retention, open_cache_pages};
    let Some(fx) = fixtures() else {
        return;
    };
    let njf =
        std::fs::File::open(fx.join("runR/cache/dbengine/journalfile-1-0000000004.njf")).unwrap();
    let size = njf.metadata().unwrap().len();
    let replay = journal_v1::replay(&njf, size).unwrap();
    let newest = open_cache_pages(&replay, 0, &mut Retention::default()).last_time_s;
    assert!(newest > 0);
    for (now, indexed) in [(newest + 86_400, false), (newest + 86_401, true)] {
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().join("dbengine");
        copy_dir(&fx.join("runR/cache/dbengine"), &dir);
        let cfg = TierConfig {
            max_disk_space: 25 * 1024 * 1024,
            ..TierConfig::new(0, dir.clone())
        };
        let (loaded, records) = netdata_agent_log::capture(|| load(cfg, &Mrg::new(), now));
        let loaded = loaded.unwrap();
        let messages: Vec<String> = records
            .into_iter()
            .filter(|r| r.priority != Priority::Debug)
            .filter_map(|r| r.message)
            .collect();
        if indexed {
            assert!(
                messages.contains(&"DBENGINE: tier 0: indexing journalfile-1-0000000004.njfv2: extents 1, metrics 94, pages 94".to_string()),
                "{messages:?}"
            );
            assert!(messages.contains(
                &"DBENGINE: tier 0: created datafile-1-0000000005 (.ndf, .njf).".to_string()
            ));
            assert_eq!(loaded.last_fileno, 5);
        } else {
            assert!(
                !messages
                    .iter()
                    .any(|m| m.contains("indexing") || m.contains("created")),
                "{messages:?}"
            );
            assert_eq!(loaded.last_fileno, 4);
            assert!(!loaded.open_pages.is_empty());
        }
    }
}
