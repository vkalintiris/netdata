use super::*;
use crate::dbengine::engine::load::load;
use crate::dbengine::engine::mrg::Retention;
use crate::dbengine::engine::testutil::{NOW, cfg, page, pair};
use std::path::Path;

fn uuid(i: usize) -> [u8; 16] {
    let mut u = [0u8; 16];
    u[..8].copy_from_slice(&(i as u64).to_be_bytes());
    u[15] = 0x5a;
    u
}

fn pool() -> WorkPool {
    WorkPool::new(4, 256 * 1024)
}

/// A tier whose file 1 has a v2 index of `metrics` metrics (built at a first start, then loaded at a second one with
/// a fresh registry, which the population fills).
fn tier_with_v2(dir: &Path, metrics: usize) -> (Tier, Mrg) {
    let pages: Vec<_> = (0..metrics)
        .map(|i| page(uuid(i), NOW - 1000 + i as i64, 10))
        .collect();
    pair(dir, 1, pages.len().div_ceil(109), pages);
    pair(dir, 2, 1, vec![]);
    drop(load(cfg(dir), &Mrg::new(), NOW).unwrap());
    let mrg = Mrg::new();
    let tier = load(cfg(dir), &mrg, NOW).unwrap();
    assert!(tier.files[0].v2.is_some());
    (tier, mrg)
}

/// The population fills the registry from each v2 file, indexes it for lookups, lowers the tier's first time to the
/// headers' earliest start, and reports progress on the calling thread as C does.
#[test]
fn population_fills_the_registry_and_indexes_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tier, mrg) = tier_with_v2(dir.path(), 600);
    assert!(
        mrg.get_and_acquire(&uuid(0), 0).is_none(),
        "nothing replayed: v2 only"
    );
    let ((), records) = netdata_agent_log::capture(|| populate(&mut tier, &mrg, &pool(), 2, NOW));
    let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        messages[0],
        "DBENGINE: tier 0: populating retention to MRG from 2 journal files, using a shared pool of 2 threads..."
    );
    assert!(
        messages[1].starts_with("DBENGINE: tier 0: MRG population completed: "),
        "{messages:?}"
    );
    for i in [0, 255, 256, 257, 599] {
        let start = NOW - 1000 + i as i64;
        assert_eq!(
            mrg.get_and_acquire(&uuid(i), 0).map(|m| m.retention()),
            Some(Retention {
                first_time_s: start,
                last_time_s: start + 9,
                update_every_s: 1
            }),
            "metric {i}"
        );
    }
    let index = &tier.indexes[&1];
    assert_eq!(tier.first_time_s, index.start_time_s());
    assert_eq!(index.start_time_s(), NOW - 1000);
    for i in [0, 1, 255, 256, 511, 512, 599] {
        let m = index
            .find(&uuid(i))
            .unwrap()
            .unwrap_or_else(|| panic!("metric {i}"));
        let pages = index.pages(&m).unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(
            (
                index.start_time_s() + i64::from(pages[0].delta_start_s),
                pages[0].update_every_s
            ),
            (NOW - 1000 + i as i64, 1)
        );
    }
    assert_eq!(index.find(&uuid(600)).unwrap(), None);
    assert_eq!(index.find(&[0; 16]).unwrap(), None);
    readiness(&mut tier, NOW);
    assert_eq!(tier.first_time_s, NOW - 1000, "known: unchanged");
}

/// A metric list that fails its CRC (checked here once, as the load skipped it) leaves the file unavailable and the
/// registry untouched; a tier with no retention is ready from now.
#[test]
fn a_bad_metric_list_hides_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tier, mrg) = tier_with_v2(dir.path(), 3);
    let v2 = dir.path().join("journalfile-1-0000000001.njfv2");
    let mut bytes = std::fs::read(&v2).unwrap();
    let header = Header::decode(bytes[..HEADER_SIZE].try_into().unwrap());
    let metric_offset = header.metric_offset as usize;
    bytes[metric_offset] ^= 0xff;
    std::fs::write(&v2, bytes).unwrap();
    populate(&mut tier, &mrg, &pool(), 1, NOW);
    assert!(tier.indexes.is_empty());
    assert!(mrg.get_and_acquire(&uuid(0), 0).is_none());
    assert_eq!(tier.first_time_s, i64::MAX);
    let ((), records) = netdata_agent_log::capture(|| readiness(&mut tier, NOW));
    assert_eq!(tier.first_time_s, NOW);
    let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        messages,
        ["DBENGINE: tier 0: ready for data collection and queries"]
    );
}
