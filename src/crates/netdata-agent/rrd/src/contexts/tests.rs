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

// ---- loading from SQL (rrdcontext-loading.c) ----

/// A tier that knows fixed retentions by UUID.
#[derive(Debug, Default)]
struct FakeTier(std::collections::HashMap<[u8; 16], (i64, i64)>);

impl TierRetention for FakeTier {
    fn retention_by_id(&self, uuid: &[u8; 16]) -> Option<(i64, i64)> {
        self.0.get(uuid).copied()
    }
}

#[derive(Debug)]
struct FakeLabels;

impl LabelSource for FakeLabels {
    fn chart_labels(&self, chart_uuid: &[u8; 16]) -> Vec<(Vec<u8>, Vec<u8>, u32)> {
        vec![(b"uuid".to_vec(), vec![b'0' + chart_uuid[0]], 1)]
    }
}

fn sql_chart(uuid: u8, id: &str, context: &str) -> SqlChart {
    SqlChart {
        chart_id: [uuid; 16],
        id: Some(id.into()),
        name: None,
        context: Some(context.into()),
        title: Some("Title".into()),
        units: Some("u".into()),
        priority: 1000,
        update_every: 1,
        chart_type: ChartType::Area,
        family: Some("fam".into()),
    }
}

fn sql_dim(uuid: u8, id: &str, chart: &str, context: &str) -> SqlDim {
    SqlDim {
        dim_id: [uuid; 16],
        id: Some(id.into()),
        name: Some(id.into()),
        hidden: false,
        chart_id: Some(chart.into()),
        context: Some(context.into()),
        algorithm: Algorithm::Incremental,
    }
}

#[test]
fn a_load_counts_as_c_and_keeps_what_has_retention() {
    let contexts = Contexts::default();
    let tier = FakeTier(
        [
            ([10; 16], (T, T + 100)),
            ([11; 16], (T + 50, T + 200)),
            ([12; 16], (T, T + 10)),
        ]
        .into_iter()
        .collect(),
    );
    contexts.set_tiers(vec![Arc::new(tier)]);
    contexts.set_label_source(Arc::new(FakeLabels));
    let mut loader = contexts.loader().unwrap();
    assert!(contexts.loader().is_none(), "it loads once");
    // a versionless context row is skipped; a stored hub context is taken as it was
    loader.context(&SqlContext {
        id: Some("ctx.skip".into()),
        ..SqlContext::default()
    });
    loader.context(&SqlContext {
        id: Some("ctx.a".into()),
        version: 42,
        title: Some(b"Hub".to_vec()),
        chart_type: Some("stacked".into()),
        priority: 7,
        first_time_s: T as u64,
        last_time_s: (T + 5) as u64,
        ..SqlContext::default()
    });
    loader.chart(&sql_chart(1, "t.one", "ctx.a"));
    loader.chart(&sql_chart(2, "t.empty", "ctx.a"));
    loader.chart(&sql_chart(3, "t.lonely", "ctx.b"));
    loader.chart(&SqlChart {
        context: None,
        ..sql_chart(4, "t.x", "")
    });
    loader.chart(&SqlChart {
        id: None,
        ..sql_chart(5, "", "ctx.a")
    });
    loader.dim(&sql_dim(10, "d", "t.one", "ctx.a"));
    // the same dimension id again: the later row's UUID wins
    loader.dim(&sql_dim(11, "d", "t.one", "ctx.a"));
    loader.dim(&sql_dim(12, "e", "t.nochart", "ctx.a"));
    loader.dim(&sql_dim(13, "z", "t.one", "ctx.a"));
    let (report, records) = netdata_agent_log::capture(|| loader.finish("node", || false));
    assert_eq!(
        report,
        LoadReport {
            contexts: 1,
            contexts_deleted: 1,
            instances: 1,
            instances_deleted: 2,
            instances_ignored: 2,
            metrics: 1,
            metrics_ignored: 1,
            metrics_zero_retention: 1,
            cleanup: vec!["ctx.b".into()],
            deleted_from_sql: vec![],
        }
    );
    let record = records
        .into_iter()
        .filter_map(|r| r.message)
        .next_back()
        .unwrap();
    assert_eq!(
        record,
        "RRDCONTEXT: metadata for node 'node': contexts 1 (deleted 1), instances 1 (deleted 2, ignored 2), and metrics \
         1 (ignored 1, zero retention 1)"
    );
    let rc = contexts.get("ctx.a").unwrap();
    assert!(contexts.get("ctx.b").is_none() && contexts.get("ctx.skip").is_none());
    let ri = rc.instance("t.one").unwrap();
    let rm = ri.metric("d").unwrap();
    assert_eq!(rm.state().uuid, [11; 16]);
    // retention from the tiers for the UUID in force, archived all the way up
    assert_eq!(
        (rm.state().first_time_s, rm.state().last_time_s),
        (T + 50, T + 200)
    );
    assert_eq!(
        (rc.state().first_time_s, rc.state().last_time_s),
        (T + 50, T + 200)
    );
    for f in [&rc.flags, &ri.flags, &rm.flags] {
        assert!(f.is_archived(), "{:#x}", f.get());
    }
    assert_eq!(ri.state().name, "t.one");
    assert_eq!(rc.state().priority, 1000);
    // the chart's labels come at first use
    assert!(ri.flags.check(flags::DEMAND_LABELS));
    assert_eq!(ri.labels().get(b"uuid"), Some(&b"1"[..]));
    assert!(!ri.flags.check(flags::DEMAND_LABELS));
    // a chart and dimension created again keep their UUIDs
    assert_eq!(contexts.find_chart_uuid("ctx.a", "t.one"), Some([1; 16]));
    assert_eq!(
        contexts.find_dimension_uuid("ctx.a", "t.one", "d"),
        Some([11; 16])
    );
}

#[test]
fn charts_created_again_reuse_the_loaded_uuids() {
    let (contexts, charts) = setup();
    contexts.set_tiers(vec![Arc::new(FakeTier(
        [([9; 16], (T, T + 1))].into_iter().collect(),
    ))]);
    let mut loader = contexts.loader().unwrap();
    loader.chart(&sql_chart(7, "t.a", "ctx.a"));
    loader.dim(&sql_dim(9, "d", "t.a", "ctx.a"));
    loader.finish("node", || false);
    let (chart, _) = charts.create(&spec("a", "ctx.a", "Title", 1000));
    assert_eq!(*chart.uuid(), [7; 16]);
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    assert_eq!(*dim.uuid(), [9; 16]);
    // a collected chart's context is not touched by an archived newcomer
    let (contexts, charts) = setup();
    let (chart, _) = charts.create(&spec("b", "ctx.b", "Live", 1000));
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&chart, T);
    contexts.process_queued();
    let before = (
        contexts.version(),
        contexts.get("ctx.b").unwrap().state().title,
    );
    let mut loader = contexts.loader().unwrap();
    loader.chart(&SqlChart {
        title: Some("Stored".into()),
        ..sql_chart(8, "t.other", "ctx.b")
    });
    assert_eq!(
        (
            contexts.version(),
            contexts.get("ctx.b").unwrap().state().title
        ),
        before
    );
}
