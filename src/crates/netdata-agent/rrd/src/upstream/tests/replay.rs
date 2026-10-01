//! The replication answer (`upstream::replay`): the window's rules, the walk over a ram chart, the collection state
//! and the finish, the not-found and partial answers, and the collection lock.

use std::time::Duration;

use super::*;
use crate::upstream::replay::{Answered, Request, Window, answer, normalize};

fn request(chart_id: &str, after: i64, before: i64, start_streaming: bool) -> Request {
    Request { chart_id: chart_id.to_string(), after, before, start_streaming }
}

/// `replication_response_prepare()`'s rules, the wall clock 1000 s after T and the retention [T, T + 800].
#[test]
fn a_window_is_normalized_as_c() {
    let wall = T + 1000;
    let db = (T, T + 800);
    let w = |after, before, streaming| Window { after, before, streaming };
    let cases = [
        ("inside", (T + 10, T + 500, false), w(T + 10, T + 500, false)),
        ("reversed", (T + 500, T + 10, false), w(T + 10, T + 500, false)),
        ("no after", (0, T + 500, false), w(0, 0, true)),
        ("no before", (T + 10, 0, false), w(0, 0, true)),
        ("future", (wall + 1, wall + 5, false), w(0, 0, true)),
        ("ends near now: to the retention's end", (T + 10, wall - 100, false), w(T + 10, T + 800, true)),
        ("reaches the retention's end", (T + 10, T + 850, false), w(T + 10, T + 800, true)),
        ("asks to stream", (T + 10, T + 500, true), w(T + 10, T + 800, true)),
        ("starts before the retention", (T - 100, T + 500, false), w(T, T + 500, false)),
        ("before the retention: clamped, swapped", (T - 200, T - 100, false), w(T - 100, T, false)),
    ];
    for (name, (after, before, streaming), want) in cases {
        let got = normalize(&request("c", after, before, streaming), wall, 1, db);
        assert_eq!(got, want, "{name}");
    }
}

/// A streaming localhost with `values[i]` collected at T + i on one absolute dimension of a ram chart, defined
/// upstream at its first collection (its replication claimed).
fn replicating_chart(values: &[i64]) -> (Host, Arc<Recorder>, Arc<Chart>) {
    let (host, recorder) = streaming("*", PLAIN | caps::REPLICATION);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    for (i, v) in values.iter().enumerate() {
        collect(&host, &chart, &[(&dim, *v)], T + i as i64);
    }
    recorder.take();
    (host, recorder, chart)
}

/// The answer's lines with each line's wall clock (the last field of a point's RBEGIN and of REND) as W.
fn lines(answer: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(answer)
        .lines()
        .map(|l| {
            if l.starts_with("RBEGIN '' ") || l.starts_with("REND ") {
                let (head, _) = l.rsplit_once(' ').unwrap();
                format!("{head} W")
            } else {
                l.to_string()
            }
        })
        .collect()
}

fn answered(host: &Host, req: &Request, max_msg_size: usize, same_session: bool) -> (Answered, Vec<String>) {
    let mut out = Vec::new();
    let mut committed = Vec::new();
    let a = answer(host, req, PLAIN | caps::REPLICATION, max_msg_size, &mut out, |b| {
        committed = b.to_vec();
        same_session
    });
    assert_eq!(committed, out, "the answer is committed whole");
    (a, lines(&committed))
}

fn claim(chart: &Chart) -> u32 {
    chart.flags() & (flags::SENDER_REPLICATION_FINISHED | flags::SENDER_REPLICATION_IN_PROGRESS)
}

