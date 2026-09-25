//! Holding the lab still while the runner asks the agent. The tee refuses new
//! exports while its freeze file exists; the runner then waits for what was in
//! flight to land, reads the store and the capture, asks its questions, and
//! reads them again. The answers are judged only if nothing that bears on
//! the windows moved in between.
//!
//! Everything here is decided from observations the runner passes in (clock
//! readings, lengths, store reads), so it can be tested without an agent.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::calc::Grid;
use crate::matching::overlaps;
use crate::membership::{Membership, UnitKind, WalInfo};

/// How often the runner polls while settling.
pub const SETTLE_POLL: Duration = Duration::from_millis(250);
/// How long the lengths must stay unchanged.
pub const SETTLE_STABLE: Duration = Duration::from_secs(2);
/// When settling gives up.
pub const SETTLE_CAP: Duration = Duration::from_secs(30);
/// Snapshot pairs tried before the runner gives up.
pub const MAX_ATTEMPTS: u32 = 3;

/// The agent seals a WAL this long after its first frame, on a sweep that runs
/// this often (the calculator's own copies of the lab agent's settings).
pub const ROTATION_AGE: Duration = Duration::from_secs(15 * 60);
pub const ROTATION_SWEEP: Duration = Duration::from_secs(30);

/// Windows start this long after the capture's first record: a span received
/// before the capture may end up to the ingestion window's future skew later.
pub const CAPTURE_LEAD: Duration = Duration::from_secs(10 * 60);

/// The lengths polled while settling: the capture's and every WAL's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub capture_bytes: u64,
    pub wal_bytes: Vec<(PathBuf, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    Waiting,
    Stable,
    /// Still moving at [`SETTLE_CAP`]: unfreeze and give up.
    TimedOut,
}

/// Waits, once the tee is frozen, for the capture and the WALs to stop
/// growing. Times are readings of one monotonic clock.
#[derive(Debug, Clone)]
pub struct Settle {
    started: Duration,
    /// The last probe and since when it has been unchanged.
    last: Option<(Probe, Duration)>,
}

impl Settle {
    pub fn new(now: Duration) -> Settle {
        Settle {
            started: now,
            last: None,
        }
    }

    pub fn observe(&mut self, now: Duration, probe: Probe) -> Settled {
        let since = match &self.last {
            Some((last, since)) if *last == probe => *since,
            _ => now,
        };
        self.last = Some((probe, since));
        if now.saturating_sub(since) >= SETTLE_STABLE {
            Settled::Stable
        } else if now.saturating_sub(self.started) >= SETTLE_CAP {
            Settled::TimedOut
        } else {
            Settled::Waiting
        }
    }
}

/// Whether the agent may seal a WAL before a freeze of `longest_freeze`
/// starting at `now_ns` ends, with a sweep's margin. The runner waits for the
/// rotation instead of freezing across it.
pub fn rotation_due(wals: &[WalInfo], now_ns: u64, longest_freeze: Duration) -> bool {
    let margin = longest_freeze + ROTATION_SWEEP;
    let margin_ns = u64::try_from(margin.as_nanos()).unwrap_or(u64::MAX);
    let age_ns = u64::try_from(ROTATION_AGE.as_nanos()).unwrap_or(u64::MAX);
    for wal in wals {
        if let Some((first_ns, _)) = wal.frame_times {
            let sealed_at = first_ns.saturating_add(age_ns);
            if sealed_at <= now_ns.saturating_add(margin_ns) {
                return true;
            }
        }
    }
    false
}

/// A window the runner judges: the one it asks for, and the grid the agent
/// reads it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub name: &'static str,
    pub after_s: u32,
    pub before_s: u32,
    pub grid: Grid,
}

/// The windows a freeze at `freeze_s` can judge: the last 15 minutes (the live
/// WAL's chunks and tail), 15 minutes ending 20 minutes earlier (sealed files,
/// as a rule) and the last hour; each only when its grid starts
/// [`CAPTURE_LEAD`] or more after the capture's first record.
pub fn windows(first_record_ns: u64, freeze_s: u32) -> Vec<Window> {
    let lead_ns = u64::try_from(CAPTURE_LEAD.as_nanos()).unwrap_or(u64::MAX);
    let earliest_ns = first_record_ns.saturating_add(lead_ns);
    let candidates = [
        ("W15", 15 * 60, 0),
        ("W-sealed", 15 * 60, 20 * 60),
        ("W60", 60 * 60, 0),
    ];
    let mut out = Vec::new();
    for (name, length_s, ends_before_s) in candidates {
        let Some(before_s) = freeze_s.checked_sub(ends_before_s) else {
            continue;
        };
        let Some(after_s) = before_s.checked_sub(length_s) else {
            continue;
        };
        let grid = Grid::for_window(after_s, before_s);
        if u64::from(grid.after_s) * 1_000_000_000 >= earliest_ns {
            out.push(Window {
                name,
                after_s,
                before_s,
                grid,
            });
        }
    }
    out
}

