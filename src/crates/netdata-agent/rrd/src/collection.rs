//! The v1 data-collection math of a chart, ported from `src/database/rrdset-collection.c`: the collection clock
//! (`rrdset_timed_next()` and the alignment helpers). `rrdset_timed_done()` and its interpolation follow in this
//! module; the C unit tests of `src/daemon/unit_test.c` are the ground truth.

use crate::chart::{Algorithm, Chart, ChartCollection, DimCollection, dim_flags, flags};
use crate::contexts;
use crate::mode::DbMode;

use netdata_agent_storage::storage_number::{SN_DEFAULT_FLAGS, SN_FLAG_RESET};

const USEC_PER_SEC: i64 = 1_000_000;

fn as_ut(t: (i64, i64)) -> i64 {
    t.0 * USEC_PER_SEC + t.1
}

/// `last_collected_time_align()`.
fn last_collected_time_align(c: &mut ChartCollection, update_every: i64, store_first: bool) {
    c.last_collected.0 -= c.last_collected.0 % update_every;
    c.last_collected.1 = if store_first { 0 } else { 500_000 };
}

/// `last_updated_time_align()`.
fn last_updated_time_align(c: &mut ChartCollection, update_every: i64) {
    c.last_updated.0 -= c.last_updated.0 % update_every;
    c.last_updated.1 = 0;
}

/// `rrdset_timed_next()`: the time since the last collection that the next `done` will use. A pending clock sync,
/// the first collection, a missing duration, a clock in the future or a gap over five intervals replace what the
/// collector said.
pub fn timed_next(chart: &Chart, now: (i64, i64), mut duration_since_last_update: u64) {
    let update_every = i64::from(chart.update_every());
    let sync_clock = chart.update_meta(|m| {
        let set = m.flags & flags::SYNC_CLOCK != 0;
        m.flags &= !flags::SYNC_CLOCK;
        set
    });
    if sync_clock {
        duration_since_last_update = 0;
    }
    chart.update_collection(|c| {
        if c.last_collected.0 == 0 {
            duration_since_last_update = (update_every * USEC_PER_SEC) as u64;
        } else if duration_since_last_update == 0 {
            // dt_usec(): the absolute difference.
            duration_since_last_update = (as_ut(now) - as_ut(c.last_collected)).unsigned_abs();
        } else {
            let since_last = as_ut(now) - as_ut(c.last_collected);
            if since_last < 0 {
                duration_since_last_update = 0;
            } else if since_last as u64 > (update_every * 5 * USEC_PER_SEC) as u64 {
                duration_since_last_update = since_last as u64;
            }
        }
        c.usec_since_last_update = duration_since_last_update;
    });
}

/// `rrdset_init_last_collected_time()`: the first collection lands on the interval's grid (at .5 s, or .0 s with
/// `store_first`).
pub(crate) fn init_last_collected_time(
    c: &mut ChartCollection,
    now: (i64, i64),
    update_every: i64,
    store_first: bool,
) -> i64 {
    c.last_collected = now;
    last_collected_time_align(c, update_every, store_first);
    as_ut(c.last_collected)
}

/// `rrdset_update_last_collected_time()`: returns the previous collection time.
pub(crate) fn update_last_collected_time(c: &mut ChartCollection) -> i64 {
    let last = as_ut(c.last_collected);
    let ut = last + c.usec_since_last_update as i64;
    c.last_collected = (ut / USEC_PER_SEC, ut % USEC_PER_SEC);
    last
}

/// `rrdset_init_last_updated_time()`.
pub(crate) fn init_last_updated_time(
    c: &mut ChartCollection,
    update_every: i64,
    store_first: bool,
) {
    c.last_updated = c.last_collected;
    if store_first {
        c.last_updated.0 -= update_every;
    }
    last_updated_time_align(c, update_every);
}

/// `MAX_INCREMENTAL_PERCENT_RATE`: a decrease of an incremental counter counts as a wrap only below this share.
const MAX_INCREMENTAL_PERCENT_RATE: u64 = 10;

