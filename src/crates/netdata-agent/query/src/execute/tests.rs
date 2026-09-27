use std::sync::Arc;

use netdata_agent_rrd::chart::Algorithm;
use netdata_agent_rrd::host::Host;
use netdata_agent_storage::storage_number::{SN_FLAG_NOT_ANOMALOUS, pack, unpack};

use super::*;
use crate::testing::{T0, host, v1_target, v2_target};

fn run(h: &Arc<Host>, query: &str) -> (QueryTarget, Window, Rrdr) {
    let (mut qt, mut window) = v1_target(h, query);
    let control = Control {
        received: Instant::now(),
        interrupted: &|_| false,
        windows: Windows::default(),
        pulse: None,
    };
    let r = run_v1(&mut qt, &mut window, &control);
    (qt, window, r)
}

fn rows(r: &Rrdr) -> Vec<(f64, u32)> {
    (0..r.rows).map(|i| (r.v[i], r.o[i])).collect()
}

const EMPTY_ROW: (f64, u32) = (0.0, value_flags::EMPTY);

#[test]
fn natural_points_follow_the_worked_example() {
    let h = host();
    let (qt, window, r) = run(&h, &format!("after={T0}&before={}", T0 + 6));
    assert_eq!(
        (window.after, window.before, window.points),
        (T0, T0 + 6, 7)
    );
    let thirty = unpack(pack(30.0, 0));
    assert_eq!(
        rows(&r),
        vec![
            EMPTY_ROW,
            (10.0, 0),
            EMPTY_ROW,
            (20.0, 0),
            (thirty, 0),
            EMPTY_ROW,
            (40.0, 0)
        ]
    );
    assert_eq!(r.t, (T0..=T0 + 6).collect::<Vec<_>>());
    assert_eq!((r.view.min, r.view.max), (0.0, 40.0));
    assert_eq!(r.view.flags, result_flags::ABSOLUTE);
    let selected = metric_status::SELECTED;
    assert_eq!(
        r.od[0],
        selected | metric_status::NONZERO | metric_status::QUERIED
    );
    assert_eq!((qt.db.tiers[0].queries, qt.db.tiers[0].points), (1, 7));
    assert_eq!(
        (r.queries_count, r.result_points_generated, r.db_points_read),
        (1, 7, 7)
    );
    assert_eq!(
        qt.query[0].query_points,
        StoragePoint {
            min: 10.0,
            max: 40.0,
            sum: 10.0 + 20.0 + thirty + 40.0,
            start_time_s: T0,
            end_time_s: T0 + 6,
            count: 4,
            anomaly_count: 0,
            // The first merged point is copied whole; later merges only add RESET.
            flags: SN_FLAG_NOT_ANOMALOUS,
        }
    );
    assert_eq!(
        qt.query[0].status,
        selected | metric_status::QUERIED,
        "v1 execution never copies NONZERO back to the metric"
    );
    assert_eq!(qt.dimensions[0].status & status::QUERIED, status::QUERIED);
    assert_eq!(
        (qt.instances[0].metrics.queried, qt.nodes[0].metrics.queried),
        (1, 1)
    );
}

#[test]
fn virtual_points_group_the_samples() {
    let h = host();
    let thirty = unpack(pack(30.0, 0));
    let (qt, window, r) = run(&h, &format!("after={T0}&before={}&points=3", T0 + 6));
    assert_eq!((window.after, window.points, window.group), (T0 + 1, 3, 2));
    assert_eq!(
        rows(&r),
        vec![(10.0, 0), ((20.0 + thirty) / 2.0, 0), (40.0, 0)]
    );
    assert_eq!(qt.db.tiers[0].points, 6);

    let (qt, window, r) = run(&h, &format!("after={T0}&before={}&points=2", T0 + 6));
    assert_eq!(
        (window.after, window.before, window.group),
        (T0 + 2, T0 + 7, 3)
    );
    assert_eq!(rows(&r), vec![((20.0 + thirty) / 2.0, 0), (40.0, 0)]);
    assert_eq!(qt.db.tiers[0].points, 6);
}

