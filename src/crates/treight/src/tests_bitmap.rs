    use crate::*;

    // ---- Bitmap (high-level, De Morgan) tests ----

    #[test]
    fn test_bitmap_contains_normal() {
        let bm = Bitmap::from_sorted_iter([10, 20, 30].into_iter(), 64);
        assert!(bm.contains(10));
        assert!(bm.contains(20));
        assert!(bm.contains(30));
        assert!(!bm.contains(0));
        assert!(!bm.contains(15));
        assert!(!bm.is_inverted());
    }

    #[test]
    fn test_bitmap_contains_inverted() {
        let bm = Bitmap::from_sorted_iter_complemented([10, 20, 30].into_iter(), 64);
        assert!(!bm.contains(10));
        assert!(!bm.contains(20));
        assert!(!bm.contains(30));
        assert!(bm.contains(0));
        assert!(bm.contains(15));
        assert!(bm.contains(63));
        assert!(bm.is_inverted());
    }

    #[test]
    fn test_bitmap_contains_out_of_bounds() {
        // Normal bitmap: out-of-bounds values are not contained.
        let normal = Bitmap::from_sorted_iter([0, 1, 2].into_iter(), 5);
        assert!(!normal.contains(5));
        assert!(!normal.contains(100));

        // Inverted bitmap: out-of-bounds values must NOT be contained,
        // even though the bitmap logically represents "all values except ...".
        let inverted = Bitmap::from_sorted_iter_complemented([1].into_iter(), 5);
        assert!(inverted.contains(0));
        assert!(!inverted.contains(1));
        assert!(inverted.contains(4));
        assert!(!inverted.contains(5));
        assert!(!inverted.contains(100));

        // full() bitmap: contains everything in 0..universe_size, nothing outside.
        let full = Bitmap::full(5);
        assert!(full.contains(0));
        assert!(full.contains(4));
        assert!(!full.contains(5));
        assert!(!full.contains(u32::MAX));
    }

    #[test]
    fn test_bitmap_len() {
        let bm = Bitmap::from_sorted_iter([10, 20, 30].into_iter(), 64);
        assert_eq!(bm.len(), 3);

        let bm = Bitmap::from_sorted_iter_complemented([10, 20, 30].into_iter(), 64);
        assert_eq!(bm.len(), 61);
    }

    #[test]
    fn test_bitmap_empty_full() {
        let bm = Bitmap::empty(64);
        assert!(bm.is_empty());
        assert_eq!(bm.len(), 0);
        assert!(!bm.contains(0));

        let bm = Bitmap::full(64);
        assert!(!bm.is_empty());
        assert_eq!(bm.len(), 64);
        for i in 0..64 {
            assert!(bm.contains(i));
        }
    }

    #[test]
    fn test_bitmap_and_nn() {
        let a = Bitmap::from_sorted_iter([0, 1, 2, 3].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([2, 3, 4, 5].into_iter(), 64);
        let c = &a & &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 2);
        assert!(c.contains(2));
        assert!(c.contains(3));
        assert!(!c.contains(0));
        assert!(!c.contains(4));
    }

    #[test]
    fn test_bitmap_and_ni() {
        // A = {0,1,2,3}, B stored as complement of {2,3} → B = universe \ {2,3}
        // A ∩ B = {0,1,2,3} ∩ (universe \ {2,3}) = {0,1}
        let a = Bitmap::from_sorted_iter([0, 1, 2, 3].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([2, 3].into_iter(), 64);
        let c = &a & &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 2);
        assert!(c.contains(0));
        assert!(c.contains(1));
        assert!(!c.contains(2));
        assert!(!c.contains(3));
    }

    #[test]
    fn test_bitmap_and_in() {
        let a = Bitmap::from_sorted_iter_complemented([2, 3].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([0, 1, 2, 3].into_iter(), 64);
        let c = &a & &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 2);
        assert!(c.contains(0));
        assert!(c.contains(1));
        assert!(!c.contains(2));
    }

    #[test]
    fn test_bitmap_and_ii() {
        // A = universe \ {0,1}, B = universe \ {2,3}
        // A ∩ B = universe \ ({0,1} ∪ {2,3}) = universe \ {0,1,2,3}
        let a = Bitmap::from_sorted_iter_complemented([0, 1].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([2, 3].into_iter(), 64);
        let c = &a & &b;
        assert!(c.is_inverted());
        assert!(!c.contains(0));
        assert!(!c.contains(1));
        assert!(!c.contains(2));
        assert!(!c.contains(3));
        assert!(c.contains(4));
        assert!(c.contains(63));
    }

    #[test]
    fn test_bitmap_or_nn() {
        let a = Bitmap::from_sorted_iter([0, 1].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([2, 3].into_iter(), 64);
        let c = &a | &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 4);
        assert!(c.contains(0));
        assert!(c.contains(3));
        assert!(!c.contains(4));
    }

    #[test]
    fn test_bitmap_or_ni() {
        // A = {10, 20}, B = universe \ {5, 10, 15}
        // A ∪ B = universe \ ({5, 10, 15} \ {10, 20}) = universe \ {5, 15}
        let a = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([5, 10, 15].into_iter(), 64);
        let c = &a | &b;
        assert!(c.is_inverted());
        assert!(!c.contains(5));
        assert!(c.contains(10));
        assert!(!c.contains(15));
        assert!(c.contains(20));
        assert!(c.contains(0));
        assert!(c.contains(63));
    }

    #[test]
    fn test_bitmap_or_in() {
        let a = Bitmap::from_sorted_iter_complemented([5, 10, 15].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let c = &a | &b;
        assert!(c.is_inverted());
        assert!(!c.contains(5));
        assert!(c.contains(10));
        assert!(!c.contains(15));
        assert!(c.contains(20));
    }

    #[test]
    fn test_bitmap_or_ii() {
        // A = universe \ {0,1,2}, B = universe \ {2,3,4}
        // A ∪ B = universe \ ({0,1,2} ∩ {2,3,4}) = universe \ {2}
        let a = Bitmap::from_sorted_iter_complemented([0, 1, 2].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([2, 3, 4].into_iter(), 64);
        let c = &a | &b;
        assert!(c.is_inverted());
        assert_eq!(c.len(), 63);
        assert!(c.contains(0));
        assert!(c.contains(1));
        assert!(!c.contains(2));
        assert!(c.contains(3));
        assert!(c.contains(4));
    }

    #[test]
    fn test_bitmap_and_with_empty() {
        let a = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let b = Bitmap::empty(64);
        let c = &a & &b;
        assert!(c.is_empty());

        let c = &a & &Bitmap::full(64);
        assert_eq!(c.len(), 2);
        assert!(c.contains(10));
        assert!(c.contains(20));
    }

    #[test]
    fn test_bitmap_or_with_full() {
        let a = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let b = Bitmap::full(64);
        let c = &a | &b;
        assert_eq!(c.len(), 64);
    }

    #[test]
    fn test_bitmap_assign_variants() {
        let b = Bitmap::from_sorted_iter([2, 3, 4].into_iter(), 64);

        let mut a = Bitmap::from_sorted_iter([0, 1, 2].into_iter(), 64);
        a &= &b;
        assert!(a.contains(2));
        assert!(!a.contains(0));

        let mut a = Bitmap::from_sorted_iter([0, 1, 2].into_iter(), 64);
        a |= &b;
        for v in [0, 1, 2, 3, 4] {
            assert!(a.contains(v), "missing {v}");
        }
    }

    #[test]
    fn test_bitmap_and_exhaustive() {
        // Test all 4 representation combos against brute-force reference.
        let universe = 64u32;
        let a_vals: Vec<u32> = vec![0, 1, 7, 8, 32, 63];
        let b_vals: Vec<u32> = vec![1, 8, 32, 40, 63];
        let a_complement: Vec<u32> = (0..universe).filter(|v| !a_vals.contains(v)).collect();
        let b_complement: Vec<u32> = (0..universe).filter(|v| !b_vals.contains(v)).collect();

        let representations = [
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "N&N",
            ),
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "N&I",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "I&N",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "I&I",
            ),
        ];

        for (a, b, label) in &representations {
            let c = a & b;
            for v in 0..universe {
                let expected = a_vals.contains(&v) && b_vals.contains(&v);
                assert_eq!(c.contains(v), expected, "{label}: value {v}");
            }
        }
    }

    #[test]
    fn test_bitmap_or_exhaustive() {
        let universe = 64u32;
        let a_vals: Vec<u32> = vec![0, 1, 7, 8, 32, 63];
        let b_vals: Vec<u32> = vec![1, 8, 32, 40, 63];
        let a_complement: Vec<u32> = (0..universe).filter(|v| !a_vals.contains(v)).collect();
        let b_complement: Vec<u32> = (0..universe).filter(|v| !b_vals.contains(v)).collect();

        let representations = [
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "N|N",
            ),
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "N|I",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "I|N",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "I|I",
            ),
        ];

        for (a, b, label) in &representations {
            let c = a | b;
            for v in 0..universe {
                let expected = a_vals.contains(&v) || b_vals.contains(&v);
                assert_eq!(c.contains(v), expected, "{label}: value {v}");
            }
        }
    }

