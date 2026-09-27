//! `rrdstats_retention_collect()` (`database/rrd-retention.c`): each storage tier of localhost with its size and its
//! retention, as `/api/v2/info`'s `db_size` shows them (S6, D75.4). The pulse retention charts compute their own
//! (a disk-space estimate, integer percentages).

use netdata_agent_text::duration::duration_to_string;

use crate::mode::DbMode;
use crate::storage::StorageLayout;

/// `MAX_EXPECTED_RETENTION_S`: the extrapolated retention of a mostly empty quota grows without bound; it is capped at
/// 25 years.
const MAX_EXPECTED_RETENTION_S: i64 = 25 * 365 * 86400;

/// `STORAGE_ENGINE_BACKEND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Rrddim,
    Dbengine,
}

/// `RRD_STORAGE_TIER`: a tier's numbers, its human strings empty where C leaves them unset.
#[derive(Debug, Clone, PartialEq)]
pub struct TierStats {
    pub tier: usize,
    pub backend: Backend,
    /// The tier's grouping times localhost's update every.
    pub group_seconds: u64,
    pub granularity_human: String,
    pub metrics: u64,
    pub samples: u64,
    pub disk_used: u64,
    /// The quota, or with none the free space of the tier's filesystem plus what it uses.
    pub disk_max: u64,
    pub disk_percent: f64,
    /// 0 when the engine does not know the start of its retention.
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub retention: i64,
    pub retention_human: String,
    pub requested_retention: i64,
    pub requested_retention_human: String,
    pub expected_retention: i64,
    pub expected_retention_human: String,
}

/// What a tier's storage engine reports (`storage_engine_metrics()`, `_samples()`, `_disk_space_max()`,
/// `_disk_space_used()`, `_global_first_time_s()`, and the dbengine's `max_retention_s`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineNumbers {
    pub backend: Backend,
    pub metrics: u64,
    pub samples: u64,
    pub disk_max: u64,
    pub disk_used: u64,
    pub first_time_s: i64,
    pub max_retention_s: i64,
}

/// `HOWMANY()`.
fn howmany(x: i64, y: i64) -> i64 {
    (x + y - 1) / y
}

/// `round_retention()`: days above 60 days, hours above a day, else minutes, rounded up.
fn round_retention(retention_s: i64) -> i64 {
    if retention_s > 60 * 86400 {
        howmany(retention_s, 86400) * 86400
    } else if retention_s > 86400 {
        howmany(retention_s, 3600) * 3600
    } else {
        howmany(retention_s, 60) * 60
    }
}

/// `duration_snprintf_time_t()`.
fn human(seconds: i64) -> String {
    duration_to_string(seconds, "s", false).unwrap_or_default()
}

/// One tier of `rrdstats_retention_collect()`: `free_bytes` is asked only of a dbengine tier without a quota
/// (`rrdeng_get_directory_free_bytes_space()`).
pub fn tier_stats(
    tier: usize,
    group_seconds: u64,
    numbers: EngineNumbers,
    now_s: i64,
    free_bytes: impl FnOnce() -> u64,
) -> TierStats {
    let mut disk_max = numbers.disk_max;
    if disk_max == 0 && numbers.backend == Backend::Dbengine {
        disk_max = free_bytes().wrapping_add(numbers.disk_used);
    }
    let disk_percent = if numbers.disk_used != 0 && disk_max != 0 {
        numbers.disk_used as f64 * 100.0 / disk_max as f64
    } else {
        0.0
    };
    let mut stats = TierStats {
        tier,
        backend: numbers.backend,
        group_seconds,
        granularity_human: human(group_seconds as i64),
        metrics: numbers.metrics,
        samples: numbers.samples,
        disk_used: numbers.disk_used,
        disk_max,
        disk_percent,
        first_time_s: numbers.first_time_s,
        last_time_s: now_s,
        retention: 0,
        retention_human: String::new(),
        requested_retention: 0,
        requested_retention_human: String::new(),
        expected_retention: 0,
        expected_retention_human: String::new(),
    };
    // a first time of 0 is an unknown start, not 1970
    if stats.first_time_s <= 0 || stats.first_time_s >= stats.last_time_s {
        return stats;
    }
    stats.retention = stats.last_time_s - stats.first_time_s;
    stats.retention_human = human(round_retention(stats.retention));
    if stats.disk_used == 0 && stats.disk_max == 0 {
        return stats;
    }
    if numbers.backend == Backend::Dbengine {
        stats.requested_retention = numbers.max_retention_s;
    }
    stats.requested_retention_human = human(stats.requested_retention);
    // clamped as a float: a tiny percentage extrapolates past what an integer holds
    let max_retention = MAX_EXPECTED_RETENTION_S.max(stats.requested_retention);
    let space_retention = if stats.disk_percent > 0.0 {
        let extrapolated = (now_s - stats.first_time_s) as f64 * 100.0 / stats.disk_percent;
        if extrapolated >= max_retention as f64 {
            max_retention
        } else {
            extrapolated as i64
        }
    } else {
        0
    };
    stats.expected_retention =
        if stats.requested_retention != 0 && stats.requested_retention < space_retention {
            stats.requested_retention
        } else {
            space_retention
        };
    stats.expected_retention_human = human(round_retention(stats.expected_retention));
    stats
}

/// `rrdstats_retention_collect()` of localhost (`mode`, `update_every`): every tier in use. A dbengine tier reports
/// the engine's numbers; tier 0 of another mode is C's memory engine, which reports only its window
/// (`history_entries`, `default_rrd_history_entries`, times `nd_profile.update_every`).
pub fn retention_stats(
    layout: &StorageLayout,
    mode: DbMode,
    update_every: i64,
    history_entries: i64,
    now_s: i64,
) -> Vec<TierStats> {
    (0..layout.storage_tiers())
        .map(|tier| {
            let group_seconds = layout
                .tier_grouping(tier)
                .saturating_mul(u64::try_from(update_every).unwrap_or(0));
            let dbengine = layout
                .dbengine()
                .filter(|_| layout.tier_is_dbengine(mode, tier))
                .and_then(|e| e.tiers.get(tier).map(|td| (e, td)));
            match dbengine {
                Some((e, td)) => tier_stats(
                    tier,
                    group_seconds,
                    EngineNumbers {
                        backend: Backend::Dbengine,
                        metrics: e.mrg.metrics(tier),
                        samples: td.samples(),
                        disk_max: td.config.max_disk_space,
                        disk_used: td.current_disk_space(),
                        first_time_s: td.global_first_time_s(),
                        max_retention_s: td.config.max_retention_s,
                    },
                    now_s,
                    || td.directory_free_bytes(),
                ),
                None => tier_stats(
                    tier,
                    group_seconds,
                    EngineNumbers {
                        backend: Backend::Rrddim,
                        metrics: 0,
                        samples: 0,
                        disk_max: 0,
                        disk_used: 0,
                        first_time_s: now_s - history_entries * layout.update_every(),
                        max_retention_s: 0,
                    },
                    now_s,
                    || 0,
                ),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