/// A window inside the retention: one step per second after its start, each with its point, and a REND that does
/// not start streaming, so the replication stays claimed.
#[test]
fn an_answer_walks_the_window_step_by_step() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
    let queries = &chart.storage().pulse().queries;
    let before = queries.replication();
    let (a, got) = answered(&host, &request("t.c", T + 2, T + 5, false), 1 << 20, true);
    assert_eq!(a, Answered::Executed);
    let after = queries.replication();
    // one dimension query, its three points generated
    assert_eq!((after.queries - before.queries, after.points_generated - before.points_generated), (1, 3));
    assert!(after.points_read - before.points_read >= 3);
    let (first, last) = chart.retention_for_collected(now_realtime_s());
    let mut want = vec!["RBEGIN 't.c'".to_string()];
    for t in T + 3..=T + 5 {
        want.push(format!("RBEGIN '' {} {t} W", t - 1));
        want.push(format!("RSET \"d\" {} A", t - T + 10));
    }
    want.push(format!("REND 1 {first} {last} false {} {} W", T + 2, T + 5));
    assert_eq!(got, want);
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_IN_PROGRESS);
    assert_eq!(host.sender_replicating_charts(), 1);
}

/// An answer that starts streaming carries the collection state and, once in the request's session, ends the
/// replication: the claim given back (the host running) and no more resync.
#[test]
fn a_streaming_answer_carries_the_state_and_finishes() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15]);
    assert_ne!(chart.resync_time_s(), 0);
    let (_, got) = answered(&host, &request("t.c", T + 3, T + 5, true), 1 << 20, true);
    let (first, last) = chart.retention_for_collected(now_realtime_s());
    let dim = chart.dim("d").unwrap().collection();
    let c = chart.collection();
    assert_eq!(
        got,
        vec![
            "RBEGIN 't.c'".to_string(),
            format!("RBEGIN '' {} {} W", T + 3, T + 4),
            "RSET \"d\" 14 A".to_string(),
            format!("RBEGIN '' {} {} W", T + 4, T + 5),
            "RSET \"d\" 15 A".to_string(),
            format!(
                "RDSTATE 'd' {} 15 15 15",
                dim.last_collected_time.0 * 1_000_000 + dim.last_collected_time.1
            ),
            format!(
                "RSSTATE {} {}",
                c.last_collected.0 * 1_000_000 + c.last_collected.1,
                c.last_updated.0 * 1_000_000 + c.last_updated.1
            ),
            format!("REND 1 {first} {last} true  {} {} W", T + 3, T + 5),
        ]
    );
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_FINISHED);
    assert_eq!(host.sender_replicating_charts(), 0);
    assert_eq!(host.pulse_state() & host_status::SENDER, host_status::SND_RUNNING);
    assert_eq!(chart.resync_time_s(), 0);
}

/// An answer that does not count (its buffer flushed since the request, D105.6) leaves the replication claimed.
#[test]
fn an_answer_for_a_gone_session_finishes_nothing() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15]);
    let (_, got) = answered(&host, &request("t.c", T + 3, T + 5, true), 1 << 20, false);
    assert!(got.last().unwrap().contains(" true  "), "{got:?}");
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_IN_PROGRESS);
    assert_eq!(host.sender_replicating_charts(), 1);
}

/// An unknown chart gets C's two lines and a record.
#[test]
fn an_unknown_chart_gets_the_empty_answer() {
    let (host, _, _) = replicating_chart(&[10, 11]);
    let ((a, got), records) =
        netdata_agent_log::capture(|| answered(&host, &request("no.such", 1, 2, true), 1 << 20, true));
    assert_eq!(a, Answered::NotFound);
    assert_eq!(got, vec!["RBEGIN 'no.such'".to_string(), "REND 0 0 0 true  0 0 W".to_string()]);
    assert_eq!(
        texts(&records),
        vec![(
            Priority::Err,
            "STREAM SND REPLAY ERROR: 'host:child/chart:no.such' not found, sending empty response to unblock parent"
                .to_string()
        )]
    );
}

/// Past the size bound the answer is cut before a step: partial, not streaming, ending at the last step sent.
#[test]
fn a_large_answer_is_cut_and_does_not_stream() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15, 16, 17]);
    let (_, got) = answered(&host, &request("t.c", T + 2, T + 7, true), 1, true);
    let (first, last) = chart.retention_for_collected(now_realtime_s());
    assert_eq!(
        got,
        vec![
            "RBEGIN 't.c'".to_string(),
            format!("RBEGIN '' {} {} W", T + 2, T + 3),
            "RSET \"d\" 13 A".to_string(),
            format!("REND 1 {first} {last} false {} {} W", T + 2, T + 3),
        ]
    );
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_IN_PROGRESS);
}