#[test]
fn latest_serves_the_last_stored_value() {
    let h = host();
    let dim = h.charts().find("t.a").unwrap().dim("d").unwrap();
    dim.update_collection(|c| c.last_stored_value = -41.5);
    let q = format!("after={T0}&before={}&points=1&group=latest", T0 + 6);
    let (qt, _, r) = run(&h, &q);
    assert_eq!(rows(&r), vec![(-41.5, 0)]);
    assert_eq!(
        r.od[0],
        metric_status::SELECTED | metric_status::NONZERO | metric_status::QUERIED
    );
    assert_eq!((r.view.min, r.view.max), (-41.5, -41.5));
    assert_eq!(
        (
            qt.db.tiers[0].queries,
            qt.db.tiers[0].points,
            r.db_points_read
        ),
        (0, 0, 0)
    );
    assert_eq!(
        qt.query[0].query_points,
        StoragePoint {
            min: -41.5,
            max: -41.5,
            sum: -41.5,
            start_time_s: T0 + 5,
            end_time_s: T0 + 6,
            count: 1,
            anomaly_count: 0,
            flags: 0,
        }
    );
    let (_, _, r) = run(&h, &format!("{q}&options=abs"));
    assert_eq!(rows(&r), vec![(41.5, 0)]);

    // Without a finite cached value, and with anomaly-bit, the storage path answers.
    let (qt, _, r) = run(&h, &format!("{q}&options=anomaly-bit"));
    assert_eq!(qt.db.tiers[0].queries, 1);
    assert_eq!(rows(&r), vec![(0.0, 0)], "no stored sample is anomalous");
    dim.update_collection(|c| c.last_stored_value = f64::NAN);
    let (qt, _, r) = run(&h, &q);
    assert_eq!(qt.db.tiers[0].queries, 1);
    assert_eq!(rows(&r), vec![(40.0, 0)]);
}

#[test]
fn a_metric_ending_before_the_window_fails() {
    let h = host();
    // `e` ends two seconds before `d`: admission tolerates two update intervals, the plan does not.
    let chart = h.charts().find("t.a").unwrap();
    let (e, _) = chart.dim_add("e", None, 1, 1, Algorithm::Absolute);
    for t in T0 + 1..=T0 + 4 {
        e.store_metric(t as u64 * 1_000_000, 1.0, SN_FLAG_NOT_ANOMALOUS);
    }
    h.contexts().process_queued();
    let (qt, window, r) = run(&h, &format!("after={}&before={}", T0 + 6, T0 + 6));
    assert_eq!((window.after, window.points), (T0 + 6, 1));
    assert_eq!(r.dn, ["d", "e"]);
    assert_eq!(rows(&r), vec![(40.0, 0)]);
    assert_eq!(
        (r.od[1], qt.query[1].status),
        (
            metric_status::SELECTED,
            metric_status::SELECTED | metric_status::FAILED
        )
    );
    assert_eq!(qt.dimensions[1].status & status::FAILED, status::FAILED);
    assert_eq!(
        (
            qt.instances[0].metrics.failed,
            qt.contexts[0].metrics.failed
        ),
        (1, 1)
    );
    assert_eq!(
        (
            qt.instances[0].metrics.queried,
            qt.db.tiers[0].queries,
            r.queries_count
        ),
        (1, 1, 1)
    );
}

#[test]
fn nonzero_is_dropped_when_no_metric_is_nonzero() {
    let h = host();
    let (_, window, _) = run(&h, &format!("after={T0}&before={}&options=nonzero", T0 + 6));
    assert_eq!(window.options & options::NONZERO, 0);
}

