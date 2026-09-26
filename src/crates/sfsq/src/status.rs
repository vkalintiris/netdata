//! Query-level result status — the no-silent-degrade contract.
//!
//! Every traces operation returns a [`QueryStatus`] beside its data, and the
//! logs consumer builds one for a trace-filtered answer:
//! [`Complete`](QueryStatus::Complete) means every relevant source was
//! successfully and fully examined; [`Partial`](QueryStatus::Partial)
//! carries the non-empty set of reasons why the result may be missing
//! something. Reasons coexist (a query can hit the span cap AND lose a
//! source), so the status holds every reason, not a single variant — an
//! explicit precedence rule that discards reasons would hide information.
//! Each reason carries how often it happened (and, where it applies, out
//! of how many), so the UI can say "2 of 14 files failed".

use std::collections::{BTreeMap, BTreeSet};

/// Why a result is partial. Ordered (BTreeSet) so the set — and its
/// rendering — is deterministic; the order is declaration order, so a
/// new reason is appended last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PartialReason {
    /// The span cap was reached: the result holds the globally earliest
    /// `cap` canonical spans (combiner total order); at least one more
    /// unique span exists.
    SizeCap,
    /// A source failed (map/open, TIDX, column, EVNB/LNKB, tail decode):
    /// spans that may live there are absent from the result.
    SourceFailure,
    /// The operation's work ceiling was hit before the candidate space
    /// was exhausted (search-phase concept; unused by 4a trace-by-id,
    /// which deliberately has no work ceiling — adversarial-input
    /// hardening is deferred whole).
    WorkCeiling,
    /// The caller cancelled: before all source heads were resolved the
    /// result is empty; during TRACE-BY-ID's merge it is the
    /// deterministic merged prefix (that mode's pinned exception).
    /// Search and the aggregate folds are ALL-OR-EMPTY instead — a
    /// mid-flight search prefix cannot be deterministic under canonical
    /// re-ranking and grow-K, and a mid-loop aggregate cancel discards
    /// the partial fold.
    Cancelled,
    /// The overview's own visited-rows ceiling was hit before every
    /// in-window source was binned: the grid holds the deterministic
    /// prefix of sources (SourceId order) processed so far. Distinct
    /// from [`WorkCeiling`] — the overview's cost is O(spans-in-window)
    /// per source, a different budget than search's candidate loop.
    OverviewCeiling,
    /// A sealed source has no trace rollup chunk (`TRSU`) and was
    /// EXCLUDED from trace-level aggregation: including it would
    /// silently undercount, and mixing span-level numbers into a
    /// trace-level result is forbidden (no mixed units). Two
    /// causes: the file predates the rollup (data returns when it
    /// ages out), or the file stored no real traces — an all-UNSET
    /// trace-id file writes no chunk by the seal's `is_meaningful`
    /// rule, and exclusion loses nothing.
    RollupAbsent,
    /// A source's bytes could not be obtained from remote storage (a
    /// download failed or timed out, or the catalog listing the file
    /// could not be read): spans that may live there are absent.
    /// Distinct from [`SourceFailure`](Self::SourceFailure), which
    /// reports bytes that were available but could not be read.
    RemoteUnavailable,
    /// A sealed source predates the explorer's per-span entries (no
    /// `_role`) and was left out: counting it would silently report zero
    /// for it. Its data returns only by ageing out.
    LegacyFile,
    /// The histogram's stack field is high-cardinality in some sources;
    /// their rows are counted, but not per value.
    StackFieldHighCard,
    /// A requested facet field is high-cardinality and was left out.
    FacetHighCard,
    /// A facet field had more values than the facet cap; the rest are not
    /// listed.
    FacetValueCap,
    /// The Groups table had more groups than its cap; the rest are folded
    /// into one `other` row.
    GroupsCap,
    /// The pass that derives error origin and self time over a live WAL
    /// failed; values that depend on them are missing for its rows.
    /// Declared last: wire order is declaration order.
    LivePassFailed,
}

/// How often one reason happened in a query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReasonCount {
    /// Occurrences (failed sources, left-out fields, folded groups, ...).
    pub count: u64,
    /// Out of how many, where that is meaningful (e.g. evaluated sources).
    pub of: Option<u64>,
    /// Names the reason applies to (e.g. the fields left out).
    pub detail: BTreeSet<String>,
}

/// The status of one query's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryStatus {
    /// Every relevant source was successfully and fully examined.
    Complete,
    /// Something may be missing; the map is non-empty by construction
    /// ([`StatusBuilder::finish`]).
    Partial(BTreeMap<PartialReason, ReasonCount>),
}

impl QueryStatus {
    pub fn is_complete(&self) -> bool {
        matches!(self, QueryStatus::Complete)
    }

    /// Whether `reason` is among the partial reasons (`false` for
    /// `Complete`).
    pub fn has(&self, reason: PartialReason) -> bool {
        match self {
            QueryStatus::Complete => false,
            QueryStatus::Partial(reasons) => reasons.contains_key(&reason),
        }
    }

