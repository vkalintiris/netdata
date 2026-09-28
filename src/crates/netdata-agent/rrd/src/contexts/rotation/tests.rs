use std::collections::HashMap;
use std::sync::Mutex;

use super::*;
use crate::contexts::TierRetention;
use crate::contexts::tests::{sql_chart, sql_dim};
use crate::storage::StorageLayout;
use crate::testutil::info;

const T: i64 = 1_700_000_000;
const NOW_UT: u64 = 1_800_000_000_000_000;

/// A tier whose retentions the test changes, as deletions do.
#[derive(Debug, Default)]
struct Tier(Mutex<HashMap<[u8; 16], (i64, i64)>>);

impl TierRetention for Tier {
    fn retention_by_id(&self, uuid: &[u8; 16]) -> Option<(i64, i64)> {
        self.0.lock().unwrap().get(uuid).copied()
    }
}

/// `host` loaded from SQL with contexts `ctx.a` (chart `t.a`, metric UUID 10) and `ctx.b` (chart `t.b`, metric UUID
/// 20, the wider retention), both with retention in `tier`.
fn load(host: &Host, tier: &Arc<Tier>) {
    tier.0
        .lock()
        .unwrap()
        .extend([([10; 16], (T, T + 100)), ([20; 16], (T - 50, T + 150))]);
    host.contexts().set_tiers(vec![Arc::clone(tier) as _]);
    let mut loader = host.contexts().loader().unwrap();
    loader.chart(&sql_chart(1, "t.a", "ctx.a"));
    loader.chart(&sql_chart(2, "t.b", "ctx.b"));
    loader.dim(&sql_dim(10, "d", "t.a", "ctx.a"));
    loader.dim(&sql_dim(20, "d", "t.b", "ctx.b"));
    let (_, _) = netdata_agent_log::capture(|| loader.finish("h", || false, |_| {}));
}

fn contexts_of(host: &Host) -> Vec<String> {
    host.contexts()
        .all()
        .iter()
        .map(|rc| rc.id().to_string())
        .collect()
}

/// `rrdcontext_db_rotation()`: every rotation moves the deadline 120 s past it; the pass is due strictly after it; a
/// clear leaves a deadline armed during the pass.
#[test]
fn rotations_arm_the_deadline_and_passes_clear_it() {
    let slot = DbRotation::default();
    assert_eq!(slot.due(NOW_UT), None);
    slot.rotated(NOW_UT);
    let deadline = NOW_UT + 120_000_000;
    assert_eq!(slot.due(deadline), None);
    assert_eq!(slot.due(deadline + 1), Some(deadline));
    // a later rotation pushes it out
    slot.rotated(NOW_UT + 5_000_000);
    assert_eq!(slot.due(deadline + 1), None);
    let later = deadline + 5_000_000;
    assert_eq!(slot.due(later + 1), Some(later));
    slot.done(deadline, later);
    assert_eq!(
        slot.due(later + 1),
        Some(later),
        "armed after the processed one"
    );
    slot.done(later, later);
    assert_eq!(slot.due(u64::MAX), None);
}

/// `rrdcontext_request_full_gc()`: armed only when nothing is (requests coalesce instead of pushing it out); one made
/// once the armed deadline passed, while its pass runs, arms the next pass after it; not a rotation.
#[test]
fn full_gc_requests_arm_once_and_rerun_after_a_running_pass() {
    let slot = DbRotation::default();
    slot.request_full_gc(NOW_UT);
    let deadline = NOW_UT + 120_000_000;
    slot.request_full_gc(NOW_UT + 50_000_000);
    assert_eq!(slot.due(deadline + 1), Some(deadline), "coalesced");
    assert_eq!(slot.rotations(), 0);
    // during the pass
    slot.request_full_gc(deadline + 10);
    slot.done(deadline, deadline + 1_000_000);
    let rerun = deadline + 1_000_000 + 120_000_000;
    assert_eq!(slot.due(rerun), None);
    assert_eq!(slot.due(rerun + 1), Some(rerun), "a pass after the one running");
    slot.done(rerun, rerun + 1);
    assert_eq!(slot.due(u64::MAX), None, "one rerun only");
    // a request before an armed deadline needs no rerun: that pass will see it
    slot.rotated(NOW_UT);
    slot.request_full_gc(NOW_UT + 1);
    slot.done(deadline, deadline + 1);
    assert_eq!(slot.due(u64::MAX), None);
}