#[test]
fn a_negative_timeout_cancels_after_the_first_metric() {
    let h = host();
    let (_, _, r) = run(&h, &format!("after={T0}&before={}&timeout=-1", T0 + 6));
    assert_eq!(r.view.flags & result_flags::CANCEL, result_flags::CANCEL);
}

#[test]
fn v2_groups_the_metric_and_averages_its_rows() {
    let h = host();
    let (mut qt, mut window) = v2_target(
        &h,
        &format!("scope_contexts=ctx.a&after={T0}&before={}&points=6", T0 + 6),
    );
    let control = Control {
        received: Instant::now(),
        interrupted: &|_| false,
        windows: Windows::default(),
        pulse: None,
    };
    let r = run_v2(&mut qt, &mut window, &control).unwrap();
    let thirty = unpack(pack(30.0, 0));
    assert_eq!(
        (r.columns, r.di.as_slice(), r.dgbc.as_slice()),
        (1, &["d".to_string()][..], &[1][..])
    );
    let shown: Vec<Option<f64>> = (0..r.rows)
        .map(|i| (r.o[i] & value_flags::EMPTY == 0).then_some(r.v[i]))
        .collect();
    // The window ends within two update intervals of now: the empty row at T0+5 trims the live edge; the view
    // statistics still cover every row (C's quirk).
    assert_eq!(shown, [Some(10.0), None, Some(20.0), Some(thirty)]);
    assert_eq!(
        (r.n, r.trimming.expected_after, r.trimming.trimmed_after),
        (6, T0 + 4, T0 + 5)
    );
    assert_eq!(r.gbc, [1, 0, 1, 1, 0, 1]);
    assert_eq!(
        r.od[0] & (metric_status::QUERIED | metric_status::GROUPED | metric_status::NONZERO),
        metric_status::QUERIED | metric_status::GROUPED | metric_status::NONZERO
    );
    assert_eq!(
        (r.dview[0].count, r.dview[0].min, r.dview[0].max),
        (4, 10.0, 40.0)
    );
    assert_eq!(qt.query_points.count, 4);
    assert_eq!(
        (
            qt.instances[0].query_points.count,
            qt.nodes[0].query_points.count
        ),
        (4, 4)
    );
    assert_eq!(qt.contexts[0].instances.queried, 1);
}

/// What a three-tier query planned for its metric: the window, the plans, the queries and the weights per tier.
struct Planned {
    window: (i64, i64),
    plans: Vec<PlanEntry>,
    queries: Vec<usize>,
    weights: Vec<i64>,
}

fn planned(h: &Arc<Host>, query: &str, now: i64) -> (QueryTarget, Rrdr, Planned) {
    let (mut qt, mut window) = crate::testing::v1_tiers_target(h, query, now);
    let control = Control {
        received: Instant::now(),
        interrupted: &|_| false,
        windows: Windows::default(),
        pulse: None,
    };
    let bounds = (window.after, window.before);
    let r = run_v1(&mut qt, &mut window, &control);
    let planned = Planned {
        window: bounds,
        plans: qt.query[0].plan.clone(),
        queries: qt.db.tiers[..3].iter().map(|t| t.queries).collect(),
        weights: qt.query[0].tiers[..3].iter().map(|t| t.weight).collect(),
    };
    (qt, r, planned)
}

fn entry(tier: usize, after: i64, before: i64) -> PlanEntry {
    PlanEntry {
        tier,
        after: T0 + after,
        before: T0 + before,
    }
}

