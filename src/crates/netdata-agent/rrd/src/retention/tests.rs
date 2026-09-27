use super::*;

const NOW: i64 = 1_800_000_000;
const MIB: u64 = 1 << 20;
const YEAR: i64 = 365 * 86400;

fn dbengine(
    disk_used: u64,
    disk_max: u64,
    first_time_s: i64,
    max_retention_s: i64,
) -> EngineNumbers {
    EngineNumbers {
        backend: Backend::Dbengine,
        metrics: 7,
        samples: 11,
        disk_max,
        disk_used,
        first_time_s,
        max_retention_s,
    }
}

/// The part of the numbers the formulas derive: disk max and percent, retention, requested and expected, each with
/// its human string.
fn derived(s: &TierStats) -> (u64, f64, i64, &str, i64, &str, i64, &str) {
    (
        s.disk_max,
        s.disk_percent,
        s.retention,
        &s.retention_human,
        s.requested_retention,
        &s.requested_retention_human,
        s.expected_retention,
        &s.expected_retention_human,
    )
}

/// `round_retention()`: minutes up to a day, hours up to 60 days, days above, each rounded up.
#[test]
fn retention_rounds_to_minutes_hours_or_days() {
    for (seconds, want) in [
        (0, 0),
        (1, 60),
        (60, 60),
        (61, 120),
        (86400, 86400),
        (86401, 90000),
        (60 * 86400, 60 * 86400),
        (60 * 86400 + 1, 61 * 86400),
    ] {
        assert_eq!(round_retention(seconds), want, "{seconds}");
    }
}

/// `rrdstats_retention_collect()`'s formulas for one tier.
#[test]
fn tier_numbers_follow_c() {
    let no_free = || -> u64 { panic!("a tier with a quota asks no free space") };
    for (name, numbers, free, want) in [
        (
            "a quarter of the quota: the extrapolation",
            dbengine(5 * MIB, 20 * MIB, NOW - 1000, 0),
            None,
            (20 * MIB, 25.0, 1000, "17m", 0, "off", 4000, "1h7m"),
        ),
        (
            "a requested retention under the extrapolation",
            dbengine(5 * MIB, 20 * MIB, NOW - 1000, 3600),
            None,
            (20 * MIB, 25.0, 1000, "17m", 3600, "1h", 3600, "1h"),
        ),
        (
            "a requested retention over it",
            dbengine(5 * MIB, 20 * MIB, NOW - 1000, 7200),
            None,
            (20 * MIB, 25.0, 1000, "17m", 7200, "2h", 4000, "1h7m"),
        ),
        (
            "a nearly empty quota: 25 years",
            dbengine(1, 1 << 40, NOW - 86400, 0),
            None,
            (
                1 << 40,
                100.0 / (1u64 << 40) as f64,
                86400,
                "1d",
                0,
                "off",
                25 * YEAR,
                "25y",
            ),
        ),
        (
            "a requested retention over 25 years raises the cap",
            dbengine(1, 1 << 40, NOW - 86400, 30 * YEAR),
            None,
            (
                1 << 40,
                100.0 / (1u64 << 40) as f64,
                86400,
                "1d",
                30 * YEAR,
                "30y",
                30 * YEAR,
                "30y",
            ),
        ),
        (
            "no quota: the free space",
            dbengine(1000, 0, NOW - 1000, 0),
            Some(3000),
            (4000, 25.0, 1000, "17m", 0, "off", 4000, "1h7m"),
        ),
        (
            "an unknown start",
            dbengine(5 * MIB, 20 * MIB, 0, 3600),
            None,
            (20 * MIB, 25.0, 0, "", 0, "", 0, ""),
        ),
        (
            "a start not before now",
            dbengine(5 * MIB, 20 * MIB, NOW, 3600),
            None,
            (20 * MIB, 25.0, 0, "", 0, "", 0, ""),
        ),
        (
            "no disk at all: the retention only",
            dbengine(0, 0, NOW - 3600, 3600),
            Some(0),
            (0, 0.0, 3600, "1h", 0, "", 0, ""),
        ),
        (
            "the memory engine",
            EngineNumbers {
                backend: Backend::Rrddim,
                metrics: 0,
                samples: 0,
                disk_max: 0,
                disk_used: 0,
                first_time_s: NOW - 3600,
                max_retention_s: 0,
            },
            None,
            (0, 0.0, 3600, "1h", 0, "", 0, ""),
        ),
    ] {
        let stats = match free {
            Some(bytes) => tier_stats(2, 60, numbers, NOW, || bytes),
            None => tier_stats(2, 60, numbers, NOW, no_free),
        };
        assert_eq!(derived(&stats), want, "{name}");
        assert_eq!(
            (
                stats.tier,
                stats.backend,
                stats.group_seconds,
                stats.granularity_human.as_str(),
                stats.metrics,
                stats.samples,
                stats.disk_used,
                stats.first_time_s,
                stats.last_time_s,
            ),
            (
                2,
                numbers.backend,
                60,
                "1m",
                numbers.metrics,
                numbers.samples,
                numbers.disk_used,
                numbers.first_time_s,
                NOW,
            ),
            "{name}"
        );
    }
}

/// Without the engine localhost has one tier, C's memory engine, which reports its window.
#[test]
fn a_host_without_the_engine_reports_its_memory_window() {
    let layout = StorageLayout::new(None);
    let stats = retention_stats(&layout, DbMode::Ram, 1, 3600, NOW);
    assert_eq!(stats.len(), 1);
    assert_eq!(
        (
            stats[0].backend,
            stats[0].group_seconds,
            stats[0].first_time_s,
            stats[0].retention,
        ),
        (Backend::Rrddim, 1, NOW - 3600, 3600)
    );
}

/// With the engine every tier in use reports: a dbengine host's all from the engine, another mode's tier 0 from the
/// memory engine. The engine's numbers are the tier's: its registry metrics, and with no quota the free space.
#[test]
fn engine_tiers_report_the_engine_numbers() {
    let (_dirs, layout) = crate::testutil::engine(3);
    let e = layout.dbengine().unwrap();
    let _held = e.mrg.add_and_acquire(&[1; 16], 1, NOW - 100, NOW, 1);
    let stats = retention_stats(&layout, DbMode::Dbengine, 1, 3600, NOW);
    assert_eq!(
        stats
            .iter()
            .map(|s| (s.tier, s.backend, s.group_seconds, s.metrics))
            .collect::<Vec<_>>(),
        [
            (0, Backend::Dbengine, 1, 0),
            (1, Backend::Dbengine, 60, 1),
            (2, Backend::Dbengine, 3600, 0)
        ]
    );
    for s in &stats {
        let td = &e.tiers[s.tier];
        assert_eq!(s.disk_used, td.current_disk_space());
        assert!(
            s.disk_max > s.disk_used,
            "no quota: the free space plus the used"
        );
        assert_eq!(
            (s.first_time_s, s.retention),
            (0, 0),
            "loaded, never ready: an unknown start"
        );
    }
    let stats = retention_stats(&layout, DbMode::Ram, 1, 3600, NOW);
    assert_eq!(
        stats.iter().map(|s| s.backend).collect::<Vec<_>>(),
        [Backend::Rrddim, Backend::Dbengine, Backend::Dbengine]
    );
    assert_eq!(stats[0].first_time_s, NOW - 3600);
}