/// `rrdset_collection_reset()`.
fn collection_reset(chart: &Chart) {
    chart.update_collection(|c| {
        c.last_collected = (0, 0);
        c.last_updated = (0, 0);
        c.current_entry = 0;
        c.counter = 0;
        c.counter_done = 0;
    });
    for dim in chart.dims() {
        dim.update_collection(|d| {
            d.last_collected_time = (0, 0);
            d.counter = 0;
        });
        if let Some(ring) = dim.ring() {
            ring.flush();
        }
    }
}

fn collected_as_double(d: &DimCollection, is_float: bool) -> f64 {
    if is_float {
        d.collected_value_float
    } else {
        d.collected_value as f64
    }
}

fn last_collected_as_double(d: &DimCollection, is_float: bool) -> f64 {
    if is_float {
        d.last_collected_value_float
    } else {
        d.last_collected_value as f64
    }
}

/// `rrdset_timed_done()`: turns the values collected since the last call into stored points on the update grid,
/// interpolating between collections. `gap_when_lost_iterations_above` is `[db] gap when lost iterations above`.
pub fn timed_done(
    chart: &Chart,
    now: (i64, i64),
    pending_next: bool,
    gap_when_lost_iterations_above: i64,
) {
    if pending_next {
        timed_next(chart, now, 0);
    }
    let meta = chart.meta();
    let update_every = i64::from(meta.update_every);
    let update_every_ut = update_every * USEC_PER_SEC;
    let store_first = meta.flags & flags::STORE_FIRST != 0;
    let entries = chart.entries() as i64;
    let max_update_gap_iterations = entries.max(60);
    let max_update_gap_ut = max_update_gap_iterations * update_every_ut;
    chart.isnot_obsolete();
    let mut store_this_entry = true;
    let mut first_entry = false;

    if chart.collection().usec_since_last_update as i64 > max_update_gap_ut {
        collection_reset(chart);
        chart.update_collection(|c| c.usec_since_last_update = update_every_ut as u64);
        store_this_entry = false;
        first_entry = true;
    }
    let mut c = chart.collection();
    let mut last_collect_ut = if c.last_collected.0 == 0 {
        store_this_entry = false;
        first_entry = true;
        init_last_collected_time(&mut c, now, update_every, store_first) - update_every_ut
    } else {
        update_last_collected_time(&mut c)
    };
    if c.last_updated.0 == 0 {
        init_last_updated_time(&mut c, update_every, store_first);
        store_this_entry = false;
        first_entry = true;
    }
    let max_stored_gap_iterations = if chart.mode() == DbMode::Dbengine {
        max_update_gap_iterations
    } else {
        entries
    };
    if (as_ut(c.last_collected) - as_ut(c.last_updated)).abs()
        > max_stored_gap_iterations * update_every_ut
    {
        chart.update_collection(|x| *x = c);
        collection_reset(chart);
        c = chart.collection();
        init_last_updated_time(&mut c, update_every, store_first);
        c.usec_since_last_update = update_every_ut as u64;
        store_this_entry = false;
        first_entry = true;
    }
    let now_collect_ut = as_ut(c.last_collected);
    let mut last_stored_ut = as_ut(c.last_updated);
    let mut next_store_ut = (c.last_updated.0 + update_every) * USEC_PER_SEC;
    if c.counter_done == 0 {
        init_last_updated_time(&mut c, update_every, store_first);
        last_stored_ut = as_ut(c.last_updated);
        next_store_ut = (c.last_updated.0 + update_every) * USEC_PER_SEC;
        if store_first {
            store_this_entry = true;
            last_collect_ut = next_store_ut - update_every_ut;
        } else {
            store_this_entry = false;
        }
    }
    c.counter_done += 1;
    chart.update_collection(|x| *x = c);

    // Per dimension: the totals, then the calculated value of this collection.
    let dims = chart.dims();
    let mut reset_or_overflow = vec![false; dims.len()];
    let mut collected_total = 0.0;
    let mut last_collected_total = 0.0;
    for (i, dim) in dims.iter().enumerate() {
        let m = dim.meta();
        if m.flags & dim_flags::UPDATED == 0 {
            continue;
        }
        let is_float = m.flags & dim_flags::FLOAT != 0;
        dim.update_collection(|d| {
            if m.algorithm == Algorithm::PcentOverDiffTotal
                && last_collected_as_double(d, is_float) > collected_as_double(d, is_float)
            {
                if m.flags & dim_flags::DONT_DETECT_RESETS_OR_OVERFLOWS == 0 {
                    reset_or_overflow[i] = true;
                }
                if is_float {
                    d.last_collected_value_float = d.collected_value_float;
                } else {
                    d.last_collected_value = d.collected_value;
                }
            }
            last_collected_total += last_collected_as_double(d, is_float);
            collected_total += collected_as_double(d, is_float);
        });
        chart.dim_isnot_obsolete(dim);
    }
    for (i, dim) in dims.iter().enumerate() {
        let m = dim.meta();
        let is_float = m.flags & dim_flags::FLOAT != 0;
        let (mul, div) = (f64::from(m.multiplier), f64::from(m.divisor));
        dim.update_collection(|d| {
            if m.flags & dim_flags::UPDATED == 0 {
                d.calculated_value = 0.0;
                return;
            }
            match m.algorithm {
                Algorithm::Absolute => {
                    d.calculated_value = collected_as_double(d, is_float) * mul / div
                }
                Algorithm::PcentOverRowTotal => {
                    d.calculated_value = if collected_total == 0.0 {
                        0.0
                    } else {
                        100.0 * collected_as_double(d, is_float) / collected_total
                    };
                }
                Algorithm::Incremental => {
                    if d.counter <= 1 {
                        d.calculated_value = 0.0;
                        return;
                    }
                    if !is_float {
                        let last = d.last_collected_value as u64;
                        let new = d.collected_value as u64;
                        let max = d.collected_value_max as u64;
                        if last > new {
                            if m.flags & dim_flags::DONT_DETECT_RESETS_OR_OVERFLOWS == 0 {
                                reset_or_overflow[i] = true;
                            }
                            let cap: u64 = if max > 0x0000_0000_FFFF_FFFF {
                                u64::MAX
                            } else {
                                0x0000_0000_FFFF_FFFF
                            };
                            let delta = cap.wrapping_sub(last).wrapping_add(new);
                            let max_acceptable_rate = (cap / 100) * MAX_INCREMENTAL_PERCENT_RATE;
                            if delta < max_acceptable_rate {
                                d.calculated_value += delta as f64 * mul / div;
                            }
                        } else {
                            d.calculated_value +=
                                (new.wrapping_sub(last) as i64) as f64 * mul / div;
                        }
                    } else {
                        let last = last_collected_as_double(d, true);
                        let cur = collected_as_double(d, true);
                        if cur < last {
                            if m.flags & dim_flags::DONT_DETECT_RESETS_OR_OVERFLOWS == 0 {
                                reset_or_overflow[i] = true;
                            }
                        } else {
                            d.calculated_value += (cur - last) * mul / div;
                        }
                    }
                }
                Algorithm::PcentOverDiffTotal => {
                    if d.counter <= 1 {
                        d.calculated_value = 0.0;
                        return;
                    }
                    d.calculated_value = if collected_total == last_collected_total {
                        0.0
                    } else {
                        100.0
                            * (collected_as_double(d, is_float)
                                - last_collected_as_double(d, is_float))
                            / (collected_total - last_collected_total)
                    };
                }
            }
        });
    }

    // rrdset_done_interpolate(): one point per grid step up to this collection.
    let mut iterations = (now_collect_ut - last_stored_ut) / update_every_ut;
    if now_collect_ut % update_every_ut == 0 {
        iterations += 1;
    }
    let mut loop_iterations = if next_store_ut <= now_collect_ut {
        (now_collect_ut - next_store_ut) / update_every_ut + 1
    } else {
        0
    };
    while loop_iterations > 0 && next_store_ut <= now_collect_ut {
        let last_ut = next_store_ut;
        for (i, dim) in dims.iter().enumerate() {
            let m = dim.meta();
            let mut storage_flags = SN_DEFAULT_FLAGS;
            if reset_or_overflow[i] {
                storage_flags |= SN_FLAG_RESET;
            }
            let updated = m.flags & dim_flags::UPDATED != 0;
            let (value, sn_flags) = dim.update_collection(|d| {
                let new_value = match m.algorithm {
                    Algorithm::Incremental => {
                        let mut v = d.calculated_value * (next_store_ut - last_collect_ut) as f64
                            / (now_collect_ut - last_collect_ut) as f64;
                        d.calculated_value -= v;
                        v += d.last_calculated_value;
                        d.last_calculated_value = 0.0;
                        v /= update_every as f64;
                        if next_store_ut - last_stored_ut < update_every_ut {
                            v = v * (update_every * USEC_PER_SEC) as f64
                                / (next_store_ut - last_stored_ut) as f64;
                        }
                        v
                    }
                    _ => {
                        if iterations == 1 {
                            d.calculated_value
                        } else {
                            (d.calculated_value - d.last_calculated_value)
                                * (next_store_ut - last_collect_ut) as f64
                                / (now_collect_ut - last_collect_ut) as f64
                                + d.last_calculated_value
                        }
                    }
                };
                if !store_this_entry {
                    (f64::NAN, 0)
                } else if updated && d.counter > 1 && iterations < gap_when_lost_iterations_above {
                    d.last_stored_value = new_value;
                    (new_value, storage_flags)
                } else {
                    d.last_stored_value = f64::NAN;
                    (f64::NAN, 0)
                }
            });
            dim.store_metric(next_store_ut as u64, value, sn_flags);
        }
        chart.update_collection(|c| {
            c.counter += 1;
            c.current_entry = if c.current_entry + 1 >= entries as usize {
                0
            } else {
                c.current_entry + 1
            };
            c.last_updated = (last_ut / USEC_PER_SEC, 0);
        });
        last_stored_ut = next_store_ut;
        last_collect_ut = next_store_ut;
        next_store_ut += update_every_ut;
        iterations -= 1;
        loop_iterations -= 1;
    }

    // Carry this collection into the next one.
    for dim in &dims {
        let m = dim.meta();
        if m.flags & dim_flags::UPDATED == 0 {
            continue;
        }
        let is_float = m.flags & dim_flags::FLOAT != 0;
        dim.update_collection(|d| {
            if is_float {
                d.last_collected_value_float = d.collected_value_float;
            } else {
                d.last_collected_value = d.collected_value;
            }
            match m.algorithm {
                Algorithm::Incremental => {
                    if !first_entry {
                        d.last_calculated_value += d.calculated_value;
                    }
                }
                _ => d.last_calculated_value = d.calculated_value,
            }
            d.calculated_value = 0.0;
            if is_float {
                d.collected_value_float = 0.0;
            } else {
                d.collected_value = 0;
            }
        });
        dim.update_meta(|m| m.flags &= !dim_flags::UPDATED);
    }
    contexts::collected_rrdset(chart);
}

