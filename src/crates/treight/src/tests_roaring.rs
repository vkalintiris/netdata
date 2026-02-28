use crate::*;
use proptest::prelude::*;
use ::roaring::RoaringBitmap;

/// Maximum universe size for property tests. Kept small enough that
/// exhaustive membership checks (0..universe) are fast.
const MAX_UNIVERSE: u32 = 4096;

/// Strategy: generate a (universe_size, sorted-deduped values) pair.
fn arb_bitmap() -> impl Strategy<Value = (u32, Vec<u32>)> {
    (1u32..=MAX_UNIVERSE).prop_flat_map(|universe| {
        proptest::collection::vec(0..universe, 0..=(universe.min(256) as usize)).prop_map(
            move |mut vals| {
                vals.sort_unstable();
                vals.dedup();
                (universe, vals)
            },
        )
    })
}

/// Build both a RawBitmap and a RoaringBitmap from the same values.
fn make_pair(universe: u32, vals: &[u32]) -> (RawBitmap, RoaringBitmap) {
    let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
    let roaring = RoaringBitmap::from_sorted_iter(vals.iter().copied()).unwrap();
    (raw, roaring)
}

fn make_bitmap(universe_size: u32, values: &[u32]) -> RawBitmap {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    RawBitmap::from_sorted_iter(sorted.into_iter(), universe_size)
}

// ===== Construction & queries =====

proptest! {
    #[test]
    fn contains_matches_roaring((universe, vals) in arb_bitmap()) {
        let (raw, roaring) = make_pair(universe, &vals);
        for v in 0..universe {
            prop_assert_eq!(
                raw.contains(v),
                roaring.contains(v),
                "contains({}) mismatch, universe={}", v, universe
            );
        }
    }

    #[test]
    fn iter_matches_roaring((universe, vals) in arb_bitmap()) {
        let (raw, roaring) = make_pair(universe, &vals);
        let raw_vals: Vec<u32> = raw.iter().collect();
        let roaring_vals: Vec<u32> = roaring.iter().collect();
        prop_assert_eq!(raw_vals, roaring_vals);
    }

    #[test]
    fn len_matches_roaring((universe, vals) in arb_bitmap()) {
        let (raw, roaring) = make_pair(universe, &vals);
        prop_assert_eq!(raw.len(), roaring.len());
    }

    #[test]
    fn min_max_match_roaring((universe, vals) in arb_bitmap()) {
        let (raw, roaring) = make_pair(universe, &vals);
        prop_assert_eq!(raw.min(), roaring.min());
        prop_assert_eq!(raw.max(), roaring.max());
    }

    #[test]
    fn estimate_data_size_is_exact((universe, vals) in arb_bitmap()) {
        let est = estimate_data_size(universe, vals.iter().copied());
        let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
        prop_assert_eq!(est, raw.data().len());
    }
}

// ===== Insert / remove =====

proptest! {
    #[test]
    fn insert_matches_roaring(
        (universe, vals) in arb_bitmap(),
        extra in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..32),
    ) {
        let (mut raw, mut roaring) = make_pair(universe, &vals);
        for v in extra {
            let v = v % universe;
            raw.insert(v);
            roaring.insert(v);
        }

        let raw_vals: Vec<u32> = raw.iter().collect();
        let roaring_vals: Vec<u32> = roaring.iter().collect();
        prop_assert_eq!(raw_vals, roaring_vals);
    }

    #[test]
    fn remove_matches_roaring(
        (universe, vals) in arb_bitmap(),
        to_remove in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..32),
    ) {
        let (mut raw, mut roaring) = make_pair(universe, &vals);
        for v in to_remove {
            let v = v % universe;
            raw.remove(v);
            roaring.remove(v);
        }

        let raw_vals: Vec<u32> = raw.iter().collect();
        let roaring_vals: Vec<u32> = roaring.iter().collect();
        prop_assert_eq!(raw_vals, roaring_vals);
    }
}

// ===== Set operations =====

