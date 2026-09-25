//! A tier-2 run against the lab, with the outside world injected ([`Lab`]) so
//! the whole sequence runs offline in tests:
//!
//! 1. before freezing, the heavy work: the capture so far, the store, the
//!    windows and the requests for each;
//! 2. wait while the active WAL could be sealed during the freeze;
//! 3. freeze the tee, let writes settle, snapshot, ask (row pages included),
//!    snapshot again; ask again while anything bearing on the windows moved,
//!    and give up after the last attempt; unfreeze on every path;
//! 4. after unfreezing, expectations and matching at the frozen capture
//!    length and store, then each judged window's answers against the
//!    calculator.
//!
//! Only what bears on a window is held: the sealed files a window overlaps,
//! and from the capture received since [`CAPTURE_MARGIN`] before the earliest
//! window, the spans starting inside a window or stored in a held unit. A
//! wrong assumption there shows up as unmatched rows, never as a pass.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::capture::Reader;
use crate::freeze::{self, Change, Decision, Probe, Settle, Settled, Snapshot, Window};
use crate::ingest::IngestWindow;
use crate::matching::{self, Expected, WindowCheck};
use crate::membership::{self, Membership, MembershipError, RowKey, SealedCache, Unit, UnitKind};
use crate::model::OracleSpan;
use crate::report::{self, Aliases, CheckCount, Finding, Summary};
use crate::tier2::{self, Plan, Request};

const NS: u64 = 1_000_000_000;

/// Capture records received this long before the earliest window are not
/// replayed: no row of a unit overlapping a window arrived that early.
pub const CAPTURE_MARGIN: Duration = Duration::from_secs(30 * 60);
/// How often, and how long, to wait for a due WAL rotation.
pub const ROTATION_POLL: Duration = Duration::from_secs(10);
pub const ROTATION_WAIT: Duration = Duration::from_secs(17 * 60);
/// How often, and how many times, to re-read a store whose WAL is being
/// written before the freeze.
const STORE_POLL: Duration = Duration::from_millis(250);
const STORE_TRIES: u32 = 40;
/// Rounds of asking: the requests, then older pages, then newer pages.
const PAGE_ROUNDS: usize = 3;
/// Top values per field kept out of the report.
pub const SENSITIVE_TOP: usize = 20;

/// Why a request has no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    /// The agent refused the caller (an expired bearer): the run stops.
    Refused(String),
    /// This request failed; its answer is missing and judged so.
    Failed(String),
}

/// The world a run acts on: the lab for the binary, a stand-in in tests.
pub trait Lab {
    /// Wall-clock time, unix nanoseconds.
    fn now_ns(&self) -> u64;
    /// A monotonic clock reading.
    fn monotonic(&self) -> Duration;
    fn sleep(&mut self, duration: Duration);
    /// Creates (true) or removes (false) the tee's freeze file.
    fn set_frozen(&mut self, frozen: bool) -> io::Result<()>;
    /// Sends `request` of `plan` to the agent's Function and returns its JSON.
    fn ask(&mut self, plan: &Plan, request: &Request) -> Result<Value, AskError>;
}

#[derive(Debug, Clone)]
pub struct Config {
    pub capture: PathBuf,
    /// The traces store directory (`<run>/lib/otel/traces`).
    pub store: PathBuf,
    pub chunk_entries: u32,
    pub ingest: IngestWindow,
    /// The longest the tee may stay frozen; a rotation due within it is
    /// waited out first.
    pub longest_freeze: Duration,
    /// Ask the tier-2 questions; without them the run checks ingestion only.
    pub ask: bool,
}

#[derive(Debug)]
pub enum RunError {
    Capture(String),
    Store(MembershipError),
    /// The capture is too young for any window.
    NoWindow,
    /// A due WAL rotation did not happen within [`ROTATION_WAIT`].
    RotationWait,
    /// Writes did not settle within the freeze's settling cap.
    NotSettled,
    Refused(String),
    /// The freeze file could not be created or removed; a failed removal
    /// leaves the tee refusing exports.
    Freeze(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Capture(why) => write!(f, "capture: {why}"),
            RunError::Store(error) => write!(f, "store: {error:?}"),
            RunError::NoWindow => write!(f, "the capture is too young for any window"),
            RunError::RotationWait => write!(f, "a due WAL rotation never happened"),
            RunError::NotSettled => write!(f, "writes did not settle while frozen"),
            RunError::Refused(why) => write!(f, "the agent refused the caller: {why}"),
            RunError::Freeze(why) => write!(f, "freeze file: {why}"),
        }
    }
}