/// A chart none of whose dimensions went upstream this session has nothing to query: a streaming answer carries
/// only the chart's state, and the chart, never claimed, stays finished.
#[test]
fn a_chart_with_no_exposed_dimension_answers_its_state_only() {
    let (host, _) = streaming("*", PLAIN | caps::REPLICATION);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    for i in 0..5 {
        collect(&host, &chart, &[(&dim, 10 + i)], T + i);
    }
    let (_, got) = answered(&host, &request("t.c", T + 1, T + 4, true), 1 << 20, true);
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(got[0], "RBEGIN 't.c'");
    assert!(got[1].starts_with("RSSTATE "), "{got:?}");
    assert!(got[2].starts_with("REND 1 ") && got[2].contains(" true  "), "{got:?}");
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_FINISHED);
    assert_eq!(host.sender_replicating_charts(), 0);
}

/// A collection waits while the chart's collection lock is held, as a finishing answer holds it.
#[test]
fn a_collection_waits_for_the_collection_lock() {
    let (host, _, chart) = replicating_chart(&[10, 11]);
    let dim = chart.dim("d").unwrap();
    let guard = Chart::lock_collection(chart.as_ref());
    let (done, collected) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        s.spawn(|| {
            collect(&host, &chart, &[(&dim, 12)], T + 2);
            let _ = done.send(());
        });
        assert!(collected.recv_timeout(Duration::from_millis(200)).is_err(), "collected under the lock");
        drop(guard);
        collected.recv_timeout(Duration::from_secs(5)).expect("collected once the lock was released");
    });
}

/// A second answer that starts streaming finds the chart finished: nothing is given back twice.
#[test]
fn a_finished_chart_gives_nothing_back_twice() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15]);
    // a second chart keeps its claim, so the host's counter shows a double give-back
    let c2 = host.charts().create(&ChartSpec { id: "c2", ..chart_spec(DbMode::Ram) }).0;
    let (d2, _) = c2.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&host, &c2, &[(&d2, 1)], T + 6);
    assert_eq!(host.sender_replicating_charts(), 2);
    answered(&host, &request("t.c", T + 3, T + 5, true), 1 << 20, true);
    assert_eq!(host.sender_replicating_charts(), 1);
    answered(&host, &request("t.c", T + 3, T + 5, true), 1 << 20, true);
    assert_eq!(host.sender_replicating_charts(), 1, "the second answer gives nothing back");
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_FINISHED);
}

/// A dimension added since the chart's definition went upstream is not in its answer: no points, no state.
#[test]
fn a_dimension_not_sent_yet_is_left_out() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13]);
    let (fresh, _) = chart.dim_add("e", None, 1, 1, Algorithm::Absolute);
    assert!(!fresh.is_sent_upstream());
    let (_, got) = answered(&host, &request("t.c", T + 1, T + 3, true), 1 << 20, true);
    assert!(got.iter().any(|l| l.starts_with("RSET \"d\" ")), "{got:?}");
    assert!(got.iter().any(|l| l.starts_with("RDSTATE 'd' ")), "{got:?}");
    assert!(!got.iter().any(|l| l.contains("\"e\"") || l.contains("'e'")), "{got:?}");
}

