//! The extreme cardinality protection (`rrdinstance_forcefully_clear_retention()` and its gate in
//! `rrdcontext_post_process_updates()`, `rrdcontext-worker.c`): once the dbengine rotated, a context whose instances
//! mostly live on the higher tiers only (ephemeral charts, gone from tier 0) loses the retention of the excess ones on
//! every tier. D75.8, D77.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use netdata_agent_log::{Field, Priority, Source, Value, msgid, nd_log, push};
use netdata_agent_text::datetime::rfc3339_datetime_utc;

use super::{Context, flags};
use crate::host::Host;

/// `extreme_cardinality`: `[db] extreme cardinality protection`, `keep instances` and `min ephemerality`, which the
/// daemon reads before the RRDCONTEXT thread starts; until then C's initial values.
#[derive(Debug)]
pub struct ExtremeCardinality {
    enabled: AtomicBool,
    keep_instances: AtomicUsize,
    min_ephemerality: AtomicUsize,
}

impl Default for ExtremeCardinality {
    fn default() -> Self {
        ExtremeCardinality {
            enabled: AtomicBool::new(true),
            keep_instances: AtomicUsize::new(1000),
            min_ephemerality: AtomicUsize::new(50),
        }
    }
}

impl ExtremeCardinality {
    /// The configured settings: whether it runs, the instances without tier-0 retention a context keeps at least,
    /// and the percentage of its active instances they must reach.
    pub fn configure(&self, enabled: bool, keep_instances: usize, min_ephemerality: usize) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.keep_instances.store(keep_instances, Ordering::Relaxed);
        self.min_ephemerality
            .store(min_ephemerality, Ordering::Relaxed);
    }

    /// The gate of `rrdcontext_post_process_updates()`: how many of a context's `no_tier0` instances without tier-0
    /// retention, out of `active`, lose their retention once the engine rotated `rotations` times, with the NOTICE's
    /// description.
    fn to_remove(
        &self,
        rotations: usize,
        active: usize,
        no_tier0: usize,
    ) -> Option<(usize, String)> {
        let keep = self.keep_instances.load(Ordering::Relaxed);
        let min = self.min_ephemerality.load(Ordering::Relaxed);
        if !self.enabled.load(Ordering::Relaxed) || rotations == 0 || active == 0 || no_tier0 < keep
        {
            return None;
        }
        let percent = 100 * no_tier0 / active;
        if percent < min {
            return None;
        }
        let to_keep = (min * active / 100).max(keep);
        let to_remove = no_tier0.saturating_sub(to_keep);
        (to_remove > 0).then(|| {
            (
                to_remove,
                format!("total active instances {active}, not in tier0 {no_tier0}, ephemerality {percent}%"),
            )
        })
    }
}

/// The protection of one post-processed context of `host`, `active` instances of which `no_tier0` have no tier-0
/// retention: whether it cleared anything (the retention is then recomputed again).
pub(super) fn protect(rc: &Context, host: &Host, active: usize, no_tier0: usize) -> bool {
    let storage = host.storage();
    let rotations = storage.db_rotation().rotations();
    match storage
        .extreme_cardinality()
        .to_remove(rotations, active, no_tier0)
    {
        Some((count, descr)) => forcefully_clear_retention(rc, host, count, &descr),
        None => false,
    }
}

/// `rrdinstance_forcefully_clear_retention()`: up to `count` archived instances without tier-0 retention (and without
/// a chart), in the context's order, lose the retention of their archived metrics without tier-0 retention (and
/// without a dimension) on every tier, with C's NOTICE; whether any metric was cleared.
fn forcefully_clear_retention(rc: &Context, host: &Host, mut count: usize, descr: &str) -> bool {
    if count == 0 {
        return false;
    }
    let tiers = rc.tiers();
    let (mut from_s, mut to_s) = (i64::MAX, 0);
    let (mut instances_deleted, mut metrics_deleted) = (0usize, 0usize);
    for ri in rc.instances() {
        if !ri.flags.check(flags::NO_TIER0_RETENTION)
            || ri.flags.is_collected()
            || ri.chart().is_some()
        {
            continue;
        }
        let mut metrics_cleared = 0;
        for rm in ri.metrics() {
            if !rm.flags.check(flags::NO_TIER0_RETENTION)
                || rm.flags.is_collected()
                || rm.dim().is_some()
            {
                continue;
            }
            rm.update_retention();
            let state = rm.state();
            from_s = from_s.min(state.first_time_s);
            to_s = to_s.max(state.last_time_s);
            for tier in &tiers {
                tier.delete_by_id(&state.uuid);
            }
            metrics_cleared += 1;
            metrics_deleted += 1;
            rm.update_retention();
            rm.trigger_updates();
        }
        if metrics_cleared > 0 {
            ri.trigger_updates();
            instances_deleted += 1;
            count -= 1;
            if count == 0 {
                break;
            }
        }
    }
    if metrics_deleted == 0 {
        return false;
    }
    let time = |t: i64| {
        if t == 0 || t == i64::MAX {
            "NONE".to_string()
        } else {
            rfc3339_datetime_utc(t as u64 * 1_000_000, 0)
        }
    };
    let hostname = host.hostname();
    let _frame = push(vec![
        (
            Field::Module,
            Value::Txt("extreme cardinality protection".into()),
        ),
        (Field::NidlNode, Value::Str(hostname.clone())),
        (Field::NidlContext, Value::Str(rc.id().to_string())),
        (Field::MessageId, Value::Uuid(msgid::EXTREME_CARDINALITY)),
    ]);
    nd_log!(
        Source::Daemon,
        Priority::Notice,
        "EXTREME CARDINALITY PROTECTION: host '{hostname}', context '{}', {descr}: forcefully cleared the retention of \
         {metrics_deleted} metrics and {instances_deleted} instances, having non-tier0 retention from {} to {}.",
        rc.id(),
        time(from_s),
        time(to_s)
    );
    true
}

#[cfg(test)]
mod tests;