proptest! {
    #[test]
    fn union_matches_roaring(
        (universe, a_vals) in arb_bitmap(),
        b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
    ) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        let (raw_a, roaring_a) = make_pair(universe, &a_vals);
        let (raw_b, roaring_b) = make_pair(universe, &b_vals);

        let raw_result = &raw_a | &raw_b;
        let roaring_result = &roaring_a | &roaring_b;

        let raw_out: Vec<u32> = raw_result.iter().collect();
        let roaring_out: Vec<u32> = roaring_result.iter().collect();
        prop_assert_eq!(raw_out, roaring_out);
    }

    #[test]
    fn intersection_matches_roaring(
        (universe, a_vals) in arb_bitmap(),
        b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
    ) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        let (raw_a, roaring_a) = make_pair(universe, &a_vals);
        let (raw_b, roaring_b) = make_pair(universe, &b_vals);

        let raw_result = &raw_a & &raw_b;
        let roaring_result = &roaring_a & &roaring_b;

        let raw_out: Vec<u32> = raw_result.iter().collect();
        let roaring_out: Vec<u32> = roaring_result.iter().collect();
        prop_assert_eq!(raw_out, roaring_out);
    }

    #[test]
    fn difference_matches_roaring(
        (universe, a_vals) in arb_bitmap(),
        b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
    ) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        let (raw_a, roaring_a) = make_pair(universe, &a_vals);
        let (raw_b, roaring_b) = make_pair(universe, &b_vals);

        let raw_result = &raw_a - &raw_b;
        let roaring_result = &roaring_a - &roaring_b;

        let raw_out: Vec<u32> = raw_result.iter().collect();
        let roaring_out: Vec<u32> = roaring_result.iter().collect();
        prop_assert_eq!(raw_out, roaring_out);
    }

    #[test]
    fn symmetric_difference_matches_roaring(
        (universe, a_vals) in arb_bitmap(),
        b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
    ) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        let (raw_a, roaring_a) = make_pair(universe, &a_vals);
        let (raw_b, roaring_b) = make_pair(universe, &b_vals);

        let raw_result = &raw_a ^ &raw_b;
        let roaring_result = &roaring_a ^ &roaring_b;

        let raw_out: Vec<u32> = raw_result.iter().collect();
        let roaring_out: Vec<u32> = roaring_result.iter().collect();
        prop_assert_eq!(raw_out, roaring_out);
    }
}

// ===== Algebraic set properties =====

proptest! {
    #[test]
    fn union_is_commutative((universe, a_vals) in arb_bitmap(), b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize)) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (a, _) = make_pair(universe, &a_vals);
        let (b, _) = make_pair(universe, &b_vals);
        prop_assert_eq!(&a | &b, &b | &a);
    }

    #[test]
    fn intersection_is_commutative((universe, a_vals) in arb_bitmap(), b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize)) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (a, _) = make_pair(universe, &a_vals);
        let (b, _) = make_pair(universe, &b_vals);
        prop_assert_eq!(&a & &b, &b & &a);
    }

    #[test]
    fn xor_is_commutative((universe, a_vals) in arb_bitmap(), b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize)) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (a, _) = make_pair(universe, &a_vals);
        let (b, _) = make_pair(universe, &b_vals);
        prop_assert_eq!(&a ^ &b, &b ^ &a);
    }

    #[test]
    fn union_with_empty_is_identity((universe, vals) in arb_bitmap()) {
        let (a, _) = make_pair(universe, &vals);
        let empty = RawBitmap::empty(universe);
        prop_assert_eq!(&a | &empty, a.clone());
        prop_assert_eq!(&empty | &a, a);
    }

    #[test]
    fn intersection_with_empty_is_empty((universe, vals) in arb_bitmap()) {
        let (a, _) = make_pair(universe, &vals);
        let empty = RawBitmap::empty(universe);
        prop_assert!((&a & &empty).is_empty());
        prop_assert!((&empty & &a).is_empty());
    }

    #[test]
    fn difference_with_self_is_empty((universe, vals) in arb_bitmap()) {
        let (a, _) = make_pair(universe, &vals);
        prop_assert!((&a - &a).is_empty());
    }

    #[test]
    fn xor_with_self_is_empty((universe, vals) in arb_bitmap()) {
        let (a, _) = make_pair(universe, &vals);
        prop_assert!((&a ^ &a).is_empty());
    }

    #[test]
    fn intersection_is_subset_of_union(
        (universe, a_vals) in arb_bitmap(),
        b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize),
    ) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (a, _) = make_pair(universe, &a_vals);
        let (b, _) = make_pair(universe, &b_vals);

        let inter = &a & &b;
        let union = &a | &b;

        // Every element in the intersection must be in the union.
        for v in inter.iter() {
            prop_assert!(union.contains(v), "intersection element {} not in union", v);
        }
        prop_assert!(inter.len() <= union.len());
    }
}