/// An answer that starts streaming but ends in a gap ends the replication and keeps the resync horizon: after a
/// reconnect's reset the chart is defined again at T + 6 (horizon T + 5 + 3), and the parent asks from the chart's
/// last point, so no step is walked and the last point sent (none) is short of the window's end.
#[test]
fn a_streaming_answer_ending_in_a_gap_keeps_the_resync_horizon() {
    let (host, recorder, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15]);
    reset_charts(&host);
    let dim = chart.dim("d").unwrap();
    collect(&host, &chart, &[(&dim, 16)], T + 6);
    recorder.take();
    assert_eq!((chart.resync_time_s(), claim(&chart)), (T + 8, flags::SENDER_REPLICATION_IN_PROGRESS));
    let (_, got) = answered(&host, &request("t.c", T + 6, T + 6, true), 1 << 20, true);
    let (first, last) = chart.retention_for_collected(now_realtime_s());
    let d = dim.collection();
    let c = chart.collection();
    assert_eq!(
        got,
        vec![
            "RBEGIN 't.c'".to_string(),
            format!("RDSTATE 'd' {} 16 16 16", d.last_collected_time.0 * 1_000_000 + d.last_collected_time.1),
            format!(
                "RSSTATE {} {}",
                c.last_collected.0 * 1_000_000 + c.last_collected.1,
                c.last_updated.0 * 1_000_000 + c.last_updated.1
            ),
            format!("REND 1 {first} {last} true  {} {} W", T + 6, T + 6),
        ]
    );
    assert_eq!(last, T + 6);
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_FINISHED);
    assert_eq!(host.sender_replicating_charts(), 0);
    assert_eq!(chart.resync_time_s(), T + 8, "the horizon is kept");
}

/// Only tier 0 is queried: a three-tier dbengine host (windows of 5 and 15 s) answers one step per second with each
/// stored value, and its REND carries tier 0's retention.
#[test]
fn a_multi_tier_host_answers_from_tier_0() {
    use crate::storage::Backfill;
    use crate::testutil::{backfill_fixture, store};
    // a multiple of 15, a week before the tests' wall clock
    const B: i64 = 1_790_179_995;
    let f = backfill_fixture(Backfill::New, DbMode::Dbengine);
    for t in B..=B + 18 {
        store(&f.dim, t, (t - B) as f64);
    }
    f.dim.set_exposed_upstream(1);
    let mut out = Vec::new();
    let a = answer(&f.host, &request("t.c", B + 2, B + 12, false), PLAIN | caps::REPLICATION, 1 << 20, &mut out, |_| true);
    assert_eq!(a, Answered::Executed);
    let mut want = vec!["RBEGIN 't.c'".to_string()];
    for t in B + 3..=B + 12 {
        want.push(format!("RBEGIN '' {} {t} W", t - 1));
        want.push(format!("RSET \"d\" {} A", t - B));
    }
    want.push(format!("REND 1 {B} {} false {} {} W", B + 18, B + 2, B + 12));
    assert_eq!(lines(&out), want);
}

/// Past the size bound means strictly past it: an answer exactly at the bound takes one more step.
#[test]
fn an_answer_is_cut_only_once_past_its_bound() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15, 16, 17]);
    // the chart's line and one step
    let one_step = "RBEGIN 't.c'\n".len()
        + format!("RBEGIN '' {} {} {}\n", T + 2, T + 3, now_realtime_s()).len()
        + "RSET \"d\" 13 A\n".len();
    assert_eq!(one_step, 70);
    let (_, at) = answered(&host, &request("t.c", T + 2, T + 7, true), one_step, true);
    let (_, below) = answered(&host, &request("t.c", T + 2, T + 7, true), one_step - 1, true);
    let (first, last) = chart.retention_for_collected(now_realtime_s());
    let step = |t: i64| vec![format!("RBEGIN '' {} {t} W", t - 1), format!("RSET \"d\" {} A", t - T + 10)];
    let mut want_at = vec!["RBEGIN 't.c'".to_string()];
    want_at.extend(step(T + 3));
    want_at.extend(step(T + 4));
    want_at.push(format!("REND 1 {first} {last} false {} {} W", T + 2, T + 4));
    assert_eq!(at, want_at);
    let mut want_below = vec!["RBEGIN 't.c'".to_string()];
    want_below.extend(step(T + 3));
    want_below.push(format!("REND 1 {first} {last} false {} {} W", T + 2, T + 3));
    assert_eq!(below, want_below);
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_IN_PROGRESS);
}

