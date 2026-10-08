//! `stream-control.c`'s gates over the two user query counters. Its own test binary with one test: the counters are
//! process-wide and the crate's own tests run backfills and user queries concurrently.

use netdata_agent_rrd::stream_control::{
    UserDataQuery, UserWeightsQuery, backfill_runners, children_should_be_accepted, health_should_be_running,
    replication_should_be_running,
};

/// (children accepted, replication runs, health runs).
fn gates() -> (bool, bool, bool) {
    (children_should_be_accepted(), replication_should_be_running(), health_should_be_running())
}

/// Children are accepted whatever the user queries; replication runs with none; health with at most one. A weights
/// query counts as a data query does, and the two counters are summed (`stream-control.c:91-103`).
#[test]
fn the_user_query_counters_hold_replication_at_one_and_health_at_two() {
    assert_eq!((backfill_runners(), gates()), (0, (true, true, true)));
    let one = UserDataQuery::start();
    assert_eq!(gates(), (true, false, true));
    let two = UserDataQuery::start();
    assert_eq!(gates(), (true, false, false));
    drop(two);
    assert_eq!(gates(), (true, false, true));
    drop(one);
    assert_eq!(gates(), (true, true, true));

    // one weights query holds replication, two hold health
    let one = UserWeightsQuery::start();
    assert_eq!(gates(), (true, false, true));
    let two = UserWeightsQuery::start();
    assert_eq!(gates(), (true, false, false));
    drop(two);
    assert_eq!(gates(), (true, false, true));
    // one of each is two user queries
    let data = UserDataQuery::start();
    assert_eq!(gates(), (true, false, false));
    drop(data);
    assert_eq!(gates(), (true, false, true));
    drop(one);
    assert_eq!(gates(), (true, true, true));
}
