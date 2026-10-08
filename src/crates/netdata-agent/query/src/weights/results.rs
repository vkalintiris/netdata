//! The results of a weights request (`struct register_result` and `register_result()`,
//! `src/web/api/queries/weights.c`): one per metric whose method gave a number, in the order the metrics were
//! visited.

use std::sync::Arc;

use netdata_agent_rrd::contexts::{Context, Instance, Metric};
use netdata_agent_rrd::host::Host;
use netdata_agent_storage::storage_point::StoragePoint;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::weights_host;

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