/// A streaming answer whose walk ends more than an interval before its window's end (nothing after the parent's
/// last point) finishes the replication but keeps the resync horizon.
#[test]
fn a_streaming_answer_ending_with_a_gap_keeps_the_resync() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15]);
    let resync = chart.resync_time_s();
    assert_ne!(resync, 0);
    let (_, got) = answered(&host, &request("t.c", T + 5, T + 5, true), 1 << 20, true);
    let (first, last) = chart.retention_for_collected(now_realtime_s());
    assert_eq!(got.len(), 4, "{got:?}");
    assert_eq!(got[0], "RBEGIN 't.c'");
    assert!(got[1].starts_with("RDSTATE 'd' ") && got[2].starts_with("RSSTATE "), "{got:?}");
    assert_eq!(got[3], format!("REND 1 {first} {last} true  {} {} W", T + 5, T + 5));
    assert_eq!(claim(&chart), flags::SENDER_REPLICATION_FINISHED);
    assert_eq!(host.sender_replicating_charts(), 0);
    assert_eq!(host.pulse_state() & host_status::SENDER, host_status::SND_RUNNING);
    assert_eq!(chart.resync_time_s(), resync);
}

/// With start_streaming the end is raised to the last update when a collection came between the retention read and
/// the lock: the answer waits for the lock, then walks to the new last update.
#[test]
fn a_streaming_answer_ends_at_the_last_update_it_finds_under_the_lock() {
    use crate::testutil::store;
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15]);
    let dim = chart.dim("d").unwrap();
    let guard = Chart::lock_collection(chart.as_ref());
    let out = std::thread::scope(|s| {
        let answering = s.spawn(|| {
            let mut out = Vec::new();
            answer(&host, &request("t.c", T + 3, T + 4, true), PLAIN | caps::REPLICATION, 1 << 20, &mut out, |_| true);
            out
        });
        // the answer read the retention (to T + 5) and waits for the lock
        std::thread::sleep(Duration::from_millis(200));
        // a collection under the lock: two more points, the last update moved
        store(&dim, T + 6, 16.0);
        store(&dim, T + 7, 17.0);
        chart.update_collection(|c| c.last_updated.0 = T + 7);
        drop(guard);
        answering.join().unwrap()
    });
    let got = lines(&out);
    let (first, last) = chart.retention_for_collected(now_realtime_s());
    let mut want = vec!["RBEGIN 't.c'".to_string()];
    for t in T + 4..=T + 7 {
        want.push(format!("RBEGIN '' {} {t} W", t - 1));
        want.push(format!("RSET \"d\" {} A", t - T + 10));
    }
    want.extend(got.iter().filter(|l| l.starts_with("RDSTATE ") || l.starts_with("RSSTATE ")).cloned());
    want.push(format!("REND 1 {first} {last} true  {} {} W", T + 3, T + 7));
    assert_eq!(got, want);
}

/// A chart updated ahead of the wall clock: the raised end is bounded by now (`MIN(last_updated, wall clock)`).
#[test]
fn a_streaming_answer_ends_no_later_than_now() {
    let (host, _) = streaming("*", PLAIN | caps::REPLICATION);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let ahead = now_realtime_s() + 1000;
    for i in 0..6 {
        collect(&host, &chart, &[(&dim, 10 + i)], ahead + i);
    }
    for _ in 0..3 {
        let w = now_realtime_s();
        let mut out = Vec::new();
        answer(&host, &request("t.c", w - 10, w - 5, false), PLAIN | caps::REPLICATION, 1 << 20, &mut out, |_| true);
        if now_realtime_s() != w {
            continue;
        }
        let got = lines(&out);
        let state: Vec<String> =
            got.iter().filter(|l| l.starts_with("RDSTATE ") || l.starts_with("RSSTATE ")).cloned().collect();
        let mut want = vec!["RBEGIN 't.c'".to_string(), format!("RBEGIN '' {} {w} W", w - 1)];
        want.extend(state);
        want.push(format!("REND 1 {} {w} true  {} {w} W", w - 1, w - 1));
        assert_eq!(got, want);
        return;
    }
    panic!("no answer within one second of the wall clock");
}