// ===== Bitmap (De Morgan wrapper) property tests =====

/// Strategy: generate a (universe, vals, inverted) triple.
fn arb_bitmap_wrapper() -> impl Strategy<Value = (u32, Vec<u32>, bool)> {
    arb_bitmap().prop_flat_map(|(universe, vals)| {
        proptest::bool::ANY.prop_map(move |inv| (universe, vals.clone(), inv))
    })
}

fn make_bitmap_wrapper(universe: u32, vals: &[u32], inverted: bool) -> Bitmap {
    if inverted {
        // vals are the complement (unset values)
        Bitmap::from_sorted_iter_complemented(vals.iter().copied(), universe)
    } else {
        Bitmap::from_sorted_iter(vals.iter().copied(), universe)
    }
}

proptest! {
    #[test]
    fn bitmap_contains_matches_roaring(
        (universe, vals, inverted) in arb_bitmap_wrapper(),
    ) {
        let bm = make_bitmap_wrapper(universe, &vals, inverted);

        // Build the expected set using roaring.
        let stored = RoaringBitmap::from_sorted_iter(vals.iter().copied()).unwrap();

        for v in 0..universe {
            let in_stored = stored.contains(v);
            let expected = if inverted { !in_stored } else { in_stored };
            prop_assert_eq!(
                bm.contains(v), expected,
                "Bitmap.contains({}) wrong, inverted={}", v, inverted
            );
        }
    }

    #[test]
    fn bitmap_len_matches_roaring(
        (universe, vals, inverted) in arb_bitmap_wrapper(),
    ) {
        let bm = make_bitmap_wrapper(universe, &vals, inverted);
        let stored = RoaringBitmap::from_sorted_iter(vals.iter().copied()).unwrap();
        let expected = if inverted {
            universe as u64 - stored.len()
        } else {
            stored.len()
        };
        prop_assert_eq!(bm.len(), expected);
    }

    #[test]
    fn bitmap_and_matches_oracle(
        (universe, a_vals) in arb_bitmap(),
        b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize),
        a_inv in proptest::bool::ANY,
        b_inv in proptest::bool::ANY,
    ) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        let bm_a = make_bitmap_wrapper(universe, &a_vals, a_inv);
        let bm_b = make_bitmap_wrapper(universe, &b_vals, b_inv);
        let result = &bm_a & &bm_b;

        // Oracle: compute expected membership directly.
        let stored_a = RoaringBitmap::from_sorted_iter(a_vals.iter().copied()).unwrap();
        let stored_b = RoaringBitmap::from_sorted_iter(b_vals.iter().copied()).unwrap();

        for v in 0..universe {
            let a_has = if a_inv { !stored_a.contains(v) } else { stored_a.contains(v) };
            let b_has = if b_inv { !stored_b.contains(v) } else { stored_b.contains(v) };
            let expected = a_has && b_has;
            prop_assert_eq!(
                result.contains(v), expected,
                "AND({}): a_inv={} b_inv={}", v, a_inv, b_inv
            );
        }
    }

    #[test]
    fn bitmap_or_matches_oracle(
        (universe, a_vals) in arb_bitmap(),
        b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize),
        a_inv in proptest::bool::ANY,
        b_inv in proptest::bool::ANY,
    ) {
        let b_vals = {
            let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        let bm_a = make_bitmap_wrapper(universe, &a_vals, a_inv);
        let bm_b = make_bitmap_wrapper(universe, &b_vals, b_inv);
        let result = &bm_a | &bm_b;

        let stored_a = RoaringBitmap::from_sorted_iter(a_vals.iter().copied()).unwrap();
        let stored_b = RoaringBitmap::from_sorted_iter(b_vals.iter().copied()).unwrap();

        for v in 0..universe {
            let a_has = if a_inv { !stored_a.contains(v) } else { stored_a.contains(v) };
            let b_has = if b_inv { !stored_b.contains(v) } else { stored_b.contains(v) };
            let expected = a_has || b_has;
            prop_assert_eq!(
                result.contains(v), expected,
                "OR({}): a_inv={} b_inv={}", v, a_inv, b_inv
            );
        }
    }
}

// ===== Serialization roundtrip =====

