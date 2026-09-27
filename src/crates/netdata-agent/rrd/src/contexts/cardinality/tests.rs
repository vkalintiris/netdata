use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use netdata_agent_log::{Priority, capture};

use super::*;
use crate::contexts::TierRetention;
use crate::contexts::tests::{sql_chart, sql_dim};
use crate::host::Hosts;
use crate::storage::StorageLayout;
use crate::testutil::info;

const T: i64 = 1_700_000_000;

/// The settings and a count of rotations.
fn gate(
    enabled: bool,
    keep: usize,
    min: usize,
    rotations: usize,
    active: usize,
    no_tier0: usize,
) -> Option<usize> {
    let ec = ExtremeCardinality::default();
    ec.configure(enabled, keep, min);
    ec.to_remove(rotations, active, no_tier0).map(|(n, _)| n)
}

/// The gate of `rrdcontext_post_process_updates()`: off, before the first rotation, without active instances or
/// below `keep`; below the minimum percentage (integer); otherwise the excess over the larger of `keep` and the
/// percentage of the active ones.
#[test]
fn the_gate_follows_c() {
    for (name, args, want) in [
        ("disabled", (false, 1, 0, 1, 10, 10), None),
        ("no rotation yet", (true, 1, 0, 0, 10, 10), None),
        ("no active instance", (true, 1, 0, 1, 0, 0), None),
        ("below keep", (true, 1000, 50, 1, 2000, 999), None),
        (
            "keep reached, below the percentage",
            (true, 1000, 50, 1, 2001, 1000),
            None,
        ),
        (
            "the percentage by integer division",
            (true, 1000, 50, 1, 2000, 1000),
            None,
        ),
        ("keep wins", (true, 1000, 50, 1, 1500, 1200), Some(200)),
        ("the percentage wins", (true, 10, 50, 1, 100, 80), Some(30)),
        ("everything no tier 0", (true, 1, 0, 1, 3, 3), Some(2)),
    ] {
        let (enabled, keep, min, rotations, active, no_tier0) = args;
        assert_eq!(
            gate(enabled, keep, min, rotations, active, no_tier0),
            want,
            "{name}"
        );
    }
    let ec = ExtremeCardinality::default();
    ec.configure(true, 10, 50);
    assert_eq!(
        ec.to_remove(1, 100, 80).unwrap().1,
        "total active instances 100, not in tier0 80, ephemerality 80%"
    );
}

/// A tier whose metrics the protection can delete.
#[derive(Debug, Default)]
struct Tier(Mutex<HashMap<[u8; 16], (i64, i64)>>);

impl TierRetention for Tier {
    fn retention_by_id(&self, uuid: &[u8; 16]) -> Option<(i64, i64)> {
        self.0.lock().unwrap().get(uuid).copied()
    }

    fn delete_by_id(&self, uuid: &[u8; 16]) {
        self.0.lock().unwrap().remove(uuid);
    }
}

/// A host whose context `ctx.y` was loaded with three charts (1-3, metric UUIDs 11-13), the first two with retention on
/// tier 1 only, the third's metric without any (so the load keeps two instances); the protection configured and the
/// engine rotated `rotations` times.
fn fixture(keep: usize, min: usize, rotations: usize) -> (Hosts, Arc<Tier>) {
    let storage = Arc::new(StorageLayout::default());
    storage.extreme_cardinality().configure(true, keep, min);
    let hosts = Hosts::with_storage(
        Host::with_storage("guid-l", true, info("l"), &storage),
        Arc::clone(&storage),
    );
    let (tier0, tier1) = (Arc::new(Tier::default()), Arc::new(Tier::default()));
    tier1
        .0
        .lock()
        .unwrap()
        .extend([([11; 16], (T, T + 100)), ([12; 16], (T + 10, T + 200))]);
    let contexts = hosts.localhost().contexts();
    contexts.set_tiers(vec![tier0 as _, Arc::clone(&tier1) as _]);
    let mut loader = contexts.loader().unwrap();
    for (chart, id) in [(1, "t.e1"), (2, "t.e2"), (3, "t.e3")] {
        loader.chart(&sql_chart(chart, id, "ctx.y"));
        loader.dim(&sql_dim(10 + chart, "d", id, "ctx.y"));
    }
    let ((), _) = capture(|| {
        loader.finish("l", || false, |_| {});
    });
    for _ in 0..rotations {
        storage.db_rotation().rotated(1);
    }
    (hosts, tier1)
}

