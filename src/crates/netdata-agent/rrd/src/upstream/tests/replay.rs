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

/// An answer that did not go into the request's session (a reconnect since) leaves the replication claimed.
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
