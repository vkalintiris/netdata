//! `stream_control` (`src/streaming/stream-control.c`): what the heavy work counts, so that the background threads
//! yield to it. The tier backfills, the users' data queries and the users' weights queries are counted; the
//! replication queries count only for ML, which is not ported (D105.12).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// `backfill_runners`: the tier backfills running now, which streaming reports and waits on.
static BACKFILL_RUNNERS: AtomicUsize = AtomicUsize::new(0);

/// `user_data_query_runners`: the data queries executing now.
static USER_DATA_QUERY_RUNNERS: AtomicUsize = AtomicUsize::new(0);

/// `user_weights_query_runners`: the queries a weights request executes now.
static USER_WEIGHTS_QUERY_RUNNERS: AtomicUsize = AtomicUsize::new(0);

/// How many tier backfills run now.
pub fn backfill_runners() -> usize {
    BACKFILL_RUNNERS.load(Ordering::Acquire)
}

/// `stream_control_backfill_query_started()` and `_finished()`: a backfill counts while its guard lives.
pub(crate) struct BackfillRunning;

impl BackfillRunning {
    pub(crate) fn start() -> Self {
        BACKFILL_RUNNERS.fetch_add(1, Ordering::AcqRel);
        BackfillRunning
    }
}

impl Drop for BackfillRunning {
    fn drop(&mut self) {
        BACKFILL_RUNNERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// `stream_control_user_data_query_started()` and `_finished()`: a data query counts while its guard lives.
pub struct UserDataQuery;

impl UserDataQuery {
    pub fn start() -> Self {
        USER_DATA_QUERY_RUNNERS.fetch_add(1, Ordering::AcqRel);
        UserDataQuery
    }
}

impl Drop for UserDataQuery {
    fn drop(&mut self) {
        USER_DATA_QUERY_RUNNERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// `stream_control_user_weights_query_started()` and `_finished()`: a query of a weights request counts while its
/// guard lives.
pub struct UserWeightsQuery;

impl UserWeightsQuery {
    pub fn start() -> Self {
        USER_WEIGHTS_QUERY_RUNNERS.fetch_add(1, Ordering::AcqRel);
        UserWeightsQuery
    }
}

impl Drop for UserWeightsQuery {
    fn drop(&mut self) {
        USER_WEIGHTS_QUERY_RUNNERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The users' queries executing now, data and weights together.
fn user_query_runners() -> usize {
    USER_DATA_QUERY_RUNNERS.load(Ordering::Acquire) + USER_WEIGHTS_QUERY_RUNNERS.load(Ordering::Acquire)
}

/// `stream_control_children_should_be_accepted()`: no backfill. Replication is not counted: it gains from merging the
/// children's extents, and counting it would lock out the last children while all the others replicate.
pub fn children_should_be_accepted() -> bool {
    backfill_runners() == 0
}

/// `stream_control_health_should_be_running()`: no backfill, and at most one user query, of data or weights.
pub fn health_should_be_running() -> bool {
    backfill_runners() == 0 && user_query_runners() <= 1
}

/// `stream_control_replication_should_be_running()`: no backfill and no user query, of data or weights.
pub fn replication_should_be_running() -> bool {
    backfill_runners() == 0 && user_query_runners() == 0
}

/// `STREAM_CONTROL_SLEEP_UT`: what `stream_control_throttle()` sleeps, 10 ms plus up to 10 ms more.
pub fn throttle_wait() -> Duration {
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_nanos()))
        % 10_000;
    Duration::from_micros(10_000 + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A running backfill holds the waiting list's children back (the counter is shared with every test of the
    /// crate, so only the refusal is asserted).
    #[test]
    fn a_running_backfill_holds_children_back() {
        let running = BackfillRunning::start();
        assert!(!children_should_be_accepted());
        drop(running);
    }

    /// Replication runs with no backfill and no user query; health with no backfill and at most one user query, a
    /// weights query counting as a data query does (the counters are shared too: only the refusals are asserted).
    #[test]
    fn backfills_and_user_queries_hold_replication_and_health_back() {
        let one = UserDataQuery::start();
        assert!(!replication_should_be_running());
        let two = UserDataQuery::start();
        assert!(!health_should_be_running());
        drop((one, two));
        let weights = UserWeightsQuery::start();
        assert!(!replication_should_be_running());
        let data = UserDataQuery::start();
        assert!(!health_should_be_running());
        drop((weights, data));
        let running = BackfillRunning::start();
        assert!(!replication_should_be_running() && !health_should_be_running());
        drop(running);
    }

    /// `STREAM_CONTROL_SLEEP_UT`: 10 ms plus up to 10 ms more.
    #[test]
    fn the_throttle_waits_10_to_20_ms() {
        for _ in 0..1000 {
            let wait = throttle_wait();
            assert!((10_000..20_000).contains(&wait.as_micros()), "{wait:?}");
        }
    }
}