fn kind_label(kind: &UnitKind) -> &'static str {
    match kind {
        UnitKind::Sealed => "sealed file",
        UnitKind::Chunk(_) => "chunk",
        UnitKind::Tail(_) => "tail",
    }
}

/// What the runner compares before and after its questions: the capture's
/// length, the units any window overlaps (by name, with their row counts),
/// every WAL it read, and the set-aside WALs any window overlaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub capture_bytes: u64,
    units: BTreeMap<String, (&'static str, usize)>,
    wals: BTreeMap<PathBuf, WalInfo>,
    stale: Vec<PathBuf>,
}

impl Snapshot {
    pub fn of(capture_bytes: u64, store: &Membership, windows: &[Window]) -> Snapshot {
        let in_any = |seconds: Option<(u32, u32)>| {
            windows
                .iter()
                .any(|w| overlaps(seconds, w.grid.after_s, w.grid.before_s))
        };
        let mut units = BTreeMap::new();
        for unit in &store.units {
            if in_any(unit.seconds) {
                units.insert(unit.name(), (kind_label(&unit.kind), unit.rows.len()));
            }
        }
        let mut wals = BTreeMap::new();
        for wal in &store.wals {
            wals.insert(wal.path.clone(), wal.clone());
        }
        let mut stale = Vec::new();
        for wal in &store.stale_wals {
            if windows
                .iter()
                .any(|w| wal.overlaps(w.grid.after_s, w.grid.before_s))
            {
                stale.push(wal.path.clone());
            }
        }
        Snapshot {
            capture_bytes,
            units,
            wals,
            stale,
        }
    }
}

/// One difference between two snapshots, described without file names (they
/// carry machine ids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Capture {
        from: u64,
        to: u64,
    },
    Unit {
        kind: &'static str,
        from: Option<usize>,
        to: Option<usize>,
    },
    Wal {
        from: Option<(usize, u64)>,
        to: Option<(usize, u64)>,
    },
    Stale {
        from: usize,
        to: usize,
    },
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rows = |n: &Option<usize>| match n {
            Some(n) => format!("{n} rows"),
            None => "none".to_string(),
        };
        let wal = |n: &Option<(usize, u64)>| match n {
            Some((frames, bytes)) => format!("{frames} frames, {bytes} bytes"),
            None => "none".to_string(),
        };
        match self {
            Change::Capture { from, to } => write!(f, "capture {from} -> {to} bytes"),
            Change::Unit { kind, from, to } => {
                write!(f, "a {kind}: {} -> {}", rows(from), rows(to))
            }
            Change::Wal { from, to } => write!(f, "a WAL: {} -> {}", wal(from), wal(to)),
            Change::Stale { from, to } => write!(f, "set-aside WALs {from} -> {to}"),
        }
    }
}