proptest! {
    #[test]
    fn serialize_roundtrip((universe, vals) in arb_bitmap()) {
        let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
        let mut buf = Vec::new();
        raw.serialize_into(&mut buf).unwrap();
        let raw2 = RawBitmap::deserialize_from(&buf[..]).unwrap();
        prop_assert_eq!(raw, raw2);
    }

    #[test]
    fn roaring_roundtrip((universe, vals) in arb_bitmap()) {
        let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
        let rb = RoaringBitmap::from(&raw);
        let raw2 = RawBitmap::from_roaring(&rb, universe);
        prop_assert_eq!(raw, raw2);
    }

    #[test]
    fn range_cardinality_matches_roaring(
        (universe, vals) in arb_bitmap(),
        range_start in 0u32..MAX_UNIVERSE,
        range_end in 0u32..MAX_UNIVERSE,
    ) {
        let lo = range_start.min(range_end) % universe;
        let hi = (range_start.max(range_end) % universe).saturating_add(1);
        let range = lo..hi;

        let (raw, roaring) = make_pair(universe, &vals);
        prop_assert_eq!(
            raw.range_cardinality(range.clone()),
            roaring.range_cardinality(range)
        );
    }

    #[test]
    fn range_cardinality_full_equals_len((universe, vals) in arb_bitmap()) {
        let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
        prop_assert_eq!(raw.range_cardinality(..), raw.len());
    }

    #[test]
    fn remove_range_matches_roaring(
        (universe, vals) in arb_bitmap(),
        range_start in 0u32..MAX_UNIVERSE,
        range_end in 0u32..MAX_UNIVERSE,
    ) {
        let lo = range_start.min(range_end) % universe;
        let hi = (range_start.max(range_end) % universe).saturating_add(1);

        let (mut raw, mut roaring) = make_pair(universe, &vals);
        raw.remove_range(lo..hi);
        roaring.remove_range(lo..hi);

        let raw_vals: Vec<u32> = raw.iter().collect();
        let roaring_vals: Vec<u32> = roaring.iter().collect();
        prop_assert_eq!(raw_vals, roaring_vals);
    }
}

// ===== Roaring conversion unit tests =====

#[test]
fn test_rawbitmap_from_roaring() {
    let mut rb = RoaringBitmap::new();
    rb.insert(0);
    rb.insert(42);
    rb.insert(511);

    let bm = RawBitmap::from_roaring(&rb, 512);
    assert_eq!(bm.universe_size(), 512);
    assert!(bm.contains(0));
    assert!(bm.contains(42));
    assert!(bm.contains(511));
    assert!(!bm.contains(1));
    assert_eq!(bm.len(), 3);
}

#[test]
fn test_rawbitmap_from_roaring_ref() {
    let mut rb = RoaringBitmap::new();
    rb.insert(10);
    rb.insert(100);

    let bm = RawBitmap::from(&rb);
    // universe_size = max + 1 = 101
    assert_eq!(bm.universe_size(), 101);
    assert!(bm.contains(10));
    assert!(bm.contains(100));
    assert_eq!(bm.len(), 2);
}

#[test]
fn test_rawbitmap_from_roaring_empty() {
    let rb = RoaringBitmap::new();
    let bm = RawBitmap::from(&rb);
    assert!(bm.is_empty());
    assert_eq!(bm.universe_size(), 0);

    let bm = RawBitmap::from_roaring(&rb, 64);
    assert!(bm.is_empty());
    assert_eq!(bm.universe_size(), 64);
}

#[test]
fn test_roaring_from_rawbitmap() {
    let bm = make_bitmap(512, &[0, 42, 255, 511]);
    let rb = RoaringBitmap::from(&bm);
    assert_eq!(rb.len(), 4);
    assert!(rb.contains(0));
    assert!(rb.contains(42));
    assert!(rb.contains(255));
    assert!(rb.contains(511));
}

#[test]
fn test_roaring_from_rawbitmap_empty() {
    let bm = RawBitmap::empty(64);
    let rb = RoaringBitmap::from(&bm);
    assert!(rb.is_empty());
}

#[test]
fn test_roaring_roundtrip() {
    let values = [0, 1, 7, 8, 63, 64, 255, 256, 511];
    let mut rb = RoaringBitmap::new();
    for &v in &values {
        rb.insert(v);
    }

    let bm = RawBitmap::from_roaring(&rb, 512);
    let rb2 = RoaringBitmap::from(&bm);
    assert_eq!(rb, rb2);
}