    /// The reasons, without their counts (empty for `Complete`).
    pub fn reasons(&self) -> BTreeSet<PartialReason> {
        match self {
            QueryStatus::Complete => BTreeSet::new(),
            QueryStatus::Partial(reasons) => reasons.keys().copied().collect(),
        }
    }

    /// How often `reason` happened (`None` when it did not).
    pub fn count(&self, reason: PartialReason) -> Option<&ReasonCount> {
        match self {
            QueryStatus::Complete => None,
            QueryStatus::Partial(reasons) => reasons.get(&reason),
        }
    }
}

/// Accumulates partial reasons during a query; the empty accumulation IS
/// the `Complete` status, so the "non-empty reasons" invariant holds by
/// construction rather than by discipline at every return site.
#[derive(Debug, Clone, Default)]
pub struct StatusBuilder {
    reasons: BTreeMap<PartialReason, ReasonCount>,
}

impl StatusBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one occurrence of a reason; reasons coexist.
    pub fn add(&mut self, reason: PartialReason) {
        self.add_n(reason, 1);
    }

    /// Record `n` occurrences at once; `n == 0` records nothing.
    pub fn add_n(&mut self, reason: PartialReason, n: u64) {
        if n > 0 {
            self.reasons.entry(reason).or_default().count += n;
        }
    }

    /// Say out of how many a recorded reason's count is; a reason not
    /// recorded is left absent (a total alone is not a failure).
    pub fn of(&mut self, reason: PartialReason, total: u64) {
        if let Some(entry) = self.reasons.get_mut(&reason) {
            entry.of = Some(entry.of.map_or(total, |of| of.max(total)));
        }
    }

    /// Name one thing a recorded reason applies to.
    pub fn detail(&mut self, reason: PartialReason, detail: impl Into<String>) {
        if let Some(entry) = self.reasons.get_mut(&reason) {
            entry.detail.insert(detail.into());
        }
    }

    /// Fold another builder in: counts add, totals take the larger, details
    /// unite.
    pub fn merge(&mut self, other: StatusBuilder) {
        for (reason, theirs) in other.reasons {
            let ours = self.reasons.entry(reason).or_default();
            ours.count += theirs.count;
            ours.of = match (ours.of, theirs.of) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            ours.detail.extend(theirs.detail);
        }
    }

    pub fn finish(self) -> QueryStatus {
        if self.reasons.is_empty() {
            QueryStatus::Complete
        } else {
            QueryStatus::Partial(self.reasons)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_builder_is_complete() {
        assert_eq!(StatusBuilder::new().finish(), QueryStatus::Complete);
        assert!(StatusBuilder::new().finish().is_complete());
    }

    #[test]
    fn reasons_coexist_and_render_deterministically() {
        // Insertion order must not matter; e.g. SizeCap + SourceFailure both
        // survive.
        let mut a = StatusBuilder::new();
        a.add(PartialReason::SourceFailure);
        a.add(PartialReason::SizeCap);

        let mut b = StatusBuilder::new();
        b.add(PartialReason::SizeCap);
        b.add(PartialReason::SourceFailure);

        let a = a.finish();
        assert_eq!(a, b.finish());
        assert!(!a.is_complete());
        assert!(a.has(PartialReason::SizeCap));
        assert!(a.has(PartialReason::SourceFailure));
        assert!(!a.has(PartialReason::Cancelled));
    }

    #[test]
    fn reasons_count_occurrences_totals_and_details() {
        let mut a = StatusBuilder::new();
        a.add(PartialReason::SourceFailure);
        a.add(PartialReason::SourceFailure);
        a.of(PartialReason::SourceFailure, 14);
        a.add_n(PartialReason::FacetHighCard, 0);
        a.of(PartialReason::LegacyFile, 3);
        a.detail(PartialReason::GroupsCap, "ignored");

        let mut b = StatusBuilder::new();
        b.add_n(PartialReason::SourceFailure, 3);
        b.of(PartialReason::SourceFailure, 9);
        b.add(PartialReason::FacetHighCard);
        b.detail(PartialReason::FacetHighCard, "attributes.request.id");
        a.merge(b);

        let status = a.finish();
        assert_eq!(
            status,
            QueryStatus::Partial(BTreeMap::from([
                (
                    PartialReason::SourceFailure,
                    ReasonCount {
                        count: 5,
                        of: Some(14),
                        detail: BTreeSet::new(),
                    }
                ),
                (
                    PartialReason::FacetHighCard,
                    ReasonCount {
                        count: 1,
                        of: None,
                        detail: BTreeSet::from(["attributes.request.id".to_string()]),
                    }
                ),
            ]))
        );
        assert_eq!(status.count(PartialReason::SourceFailure).map(|c| c.count), Some(5));
        assert!(status.count(PartialReason::LegacyFile).is_none());
    }
}
