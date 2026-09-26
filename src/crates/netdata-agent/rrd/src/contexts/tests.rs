use std::sync::Arc;

use super::*;
use crate::chart::{ChartSpec, Charts};
use crate::collection;
use crate::mode::DbMode;

const T: i64 = 1_700_000_000;

fn setup() -> (Arc<Contexts>, Charts) {
    let contexts = Arc::new(Contexts::default());
    let charts = Charts::new(Arc::clone(&contexts));
    (contexts, charts)
}

fn spec<'a>(id: &'a str, context: &'a str, title: &'a str, priority: i64) -> ChartSpec<'a> {
    ChartSpec {
        type_: "t",
        id,
        name: None,
        family: Some("fam"),
        context: Some(context),
        title,
        units: "u",
        plugin: "p",
        module: None,
        priority,
        update_every: 1,
        chart_type: ChartType::Line,
        mode: DbMode::Ram,
        history_entries: 3600,
        page_size: 4096,
    }
}

/// A v2 collection (SET2 + END2): one stored point per dimension, then the chart's collected hook.
fn collect(chart: &Chart, t: i64) {
    for dim in chart.dims() {
        dim.store_metric(t as u64 * 1_000_000, 1.0, 0);
    }
    collected_rrdset(chart);
}

#[test]
fn a_chart_is_deleted_until_it_stores_then_collected() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "ctx.a", "Title", 1000));
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    assert_eq!(contexts.queued(), 1);
    // A worker tick between CHART/DIMENSION and the first store: no retention anywhere.
    contexts.process_queued();
    let rc = contexts.get("ctx.a").unwrap();
    let ri = rc.instance("t.a").unwrap();
    let rm = ri.metric("d").unwrap();
    for f in [&rc.flags, &ri.flags, &rm.flags] {
        assert!(f.is_deleted(), "{:#x}", f.get());
        assert!(f.check(flags::LIVE_RETENTION));
        assert!(!f.check(flags::UPDATED | flags::QUEUED_FOR_PP));
    }
    // The first store revives the metric, END2 the instance; the context follows on the next tick.
    collect(&chart, T);
    assert!(rm.flags.is_collected() && !rm.flags.is_deleted());
    assert!(ri.flags.is_collected());
    assert!(rc.flags.is_deleted());
    contexts.process_queued();
    assert!(rc.flags.is_collected() && !rc.flags.is_deleted());
    let state = rc.state();
    let dim = chart.dim("d").unwrap();
    let ring = (dim.first_entry_s(), dim.last_entry_s());
    assert_eq!(ring.1, T);
    assert_eq!((state.first_time_s, state.last_time_s), ring);
    assert_eq!(ri.state().first_time_s, ring.0);
    // Every post-processing that changes what Cloud would see widens the host retention.
    assert_eq!(contexts.retention(), ring);
    // Later stores leave the lifecycle alone: nothing is queued.
    collect(&chart, T + 1);
    assert_eq!(contexts.queued(), 0);
}

#[test]
fn begin_end_without_set_does_not_collect_the_instance() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "ctx.a", "Title", 1000));
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collected_rrdset(&chart);
    let ri = contexts.get("ctx.a").unwrap().instance("t.a").unwrap();
    assert!(!ri.flags.is_collected());
    // The chart's collected cache is now set: a later store makes the metric collected, not the instance.
    collect(&chart, T);
    assert!(!ri.flags.is_collected());
    assert!(ri.metric("d").unwrap().flags.is_collected());
    // After a reconnect (or a flags change) the caches reset and the instance is collected.
    rrdset_not_collected(&chart);
    collect(&chart, T + 1);
    assert!(ri.flags.is_collected());
    // The worker agrees: the instance has a collected metric with retention.
    contexts.process_queued();
    assert!(ri.flags.is_collected());
}

#[test]
fn contexts_merge_collected_instances() {
    let (contexts, charts) = setup();
    let (a, _) = charts.create(&spec("a", "ctx", "abcXYZdef", 500));
    let (b, _) = charts.create(&spec("b", "ctx", "abcQdef", 300));
    for chart in [&a, &b] {
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        collect(chart, T);
    }
    contexts.process_queued();
    let rc = contexts.get("ctx").unwrap();
    let state = rc.state();
    // Each CHART merged its title in; the collected instances merged again on post-processing.
    assert_eq!(state.title, b"abc[x]def");
    assert_eq!(
        state.priority, 300,
        "the lowest priority of the collected instances"
    );
    assert!(!rc.flags.check(flags::HIDDEN));
    assert!(rc.state().version > 0);
}

#[test]
fn string_2way_merge_matches_c() {
    let cases: [(&[u8], &[u8], &[u8]); 7] = [
        (b"abc", b"abc", b"abc"),
        (b"abcXYZdef", b"abcQdef", b"abc[x]def"),
        (b"abc", b"abcd", b"abc[x]"),
        (b"xyz", b"abc", b"[x]"),
        (b"[x]", b"abc", b"[x]"),
        (b"abc", b"[x]", b"[x]"),
        // A shared lead byte of a multi-byte character stays, split, as in C.
        ("a\u{e9}".as_bytes(), "a\u{e8}".as_bytes(), b"a\xc3[x]"),
    ];
    for (a, b, expected) in cases {
        assert_eq!(string_2way_merge(a, b), expected, "{a:?} + {b:?}");
    }
}

