//! The C planner's unit tests (`query_plan_unittest()`, `src/web/api/queries/query-plan.c:837-1105`, run by
//! `netdata -W queryplantest`), with three tiers and C's expectations; the ops-cache case has no Rust analogue and the
//! result-expiry case is `execute/tests.rs`'s. Plus the reference's order of equal starts (D74.2) and the choice of the next
//! plan.

use super::*;

const STORAGE_TIERS: usize = 3;

fn views(set: &[(usize, i64, i64, i64)]) -> [TierView; RRD_STORAGE_TIERS] {
    let mut v = [TierView::default(); RRD_STORAGE_TIERS];
    for &(tier, first, last, ue) in set {
        v[tier] = TierView {
            has_handle: true,
            first_time_s: first,
            last_time_s: last,
            update_every_s: ue,
        };
    }
    v
}

fn tiers(v: &[TierView]) -> Tiers<'_> {
    Tiers {
        views: v,
        storage_tiers: STORAGE_TIERS,
    }
}

fn entry(tier: usize, after: i64, before: i64) -> PlanEntry {
    PlanEntry {
        tier,
        after,
        before,
    }
}

fn best(v: &[TierView], after: i64, before: i64, points: usize) -> usize {
    tiers(v).best_tier(after, before, points, &mut [0; RRD_STORAGE_TIERS])
}

fn plan(v: &[TierView], selected: Option<usize>, after: i64, before: i64, points: usize) -> Built {
    build_entries(
        &tiers(v),
        selected,
        after,
        before,
        points,
        &mut [0; RRD_STORAGE_TIERS],
    )
}

#[test]
fn entry_validity() {
    let v = views(&[(0, 10, 100, 10)]);
    let t = tiers(&v);
    for (name, e, want) in [
        ("valid in-window entry", entry(0, 20, 80), true),
        ("zero start is invalid", entry(0, 0, 80), false),
        ("flipped entry is invalid", entry(0, 90, 80), false),
        (
            "entry before requested window is invalid",
            entry(0, 9, 80),
            false,
        ),
        (
            "entry after requested window is invalid",
            entry(0, 20, 101),
            false,
        ),
        ("out-of-range tier is invalid", entry(3, 20, 80), false),
    ] {
        assert_eq!(t.entry_is_valid(&e, 10, 100), want, "{name}");
    }
}

#[test]
fn activation() {
    let v = views(&[(0, 10, 100, 10)]);
    let t = tiers(&v);
    let one = [entry(0, 10, 100)];
    for (name, entries, state, plan, want) in [
        ("valid initialized plan", one, PlanState::Open, 0, true),
        (
            "invalid plan id is rejected",
            one,
            PlanState::Open,
            1,
            false,
        ),
        (
            "uninitialized plan is rejected",
            one,
            PlanState::Uninitialized,
            0,
            false,
        ),
        (
            "finalized plan is rejected",
            one,
            PlanState::Finalized,
            0,
            false,
        ),
        (
            "flipped plan is rejected",
            [entry(0, 100, 10)],
            PlanState::Open,
            0,
            false,
        ),
        (
            "out-of-range tier is rejected",
            [entry(3, 10, 100)],
            PlanState::Open,
            0,
            false,
        ),
    ] {
        assert_eq!(can_activate(&t, &entries, &[state], plan), want, "{name}");
    }
}

#[test]
fn best_tier_selection() {
    for (name, set, after, before, points, want) in [
        (
            "sub-resolution window ignores non-overlapping coarser tier",
            vec![(0, 1, 200, 10), (1, 1, 200, 600), (2, 1, 100, 36000)],
            103,
            108,
            5,
            0,
        ),
        (
            "sub-resolution window chooses densest overlapping tier",
            vec![(0, 1, 200, 10), (1, 1, 200, 600), (2, 1, 200, 36000)],
            103,
            108,
            5,
            0,
        ),
        (
            "50 percent tolerance chooses sparsest acceptable tier",
            vec![
                (0, 1, 400000, 1),
                (1, 1, 400000, 600),
                (2, 1, 400000, 36000),
            ],
            1000,
            301000,
            500,
            1,
        ),
        (
            "50 percent tolerance includes exact threshold",
            vec![(0, 1, 1000, 1), (1, 1, 1000, 10), (2, 1, 1000, 11)],
            100,
            400,
            60,
            1,
        ),
        (
            "under-resolution request chooses densest tier",
            vec![(0, 1, 1000, 10), (1, 1, 1000, 600), (2, 1, 1000, 36000)],
            100,
            700,
            600,
            0,
        ),
        (
            "zero-overlap tiers are not candidates",
            vec![(0, 1, 50, 10), (1, 100, 200, 600), (2, 1, 50, 36000)],
            103,
            108,
            5,
            1,
        ),
        (
            "invalid duration returns first working tier",
            vec![(1, 1, 200, 600), (2, 1, 200, 36000)],
            108,
            108,
            5,
            1,
        ),
    ] {
        assert_eq!(best(&views(&set), after, before, points), want, "{name}");
    }
}

