//! The runner against a stand-in lab: the freeze file is removed on every
//! path, writes that never settle and an agent that refuses the caller stop
//! the run, a capture that keeps growing is given up on after the last
//! attempt, a due rotation that never comes stops the run before it freezes,
//! an ingestion-only run asks nothing, and a window with trace asks is judged
//! only when the units they read are.

mod common;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use common::{BASE_NS, request, write_wal};
use otel_oracle::capture::{self, Record, Signal};
use otel_oracle::freeze::{Change, MAX_ATTEMPTS};
use otel_oracle::ingest::IngestWindow;
use otel_oracle::run::{self, AskError, Config, Lab, ROTATION_WAIT, RunError};
use otel_oracle::tier2::{Plan, Request};
use prost::Message;
use serde_json::Value;

const MINUTE_NS: u64 = 60_000_000_000;

struct StandIn {
    now_ns: u64,
    clock: Duration,
    frozen: bool,
    freezes: u32,
    asks: u32,
    capture: PathBuf,
    /// Appends a record to the capture on every sleep while frozen.
    grow_while_settling: bool,
    /// Appends a record to the capture on every request.
    grow_while_asking: bool,
    refuse: bool,
}

fn append(capture: &PathBuf) {
    let mut file = fs::File::options().append(true).open(capture).unwrap();
    let record = Record {
        received_unix_ns: 0,
        signal: Signal::Logs,
        grpc_code: 0,
        rejected: 0,
        request: Vec::new(),
    };
    capture::write_record(&mut file, &record).unwrap();
}

impl Lab for StandIn {
    fn now_ns(&self) -> u64 {
        self.now_ns
    }

    fn monotonic(&self) -> Duration {
        self.clock
    }

    fn sleep(&mut self, duration: Duration) {
        self.clock += duration;
        if self.frozen && self.grow_while_settling {
            append(&self.capture);
        }
    }

    fn set_frozen(&mut self, frozen: bool) -> std::io::Result<()> {
        assert_ne!(self.frozen, frozen);
        self.frozen = frozen;
        self.freezes += u32::from(frozen);
        Ok(())
    }

    fn ask(&mut self, _plan: &Plan, _request: &Request) -> Result<Value, AskError> {
        assert!(self.frozen, "asked while the tee takes exports");
        self.asks += 1;
        if self.grow_while_asking {
            append(&self.capture);
        }
        if self.refuse {
            return Err(AskError::Refused("HTTP 412".to_string()));
        }
        Err(AskError::Failed("no agent here".to_string()))
    }
}

/// A capture whose first record arrived a minute after `BASE_NS`, an empty
/// store, and a lab clock `minutes` after `BASE_NS`.
fn lab(minutes: u64) -> (tempfile::TempDir, Config, StandIn) {
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("capture.otee");
    let mut out = Vec::new();
    capture::write_header(&mut out).unwrap();
    for i in 0..5u8 {
        let record = Record {
            received_unix_ns: i64::try_from(BASE_NS + (1 + 10 * u64::from(i)) * MINUTE_NS).unwrap(),
            signal: Signal::Traces,
            grpc_code: 0,
            rejected: 0,
            request: request(i + 1, 10 * i + 1, 3).encode_to_vec(),
        };
        capture::write_record(&mut out, &record).unwrap();
    }
    fs::write(&capture, out).unwrap();
    let config = Config {
        capture: capture.clone(),
        store: dir.path().join("store"),
        chunk_entries: 100,
        ingest: IngestWindow::LAB,
        longest_freeze: Duration::from_secs(150),
        ask: true,
    };
    let stand_in = StandIn {
        now_ns: BASE_NS + minutes * MINUTE_NS,
        clock: Duration::ZERO,
        frozen: false,
        freezes: 0,
        asks: 0,
        capture,
        grow_while_settling: false,
        grow_while_asking: false,
        refuse: false,
    };
    (dir, config, stand_in)
}

#[test]
fn an_agent_refusing_the_caller_stops_the_run_unfrozen() {
    let (_dir, config, mut lab) = lab(60);
    lab.refuse = true;

    let error = run::run(&mut lab, &config).unwrap_err();

    assert!(matches!(error, RunError::Refused(_)), "{error}");
    assert_eq!((lab.frozen, lab.freezes, lab.asks), (false, 1, 1));
}

