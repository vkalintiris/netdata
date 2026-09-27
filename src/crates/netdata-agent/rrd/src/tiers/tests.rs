use super::*;
use netdata_agent_storage::storage_number::SN_DEFAULT_FLAGS;

/// A multiple of 60 and of 3600.
const T0: i64 = 1_790_179_200;
/// A modulo no point start in these tests is a multiple of.
const NEVER: u16 = 65_521;

fn point(end_s: i64, value: f64) -> StoragePoint {
    collected_point(end_s as u64 * 1_000_000, value, SN_DEFAULT_FLAGS, 1)
}

fn gap(end_s: i64) -> StoragePoint {
    StoragePoint {
        sum: f64::NAN,
        min: f64::NAN,
        max: f64::NAN,
        ..point(end_s, 0.0)
    }
}

/// Stores the points ending at each second of `ends`, value `f(end)`; the records written, in order.
fn feed(
    r: &mut Rollup,
    ends: impl IntoIterator<Item = i64>,
    f: impl Fn(i64) -> f64,
) -> Vec<TierRecord> {
    ends.into_iter()
        .filter_map(|t| r.store(1, point(t, f(t))))
        .collect()
}

fn record(end_time_s: i64, values: &[f64]) -> TierRecord {
    TierRecord {
        end_time_s,
        sum: values.iter().sum(),
        min: values.iter().copied().fold(f64::INFINITY, f64::min),
        max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        count: values.len() as u16,
        anomaly_count: 0,
        flags: SN_DEFAULT_FLAGS,
    }
}

fn values(from: i64, to: i64) -> Vec<f64> {
    (from..=to).map(|t| (t % 100) as f64).collect()
}

/// C's `rrddim_collection_modulo()`.
#[test]
fn flush_modulos_are_cs() {
    assert_eq!(flush_modulo(0, 60), 1);
    assert_eq!(flush_modulo(61, 60), 2);
    assert_eq!(flush_modulo(7, 3600), 8);
    assert_eq!(flush_modulo(65_534, 0), 65_535);
    assert_eq!(flush_modulo(70, 100_000), 71);
}

/// The point of a collected value: one update every long, anomalous when its flags say so.
#[test]
fn collected_points_are_cs() {
    let p = collected_point(T0 as u64 * 1_000_000 + 999_999, 2.5, 0, 5);
    assert_eq!(
        (
            p.start_time_s,
            p.end_time_s,
            p.sum,
            p.min,
            p.max,
            p.count,
            p.anomaly_count
        ),
        (T0 - 5, T0, 2.5, 2.5, 2.5, 1, 1)
    );
    assert_eq!(point(T0, 1.0).anomaly_count, 0);
}

/// A window ends at the next multiple of `update every × grouping` after the first point; it closes when a point
/// starts at or after its end, and is written when the next one closes. A first point on a boundary makes the first
/// window hold one point more (spec B1 §2.2).
#[test]
fn windows_close_at_multiples_and_wait_for_the_next() {
    let f = |t: i64| (t % 100) as f64;
    let mut r = Rollup::new(60, NEVER);
    // the first point ends on a boundary: the window ends a minute later and holds 61 points
    assert!(feed(&mut r, T0..=T0 + 60, f).is_empty());
    // the first point of the next window parks the first; nothing is written yet
    assert!(feed(&mut r, [T0 + 61], f).is_empty());
    assert!(feed(&mut r, T0 + 62..=T0 + 120, f).is_empty());
    // the third window's first point writes the first window and parks the second
    assert_eq!(
        feed(&mut r, [T0 + 121], f),
        [record(T0 + 60, &values(T0, T0 + 60))]
    );
    // the collection ends: the parked window is written, the one being filled is lost
    assert_eq!(
        r.flush(),
        Some(record(T0 + 120, &values(T0 + 61, T0 + 120)))
    );
    assert_eq!(r.flush(), None);
}