/// The collection lock: a streaming answer holds it through its commit and releases it after the finish; an answer
/// that does not stream, one with an empty window and one with no exposed dimension hold none at their commit.
#[test]
fn a_streaming_answer_holds_the_collection_lock_through_its_commit() {
    let (host, _, chart) = replicating_chart(&[10, 11, 12, 13, 14, 15]);
    let (bare, _) = streaming("*", PLAIN | caps::REPLICATION);
    let unexposed = bare.charts().create(&chart_spec(DbMode::Ram)).0;
    let (d, _) = unexposed.dim_add("d", None, 1, 1, Algorithm::Absolute);
    for i in 0..5 {
        collect(&bare, &unexposed, &[(&d, 10 + i)], T + i);
    }
    let cases = [
        ("streaming", &host, &chart, request("t.c", T + 3, T + 5, true), true),
        ("not streaming", &host, &chart, request("t.c", T + 1, T + 2, false), false),
        ("empty window", &host, &chart, request("t.c", 0, T + 5, true), false),
        ("no exposed dimension", &bare, &unexposed, request("t.c", T + 1, T + 4, true), false),
    ];
    for (name, host, chart, req, held) in cases {
        let (locked, at) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            let mut out = Vec::new();
            answer(host, &req, PLAIN | caps::REPLICATION, 1 << 20, &mut out, |_| {
                let locked = locked.clone();
                s.spawn(move || {
                    let _g = Chart::lock_collection(chart.as_ref());
                    let _ = locked.send(claim(chart));
                });
                let free = at.recv_timeout(Duration::from_millis(200)).is_ok();
                assert_eq!(free, !held, "{name}: lock free at the commit");
                true
            });
            if held {
                let flags = at.recv_timeout(Duration::from_secs(5)).expect("released after the answer");
                assert_eq!(flags, flags::SENDER_REPLICATION_FINISHED, "{name}: released after the finish");
            }
        });
    }
}

mod crafted {
    use std::collections::VecDeque;

    use netdata_agent_storage::storage_number::SN_DEFAULT_FLAGS;
    use netdata_agent_storage::storage_point::StoragePoint;

    use super::*;
    use crate::upstream::replay::{DimQuery, Points, Walk, expanded_before, moves_before, walk};

    const W: i64 = 1_800_000_000;

    struct Crafted(VecDeque<StoragePoint>);

    impl Points for Crafted {
        fn next_metric(&mut self) -> StoragePoint {
            self.0.pop_front().expect("read past the end")
        }
        fn is_finished(&self) -> bool {
            self.0.is_empty()
        }
    }

    fn p(start: i64, end: i64, v: f64) -> StoragePoint {
        StoragePoint {
            min: v,
            max: v,
            sum: v,
            start_time_s: start,
            end_time_s: end,
            count: 1,
            anomaly_count: 0,
            flags: SN_DEFAULT_FLAGS,
        }
    }