impl std::error::Error for RunError {}

fn capture_error(error: io::Error) -> RunError {
    RunError::Capture(error.to_string())
}

/// One window's result.
#[derive(Debug, Clone)]
pub struct WindowOutcome {
    pub window: Window,
    pub check: WindowCheck,
    /// Rows the calculator used for it.
    pub spans: usize,
    /// The requests asked (pages included) and their answers.
    pub plan: Option<Plan>,
    pub answers: BTreeMap<String, Value>,
    /// Findings and per-check counts when the window was judged.
    pub findings: Vec<Finding>,
    pub checks: BTreeMap<String, CheckCount>,
    /// Its rows' most frequent values, which the report must not show.
    pub sensitive: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub capture_bytes: u64,
    pub records: u64,
    /// Records received before the cutoff, not replayed.
    pub skipped: u64,
    pub kept: u64,
    pub doubtful: u64,
    pub synthesized: u64,
    pub notes: u64,
    /// Units matched, by kind.
    pub units: BTreeMap<&'static str, u64>,
    pub attempts: u32,
    pub frozen: Duration,
    /// What moved on each retried attempt.
    pub retries: Vec<Vec<Change>>,
    /// What moved on the last attempt, when the run gave up.
    pub gave_up: Option<Vec<Change>>,
    /// Requests without an answer: (request id, why), private.
    pub failed: Vec<(String, String)>,
    pub windows: Vec<WindowOutcome>,
}

fn first_record_ns(config: &Config) -> Result<u64, RunError> {
    let file = fs::File::open(&config.capture).map_err(capture_error)?;
    let mut reader = Reader::new(BufReader::new(file), None).map_err(capture_error)?;
    let record = reader
        .next_record()
        .map_err(capture_error)?
        .ok_or_else(|| RunError::Capture("no record yet".to_string()))?;
    u64::try_from(record.received_unix_ns)
        .map_err(|_| RunError::Capture("a receive time before 1970".to_string()))
}

fn capture_len(config: &Config) -> Result<u64, RunError> {
    fs::metadata(&config.capture)
        .map(|m| m.len())
        .map_err(capture_error)
}

/// Whether a row starting at `start_ns` is inside one of the windows (as the
/// agent reads them, on their grids).
fn in_a_window(windows: &[Window], start_ns: i64) -> bool {
    windows.iter().any(|w| {
        let after = i64::from(w.grid.after_s) * NS as i64;
        let before = i64::from(w.grid.before_s) * NS as i64;
        start_ns >= after && start_ns < before
    })
}

/// The expectations from the capture's records received at `cutoff_ns` or
/// later, up to `limit` bytes; a record cut short inside the limit is an
/// error, one past the end without a limit is still being written. Only the
/// spans that bear on a window are held: those starting inside one, and
/// those whose row a held unit stores.
fn expectations(
    config: &Config,
    limit: Option<u64>,
    cutoff_ns: u64,
    windows: &[Window],
    store: &Membership,
    outcome: &mut Outcome,
) -> Result<Expected, RunError> {
    let mut stored = HashSet::new();
    for unit in &store.units {
        stored.extend(unit.rows.iter().copied());
    }
    let bears = |span: &OracleSpan| {
        in_a_window(windows, span.start_ns) || stored.contains(&RowKey::of(span))
    };
    let file = fs::File::open(&config.capture).map_err(capture_error)?;
    let mut reader = Reader::new(BufReader::new(file), limit).map_err(capture_error)?;
    let mut expected = Expected::default();
    let (mut records, mut skipped) = (0, 0);
    while let Some(record) = reader.next_record().map_err(capture_error)? {
        let index = records as usize;
        records += 1;
        if u64::try_from(record.received_unix_ns).is_ok_and(|at| at >= cutoff_ns) {
            expected.add_where(index, &record, &config.ingest, &bears);
        } else {
            skipped += 1;
        }
    }
    if limit.is_some() && reader.cut().is_some() {
        return Err(RunError::Capture(
            "a record is cut inside the frozen length".to_string(),
        ));
    }
    outcome.records = records;
    outcome.skipped = skipped;
    outcome.kept = expected.kept.len() as u64;
    outcome.doubtful = expected.doubtful.len() as u64;
    outcome.synthesized = expected.synthesized;
    outcome.notes = expected.notes.len() as u64;
    Ok(expected)
}