/// The retentions of C's planner vectors (QP:1061-1074) shifted to `T0`, through the whole query path: `tier=1`
/// serves the window alone; without it the best tier (the coarsest giving half the points, at least ten) starts,
/// coarser tiers fill a head and finer ones a tail; a selected tier the metric lacks plans automatically.
#[test]
fn a_selected_tier_plans_on_that_tier() {
    use crate::testing::dbengine_host;
    let (_dirs, h) = dbengine_host([
        (T0 + 180, T0 + 260),
        (T0 + 100, T0 + 200),
        (T0 + 50, T0 + 150),
    ]);
    let now = T0 + 1000;
    let window = format!("after={}&before={}&points=1", T0 + 50, T0 + 250);
    let (_, _, got) = planned(&h, &format!("{window}&tier=1"), now);
    assert_eq!(got.window, (T0 + 113, T0 + 313), "the window's alignment");
    assert_eq!(
        (got.plans, got.queries, got.weights),
        (vec![entry(1, 113, 200)], vec![0, 1, 0], vec![0, 0, 0])
    );
    // the best tier is tier 1, whose data starts before the window: no head
    let (_, _, got) = planned(&h, &window, now);
    assert_eq!(
        (got.plans, got.queries, got.weights),
        (
            vec![entry(1, 113, 200), entry(0, 200, 260)],
            vec![1, 1, 0],
            vec![20_000_000, 6_666_666, 3_333_333]
        )
    );
    let (_, _, got) = planned(&h, &format!("{window}&options=unaligned"), now);
    assert_eq!(got.window, (T0 + 50, T0 + 250));
    assert_eq!(
        (got.plans, got.queries),
        (
            vec![entry(2, 50, 100), entry(1, 100, 200), entry(0, 200, 250)],
            vec![1, 1, 1]
        )
    );
    let (_dirs, h) = dbengine_host([(T0 + 180, T0 + 260), (T0 + 100, T0 + 200), (0, 0)]);
    let (_, _, got) = planned(&h, &format!("{window}&tier=2"), now);
    assert_eq!(
        (got.plans, got.queries, got.weights),
        (
            vec![entry(1, 113, 200), entry(0, 200, 260)],
            vec![1, 1, 0],
            vec![20_000_000, 6_666_666, -i64::MAX]
        )
    );
}

/// With twenty points only tier 0 gives half of them: it serves where its data is and the coarser tiers fill the
/// head in turn (QP:392-425). A tier the agent does not run selects nothing (`request.rs` `apply_tier()`);
/// `selected-tier` alone selects tier 0 (QP:363-367).
#[test]
fn coarser_tiers_fill_the_head() {
    use crate::testing::dbengine_host;
    let (_dirs, h) = dbengine_host([
        (T0 + 180, T0 + 260),
        (T0 + 100, T0 + 200),
        (T0 + 50, T0 + 150),
    ]);
    let now = T0 + 1000;
    let window = format!("after={}&before={}", T0 + 50, T0 + 250);
    let (_, _, got) = planned(&h, &format!("{window}&points=20"), now);
    assert_eq!(got.window, (T0 + 51, T0 + 250));
    assert_eq!(
        (got.plans, got.queries, got.weights),
        (
            vec![entry(2, 51, 100), entry(1, 100, 180), entry(0, 180, 250)],
            vec![1, 1, 1],
            vec![19_900_000, 6_633_333, 3_316_666]
        )
    );
    let (_, _, got) = planned(&h, &format!("{window}&points=1&tier=5"), now);
    assert_eq!(got.plans, [entry(1, 113, 200), entry(0, 200, 260)]);
    let (_, _, got) = planned(&h, &format!("{window}&points=1&options=selected-tier"), now);
    assert_eq!(
        (got.plans, got.weights),
        (vec![entry(0, 180, 260)], vec![0, 0, 0])
    );
}

