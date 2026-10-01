//! `stream-control.c`'s gates over the user data query counter. Its own test binary: the counters are process-wide
//! and the crate's own tests run backfills and user queries concurrently.

use netdata_agent_rrd::stream_control::{
    UserDataQuery, backfill_runners, children_should_be_accepted, health_should_be_running,
    replication_should_be_running,
};

/// (children accepted, replication runs, health runs).
fn gates() -> (bool, bool, bool) {
    (children_should_be_accepted(), replication_should_be_running(), health_should_be_running())
}

/// Children are accepted whatever the user queries; replication runs with none; health with at most one.
#[test]
fn the_user_data_query_counter_holds_replication_at_one_and_health_at_two() {
    assert_eq!((backfill_runners(), gates()), (0, (true, true, true)));
    let one = UserDataQuery::start();
    assert_eq!(gates(), (true, false, true));
    let two = UserDataQuery::start();
    assert_eq!(gates(), (true, false, false));
    drop(two);
    assert_eq!(gates(), (true, false, true));
    drop(one);
    assert_eq!(gates(), (true, true, true));
}