pub fn changes(before: &Snapshot, after: &Snapshot) -> Vec<Change> {
    let mut out = Vec::new();
    if before.capture_bytes != after.capture_bytes {
        out.push(Change::Capture {
            from: before.capture_bytes,
            to: after.capture_bytes,
        });
    }

    for (name, (kind, rows)) in &before.units {
        match after.units.get(name) {
            Some((_, now)) if now == rows => {}
            now => out.push(Change::Unit {
                kind,
                from: Some(*rows),
                to: now.map(|(_, rows)| *rows),
            }),
        }
    }
    for (name, (kind, rows)) in &after.units {
        if !before.units.contains_key(name) {
            out.push(Change::Unit {
                kind,
                from: None,
                to: Some(*rows),
            });
        }
    }

    let shape = |wal: &WalInfo| (wal.frames, wal.bytes);
    for (path, wal) in &before.wals {
        match after.wals.get(path) {
            Some(now) if now == wal => {}
            now => out.push(Change::Wal {
                from: Some(shape(wal)),
                to: now.map(shape),
            }),
        }
    }
    for (path, wal) in &after.wals {
        if !before.wals.contains_key(path) {
            out.push(Change::Wal {
                from: None,
                to: Some(shape(wal)),
            });
        }
    }

    if before.stale != after.stale {
        out.push(Change::Stale {
            from: before.stale.len(),
            to: after.stale.len(),
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing moved: judge the answers.
    Proceed,
    /// Something moved: take the snapshots and ask again.
    Retry(Vec<Change>),
    /// Something moved on the last attempt: report it, judge nothing.
    GiveUp(Vec<Change>),
}

/// The decision after attempt `attempt` (from 1).
pub fn decide(before: &Snapshot, after: &Snapshot, attempt: u32) -> Decision {
    let moved = changes(before, after);
    if moved.is_empty() {
        Decision::Proceed
    } else if attempt < MAX_ATTEMPTS {
        Decision::Retry(moved)
    } else {
        Decision::GiveUp(moved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::{RowKey, StaleWal, Stem, Unit};

    const STEM: &str = "baf93e178ba34a37bdf778e397883163-d5439cb5a64e4ceb89c297cbcd8ee6f2-00001-0000000079-0000000000000000";
    // On every window's grid, so the windows are exactly their lengths.
    const FREEZE_S: u32 = 1_790_000_040;
    const MINUTE_NS: u64 = 60_000_000_000;

    fn probe(capture_bytes: u64) -> Probe {
        Probe {
            capture_bytes,
            wal_bytes: vec![(PathBuf::from("a.wal"), 10)],
        }
    }

    #[test]
    fn settling_waits_for_two_quiet_seconds_and_gives_up_at_thirty() {
        let ms = Duration::from_millis;
        let run = |polls: &[(u64, u64)]| {
            let mut settle = Settle::new(ms(0));
            let mut out = Vec::new();
            for (at, bytes) in polls {
                out.push(settle.observe(ms(*at), probe(*bytes)));
            }
            out
        };
        use Settled::{Stable, TimedOut, Waiting};

        assert_eq!(
            run(&[(0, 1), (1_000, 1), (1_999, 1), (2_000, 1)]),
            [Waiting, Waiting, Waiting, Stable]
        );
        assert_eq!(
            run(&[(0, 1), (1_500, 2), (3_499, 2), (3_500, 2)]),
            [Waiting, Waiting, Waiting, Stable]
        );
        let mut moving: Vec<(u64, u64)> = (0..120).map(|i| (i * 250, i)).collect();
        moving.push((30_000, 120));
        let answers = run(&moving);
        assert!(answers[..120].iter().all(|a| *a == Waiting));
        assert_eq!(answers[120], TimedOut);

        let mut settle = Settle::new(ms(0));
        settle.observe(ms(0), probe(1));
        let mut other_wal = probe(1);
        other_wal.wal_bytes[0].1 = 11;
        assert_eq!(settle.observe(ms(2_000), other_wal), Waiting);
    }

    fn wal(first_ns: Option<u64>) -> WalInfo {
        WalInfo {
            path: PathBuf::from(format!("{STEM}.wal")),
            bytes: 100,
            frames: usize::from(first_ns.is_some()),
            entries: 5,
            frame_times: first_ns.map(|ns| (ns, ns)),
        }
    }

    #[test]
    fn a_rotation_is_due_when_the_first_frame_ages_out_during_the_freeze() {
        let now = u64::from(FREEZE_S) * 1_000_000_000;
        let freeze = Duration::from_secs(120);
        let margin = (ROTATION_AGE - freeze - ROTATION_SWEEP).as_nanos() as u64;
        let cases = [
            (
                "the first frame exactly that old",
                vec![wal(Some(now - margin))],
                true,
            ),
            ("1 ns younger", vec![wal(Some(now - margin + 1))], false),
            ("an empty WAL", vec![wal(None)], false),
            (
                "one of two",
                vec![wal(Some(now)), wal(Some(now - 20 * MINUTE_NS))],
                true,
            ),
            ("no WAL", vec![], false),
        ];
        for (name, wals, due) in cases {
            assert_eq!(rotation_due(&wals, now, freeze), due, "{name}");
        }
    }

    #[test]
    fn windows_start_ten_minutes_into_the_capture_on_their_grid() {
        let names = |first_ns: u64, freeze_s: u32| {
            windows(first_ns, freeze_s)
                .iter()
                .map(|w| w.name)
                .collect::<Vec<_>>()
        };
        let ago = |minutes: u64| u64::from(FREEZE_S) * 1_000_000_000 - minutes * MINUTE_NS;
        assert_eq!(names(ago(70), FREEZE_S), ["W15", "W-sealed", "W60"]);
        assert_eq!(names(ago(69), FREEZE_S), ["W15", "W-sealed"]);
        assert_eq!(names(ago(45), FREEZE_S), ["W15", "W-sealed"]);
        assert_eq!(names(ago(44), FREEZE_S), ["W15"]);
        assert_eq!(names(ago(24), FREEZE_S), Vec::<&str>::new());

        let freeze_s = FREEZE_S + 7;
        let w15 = Grid::for_window(freeze_s - 900, freeze_s);
        assert!(w15.after_s < freeze_s - 900);
        let lead = CAPTURE_LEAD.as_nanos() as u64;
        let on_grid = u64::from(w15.after_s) * 1_000_000_000 - lead;
        assert_eq!(names(on_grid, freeze_s), ["W15"]);
        assert_eq!(names(on_grid + 1, freeze_s), Vec::<&str>::new());
        let w = windows(on_grid, freeze_s)[0];
        assert_eq!(
            (w.after_s, w.before_s, w.grid),
            (freeze_s - 900, freeze_s, w15)
        );
    }

    /// A unit of the store file with sequence number `seq`.
    fn unit(seq: u32, kind: UnitKind, first_s: u32, rows: usize) -> Unit {
        let stem = STEM.replace("0000000079", &format!("{seq:010}"));
        let key = |i: usize| RowKey {
            trace_id: [1; 16],
            span_id: [i as u8; 8],
            start_ns: i64::from(first_s) * 1_000_000_000,
            duration_ns: 1,
        };
        let extension = match kind {
            UnitKind::Sealed => "sfst",
            _ => "wal",
        };
        Unit {
            path: PathBuf::from(format!("{stem}.{extension}")),
            stem: Stem::parse(&stem).unwrap(),
            kind,
            seconds: Some((first_s, first_s + 60)),
            rows: (0..rows).map(key).collect(),
        }
    }

    /// A store with an old sealed file outside the window, a sealed file and a
    /// chunk inside it, a tail and its WAL.
    fn store() -> Membership {
        Membership {
            units: vec![
                unit(1, UnitKind::Sealed, FREEZE_S - 7_200, 4),
                unit(2, UnitKind::Sealed, FREEZE_S - 600, 5),
                unit(3, UnitKind::Chunk(0), FREEZE_S - 300, 6),
                unit(3, UnitKind::Tail(3), FREEZE_S - 100, 2),
            ],
            wals: vec![wal(Some(u64::from(FREEZE_S - 300) * 1_000_000_000))],
            stale_wals: Vec::new(),
            legacy_files: Vec::new(),
        }
    }

    #[test]
    fn only_what_bears_on_the_windows_counts_as_moved() {
        let windows = [Window {
            name: "W15",
            after_s: FREEZE_S - 900,
            before_s: FREEZE_S,
            grid: Grid::for_window(FREEZE_S - 900, FREEZE_S),
        }];
        let before = Snapshot::of(1_000, &store(), &windows);
        let snapshot = |edit: &dyn Fn(&mut Membership), capture: u64| {
            let mut store = store();
            edit(&mut store);
            Snapshot::of(capture, &store, &windows)
        };

        let old_file_gone = snapshot(&|s| drop(s.units.remove(0)), 1_000);
        assert_eq!(decide(&before, &old_file_gone, 1), Decision::Proceed);

        let grew = snapshot(&|_| {}, 1_200);
        let capture = vec![Change::Capture {
            from: 1_000,
            to: 1_200,
        }];
        assert_eq!(decide(&before, &grew, 1), Decision::Retry(capture.clone()));
        assert_eq!(decide(&before, &grew, 3), Decision::GiveUp(capture));

        let tail_grew = snapshot(
            &|s| {
                let first = s.units[3].rows[0];
                s.units[3].rows.push(first);
                s.wals[0].frames += 1;
                s.wals[0].bytes += 50;
            },
            1_000,
        );
        assert_eq!(
            changes(&before, &tail_grew),
            [
                Change::Unit {
                    kind: "tail",
                    from: Some(2),
                    to: Some(3)
                },
                Change::Wal {
                    from: Some((1, 100)),
                    to: Some((2, 150))
                },
            ]
        );

        let sealed = snapshot(
            &|s| {
                s.units.truncate(2);
                s.units.push(unit(3, UnitKind::Sealed, FREEZE_S - 300, 8));
                s.wals.clear();
            },
            1_000,
        );
        let moved = changes(&before, &sealed);
        let text: Vec<String> = moved.iter().map(|c| c.to_string()).collect();
        assert_eq!(
            text,
            [
                "a chunk: 6 rows -> none",
                "a tail: 2 rows -> none",
                "a sealed file: none -> 8 rows",
                "a WAL: 1 frames, 100 bytes -> none",
            ]
        );

        let stale = snapshot(
            &|s| {
                s.stale_wals.push(StaleWal {
                    path: PathBuf::from("old.wal"),
                    seconds: None,
                    readable: false,
                })
            },
            1_000,
        );
        assert_eq!(changes(&before, &stale), [Change::Stale { from: 0, to: 1 }]);
    }
}