#[test]
fn plan_entries() {
    let two = views(&[(0, 180, 260, 10), (1, 100, 200, 30)]);
    let three = views(&[(0, 180, 260, 10), (1, 100, 200, 30), (2, 50, 150, 60)]);
    for (name, v, selected, after, before, want) in [
        (
            "selected tier covers full window",
            views(&[(0, 1, 300, 10), (1, 1, 300, 600)]),
            None,
            100,
            200,
            vec![entry(0, 100, 200)],
        ),
        (
            "coarser tier fills head gap",
            views(&[(0, 100, 180, 10), (1, 50, 150, 30)]),
            None,
            50,
            180,
            vec![entry(1, 50, 100), entry(0, 100, 180)],
        ),
        (
            "finer tier fills tail gap",
            two,
            None,
            100,
            250,
            vec![entry(1, 100, 200), entry(0, 200, 250)],
        ),
        (
            "planner fills both head and tail gaps",
            three,
            None,
            50,
            250,
            vec![entry(2, 50, 100), entry(1, 100, 200), entry(0, 200, 250)],
        ),
        (
            "explicit selected tier disables gap filling",
            three,
            Some(1),
            50,
            250,
            vec![entry(1, 100, 200)],
        ),
    ] {
        assert_eq!(
            plan(&v, selected, after, before, 10),
            Built {
                entries: want,
                ok: true
            },
            "{name}"
        );
    }
    let none = views(&[(0, 1, 50, 10), (1, 60, 90, 30), (2, 100, 150, 60)]);
    assert!(
        !plan(&none, None, 200, 250, 10).ok,
        "no overlapping tier fails planning"
    );
}

#[test]
fn natural_update_every_per_tier() {
    assert_eq!(
        min_update_every_for_tier([600, 300].into_iter(), 1, STORAGE_TIERS, 1),
        300
    );
    assert_eq!(
        min_update_every_for_tier([0, 0].into_iter(), 1, STORAGE_TIERS, 1),
        1
    );
    assert_eq!(
        min_update_every_for_tier([600].into_iter(), 3, STORAGE_TIERS, 1),
        1
    );
}

/// The weights the debug output prints: each overlapping tier's points density, `-LONG_MAX` for the others, none
/// written without a choice to make.
#[test]
fn weights_are_cs() {
    let v = views(&[(0, 1, 50, 10), (1, 100, 200, 600), (2, 1, 50, 36000)]);
    let mut weights = [0; RRD_STORAGE_TIERS];
    assert_eq!(tiers(&v).best_tier(103, 108, 5, &mut weights), 1);
    assert_eq!(weights[..3], [-i64::MAX, 5 * 1_000_000 / 600, -i64::MAX]);
    let mut untouched = [7; RRD_STORAGE_TIERS];
    assert_eq!(tiers(&v).best_tier(108, 108, 5, &mut untouched), 0);
    assert_eq!(untouched, [7; RRD_STORAGE_TIERS]);
    let one = Tiers {
        views: &v,
        storage_tiers: 1,
    };
    assert_eq!(one.best_tier(103, 108, 5, &mut untouched), 0);
    assert_eq!(density_weight(1, 0, i64::MAX), i64::MAX);
    assert_eq!(minimum_acceptable_weight(usize::MAX), i64::MAX);
}

/// A single-instant best-tier entry and the finer tier that finishes the window start together: the reference
/// puts the finer one first (D74.2).
#[test]
fn equal_starts_sort_as_the_reference() {
    // tier 1 ends where the window starts; tier 0 has the rest
    let mut entries = vec![entry(1, 100, 100), entry(0, 100, 200)];
    sort_as_the_reference(&mut entries);
    assert_eq!(entries, [entry(0, 100, 200), entry(1, 100, 100)]);
    let mut three = vec![entry(2, 50, 50), entry(1, 50, 80), entry(0, 80, 90)];
    sort_as_the_reference(&mut three);
    assert_eq!(
        three,
        [entry(1, 50, 80), entry(2, 50, 50), entry(0, 80, 90)]
    );
}

/// C's expansions with the default groupings (1, 60, 3600 s): a tier-1 head then tier 0, and tier 2 then tier 0.
#[test]
fn expansions_are_cs() {
    assert_eq!(expand_duration_in_points(60, 1), 5);
    assert_eq!(expand_duration_in_points(1, 60), 59);
    assert_eq!(expand_duration_in_points(1, 3600), 3599);
    let v = views(&[(0, 1, 1000, 1), (1, 1, 1000, 60)]);
    let t = tiers(&v);
    let entries = [entry(1, 100, 400), entry(0, 400, 900)];
    assert_eq!(
        expanded_windows(&t, &entries),
        [(100 - 5 * 60, 400 + 5 * 60), (400 - 59, 900 + 5)]
    );
    assert_eq!(expanded_windows(&t, &[entry(0, 400, 900)]), [(400, 905)]);
    assert_eq!(expire_time(&entries, 0), 400);
    assert_eq!(expire_time(&entries, 1), 900);
}

/// `query_planer_next_plan()`'s choice (QP:319-343): a plan that `now` or the last point's end has reached is
/// skipped; the chosen plan must be open.
#[test]
fn the_next_plan_is_the_first_not_reached() {
    let v = views(&[(0, 100, 300, 10), (1, 50, 200, 30), (2, 10, 100, 60)]);
    let t = tiers(&v);
    let tie = [entry(0, 100, 200), entry(1, 100, 100)];
    let three = [entry(2, 10, 50), entry(1, 50, 100), entry(0, 100, 300)];
    let open = [PlanState::Open; 3];
    let spent = [PlanState::Open, PlanState::Open, PlanState::Finalized];
    for (name, entries, states, now, last_end, want) in [
        (
            "a tie's instant plan is reached",
            &tie[..],
            &open[..2],
            100,
            0,
            None,
        ),
        (
            "the spent middle plan is skipped",
            &three,
            &open,
            120,
            0,
            Some(2),
        ),
        (
            "the last point's end alone spends a plan",
            &three,
            &open,
            60,
            100,
            Some(2),
        ),
        (
            "the next plan is not reached",
            &three,
            &open,
            60,
            99,
            Some(1),
        ),
        ("a finalized next plan", &three, &spent, 120, 0, None),
    ] {
        assert_eq!(
            next_plan(&t, entries, states, 0, now, last_end),
            want,
            "{name}"
        );
    }
}