/// Two tiers answering from the same instant (D74.2): the reference's sort puts tier 1's instant plan after tier 0's.
/// An average moves past it at the first row (QP:319-354), so tier 1 is counted but never read. A sum's offset delays
/// the row-level switch, so the first point switches to the instant plan (QE:278-280), which releases tier 0's: its
/// short query serves the first row and the rest are EMPTY (to be confirmed on the reference, S4b commit 3).
#[test]
fn a_tie_counts_the_instant_plan() {
    use crate::testing::dbengine_pages_host;
    let a = T0 + 3600;
    let (_pages, h) = dbengine_pages_host(
        [(T0 + 10, a + 5000), (T0 + 30, a), (0, 0)],
        |_, _| 7.0,
        Algorithm::Absolute,
    );
    let query = format!("after={a}&before={}&points=10&options=unaligned", a + 3599);
    let (qt, r, got) = planned(&h, &query, a + 5000);
    assert_eq!(got.window, (a, a + 3599));
    assert_eq!(
        (got.plans, got.queries, got.weights),
        (
            vec![entry(0, 3600, 7199), entry(1, 3600, 3600)],
            vec![1, 1, 0],
            vec![359_900_000, 119_966_666, -i64::MAX]
        )
    );
    assert_eq!(qt.db.tiers[1].points, 0);
    assert_eq!(rows(&r), [(7.0, 0); 10]);

    let (qt, r, _) = planned(&h, &format!("{query}&group=sum"), a + 5000);
    let points: Vec<usize> = qt.db.tiers[..3].iter().map(|t| t.points).collect();
    assert_eq!(points, [1, 6, 0]);
    // tier 1's last point (A-30, A] shares one second with the first row
    let mut want = vec![EMPTY_ROW; 10];
    want[0] = (21.0 / 30.0, 0);
    assert_eq!(rows(&r), want);
}

/// Every row of a constant series keeps its value across the seams, and the per-tier reads add up to the total.
fn assert_rows(r: &Rrdr, qt: &QueryTarget, want: impl Fn(usize) -> f64) {
    for i in 0..r.rows {
        assert!(
            (r.v[i] - want(i)).abs() < 1e-9 && r.o[i] == 0,
            "row {i} at {}: {} (flags {})",
            r.t[i] - T0,
            r.v[i],
            r.o[i]
        );
    }
    let per_tier: usize = qt.db.tiers.iter().map(|t| t.points).sum();
    assert_eq!(per_tier, r.db_points_read);
}

/// A fine tier whose first page starts inside a coarse point (the dbengine jumps to it, `query.rs` `next_metric()`):
/// the coarse point serves up to where the fine one starts, and a sum keeps only its share before it (QE:309-323),
/// scaled for stored values, cut by its duration for rates (QE:355-361); `absolute` makes the point read ahead
/// positive too (QE:295-296). No row loses or doubles a second.
#[test]
fn a_coarse_head_joins_a_finer_tier_without_loss() {
    use crate::testing::dbengine_pages_host;
    // 7 every 10 s on both tiers; tier 0 starts with (T0+905, T0+915], inside tier 1's (T0+900, T0+930]
    let points = [(T0 + 915, T0 + 2000), (T0 + 30, T0 + 2010), (0, 0)];
    let query = format!("after={}&before={}&points=70", T0 + 600, T0 + 1500);
    for (algorithm, value, group, per_second) in [
        (Algorithm::Absolute, 7.0, "average", None),
        (Algorithm::Absolute, 7.0, "sum", Some(0.7)),
        (Algorithm::Absolute, -7.0, "sum&options=absolute", Some(0.7)),
        (Algorithm::Incremental, 7.0, "sum", Some(7.0)),
    ] {
        let (_pages, h) = dbengine_pages_host(points, move |_, _| value, algorithm);
        let (qt, r, got) = planned(&h, &format!("{query}&group={group}"), T0 + 2000);
        assert_eq!(
            (got.window, got.plans),
            (
                (T0 + 602, T0 + 1511),
                vec![entry(1, 602, 915), entry(0, 915, 1511)]
            ),
            "{group}"
        );
        let vue = (r.t[1] - r.t[0]) as f64;
        assert_rows(&r, &qt, |_| per_second.map_or(7.0, |p| p * vue));
    }
}