/// A parked window is written at the first point whose start is a multiple of the modulo.
#[test]
fn parked_windows_are_written_at_their_modulo() {
    let f = |t: i64| (t % 100) as f64;
    let mut r = Rollup::new(60, 7);
    let first = T0 + 1;
    assert!(feed(&mut r, first..=T0 + 60, f).is_empty());
    assert!(feed(&mut r, [T0 + 61], f).is_empty());
    // the next start that is a multiple of 7
    let at = (T0 + 61..).find(|t| t % 7 == 0).unwrap();
    assert!(feed(&mut r, T0 + 62..=at, f).is_empty());
    assert_eq!(
        feed(&mut r, [at + 1], f),
        [record(T0 + 60, &values(first, T0 + 60))]
    );
    assert_eq!(r.flush(), None);
}

/// Gaps are not aggregated: a window of gaps is written empty (count 0), and a window without any point is not
/// written at all.
#[test]
fn gaps_and_missing_windows() {
    let mut r = Rollup::new(60, NEVER);
    for t in T0 + 1..=T0 + 60 {
        assert_eq!(r.store(1, gap(t)), None);
    }
    // a point two windows later: the window of gaps parks, the empty minute never exists
    assert_eq!(r.store(1, point(T0 + 181, 4.0)), None);
    let empty = r.flush().unwrap();
    assert_eq!(
        (
            empty.end_time_s,
            empty.count,
            empty.anomaly_count,
            empty.flags
        ),
        (T0 + 60, 0, 0, 0)
    );
    assert!(empty.sum.is_nan() && empty.min.is_nan() && empty.max.is_nan());
    // gaps inside a window leave the other points' aggregate
    for (t, v) in [(T0 + 182, f64::NAN), (T0 + 183, 6.0)] {
        let p = if v.is_nan() { gap(t) } else { point(t, v) };
        assert_eq!(r.store(1, p), None);
    }
    assert_eq!(r.store(1, point(T0 + 241, 1.0)), None);
    assert_eq!(r.flush(), Some(record(T0 + 240, &[4.0, 6.0])));
}

/// Counts add in 32 bits and are narrowed to 16 when written, as `storage_engine_store_metric()` takes them.
#[test]
fn counts_wrap_at_65536() {
    let mut r = Rollup::new(70_000, NEVER);
    let base = 1_790_040_000; // a multiple of 70,000
    for t in base + 1..=base + 65_537 {
        assert_eq!(r.store(1, point(t, 1.0)), None);
    }
    assert_eq!(r.store(1, point(base + 70_001, 1.0)), None);
    let written = r.flush().unwrap();
    assert_eq!((written.count, written.sum), (1, 65_537.0));
}

/// Anomalous points count; the window's flags are the points' flags or-ed.
#[test]
fn anomalies_and_flags_aggregate() {
    let mut r = Rollup::new(60, NEVER);
    for t in T0 + 1..=T0 + 60 {
        let flags = if t % 10 == 0 { 0 } else { SN_DEFAULT_FLAGS };
        r.store(1, collected_point(t as u64 * 1_000_000, 1.0, flags, 1));
    }
    r.store(1, point(T0 + 61, 1.0));
    let written = r.flush().unwrap();
    assert_eq!(
        (written.count, written.anomaly_count, written.flags),
        (60, 6, SN_DEFAULT_FLAGS)
    );
}

/// A new update every keeps the window being filled and its end; the windows after it follow the new update every
/// (spec B1 §7.1).
#[test]
fn a_new_update_every_honours_the_boundary_once() {
    let mut r = Rollup::new(60, NEVER);
    for t in T0 + 1..=T0 + 30 {
        r.store(1, point(t, 1.0));
    }
    // every 5 s from here: the current window still ends at T0 + 60
    let every5 = |t: i64| collected_point(t as u64 * 1_000_000, 1.0, SN_DEFAULT_FLAGS, 5);
    for t in (T0 + 35..=T0 + 60).step_by(5) {
        assert_eq!(r.store(5, every5(t)), None);
    }
    // the point starting at the boundary completes it; the next window is 5 × 60 s long
    assert_eq!(r.store(5, every5(T0 + 65)), None);
    let next_end = T0 + 300;
    for t in (T0 + 70..=next_end).step_by(5) {
        assert_eq!(r.store(5, every5(t)), None);
    }
    assert_eq!(
        r.store(5, every5(next_end + 5))
            .map(|w| (w.end_time_s, w.count)),
        Some((T0 + 60, 36))
    );
    assert_eq!(
        r.flush().map(|w| (w.end_time_s, w.count)),
        Some((next_end, 48))
    );
}
