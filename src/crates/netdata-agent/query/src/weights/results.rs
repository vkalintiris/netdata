//! The results of a weights request (`struct register_result` and `register_result()`,
//! `src/web/api/queries/weights.c`): one per metric whose method gave a number, in the order the metrics were
//! visited.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use netdata_agent_rrd::contexts::{Context, Instance, Metric};
use netdata_agent_rrd::host::Host;
use netdata_agent_storage::storage_point::StoragePoint;
use netdata_agent_text::parse::uuid_parse_flexi;

use super::methods::Stats;

/// `RESULT_FLAGS`.
pub mod flags {
    pub const BASE_HIGH_RATIO: u32 = 1 << 0;
    pub const PERCENTAGE_OF_TIME: u32 = 1 << 1;
}

/// The metric a method works on, with its host's name as the walk took it (`qwd->host_snapshot->hostname`).
#[derive(Clone, Copy)]
pub struct Of<'a> {
    pub host: &'a Arc<Host>,
    pub hostname: &'a str,
    pub context: &'a Arc<Context>,
    pub instance: &'a Arc<Instance>,
    pub metric: &'a Arc<Metric>,
}

/// What a method found for a metric (the arguments of `register_result()` that are the result itself). A point C
/// is given no pointer for stays zeroed.
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub value: f64,
    pub flags: u32,
    pub highlighted: Option<StoragePoint>,
    pub baseline: Option<StoragePoint>,
    pub duration_us: u64,
}

/// `struct register_result`.
#[derive(Debug, Clone)]
pub struct Registered {
    pub flags: u32,
    pub host: Arc<Host>,
    pub hostname: String,
    pub context: Arc<Context>,
    pub instance: Arc<Instance>,
    pub metric: Arc<Metric>,
    pub value: f64,
    pub highlighted: StoragePoint,
    pub baseline: StoragePoint,
    pub duration_us: u64,
    pub selected: bool,
    pub instance_selected: bool,
    pub context_selected: bool,
    pub node_selected: bool,
}

/// `register_result()`: a value that is no number is dropped; the others are kept without their sign, unless they
/// are zero and zeros do not count. The largest value that is a ratio of the baseline to the highlight is tracked
/// for the spreading. C keys its dictionary by the metric's address and a walk visits a metric once, so a result
/// is appended.
pub fn register(results: &mut Vec<Registered>, stats: &mut Stats, register_zero: bool, of: &Of, found: Found) {
    if !found.value.is_finite() {
        return;
    }
    let value = found.value.abs();
    // FP_ZERO: a subnormal is not zero
    if value == 0.0 && !register_zero {
        return;
    }
    if found.flags & flags::BASE_HIGH_RATIO != 0 && value > stats.max_base_high_ratio {
        stats.max_base_high_ratio = value;
    }
    results.push(Registered {
        flags: found.flags,
        host: Arc::clone(of.host),
        hostname: of.hostname.to_owned(),
        context: Arc::clone(of.context),
        instance: Arc::clone(of.instance),
        metric: Arc::clone(of.metric),
        value,
        highlighted: found.highlighted.unwrap_or_default(),
        baseline: found.baseline.unwrap_or_default(),
        duration_us: found.duration_us,
        selected: false,
        instance_selected: false,
        context_selected: false,
        node_selected: false,
    });
}

/// `spread_results_evenly()`: every result becomes its rank among the distinct values, as a share of 1 turned
/// around, so the strongest is 0 and equal values stay equal. First a value that is a share of the time is
/// scaled by the largest ratio (1 when there is none). Returns how many results there are.
pub fn spread(results: &mut [Registered], stats: &mut Stats) -> usize {
    if results.is_empty() {
        return 0;
    }
    if stats.max_base_high_ratio == 0.0 {
        stats.max_base_high_ratio = 1.0;
    }
    let mut slots = Vec::with_capacity(results.len());
    for t in results.iter_mut() {
        if t.flags & flags::PERCENTAGE_OF_TIME != 0 {
            t.value *= stats.max_base_high_ratio;
        }
        slots.push(t.value);
    }
    // the values are finite and not negative: a total order
    slots.sort_by(f64::total_cmp);
    slots.dedup();
    let slot_weight = 1.0 / slots.len() as f64;
    for t in results.iter_mut() {
        // the first distinct value greater than this one
        let slot = slots.partition_point(|value| *value <= t.value);
        let v = (slot as f64 * slot_weight).min(1.0);
        t.value = 1.0 - v;
    }
    results.len()
}