/// A finer point read ahead that starts after the row keeps the coarse point serving only up to where it starts
/// (QE:457-463), and is then consumed as the first point since the switch, which the non-advancing guard does not
/// check against the coarse point it overlaps (QE:265-273, 387). Tier 1 holds 7; tier 0 holds its end time's offset,
/// from (T0+915, T0+925] inside tier 1's (T0+900, T0+930].
#[test]
fn a_point_read_ahead_bounds_the_coarse_one() {
    use crate::testing::dbengine_pages_host;
    let (_pages, h) = dbengine_pages_host(
        [(T0 + 925, T0 + 2000), (T0 + 30, T0 + 2010), (0, 0)],
        |tier, t| if tier == 0 { (t - T0) as f64 } else { 7.0 },
        Algorithm::Absolute,
    );
    let query = format!("after={}&before={}&points=70", T0 + 600, T0 + 1500);
    let (_, r, got) = planned(&h, &query, T0 + 2000);
    assert_eq!(
        (got.window, got.plans),
        (
            (T0 + 602, T0 + 1511),
            vec![entry(1, 602, 925), entry(0, 925, 1511)]
        )
    );
    let row = |t: i64| (0..r.rows).find(|&i| r.t[i] == T0 + t).map(|i| r.v[i]);
    // (T0+900, T0+913] from tier 1; (T0+913, T0+926] averages tier 0's (915, 925] and its next point interpolated at
    // T0+926 from it
    assert_eq!(row(913), Some(7.0));
    let seam = row(926).unwrap();
    assert!((seam - (925.0 + 926.0) / 2.0).abs() < 1e-9, "{seam}");
}

/// Three plans, each seam inside the coarser point (QE:309-323 twice): a sum loses no second across both.
#[test]
fn three_plans_join_without_loss() {
    use crate::testing::dbengine_pages_host;
    // tier 1 starts with (T0+915, T0+945] inside tier 2's (T0+900, T0+960]; tier 0 with (T0+1825, T0+1835] inside
    // tier 1's (T0+1815, T0+1845]
    let points = [
        (T0 + 1835, T0 + 3000),
        (T0 + 945, T0 + 3015),
        (T0 + 60, T0 + 3060),
    ];
    let (_pages, h) = dbengine_pages_host(points, |_, _| 7.0, Algorithm::Absolute);
    let query = format!("after={}&before={}&points=150", T0 + 600, T0 + 2500);
    for (group, per_second) in [("average", None), ("sum", Some(0.7))] {
        let (qt, r, got) = planned(&h, &format!("{query}&group={group}"), T0 + 3000);
        assert_eq!(
            (
                got.plans.iter().map(|e| e.tier).collect::<Vec<_>>(),
                got.queries.clone()
            ),
            (vec![2, 1, 0], vec![1, 1, 1]),
            "{group}"
        );
        assert!(qt.db.tiers[..3].iter().all(|t| t.points > 0), "{group}");
        let vue = (r.t[1] - r.t[0]) as f64;
        assert_rows(&r, &qt, |_| per_second.map_or(7.0, |p| p * vue));
    }
}

/// A coarse tier whose last window is still open hands the tail to tier 0: an average at the row that reaches its
/// end (QE:208-212), a sum, whose switch waits for the grouping's offset, at the point crossing it, which the finer
/// point read ahead replaces (QE:302-308).
#[test]
fn a_finer_tail_follows_a_coarse_tier() {
    use crate::testing::ram_tiers_host;
    let query = format!("after={}&before={}&points=60", T0 + 700, T0 + 1002);
    for (value, options) in [(7.0, ""), (-7.0, "&options=absolute")] {
        let (_dirs, h) = ram_tiers_host(100, [5, 3], T0 + 1..=T0 + 1002, move |_| value);
        let (qt, r, got) = planned(&h, &format!("{query}{options}"), T0 + 1002);
        assert_eq!(
            (got.window, got.plans),
            (
                (T0 + 701, T0 + 1005),
                vec![entry(1, 701, 1000), entry(0, 1000, 1002)]
            )
        );
        assert!(qt.db.tiers[0].points > 0 && qt.db.tiers[1].points > 0);
        assert_rows(&r, &qt, |_| 7.0);
    }
    // a sum's last row has the samples up to the tail's end
    let (_dirs, h) = ram_tiers_host(100, [5, 3], T0 + 1..=T0 + 1002, |_| 7.0);
    let (qt, r, _) = planned(&h, &format!("{query}&group=sum"), T0 + 1002);
    let last = r.rows - 1;
    assert_rows(&r, &qt, |i| if i == last { 14.0 } else { 35.0 });
}