/// Reads the store, holding only the sealed units a window overlaps and
/// waiting while a WAL is caught mid-write.
fn read_store(
    lab: &mut impl Lab,
    config: &Config,
    cache: &mut SealedCache,
    windows: &[Window],
) -> Result<Membership, RunError> {
    let keep = |unit: &Unit| {
        windows
            .iter()
            .any(|w| matching::overlaps(unit.seconds, w.grid.after_s, w.grid.before_s))
    };
    let mut tries = 0;
    loop {
        match membership::read_store_with(&config.store, config.chunk_entries, cache, &keep) {
            Err(MembershipError::NotSettled(..)) if tries < STORE_TRIES => {
                tries += 1;
                lab.sleep(STORE_POLL);
            }
            read => return read.map_err(RunError::Store),
        }
    }
}

fn settle(lab: &mut impl Lab, config: &Config) -> Result<(), RunError> {
    let mut settle = Settle::new(lab.monotonic());
    loop {
        let probe = Probe {
            capture_bytes: capture_len(config)?,
            wal_bytes: membership::wal_lengths(&config.store),
        };
        match settle.observe(lab.monotonic(), probe) {
            Settled::Stable => return Ok(()),
            Settled::TimedOut => return Err(RunError::NotSettled),
            Settled::Waiting => lab.sleep(freeze::SETTLE_POLL),
        }
    }
}

/// Gives the matched rows of sealed files the seal's error-origin token.
fn stored_origins(matched: &mut matching::Matched, store: &Membership) {
    let sealed = |unit: usize| matches!(store.units[unit].kind, UnitKind::Sealed);
    crate::calc::add_stored_origins(&mut matched.spans, &sealed);
}

/// Asks `plan`'s requests, then the row pages their answers lead to.
fn ask_all(
    lab: &mut impl Lab,
    plan: &mut Plan,
    failed: &mut Vec<(String, String)>,
) -> Result<BTreeMap<String, Value>, RunError> {
    let mut answers = BTreeMap::new();
    let mut tried = BTreeSet::new();
    for _ in 0..PAGE_ROUNDS {
        for request in &plan.requests {
            if !tried.insert(request.id.clone()) {
                continue;
            }
            match lab.ask(plan, request) {
                Ok(answer) => {
                    answers.insert(request.id.clone(), answer);
                }
                Err(AskError::Failed(why)) => failed.push((request.id.clone(), why)),
                Err(AskError::Refused(why)) => return Err(RunError::Refused(why)),
            }
        }
        if tier2::add_pages(plan, &answers) == 0 {
            break;
        }
    }
    Ok(answers)
}

/// What the frozen part leaves for judging.
struct Frozen {
    capture_bytes: u64,
    store: Membership,
    asked: Vec<Option<(Plan, BTreeMap<String, Value>)>>,
}

fn while_frozen(
    lab: &mut impl Lab,
    config: &Config,
    cache: &mut SealedCache,
    windows: &[Window],
    plans: &[Option<Plan>],
    outcome: &mut Outcome,
) -> Result<Option<Frozen>, RunError> {
    for attempt in 1..=freeze::MAX_ATTEMPTS {
        outcome.attempts = attempt;
        settle(lab, config)?;
        let capture_bytes = capture_len(config)?;
        let store = read_store(lab, config, cache, windows)?;
        let before = Snapshot::of(capture_bytes, &store, windows);

        let mut asked = Vec::new();
        let mut failed = Vec::new();
        for plan in plans {
            match plan {
                Some(plan) => {
                    let mut plan = plan.clone();
                    let answers = ask_all(lab, &mut plan, &mut failed)?;
                    asked.push(Some((plan, answers)));
                }
                None => asked.push(None),
            }
        }

        let after_store = read_store(lab, config, cache, windows)?;
        let after = Snapshot::of(capture_len(config)?, &after_store, windows);
        match freeze::decide(&before, &after, attempt) {
            Decision::Proceed => {
                outcome.failed = failed;
                return Ok(Some(Frozen {
                    capture_bytes,
                    store,
                    asked,
                }));
            }
            Decision::Retry(changes) => outcome.retries.push(changes),
            Decision::GiveUp(changes) => {
                outcome.gave_up = Some(changes);
                return Ok(None);
            }
        }
    }
    Ok(None)
}