/// The one NOTICE of clearing `t.e1` in `fixture`.
const CLEARED: &str = "EXTREME CARDINALITY PROTECTION: host 'l', context 'ctx.y', total active instances 2, not in \
                       tier0 2, ephemerality 100%: forcefully cleared the retention of 1 metrics and 1 instances, \
                       having non-tier0 retention from 2023-11-14T22:13:20Z to 2023-11-14T22:15:00Z.";

fn notices(records: &[netdata_agent_log::Captured]) -> Vec<String> {
    records
        .iter()
        .filter(|r| r.priority == Priority::Notice)
        .filter_map(|r| r.message.clone())
        .collect()
}

/// `rrdinstance_forcefully_clear_retention()` from the recompute: of two instances without tier-0 retention with
/// `keep = 1` and no minimum, the first loses its retention on every tier, with one NOTICE; the retry sees nothing
/// left to clear.
#[test]
fn excess_instances_lose_their_retention_once() {
    let (hosts, tier1) = fixture(1, 0, 1);
    let contexts = hosts.localhost().contexts();
    let rc = contexts.get("ctx.y").unwrap();
    assert_eq!(rc.instances().len(), 2, "t.e3 had no retention at the load");
    let ((), records) = capture(|| {
        super::super::recalculate_context_retention(&rc, flags::REASON_DB_ROTATION);
    });
    assert_eq!(notices(&records), [CLEARED]);
    assert_eq!(
        tier1.0.lock().unwrap().keys().copied().collect::<Vec<_>>(),
        [[12; 16]],
        "t.e1's metric cleared, t.e2's kept"
    );
    // the cleared instance has no retention left; the collection removes it
    contexts.garbage_collect(&|| true, |_, _| {});
    let rc = contexts.get("ctx.y").unwrap();
    assert_eq!(
        rc.instances()
            .iter()
            .map(|ri| ri.id().to_string())
            .collect::<Vec<_>>(),
        ["t.e2"]
    );
}

/// Nothing is cleared before a rotation, or while the protection is off.
#[test]
fn no_clear_before_a_rotation_or_when_off() {
    for (name, rotations, enabled) in [("before a rotation", 0, true), ("off", 1, false)] {
        let (hosts, tier1) = fixture(1, 0, rotations);
        hosts
            .storage()
            .extreme_cardinality()
            .configure(enabled, 1, 0);
        let rc = hosts.localhost().contexts().get("ctx.y").unwrap();
        let ((), records) = capture(|| {
            super::super::recalculate_context_retention(&rc, flags::REASON_DB_ROTATION);
        });
        assert!(notices(&records).is_empty(), "{name}");
        assert_eq!(tier1.0.lock().unwrap().len(), 2, "{name}");
    }
}

/// The post-processing queue's pass runs the protection too, once (only the recompute retries).
#[test]
fn a_queued_context_is_protected() {
    let (hosts, tier1) = fixture(1, 0, 1);
    let contexts = hosts.localhost().contexts();
    let rc = contexts.get("ctx.y").unwrap();
    rc.flags.set_updated(flags::REASON_DB_ROTATION);
    rc.trigger_updates();
    let ((), records) = capture(|| contexts.process_queued());
    assert_eq!(notices(&records), [CLEARED]);
    assert_eq!(tier1.0.lock().unwrap().len(), 1);
}