/// `jsonwrap_query_metric_plan()` (JQP:6-29), with the short keys: every plan, and each tier in use with its weight,
/// `-LONG_MAX` for a tier that does not overlap.
#[test]
fn the_plans_and_weights_print_as_cs() {
    use crate::testing::dbengine_pages_host;
    use netdata_agent_text::json::{JsonOptions, JsonWriter};
    let a = T0 + 3600;
    let (_pages, h) = dbengine_pages_host(
        [(T0 + 10, a + 5000), (T0 + 30, a), (0, 0)],
        |_, _| 7.0,
        Algorithm::Absolute,
    );
    let query = format!("after={a}&before={}&points=10&options=unaligned", a + 3599);
    let (qt, _, _) = planned(&h, &query, a + 5000);
    let mut w = JsonWriter::new(JsonOptions::MINIFY);
    crate::jsonwrap::query_metric_plan(&mut w, &qt.query[0], 3, 0);
    w.finalize();
    let (t0, a, b) = (T0 + 10, T0 + 3600, T0 + 7199);
    assert_eq!(
        String::from_utf8(w.into_bytes()).unwrap(),
        format!(
            "{{\"plans\":[{{\"tr\":0,\"af\":{a},\"bf\":{b}}},{{\"tr\":1,\"af\":{a},\"bf\":{a}}}],\
             \"tiers\":[{{\"tr\":0,\"fe\":{t0},\"le\":{},\"wg\":359900000}},\
             {{\"tr\":1,\"fe\":{},\"le\":{a},\"wg\":119966666}},\
             {{\"tr\":2,\"fe\":0,\"le\":0,\"wg\":-9223372036854775807}}]}}",
            a + 5000,
            T0 + 30
        )
    );
}

/// `query_plan_unittest_expect_result_expiry()` (QP:782-835): the row-level switch waits for the grouping's offset
/// past the expiry, and never comes when that sum overflows.
#[test]
fn the_result_expiry_waits_for_the_offset() {
    let h = host();
    let (qt, window) = v1_target(&h, &format!("after={T0}&before={}", T0 + 6));
    let mut ops = Ops::new(
        &qt,
        &window,
        [TierView::default(); RRD_STORAGE_TIERS],
        Vec::new(),
        Vec::new(),
    );
    ops.plan_switch_time_offset = 60;
    let state = |ops: &Ops| {
        (
            ops.current_plan_expire_time,
            ops.result_plan_expire_time,
            ops.result_plan_expire_time_overflow,
        )
    };
    ops.set_expire_time(100);
    assert_eq!(state(&ops), (100, 160, false));
    assert_eq!(
        [159, 160, 161].map(|t| ops.result_should_switch(t)),
        [false, true, true]
    );
    ops.set_expire_time(i64::MAX - 30);
    assert_eq!(state(&ops), (i64::MAX - 30, i64::MAX, true));
    assert!(!ops.result_should_switch(i64::MAX));
    ops.set_expire_time(i64::MAX - 60);
    assert_eq!(state(&ops), (i64::MAX - 60, i64::MAX, false));
    assert!(ops.result_should_switch(i64::MAX));
}

