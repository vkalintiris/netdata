//! The Groups section: every span in the window of the traces with a span
//! in scope (the trace join's second pass), grouped by service and
//! operation. Pass 1 collects the scope's trace ids over every source;
//! pass 2 walks each source's window rows and keeps those of the set.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use super::super::duration_hist::DurationHistogram;
use super::query::{NAME_FIELD, SERVICE_FIELD, STATUS_FIELD};
use super::rows::self_time;
use super::shard::open_source;
use super::{GroupKey, GroupNumbers, GroupRow, OtherGroups};

/// Groups shown before the rest fold into one `other` row.
pub const GROUPS_CAP: usize = 500;

/// What pass 1 found, as one source's pass 2 needs it.
pub(super) struct Join<'a> {
    /// The scope's trace ids over every source.
    pub traces: &'a HashSet<sfst::TraceId>,
    /// This source's scope rows with an unset trace id, ascending: each
    /// counts as itself (D41).
    pub unset: &'a [u32],
}

/// One group's running numbers.
#[derive(Debug, Clone, Default)]
pub(super) struct GroupAcc {
    spans: u64,
    errors: u64,
    origins: u64,
    self_ns: u128,
    durations: DurationHistogram,
}

impl GroupAcc {
    pub fn merge(&mut self, other: &GroupAcc) {
        self.spans += other.spans;
        self.errors += other.errors;
        self.origins += other.origins;
        self.self_ns += other.self_ns;
        self.durations.merge(&other.durations);
    }

    fn numbers(&self) -> GroupNumbers {
        GroupNumbers {
            spans: self.spans,
            errors: self.errors,
            errors_originated: self.origins,
            p95_ns: self.durations.percentile(95),
            self_ns: self.self_ns,
        }
    }
}

/// Folds `from` into `into`, group by group.
pub(super) fn merge_groups(
    into: &mut HashMap<GroupKey, GroupAcc>,
    from: HashMap<GroupKey, GroupAcc>,
) {
    for (key, acc) in from {
        into.entry(key).or_default().merge(&acc);
    }
}

/// One source's groups over its window rows of the joined traces. A source
/// without child time (a live WAL whose live pass failed) adds no self time;
/// the section names that.
pub(super) fn evaluate_groups(
    bytes: &[u8],
    derived: Option<&Arc<sfst::DerivedValues>>,
    window: Range<i64>,
    join: &Join<'_>,
) -> Result<HashMap<GroupKey, GroupAcc>, sfst::Error> {
    let reader = open_source(bytes, derived)?;
    let (lo, hi) = reader.range_positions(window.clone())?;
    let mut out = HashMap::new();
    if lo >= hi {
        return Ok(out);
    }
    let trace_ids = reader.trace_ids()?;
    let mut kept = Vec::new();
    for position in lo..hi {
        let id = trace_ids.get(position as usize);
        let keep = if id.is_unset() {
            join.unset.binary_search(&position).is_ok()
        } else {
            join.traces.contains(&id)
        };
        if keep {
            kept.push(position);
        }
    }
    if kept.is_empty() {
        return Ok(out);
    }

    let services = reader.row_values(SERVICE_FIELD, lo..hi)?;
    let operations = reader.row_values(NAME_FIELD, lo..hi)?;
    let marked = |filter: sfst::Filter| -> Result<Vec<bool>, sfst::Error> {
        let compiled = reader.compile_filter(&filter, None)?;
        let mut marks = vec![false; (hi - lo) as usize];
        for position in reader.matched_positions(&compiled, window.clone())? {
            marks[(position - lo) as usize] = true;
        }
        Ok(marks)
    };
    let errors = marked(sfst::Filter::new().select(STATUS_FIELD, "ERROR"))?;
    let origins = marked(sfst::Filter::new().select(sfst::ERR_ORIGIN_FIELD, "true"))?;
    let durations = reader.durations()?;
    let children = if reader.has_child_durations() {
        Some(reader.child_durations()?)
    } else {
        None
    };

    let mut by_index: HashMap<(Option<u32>, Option<u32>), GroupAcc> = HashMap::new();
    for position in kept {
        let offset = (position - lo) as usize;
        let key = (services.value_at(position), operations.value_at(position));
        let acc = by_index.entry(key).or_default();
        acc.spans += 1;
        acc.errors += u64::from(errors[offset]);
        acc.origins += u64::from(origins[offset]);
        let duration =
            durations.0.get(position as usize).copied().ok_or_else(|| {
                sfst::Error::CorruptIndex(format!("no duration for row {position}"))
            })?;
        acc.durations.record(duration);
        if let Some(children) = children {
            acc.self_ns += self_time(durations, children, position)? as u128;
        }
    }
    for ((service, operation), acc) in by_index {
        let key = GroupKey {
            service: service.map(|index| services.value(index).to_string()),
            operation: operation.map(|index| operations.value(index).to_string()),
        };
        out.insert(key, acc);
    }
    Ok(out)
}

