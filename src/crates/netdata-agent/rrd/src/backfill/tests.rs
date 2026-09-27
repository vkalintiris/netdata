use super::*;
use crate::chart::Algorithm;
use crate::mode::DbMode;
use crate::storage::Backfill;
use crate::testutil::{BackfillFixture, backfill_fixture, chart_spec, store, tier_records};

/// A multiple of 15.
const B: i64 = 1_790_179_995;

fn fixed_clock() -> i64 {
    B + 100
}

/// A queue answering `(successful, failed)` into `answers`.
fn answering(answers: &Arc<Mutex<Vec<(usize, usize)>>>) -> Callback {
    let answers = Arc::clone(answers);
    Box::new(move |ok, failed| answers.lock().unwrap().push((ok, failed)))
}

/// Runs the queue until it is empty.
fn drain(q: &BackfillQueue) {
    q.worker(false, &|| q.queued() > 0);
}

/// Stores `B..=last` and restarts the dimension: its tier 1 has windows up to B + 15.
fn restarted(f: &BackfillFixture, last: i64) {
    for t in B..=last {
        store(&f.dim, t, (t - B) as f64);
    }
    f.dim.restarted();
}

/// `backfill_request_add()` queues nothing when the pool is not running, when the chart has no dimensions, and when
/// every dimension is backfilled; otherwise one job per dimension, in order.
#[test]
fn requests_queue_the_dimensions_not_backfilled() {
    let f = backfill_fixture(Backfill::New, DbMode::Dbengine);
    let q = BackfillQueue::new(fixed_clock);
    let answers = Arc::new(Mutex::new(Vec::new()));
    let weak = Arc::downgrade(&f.slot);
    assert!(
        q.request_add(&f.host, weak.clone(), &f.chart, answering(&answers))
            .is_err()
    );
    q.start();
    let (empty, _) = f.host.charts().create(&crate::chart::ChartSpec {
        id: "empty",
        ..chart_spec(DbMode::Dbengine)
    });
    assert!(
        q.request_add(&f.host, weak.clone(), &empty, answering(&answers))
            .is_err()
    );
    let (second, _) = f.chart.dim_add("e", None, 1, 1, Algorithm::Absolute);
    assert!(
        q.request_add(&f.host, weak.clone(), &f.chart, answering(&answers))
            .is_ok()
    );
    assert_eq!(q.queued(), 2);
    drain(&q);
    // an empty tier with the mode `new` passes no check: both fail, the callback runs once
    assert_eq!(*answers.lock().unwrap(), [(0, 2)]);
    store(&second, B, 0.0);
    store(&f.dim, B, 0.0);
    assert!(
        q.request_add(&f.host, weak, &f.chart, answering(&answers))
            .is_err()
    );
}

/// After a restart the pool backfills up to the wall clock: a seam shorter than a tier window, which the first
/// store's backfill does not reach, is restored, and the point the child replays next is not counted twice.
#[test]
fn the_pool_backfills_up_to_the_wall_clock() {
    for pool in [true, false] {
        let f = backfill_fixture(Backfill::New, DbMode::Dbengine);
        restarted(&f, B + 18);
        if pool {
            let q = BackfillQueue::new(fixed_clock);
            q.start();
            let answers = Arc::new(Mutex::new(Vec::new()));
            assert!(
                q.request_add(
                    &f.host,
                    Arc::downgrade(&f.slot),
                    &f.chart,
                    answering(&answers)
                )
                .is_ok()
            );
            drain(&q);
            assert_eq!(*answers.lock().unwrap(), [(1, 0)]);
            assert!(f.dim.is_backfilled());
        }
        for t in B + 19..=B + 21 {
            store(&f.dim, t, (t - B) as f64);
        }
        f.dim.finalize_collection();
        let restored = if pool {
            (B + 20, 90.0, 5)
        } else {
            (B + 20, 39.0, 2)
        };
        assert_eq!(
            tier_records(&f.engine, &f.dim, 1),
            [
                (B + 5, 15.0, 6),
                (B + 10, 40.0, 5),
                (B + 15, 65.0, 5),
                restored
            ],
            "pool {pool}"
        );
    }
}

/// A job of a receiver that went, or was replaced, does nothing and fails; its dimension stays to backfill.
#[test]
fn jobs_of_a_gone_receiver_fail() {
    let f = backfill_fixture(Backfill::New, DbMode::Dbengine);
    restarted(&f, B + 18);
    let q = BackfillQueue::new(fixed_clock);
    q.start();
    let answers = Arc::new(Mutex::new(Vec::new()));
    let weak = Arc::downgrade(&f.slot);
    f.host.backfill_requested();
    assert!(
        q.request_add(&f.host, weak.clone(), &f.chart, answering(&answers))
            .is_ok()
    );
    f.host.clear_receiver(&f.slot);
    drain(&q);
    assert_eq!(*answers.lock().unwrap(), [(0, 1)]);
    assert!(!f.dim.is_backfilled());
    // the reset counted the pending answer out; the stale receiver's answer is refused
    assert_eq!(f.host.backfill_pending(), 0);
    assert!(!f.host.backfill_answered(&weak));
    assert_eq!(f.host.backfill_pending(), 0);
}

/// `finish()` fails what is queued, running the callbacks, and takes no more requests.
#[test]
fn finish_fails_the_queue() {
    let f = backfill_fixture(Backfill::New, DbMode::Dbengine);
    let q = BackfillQueue::new(fixed_clock);
    q.start();
    let answers = Arc::new(Mutex::new(Vec::new()));
    assert!(
        q.request_add(
            &f.host,
            Arc::downgrade(&f.slot),
            &f.chart,
            answering(&answers)
        )
        .is_ok()
    );
    q.finish();
    assert_eq!(
        (answers.lock().unwrap().clone(), q.queued()),
        (vec![(0, 1)], 0)
    );
    assert!(
        q.request_add(
            &f.host,
            Arc::downgrade(&f.slot),
            &f.chart,
            answering(&answers)
        )
        .is_err()
    );
}

/// `backfill_thread()`'s count: half the CPUs, from 2 to 16.
#[test]
fn thread_counts_are_cs() {
    let counts: Vec<usize> = [1, 4, 6, 32, 64]
        .iter()
        .map(|&c| BackfillQueue::threads(c))
        .collect();
    assert_eq!(counts, [2, 2, 3, 16, 16]);
}