/// The bytes C orders hosts by: the host's id (its machine GUID), or its node id when that is nil.
fn host_order(host: &Host) -> [u8; 16] {
    match uuid_parse_flexi(host.machine_guid().as_bytes()) {
        Some(id) if id != [0; 16] => id,
        _ => host.node_id(),
    }
}

/// `weights_result_compare()`: by value, the smaller first when the values were spread (`normalized`: the
/// strongest is 0), the larger first otherwise; then by the host's bytes, the context's id, the instance's and
/// the metric's.
pub fn compare(a: &Registered, b: &Registered, normalized: bool) -> Ordering {
    if a.value != b.value {
        return if (a.value < b.value) == normalized { Ordering::Less } else { Ordering::Greater };
    }
    host_order(&a.host)
        .cmp(&host_order(&b.host))
        .then_with(|| a.context.id().as_bytes().cmp(b.context.id().as_bytes()))
        .then_with(|| a.instance.id().as_bytes().cmp(b.instance.id().as_bytes()))
        .then_with(|| a.metric.id().as_bytes().cmp(b.metric.id().as_bytes()))
}

/// `weights_select_results()` with its parents marked: the first `limit` results of [`compare`]'s order are
/// selected, and every result says whether its host, its context and its instance hold a selected one (by
/// identity, as C keys them by address). A limit that covers every result selects all of them.
pub fn select(results: &mut [Registered], limit: usize, normalized: bool) {
    if limit >= results.len() {
        for t in results.iter_mut() {
            (t.selected, t.instance_selected, t.context_selected, t.node_selected) = (true, true, true, true);
        }
        return;
    }
    let mut order: Vec<usize> = (0..results.len()).collect();
    order.sort_by(|&a, &b| compare(&results[a], &results[b], normalized));
    order.truncate(limit);
    let (mut nodes, mut contexts, mut instances) = (HashSet::new(), HashSet::new(), HashSet::new());
    for &i in &order {
        let t = &mut results[i];
        t.selected = true;
        nodes.insert(Arc::as_ptr(&t.host));
        contexts.insert(Arc::as_ptr(&t.context));
        instances.insert(Arc::as_ptr(&t.instance));
    }
    for t in results.iter_mut() {
        t.node_selected = nodes.contains(&Arc::as_ptr(&t.host));
        t.context_selected = contexts.contains(&Arc::as_ptr(&t.context));
        t.instance_selected = instances.contains(&Arc::as_ptr(&t.instance));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::weights_host;

    /// Results of the fixture's metrics on two hosts, with the values given: (host, metric, value, flags).
    fn results_of(values: &[(usize, &str, f64, u32)]) -> (Vec<Registered>, Stats) {
        use crate::testing::weights_host_as;
        let hosts = [
            weights_host_as("11111111-1111-1111-1111-111111111111", "one"),
            weights_host_as("22222222-2222-2222-2222-222222222222", "two"),
        ];
        let (mut results, mut stats) = (Vec::new(), Stats::default());
        for &(host, dimension, value, flags) in values {
            let h = &hosts[host];
            let rc = h.contexts().get("ctx.w").expect("the fixture's context");
            let ri = rc.instances().into_iter().next().expect("its instance");
            let rm = ri.metric(dimension).expect("the metric");
            let of = Of { host: h, hostname: "h", context: &rc, instance: &ri, metric: &rm };
            let found = Found { value, flags, highlighted: None, baseline: None, duration_us: 0 };
            register(&mut results, &mut stats, true, &of, found);
        }
        (results, stats)
    }

    fn values_of(results: &[Registered]) -> Vec<f64> {
        results.iter().map(|t| t.value).collect()
    }

    /// `spread_results_evenly()`: the rank among the distinct values, turned around: the strongest is 0, equal
    /// values stay equal, one result alone is 0. A share of the time is first scaled by the largest ratio.
    #[test]
    fn the_results_are_spread_by_their_rank_among_the_distinct_values() {
        let spread_of = |values: &[(usize, &str, f64, u32)]| {
            let (mut results, mut stats) = results_of(values);
            let count = spread(&mut results, &mut stats);
            (values_of(&results), count, stats.max_base_high_ratio)
        };
        let rate = 12000.0 / 121.0;
        assert_eq!(spread_of(&[(0, "a", rate, 0)]), (vec![0.0], 1, 1.0));
        let zeros = [(0, "a", 0.0, 0), (0, "b", 0.0, 0), (0, "z", 0.0, 0), (0, "step", rate, 0)];
        assert_eq!(spread_of(&zeros).0, [0.5, 0.5, 0.5, 0.0]);
        assert_eq!(spread_of(&[(0, "a", 1.0, 0), (0, "b", 1.0, 0), (0, "z", 0.25, 0)]).0, [0.0, 0.0, 0.5]);
        // four distinct values: quarters
        let four = [(0, "a", 3.0, 0), (0, "b", 1.0, 0), (0, "z", 4.0, 0), (0, "step", 2.0, 0)];
        assert_eq!(spread_of(&four).0, [0.25, 0.75, 0.0, 0.5]);
        // a share of the time is scaled by the largest ratio before it is ranked: 0.5 * 4 passes the ratio 1.5
        let ratio = flags::BASE_HIGH_RATIO;
        let scaled = [(0, "a", 4.0, ratio), (0, "b", 1.5, ratio), (0, "z", 0.5, flags::PERCENTAGE_OF_TIME)];
        // (C's arithmetic: one minus the slot times the weight of a slot)
        let third = 1.0 / 3.0;
        assert_eq!(spread_of(&scaled), (vec![0.0, 1.0 - 1.0 * third, 1.0 - 2.0 * third], 3, 4.0));
        // no result: nothing, and the largest ratio is left alone
        assert_eq!(spread_of(&[]), (vec![], 0, 0.0));
    }

    /// `weights_result_compare()` and `weights_select_results()`: spread values rank the smallest first, raw ones
    /// the largest first; equal values by the host's id bytes, then the metric's id; the first of that order up
    /// to the limit are selected, and a result says whether its host, context and instance hold a selected one.
    #[test]
    fn the_strongest_results_are_selected_with_their_parents() {
        let values = [(1, "a", 0.5, 0), (0, "b", 0.5, 0), (0, "a", 0.5, 0), (1, "z", 0.25, 0), (0, "step", 0.75, 0)];
        let selected = |limit: usize, normalized: bool| {
            let (mut results, _) = results_of(&values);
            select(&mut results, limit, normalized);
            results.iter().map(|t| (t.selected, t.node_selected, t.instance_selected)).collect::<Vec<_>>()
        };
        let (y, n) = (true, false);
        // spread values: 0.25 of the second host is the strongest; then the three 0.5 by host and metric
        assert_eq!(selected(1, true), [(n, y, y), (n, n, n), (n, n, n), (y, y, y), (n, n, n)]);
        assert_eq!(selected(2, true), [(n, y, y), (n, y, y), (y, y, y), (y, y, y), (n, y, y)]);
        assert_eq!(selected(3, true), [(n, y, y), (y, y, y), (y, y, y), (y, y, y), (n, y, y)]);
        // raw values: 0.75 of the first host is the strongest
        assert_eq!(selected(1, false), [(n, n, n), (n, y, y), (n, y, y), (n, n, n), (y, y, y)]);
        assert_eq!(selected(2, false), [(n, n, n), (n, y, y), (y, y, y), (n, n, n), (y, y, y)]);
        // none, and a limit that covers all
        assert_eq!(selected(0, true), [(n, n, n); 5]);
        for limit in [5, 6, 100] {
            assert_eq!(selected(limit, true), [(y, y, y); 5], "{limit}");
        }
        let (results, _) = results_of(&values);
        assert_eq!(compare(&results[2], &results[1], true), Ordering::Less);
        assert_eq!(compare(&results[1], &results[0], false), Ordering::Less);
        assert_eq!(compare(&results[3], &results[4], true), Ordering::Less);
        assert_eq!(compare(&results[3], &results[4], false), Ordering::Greater);
        assert_eq!(compare(&results[0], &results[0], true), Ordering::Equal);
    }

    /// `register_result()`: what is no number is dropped; the sign goes; a zero (of either sign) is kept only
    /// when zeros count, and a subnormal is no zero; only a base-to-highlight ratio moves the largest ratio; a
    /// point that is not given stays zeroed.
    #[test]
    fn a_result_is_registered_as_c_registers_it() {
        let h = weights_host();
        let rc = h.contexts().get("ctx.w").expect("the fixture's context");
        let ri = rc.instances().into_iter().next().expect("its instance");
        let rm = ri.metric("a").expect("the metric");
        let of = Of { host: &h, hostname: "weights", context: &rc, instance: &ri, metric: &rm };
        let found = |value: f64, flags: u32| Found { value, flags, highlighted: None, baseline: None, duration_us: 3 };
        let registered = |values: &[(f64, u32)], register_zero: bool| {
            let (mut results, mut stats) = (Vec::new(), Stats::default());
            for (value, flags) in values {
                register(&mut results, &mut stats, register_zero, &of, found(*value, *flags));
            }
            (results.iter().map(|t| t.value).collect::<Vec<_>>(), stats.max_base_high_ratio)
        };
        let ratio = flags::BASE_HIGH_RATIO;
        let values =
            [(f64::NAN, 0), (f64::INFINITY, ratio), (-2.5, 0), (0.0, 0), (-0.0, ratio), (5e-324, 0), (-0.75, ratio)];
        assert_eq!(registered(&values, true), (vec![2.5, 0.0, 0.0, 5e-324, 0.75], 0.75));
        assert_eq!(registered(&values, false), (vec![2.5, 5e-324, 0.75], 0.75));
        // a value that is no ratio leaves the largest ratio alone; a smaller ratio too
        assert_eq!(registered(&[(9.0, flags::PERCENTAGE_OF_TIME), (0.5, ratio), (0.25, ratio)], true).1, 0.5);

        let point = StoragePoint { count: 7, sum: 21.0, ..StoragePoint::default() };
        let (mut results, mut stats) = (Vec::new(), Stats::default());
        let both = Found { highlighted: Some(point), baseline: Some(StoragePoint::UNSET), ..found(-1.0, ratio) };
        register(&mut results, &mut stats, false, &of, both);
        register(&mut results, &mut stats, false, &of, found(2.0, 0));
        let [first, second] = &results[..] else { panic!("two results") };
        assert_eq!((first.value, first.flags, first.duration_us, first.highlighted.count), (1.0, ratio, 3, 7));
        assert!(first.baseline.sum.is_nan() && first.baseline.count == 0);
        assert_eq!((second.highlighted.count, second.highlighted.sum, second.baseline.sum), (0, 0.0, 0.0));
        let named = (first.hostname.as_str(), first.metric.id(), first.instance.id(), first.context.id());
        assert_eq!(named, ("weights", "a", "t.w", "ctx.w"));
        assert!(Arc::ptr_eq(&first.host, &h) && !first.selected && !first.node_selected);
    }
}