#[test]
fn hidden_charts_hide_their_context() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "ctx", "T", 1000));
    chart.update_meta(|m| m.flags |= chart_flags::HIDDEN);
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&chart, T);
    contexts.process_queued();
    let rc = contexts.get("ctx").unwrap();
    assert!(rc.instance("t.a").unwrap().flags.check(flags::HIDDEN));
    assert!(rc.flags.check(flags::HIDDEN));
}

#[test]
fn a_disconnect_archives_everything_on_the_next_cycle() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "ctx", "T", 1000));
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&chart, T);
    collect(&chart, T + 1);
    contexts.worker_cycle();
    let rc = contexts.get("ctx").unwrap();
    assert!(rc.flags.is_collected());
    contexts.child_disconnected();
    contexts.worker_cycle();
    let ri = rc.instance("t.a").unwrap();
    let rm = ri.metric("d").unwrap();
    for f in [&rc.flags, &ri.flags, &rm.flags] {
        assert!(f.is_archived() && !f.is_collected(), "{:#x}", f.get());
    }
    // The reason is consumed by the same pass (rrd_flag_unset_updated()).
    assert!(!rm.flags.check(flags::REASON_DISCONNECTED_CHILD));
    assert_eq!(contexts.retention().1, T + 1);
    // The child comes back: the caches reset and the first store collects again.
    rrdset_not_collected(&chart);
    collect(&chart, T + 2);
    contexts.worker_cycle();
    assert!(rm.flags.is_collected() && rc.flags.is_collected());
}

#[test]
fn obsolete_charts_and_dimensions_are_archived() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "ctx", "T", 1000));
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&chart, T);
    contexts.process_queued();
    let ri = contexts.get("ctx").unwrap().instance("t.a").unwrap();
    let rm = ri.metric("d").unwrap();
    chart.dim_is_obsolete(&dim);
    assert!(rm.flags.is_archived());
    chart.is_obsolete();
    assert!(ri.flags.is_archived());
    // Collecting again clears both (rrdset_timed_done() and SET2 do this).
    chart.isnot_obsolete();
    chart.dim_isnot_obsolete(&dim);
    collect(&chart, T + 1);
    assert!(rm.flags.is_collected() && ri.flags.is_collected());
}

#[test]
fn a_chart_moving_to_another_context_leaves_the_old_instance_archived() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "old", "T", 1000));
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&chart, T);
    contexts.process_queued();
    let old = contexts.get("old").unwrap().instance("t.a").unwrap();
    charts.create(&spec("a", "new", "T", 1000));
    assert!(old.flags.is_deleted() && old.chart().is_none());
    assert!(old.metric("d").unwrap().dim().is_none());
    let new = contexts.get("new").unwrap().instance("t.a").unwrap();
    assert!(Arc::ptr_eq(&new, &chart.contexts().instance().unwrap()));
    assert!(new.metric("d").unwrap().dim().is_some());
    // On the next tick the old metric finds the live ring by UUID (the RAM engine's index): the old instance and
    // context stay, archived, with that retention.
    contexts.process_queued();
    let dim = chart.dim("d").unwrap();
    let rm = old.metric("d").unwrap();
    assert_eq!(
        (rm.state().first_time_s, rm.state().last_time_s),
        (dim.first_entry_s(), dim.last_entry_s())
    );
    for f in [&old.flags, &contexts.get("old").unwrap().flags] {
        assert!(f.is_archived() && !f.is_deleted(), "{:#x}", f.get());
    }
}

#[test]
fn v1_collections_report_through_timed_done() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "ctx", "T", 1000));
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    // BEGIN (a trusted one-second step), SET, END: the first collection only starts the clock.
    for i in 0..4 {
        collection::next_usec_unfiltered(&chart, (T + i, 0), 1_000_000);
        collection::set_value(&dim, (T + i, 0), 7);
        collection::timed_done(&chart, (T + i, 0), false, 3);
    }
    contexts.process_queued();
    let rc = contexts.get("ctx").unwrap();
    assert!(rc.flags.is_collected(), "{:#x}", rc.flags.get());
    assert_eq!(rc.state().last_time_s, dim.last_entry_s());
}

/// `rrdhost_update_cached_retention()` owes a stream path for every change of the host's first time, while a
/// receiver takes them; a global recompute that keeps it owes nothing.
#[test]
fn first_time_changes_are_recorded_while_asked() {
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("a", "ctx.a", "Title", 1000));
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&chart, T);
    contexts.process_queued();
    assert!(
        contexts.take_first_time_changes().is_empty(),
        "nothing recorded before asked"
    );
    let first = contexts.retention().0;
    contexts.record_first_time_changes(true);
    // a second chart with an older first time widens the host's
    let (older, _) = charts.create(&spec("b", "ctx.b", "Title", 1000));
    older.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&older, T - 10);
    contexts.process_queued();
    let widened = contexts.retention().0;
    assert!(widened < first, "{widened} < {first}");
    assert_eq!(contexts.take_first_time_changes(), [widened]);
    contexts.recalculate_host_retention(flags::REASON_DISCONNECTED_CHILD);
    assert_eq!(contexts.retention().0, widened);
    assert!(contexts.take_first_time_changes().is_empty());
    contexts.record_first_time_changes(false);
    contexts.recalculate_host_retention(flags::REASON_DISCONNECTED_CHILD);
    assert!(contexts.take_first_time_changes().is_empty());
}
