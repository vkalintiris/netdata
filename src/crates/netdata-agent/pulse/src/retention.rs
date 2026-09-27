//! `pulse-db-dbengine-retention.c`: each dbengine tier's space and time retention, in percent of its limits.

use netdata_agent_rrd::chart::{Algorithm, ChartType};
use netdata_agent_rrd::labels::SRC_AUTO;

use crate::chart::{Def, Localhost, WithDims, dim, set};

/// `dbengine_retention_statistics()`'s update every: ten seconds whatever localhost's.
const UPDATE_EVERY: i32 = 10;

/// The space retention: the used space in percent of the quota, or of the free and used space without one.
pub(crate) fn space_percent(used: u64, quota: u64, free: u64) -> i64 {
    let limit = match quota {
        0 => free.wrapping_add(used),
        quota => quota,
    };
    used.wrapping_mul(100)
        .checked_div(limit)
        .map_or(0, |percent| percent as i64)
}

/// The time retention: the age of the oldest point in percent of the time limit, at most 100 (0 without either).
pub(crate) fn time_percent(first_s: i64, now_s: i64, max_retention_s: i64) -> i64 {
    let retention = if first_s != 0 { now_s - first_s } else { 0 };
    if max_retention_s != 0 {
        (retention.wrapping_mul(100) / max_retention_s).min(100)
    } else {
        0
    }
}

#[derive(Default)]
pub(crate) struct Charts {
    tiers: Vec<Option<WithDims>>,
}

impl Charts {
    /// `dbengine_retention_statistics()`, run while the dbengine is (C's `dbengine_enabled`): the tiers localhost
    /// keeps in it. C's `rrdeng_calculate_tier_disk_space_percentage()` feeds only a share of the database files,
    /// which `get_total_database_space()` gives as 0, so it is left out.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        let storage = localhost.host.storage();
        let Some(engine) = storage.dbengine() else {
            return;
        };
        let tiers = storage.storage_tiers();
        self.tiers.resize_with(tiers, || None);
        for (tier, slot) in self.tiers.iter_mut().enumerate() {
            if !storage.tier_is_dbengine(localhost.mode(), tier) {
                continue;
            }
            let (chart, dims) = slot.get_or_insert_with(|| {
                let chart = localhost.create_every(
                    &Def {
                        id: &format!("dbengine_retention_tier{tier}"),
                        family: "dbengine retention",
                        context: Some("netdata.dbengine_tier_retention"),
                        title: "dbengine space and time retention",
                        units: "%",
                        module: "stats",
                        priority: 134900,
                        chart_type: ChartType::Line,
                    },
                    UPDATE_EVERY,
                );
                let dims = vec![
                    dim(&chart, "space", 1, 1, Algorithm::Absolute),
                    dim(&chart, "time", 1, 1, Algorithm::Absolute),
                ];
                chart.update_meta(|m| m.labels.add(b"tier", tier.to_string().as_bytes(), SRC_AUTO));
                chart.set_metadata_update();
                chart.metadata_updated();
                (chart, dims)
            });
            let td = &engine.tiers[tier];
            // the free space is read only without a quota, as C
            let free = if td.config.max_disk_space == 0 {
                td.directory_free_bytes()
            } else {
                0
            };
            let space = space_percent(td.used_disk_space(), td.config.max_disk_space, free);
            // C's clock is now_realtime_sec(), the engine's in production
            let time = time_percent(
                td.global_first_time_s(),
                engine.now_s(),
                td.config.max_retention_s,
            );
            set(&dims[0], space);
            set(&dims[1], time);
            localhost.done(chart);
        }
    }
}