/// Admission over the tiers (QT:258-384): a metric only tier 2 holds in the window is admitted with the common
/// retention; every tier's statistics count it; each tier's update every is its grouping of the chart's.
#[test]
fn admission_takes_the_common_retention_of_the_tiers() {
    use crate::testing::{dbengine_host, v1_tiers_target};
    let (_dirs, h) = dbengine_host([(0, 0), (0, 0), (T0 + 50, T0 + 150)]);
    let (qt, _) = v1_tiers_target(
        &h,
        &format!("after={}&before={}&points=1", T0 + 100, T0 + 120),
        T0 + 1000,
    );
    assert_eq!(qt.query.len(), 1);
    let tiers: Vec<(bool, i64, i64, i64)> = qt.query[0].tiers[..3]
        .iter()
        .map(|t| {
            (
                t.handle.is_some(),
                t.first_time_s,
                t.last_time_s,
                t.update_every_s,
            )
        })
        .collect();
    assert_eq!(
        tiers,
        [
            (false, 0, 0, 0),
            (false, 0, 0, 0),
            (true, T0 + 50, T0 + 150, 60)
        ]
    );
    assert_eq!(
        (
            qt.db.first_time_s,
            qt.db.last_time_s,
            qt.db.tiers[2].update_every
        ),
        (T0 + 50, T0 + 150, 60)
    );
    // outside every tier's retention: not admitted, still counted in the tiers' statistics
    let (qt, _) = v1_tiers_target(
        &h,
        &format!("after={}&before={}&points=1", T0 + 400, T0 + 500),
        T0 + 1000,
    );
    assert!(qt.query.is_empty());
    assert_eq!(
        (qt.db.tiers[2].first_time_s, qt.db.tiers[2].last_time_s),
        (T0 + 50, T0 + 150)
    );
}

/// Natural points on a selected tier above 0 step by that tier's update every (query-window.c:153-159).
#[test]
fn natural_points_on_a_selected_tier_use_its_update_every() {
    use crate::testing::{dbengine_host, v1_tiers_target};
    let (_dirs, h) = dbengine_host([(T0 + 100, T0 + 900), (T0 + 100, T0 + 900), (0, 0)]);
    let (_, window) = v1_tiers_target(&h, "after=-300&before=0&tier=1", T0 + 900);
    assert_eq!(window.group, 1);
    assert_eq!(window.points, 10, "300 s of 30 s points");
    let (_, window) = v1_tiers_target(&h, "after=-300&before=0", T0 + 900);
    assert_eq!(
        window.points, 29,
        "tier 0's 10 s points, as C's window counts them"
    );
}

/// `pulse_queries_rrdr_query_completed()` per queried metric: a counted v1 and v2 query each add one query per
/// metric and the points its execution read and generated, to its source.
#[test]
fn counted_queries_add_their_points_to_their_source() {
    let h = host();
    let queries = Queries::default();
    let control = Control {
        received: Instant::now(),
        interrupted: &|_| false,
        windows: Windows::default(),
        pulse: Some((&queries, QuerySource::ApiData)),
    };
    let (mut qt, mut window) = v1_target(&h, &format!("after={T0}&before={}", T0 + 6));
    let r = run_v1(&mut qt, &mut window, &control);
    let v1 = queries.source(QuerySource::ApiData);
    assert_eq!(
        (v1.queries, v1.points_read, v1.points_generated),
        (
            qt.query.len() as u64,
            r.db_points_read as u64,
            r.result_points_generated as u64
        )
    );
    assert!(v1.points_read > 0);
    let (mut qt, mut window) = v2_target(
        &h,
        &format!("scope_contexts=ctx.a&after={T0}&before={}&points=6", T0 + 6),
    );
    run_v2(&mut qt, &mut window, &control).unwrap();
    let both = queries.source(QuerySource::ApiData);
    assert_eq!(both.queries, v1.queries + qt.query.len() as u64);
    assert!(both.points_read > v1.points_read && both.points_generated > v1.points_generated);
    assert_eq!(queries.source(QuerySource::Health).queries, 0);
}