pub fn run(lab: &mut impl Lab, config: &Config) -> Result<Outcome, RunError> {
    let mut outcome = Outcome::default();
    let first_ns = first_record_ns(config)?;
    let now_s = u32::try_from(lab.now_ns() / NS).unwrap_or(u32::MAX);
    let windows = freeze::windows(first_ns, now_s);
    let Some(earliest) = windows.iter().map(|w| w.grid.after_s).min() else {
        return Err(RunError::NoWindow);
    };
    let margin_ns = u64::try_from(CAPTURE_MARGIN.as_nanos()).unwrap_or(u64::MAX);
    let cutoff_ns = (u64::from(earliest) * NS).saturating_sub(margin_ns);

    let mut cache = SealedCache::default();
    let mut plans: Vec<Option<Plan>> = vec![None; windows.len()];
    if config.ask {
        let store = read_store(lab, config, &mut cache, &windows)?;
        let expected = expectations(config, None, cutoff_ns, &windows, &store, &mut outcome)?;
        let mut matched = matching::match_rows(expected, &store.units);
        stored_origins(&mut matched, &store);
        for (plan, w) in plans.iter_mut().zip(&windows) {
            let check = matching::check_window(
                &store.units,
                &matched,
                &store.stale_wals,
                w.grid.after_s,
                w.grid.before_s,
            );
            let spans = matching::window_spans(&matched, &check);
            *plan = Some(tier2::plan(
                w.after_s,
                w.before_s,
                &spans,
                check.units.len() as u64,
            ));
        }
    }

    let waiting_since = lab.monotonic();
    loop {
        let store = read_store(lab, config, &mut cache, &windows)?;
        if !freeze::rotation_due(&store.wals, lab.now_ns(), config.longest_freeze) {
            break;
        }
        if lab.monotonic().saturating_sub(waiting_since) >= ROTATION_WAIT {
            return Err(RunError::RotationWait);
        }
        lab.sleep(ROTATION_POLL);
    }

    lab.set_frozen(true)
        .map_err(|e| RunError::Freeze(e.to_string()))?;
    let started = lab.monotonic();
    let result = while_frozen(lab, config, &mut cache, &windows, &plans, &mut outcome);
    outcome.frozen = lab.monotonic().saturating_sub(started);
    lab.set_frozen(false)
        .map_err(|e| RunError::Freeze(e.to_string()))?;
    let Some(frozen) = result? else {
        return Ok(outcome);
    };

    outcome.capture_bytes = frozen.capture_bytes;
    let store = frozen.store;
    let expected = expectations(
        config,
        Some(frozen.capture_bytes),
        cutoff_ns,
        &windows,
        &store,
        &mut outcome,
    )?;
    for unit in &store.units {
        let kind = match unit.kind {
            UnitKind::Sealed => "sealed",
            UnitKind::Chunk(_) => "chunk",
            UnitKind::Tail(_) => "tail",
        };
        *outcome.units.entry(kind).or_default() += 1;
    }
    let mut matched = matching::match_rows(expected, &store.units);
    stored_origins(&mut matched, &store);
    for (w, asked) in windows.iter().zip(frozen.asked) {
        let check = matching::check_window(
            &store.units,
            &matched,
            &store.stale_wals,
            w.grid.after_s,
            w.grid.before_s,
        );
        let spans = matching::window_spans(&matched, &check);
        let mut result = WindowOutcome {
            window: *w,
            spans: spans.len(),
            sensitive: report::sensitive_values(&spans, SENSITIVE_TOP),
            plan: None,
            answers: BTreeMap::new(),
            findings: Vec::new(),
            checks: BTreeMap::from([(
                "ORC-INGEST".to_string(),
                CheckCount {
                    compared: (spans.len() + check.lost) as u64,
                    differing: (check.lost + check.unmatched) as u64,
                },
            )]),
            check,
        };
        if let Some((mut plan, answers)) = asked {
            if result.check.judged() {
                plan.candidates = result.check.units.len() as u64;
                let (findings, checks) = tier2::judge(&plan, &spans, &answers);
                for mut finding in findings {
                    finding.scenario = format!("{} {}", w.name, finding.scenario);
                    result.findings.push(finding);
                }
                result.checks.extend(checks);
            }
            result.plan = Some(plan);
            result.answers = answers;
        }
        outcome.windows.push(result);
    }
    Ok(outcome)
}