/// `rrddim_timed_set_by_pointer()`: a collected value for the next `timed_done`.
pub fn set_value(dim: &crate::chart::Dim, collected_time: (i64, i64), value: i64) {
    let is_float = dim.meta().flags & dim_flags::FLOAT != 0;
    dim.update_collection(|d| {
        d.last_collected_time = collected_time;
        if is_float {
            d.collected_value_float = value as f64;
        } else {
            d.collected_value = value;
            let magnitude = value.unsigned_abs();
            if magnitude > d.collected_value_max as u64 {
                d.collected_value_max = if magnitude > i64::MAX as u64 {
                    i64::MIN
                } else {
                    magnitude as i64
                };
            }
        }
        d.counter += 1;
    });
    dim.update_meta(|m| m.flags |= dim_flags::UPDATED);
}

/// `rrddim_set_by_pointer_double()` for a float dimension (an int one goes through `set_value`).
pub fn set_value_float(dim: &crate::chart::Dim, collected_time: (i64, i64), value: f64) {
    dim.update_collection(|d| {
        d.last_collected_time = collected_time;
        d.collected_value_float = value;
        d.counter += 1;
    });
    dim.update_meta(|m| m.flags |= dim_flags::UPDATED);
}

/// `rrdset_next_usec_unfiltered()`: a trusted duration is used as given, unless the clock must be synced first.
pub fn next_usec_unfiltered(chart: &Chart, now: (i64, i64), duration: u64) {
    let sync = chart.meta().flags & flags::SYNC_CLOCK != 0;
    let first = chart.collection().last_collected.0 == 0;
    if first || duration == 0 || sync {
        timed_next(chart, now, duration);
    } else {
        chart.update_collection(|c| c.usec_since_last_update = duration);
    }
}
