//! `stream_control` (`src/streaming/stream-control.c`): what the heavy work counts, so that the background threads
//! yield to it. The tier backfills and the users' data queries are counted; replication and the weights queries are
//! not ported yet, so they count none.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// `backfill_runners`: the tier backfills running now, which streaming reports and waits on.
static BACKFILL_RUNNERS: AtomicUsize = AtomicUsize::new(0);

/// `user_data_query_runners`: the data queries executing now.
static USER_DATA_QUERY_RUNNERS: AtomicUsize = AtomicUsize::new(0);

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

/// `stream_control_health_should_be_running()`: no backfill, and at most one user query.
pub fn health_should_be_running() -> bool {
    backfill_runners() == 0 && USER_DATA_QUERY_RUNNERS.load(Ordering::Acquire) <= 1
}

/// `STREAM_CONTROL_SLEEP_UT`: what `stream_control_throttle()` sleeps, 10 ms plus up to 10 ms more.
pub fn throttle_wait() -> Duration {
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_nanos()))
        % 10_000;
    Duration::from_micros(10_000 + jitter)
}