    /// A chart of update every 2 s with dimensions d and e (not exposed: the walk does not look).
    fn chart() -> (Host, Arc<Chart>, Arc<Dim>, Arc<Dim>) {
        let (host, _) = streaming("*", PLAIN | caps::REPLICATION);
        let chart = host.charts().create(&ChartSpec { update_every: 2, ..chart_spec(DbMode::Ram) }).0;
        let (d, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        let (e, _) = chart.dim_add("e", None, 1, 1, Algorithm::Absolute);
        (host, chart, d, e)
    }

    /// The walk over `points` per dimension from `after` to `before` (not streaming): its lines, the window it
    /// leaves and what it counted.
    fn walked(
        host: &Host,
        chart: &Chart,
        points: Vec<(&Arc<Dim>, Vec<StoragePoint>)>,
        after: i64,
        before: i64,
    ) -> (Vec<String>, Window, Walk) {
        let mut dims: Vec<DimQuery<'_, Crafted>> = points
            .into_iter()
            .map(|(dim, pts)| DimQuery {
                dim: dim.as_ref(),
                query: Crafted(pts.into()),
                sp: StoragePoint::default(),
                skip: false,
            })
            .collect();
        let mut window = Window { after, before, streaming: false };
        let mut out = Vec::new();
        let enc = emit::Enc::replication(PLAIN | caps::REPLICATION);
        let w = walk(&mut out, &enc, host, chart, &mut dims, &mut window, W, true, 1 << 20);
        let lines = String::from_utf8(out).unwrap().lines().map(str::to_string).collect();
        (lines, window, w)
    }

    fn rbegin(start: i64, end: i64) -> String {
        format!("RBEGIN '' {start} {end} {W}")
    }

    fn rset(dim: &str, v: i64) -> String {
        format!("RSET \"{dim}\" {v} A")
    }

    fn e_steps() -> Vec<StoragePoint> {
        vec![p(T, T + 1, 20.0), p(T + 1, T + 2, 21.0), p(T + 2, T + 3, 22.0)]
    }

    /// `max_skip-- >= 0`: a dimension 1001 points behind is read 1001 times and left out with one record; exactly
    /// 1000 reads trip it too, even ending on a good point, which still goes out in that step (C emits every enabled
    /// dimension); 999 reads do not.
    #[test]
    fn a_dimension_that_does_not_advance_is_left_out() {
        let (host, chart, d, e) = chart();
        let stale = |n: i64| (0..n).map(|i| p(T - n + i, T - n + i + 1, 1.0)).collect::<Vec<_>>();
        let ((a, b, c), records) = netdata_agent_log::capture(|| {
            let mut d_a = stale(1001);
            d_a.push(p(T, T + 1, 7.0));
            let a = walked(&host, &chart, vec![(&d, d_a), (&e, e_steps())], T, T + 3);
            let mut d_b = stale(999);
            d_b.extend([p(T, T + 1, 7.0), p(T + 1, T + 2, 8.0)]);
            let b = walked(&host, &chart, vec![(&d, d_b), (&e, e_steps())], T, T + 3);
            let mut d_c = stale(998);
            d_c.extend([p(T, T + 1, 7.0), p(T + 1, T + 2, 8.0)]);
            let c = walked(&host, &chart, vec![(&d, d_c), (&e, e_steps())], T, T + 3);
            (a, b, c)
        });
        let window = Window { after: T, before: T + 3, streaming: false };
        assert_eq!(
            a,
            (
                vec![rbegin(T, T + 1), rset("e", 20), rbegin(T + 1, T + 2), rset("e", 21), rbegin(T + 2, T + 3), rset("e", 22)],
                window,
                Walk { finished_with_gap: false, points_read: 1004, points_generated: 3 }
            )
        );
        assert_eq!(
            b,
            (
                vec![
                    rbegin(T, T + 1),
                    rset("d", 7),
                    rset("e", 20),
                    rbegin(T + 1, T + 2),
                    rset("e", 21),
                    rbegin(T + 2, T + 3),
                    rset("e", 22)
                ],
                window,
                Walk { finished_with_gap: false, points_read: 1003, points_generated: 4 }
            )
        );
        assert_eq!(
            c,
            (
                vec![
                    rbegin(T, T + 1),
                    rset("d", 7),
                    rset("e", 20),
                    rbegin(T + 1, T + 2),
                    rset("d", 8),
                    rset("e", 21),
                    rbegin(T + 2, T + 3),
                    rset("e", 22)
                ],
                window,
                Walk { finished_with_gap: false, points_read: 1003, points_generated: 5 }
            )
        );
        // the capture takes every call, before the one-a-second limit
        let record = (
            Priority::Err,
            format!(
                "STREAM SND REPLAY: 'host:child/chart:t.c/dim:d': db does not advance the query beyond time {} \
                 (tried 1000 times to get the next point and always got back a point in the past)",
                T + 1
            ),
        );
        assert_eq!(texts(&records), vec![record.clone(), record]);
    }

    /// Misaligned dimensions: the step starts at the minimum end less the minimum interval, or where the last step
    /// ended when that lies among their starts; a step can then start before the last one ended.
    #[test]
    fn misaligned_dimensions_start_where_the_last_step_ended() {
        let (host, chart, d, e) = chart();
        let got = walked(
            &host,
            &chart,
            vec![(&d, vec![p(T, T + 10, 100.0)]), (&e, vec![p(T + 2, T + 3, 20.0), p(T + 5, T + 6, 21.0)])],
            T + 2,
            T + 10,
        );
        assert_eq!(
            got,
            (
                vec![
                    rbegin(T + 2, T + 3),
                    rset("d", 100),
                    rset("e", 20),
                    rbegin(T + 3, T + 6),
                    rset("d", 100),
                    rset("e", 21),
                    rbegin(T, T + 10),
                    rset("d", 100)
                ],
                Window { after: T + 2, before: T + 10, streaming: false },
                Walk { finished_with_gap: false, points_read: 3, points_generated: 5 }
            )
        );
    }

    /// A point of no length starts one chart interval before its end.
    #[test]
    fn a_zero_length_point_spans_the_charts_interval() {
        let (host, chart, d, _) = chart();
        let got = walked(&host, &chart, vec![(&d, vec![p(T + 3, T + 3, 30.0), p(T + 4, T + 4, 31.0)])], T + 2, T + 4);
        assert_eq!(
            got,
            (
                vec![rbegin(T + 1, T + 3), rset("d", 30), rbegin(T + 2, T + 4), rset("d", 31)],
                Window { after: T + 2, before: T + 4, streaming: false },
                Walk { finished_with_gap: false, points_read: 2, points_generated: 2 }
            )
        );
    }

    /// With every next point in the future the walk jumps to it; past the window's end with nothing sent, the
    /// window ends just before it, with a gap; with something sent it stays and the walk ends with a gap.
    #[test]
    fn a_gap_jumps_ahead_and_past_the_end_moves_it() {
        let (host, chart, d, _) = chart();
        let jump = walked(&host, &chart, vec![(&d, vec![p(T + 2, T + 3, 1.0), p(T + 7, T + 8, 2.0)])], T + 2, T + 9);
        assert_eq!(
            jump,
            (
                vec![rbegin(T + 2, T + 3), rset("d", 1), rbegin(T + 7, T + 8), rset("d", 2)],
                Window { after: T + 2, before: T + 9, streaming: false },
                Walk { finished_with_gap: false, points_read: 2, points_generated: 2 }
            )
        );
        let nothing = walked(&host, &chart, vec![(&d, vec![p(T + 20, T + 21, 5.0)])], T + 2, T + 9);
        assert_eq!(
            nothing,
            (
                vec![],
                Window { after: T + 2, before: T + 19, streaming: false },
                Walk { finished_with_gap: true, points_read: 1, points_generated: 0 }
            )
        );
        let after_one =
            walked(&host, &chart, vec![(&d, vec![p(T + 2, T + 3, 1.0), p(T + 20, T + 21, 5.0)])], T + 2, T + 9);
        assert_eq!(
            after_one,
            (
                vec![rbegin(T + 2, T + 3), rset("d", 1)],
                Window { after: T + 2, before: T + 9, streaming: false },
                Walk { finished_with_gap: true, points_read: 2, points_generated: 1 }
            )
        );
    }

    /// The end moves to the earliest page end only when later, by fewer than 1024 intervals (the division
    /// truncates), and before both the chart's last update and now; a dimension answering 0 restarts the minimum.
    #[test]
    fn the_end_moves_to_the_pages_end_within_cs_bounds() {
        let before = T + 10;
        let moves = |expanded: i64, last_updated: i64, wall: i64| moves_before(expanded, before, 2, last_updated, wall);
        let far = T + 10_000;
        assert_eq!(
            [
                moves(T + 12, far, far),
                moves(before, far, far),
                moves(before - 2, far, far),
                moves(before + 2 * 1023, far, far),
                moves(before + 2 * 1024 - 1, far, far),
                moves(before + 2 * 1024, far, far),
                moves(T + 12, T + 12, far),
                moves(T + 12, T + 13, far),
                moves(T + 12, far, T + 12),
                moves(T + 12, far, T + 13),
            ],
            [true, false, false, true, true, false, false, true, false, true]
        );
        assert_eq!(
            [
                expanded_before([T + 20, T + 12, T + 30].into_iter()),
                expanded_before([T + 20, 0, T + 30].into_iter()),
                expanded_before([T + 20, 0].into_iter()),
                expanded_before([].into_iter()),
            ],
            [T + 12, T + 30, 0, 0]
        );
    }
}