/// Once due, every loaded host's retention is recomputed and its garbage collected: a context whose metrics lost
/// their retention goes, through the SQL delete with its host and hub version, and the host's retention narrows;
/// hosts waiting for their load or being loaded are left out; the deadline is cleared.
#[test]
fn a_due_pass_recomputes_and_collects_every_loaded_host() {
    let storage = Arc::new(StorageLayout::default());
    let hosts = Hosts::with_storage(
        Host::with_storage("guid-l", true, info("l"), &storage),
        Arc::clone(&storage),
    );
    let tier = Arc::new(Tier::default());
    load(hosts.localhost(), &tier);
    let pending = hosts.add_archived("guid-p", info("p"), |_| {});
    load(&pending, &tier);
    // a host whose load is under way: a chart read, its dimension not yet
    let loading = hosts.add_archived("guid-g", info("g"), |_| {});
    loading.clear_pending_context_load();
    loading.contexts().set_tiers(vec![Arc::clone(&tier) as _]);
    let mut loader = loading.contexts().loader().unwrap();
    loader.chart(&sql_chart(3, "t.c", "ctx.c"));
    assert_eq!(hosts.localhost().contexts().retention(), (T - 50, T + 150));
    let version = hosts
        .localhost()
        .contexts()
        .get("ctx.b")
        .unwrap()
        .state()
        .hub
        .version;
    let mut deleted = Vec::new();
    let running = || true;
    assert!(
        !deep_pass(&hosts, NOW_UT, &running, |_, _, _| {}),
        "none armed"
    );
    // ctx.b's data is gone
    tier.0.lock().unwrap().remove(&[20; 16]);
    storage.db_rotation().rotated(NOW_UT);
    assert!(!deep_pass(
        &hosts,
        NOW_UT + 120_000_000,
        &running,
        |_, _, _| {}
    ));
    assert_eq!(contexts_of(hosts.localhost()), ["ctx.a", "ctx.b"]);
    assert!(deep_pass(
        &hosts,
        NOW_UT + 120_000_001,
        &running,
        |host, id, version| deleted.push((host.hostname(), id.to_string(), version))
    ));
    assert_eq!(contexts_of(hosts.localhost()), ["ctx.a"]);
    assert_eq!(deleted, [("l".to_string(), "ctx.b".to_string(), version)]);
    assert_eq!(
        contexts_of(&pending),
        ["ctx.a", "ctx.b"],
        "a host waiting for its load is left out"
    );
    assert_eq!(
        contexts_of(&loading),
        ["ctx.c"],
        "a host whose contexts are being loaded is left out"
    );
    assert!(loading.contexts().is_loading());
    drop(loader);
    assert!(!loading.contexts().is_loading());
    assert_eq!(hosts.localhost().contexts().retention(), (T, T + 100));
    assert_eq!(storage.db_rotation().due(u64::MAX), None);
}

/// A pass that finds the daemon stopping collects nothing, and clears the deadline as C does.
#[test]
fn a_stopping_pass_collects_nothing() {
    let storage = Arc::new(StorageLayout::default());
    let hosts = Hosts::with_storage(
        Host::with_storage("guid-l", true, info("l"), &storage),
        Arc::clone(&storage),
    );
    let tier = Arc::new(Tier::default());
    load(hosts.localhost(), &tier);
    tier.0.lock().unwrap().clear();
    storage.db_rotation().rotated(NOW_UT);
    let stopped = || false;
    assert!(deep_pass(&hosts, u64::MAX, &stopped, |_, _, _| {
        panic!("nothing is deleted")
    }));
    assert_eq!(contexts_of(hosts.localhost()), ["ctx.a", "ctx.b"]);
    assert_eq!(storage.db_rotation().due(u64::MAX), None);
}