#[test]
fn writes_that_never_settle_stop_the_run_unfrozen() {
    let (_dir, config, mut lab) = lab(60);
    lab.grow_while_settling = true;

    let error = run::run(&mut lab, &config).unwrap_err();

    assert!(matches!(error, RunError::NotSettled), "{error}");
    assert_eq!((lab.frozen, lab.freezes, lab.asks), (false, 1, 0));
    assert!(lab.clock >= otel_oracle::freeze::SETTLE_CAP);
}

#[test]
fn a_capture_that_keeps_growing_is_given_up_after_the_last_attempt() {
    let (_dir, config, mut lab) = lab(60);
    lab.grow_while_asking = true;

    let outcome = run::run(&mut lab, &config).unwrap();

    assert_eq!((lab.frozen, lab.freezes), (false, 1));
    assert_eq!(outcome.attempts, MAX_ATTEMPTS);
    assert_eq!(outcome.retries.len(), MAX_ATTEMPTS as usize - 1);
    let gave_up = outcome.gave_up.as_ref().unwrap();
    assert!(matches!(gave_up[..], [Change::Capture { from, to }] if to > from));
    assert!(outcome.windows.is_empty());
    let report = run::report(&outcome, Default::default(), 10).unwrap();
    assert!(
        report.markdown.contains("## Gave up"),
        "{}",
        report.markdown
    );
    assert!(report.markdown.contains("## Attempt 2 retried"));
}

#[test]
fn a_rotation_that_never_comes_stops_the_run_before_it_freezes() {
    let (dir, mut config, mut lab) = lab(60);
    config.store = dir.path().join("store");
    write_wal(&config.store, &[&request(9, 1, 2)], 0);

    let error = run::run(&mut lab, &config).unwrap_err();

    assert!(matches!(error, RunError::RotationWait), "{error}");
    assert_eq!((lab.freezes, lab.asks), (0, 0));
    assert!(lab.clock >= ROTATION_WAIT);
}

#[test]
fn a_capture_younger_than_any_window_has_nothing_to_judge() {
    let (_dir, config, mut lab) = lab(25);

    let error = run::run(&mut lab, &config).unwrap_err();

    assert!(matches!(error, RunError::NoWindow), "{error}");
    assert_eq!(lab.freezes, 0);
}

#[test]
fn an_ingestion_run_asks_nothing_and_judges_every_window() {
    let (_dir, mut config, mut lab) = lab(60);
    config.ask = false;

    let outcome = run::run(&mut lab, &config).unwrap();

    assert_eq!((lab.frozen, lab.freezes, lab.asks), (false, 1, 0));
    assert_eq!(outcome.attempts, 1);
    let names: Vec<&str> = outcome.windows.iter().map(|w| w.window.name).collect();
    assert_eq!(names, ["W15", "W-sealed"]);
    for window in &outcome.windows {
        assert!(window.check.judged(), "{:?}", window.check);
        assert!(window.plan.is_none() && window.findings.is_empty());
        assert_eq!(window.checks["ORC-INGEST"].differing, 0);
    }
    assert_eq!(outcome.records, 5);
    let report = run::report(&outcome, Default::default(), 10).unwrap();
    assert!(
        report.markdown.contains("- W15 judged: 1"),
        "{}",
        report.markdown
    );
}

#[test]
fn a_window_with_trace_asks_is_judged_only_when_their_units_are() {
    let check = |unknown: Vec<usize>| otel_oracle::matching::WindowCheck {
        units: vec![0, 1],
        lost: 0,
        unmatched: 0,
        unknown,
        stale: false,
    };
    let trace = Request {
        id: "trace largest".to_string(),
        ask: otel_oracle::tier2::Ask::Trace {
            trace_id: [1; 16],
            after_s: 0,
            before_s: 60,
            span_cap: 65_536,
        },
        body: Value::Null,
    };
    let outcome = |requests: Vec<Request>| run::WindowOutcome {
        window: otel_oracle::freeze::Window {
            name: "W15",
            after_s: 0,
            before_s: 60,
            grid: otel_oracle::calc::Grid::for_window(0, 60),
        },
        check: check(Vec::new()),
        trace_check: check(vec![1]),
        spans: 0,
        plan: Some(Plan {
            after_s: 0,
            before_s: 60,
            candidates: 2,
            scenarios: Vec::new(),
            requests,
        }),
        answers: Default::default(),
        findings: Vec::new(),
        checks: Default::default(),
        sensitive: Default::default(),
    };

    assert!(outcome(Vec::new()).judged());
    assert!(!outcome(vec![trace]).judged());
}