/// A run's report: the markdown and summary for the evidence directory, and
/// the alias map for the private run directory.
#[derive(Debug, Clone)]
pub struct Report {
    pub markdown: String,
    pub summary: Value,
    pub aliases: Value,
}

/// Renders `outcome` with the first `first_n` findings; `Err` holds the raw
/// values that would have leaked into it.
pub fn report(
    outcome: &Outcome,
    commits: BTreeMap<String, String>,
    first_n: usize,
) -> Result<Report, Vec<String>> {
    let mut counts = BTreeMap::from([
        ("capture bytes".to_string(), outcome.capture_bytes),
        ("capture records".to_string(), outcome.records),
        ("records before the cutoff".to_string(), outcome.skipped),
        ("spans kept".to_string(), outcome.kept),
        ("spans in doubt".to_string(), outcome.doubtful),
        ("spans without a start".to_string(), outcome.synthesized),
        ("ingest notes".to_string(), outcome.notes),
        ("attempts".to_string(), u64::from(outcome.attempts)),
        (
            "frozen ms".to_string(),
            u64::try_from(outcome.frozen.as_millis()).unwrap_or(u64::MAX),
        ),
        (
            "requests unanswered".to_string(),
            outcome.failed.len() as u64,
        ),
    ]);
    for (kind, count) in &outcome.units {
        counts.insert(format!("units {kind}"), *count);
    }

    let mut summary = Summary {
        commits,
        ..Summary::default()
    };
    let mut findings = Vec::new();
    let mut sensitive = BTreeSet::new();
    for w in &outcome.windows {
        let name = w.window.name;
        let check = &w.check;
        for (label, value) in [
            ("judged", u64::from(check.judged())),
            ("rows", w.spans as u64),
            ("units", check.units.len() as u64),
            ("lost", check.lost as u64),
            ("unmatched", check.unmatched as u64),
            ("units not fully known", check.unknown.len() as u64),
            ("set-aside WAL overlaps", u64::from(check.stale)),
        ] {
            counts.insert(format!("{name} {label}"), value);
        }
        summary
            .windows
            .push((w.window.grid.after_s, w.window.grid.before_s));
        for (check, count) in &w.checks {
            let total = summary.checks.entry(check.clone()).or_default();
            total.compared += count.compared;
            total.differing += count.differing;
        }
        findings.extend(w.findings.iter().cloned());
        sensitive.extend(w.sensitive.iter().cloned());
    }
    summary.counts = counts;

    let mut aliases = Aliases::default();
    let (mut markdown, summary) = report::render(&summary, &findings, first_n, &mut aliases);
    let mut moved: Vec<(String, &Vec<Change>)> = Vec::new();
    for (index, changes) in outcome.retries.iter().enumerate() {
        moved.push((format!("Attempt {} retried", index + 1), changes));
    }
    if let Some(changes) = &outcome.gave_up {
        moved.push(("Gave up".to_string(), changes));
    }
    for (title, changes) in moved {
        markdown.push_str(&format!("\n## {title}\n\n"));
        for change in changes {
            markdown.push_str(&format!("- {change}\n"));
        }
    }
    let leaked = report::leaks(&markdown, &sensitive);
    if !leaked.is_empty() {
        return Err(leaked);
    }
    Ok(Report {
        markdown,
        summary,
        aliases: aliases.map(),
    })
}