/// Every group ranked, the first `cap` kept and the rest folded.
pub(super) struct Capped {
    pub rows: Vec<GroupRow>,
    pub other: Option<OtherGroups>,
    /// Groups folded into `other`, out of `total`.
    pub folded: u64,
    pub total: u64,
    /// Self time over every group, `other` included.
    pub self_ns_total: u128,
}

/// Ranks groups by span count (then by key, `None` after any value) and
/// keeps the first `cap`.
pub(super) fn cap_groups(groups: HashMap<GroupKey, GroupAcc>, cap: usize) -> Capped {
    let mut ranked: Vec<(GroupKey, GroupAcc)> = groups.into_iter().collect();
    ranked.sort_by(|(a_key, a), (b_key, b)| b.spans.cmp(&a.spans).then_with(|| a_key.cmp(b_key)));
    let total = ranked.len() as u64;
    let mut capped = Capped {
        rows: Vec::with_capacity(ranked.len().min(cap)),
        other: None,
        folded: 0,
        total,
        self_ns_total: 0,
    };
    let mut rest = GroupAcc::default();
    for (index, (key, acc)) in ranked.into_iter().enumerate() {
        capped.self_ns_total += acc.self_ns;
        if index < cap {
            capped.rows.push(GroupRow {
                key,
                numbers: acc.numbers(),
            });
        } else {
            rest.merge(&acc);
            capped.folded += 1;
        }
    }
    if capped.folded > 0 {
        capped.other = Some(OtherGroups {
            groups: capped.folded,
            numbers: rest.numbers(),
        });
    }
    capped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(service: Option<&str>, operation: Option<&str>) -> GroupKey {
        GroupKey {
            service: service.map(str::to_string),
            operation: operation.map(str::to_string),
        }
    }

    fn acc(spans: u64, self_ns: u128, duration_ns: i64) -> GroupAcc {
        let mut acc = GroupAcc {
            spans,
            errors: spans / 2,
            origins: spans / 4,
            self_ns,
            durations: DurationHistogram::new(),
        };
        for _ in 0..spans {
            acc.durations.record(duration_ns);
        }
        acc
    }

    #[test]
    fn groups_cap_folds_into_other() {
        let groups = HashMap::from([
            (key(Some("b"), Some("x")), acc(10, 100, 1_000)),
            (key(Some("a"), Some("y")), acc(10, 200, 1_000)),
            (key(None, Some("x")), acc(10, 300, 1_000)),
            (key(Some("a"), None), acc(10, 400, 1_000)),
            (key(Some("c"), Some("z")), acc(4, 50, 5_000_000)),
            (key(Some("d"), Some("z")), acc(2, 25, 9_000_000)),
        ]);
        let capped = cap_groups(groups, 3);

        let ranked: Vec<GroupKey> = capped.rows.iter().map(|row| row.key.clone()).collect();
        assert_eq!(
            ranked,
            vec![
                key(Some("a"), Some("y")),
                key(Some("a"), None),
                key(Some("b"), Some("x")),
            ],
            "ties by key, a missing value after every value"
        );
        assert_eq!((capped.folded, capped.total), (3, 6));
        assert_eq!(capped.self_ns_total, 1_075);
        let other = capped.other.expect("three groups folded");
        assert_eq!(other.groups, 3);
        assert_eq!(
            (
                other.numbers.spans,
                other.numbers.errors,
                other.numbers.errors_originated,
                other.numbers.self_ns
            ),
            (16, 8, 3, 375)
        );
        let mut histogram = DurationHistogram::new();
        for (count, duration) in [(10, 1_000), (4, 5_000_000), (2, 9_000_000)] {
            for _ in 0..count {
                histogram.record(duration);
            }
        }
        assert_eq!(other.numbers.p95_ns, histogram.percentile(95));
    }

    #[test]
    fn groups_under_the_cap_fold_nothing() {
        let groups = HashMap::from([(key(Some("a"), Some("x")), acc(3, 7, 1_000))]);
        let capped = cap_groups(groups, GROUPS_CAP);
        assert_eq!(capped.rows.len(), 1);
        assert!(capped.other.is_none());
        assert_eq!(
            (capped.folded, capped.total, capped.self_ns_total),
            (0, 1, 7)
        );
    }
}
