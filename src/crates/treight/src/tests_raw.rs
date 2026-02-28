    use crate::*;

    #[test]
    fn test_ceil_log8() {
        assert_eq!(ceil_log8(0), 0);
        assert_eq!(ceil_log8(1), 1);
        assert_eq!(ceil_log8(8), 1);
        assert_eq!(ceil_log8(9), 2);
        assert_eq!(ceil_log8(64), 2);
        assert_eq!(ceil_log8(65), 3);
        assert_eq!(ceil_log8(512), 3);
        assert_eq!(ceil_log8(513), 4);
    }

    #[test]
    fn test_empty_bitmap() {
        let bm = RawBitmap::empty(0);
        assert_eq!(bm.universe_size(), 0);
        assert_eq!(bm.levels(), 0);
        assert!(bm.data().is_empty());

        let bm = RawBitmap::empty(1);
        assert_eq!(bm.universe_size(), 1);
        assert_eq!(bm.levels(), 1);
        assert!(bm.data().is_empty());

        let bm = RawBitmap::empty(64);
        assert_eq!(bm.universe_size(), 64);
        assert_eq!(bm.levels(), 2);
        assert!(bm.data().is_empty());

        let bm = RawBitmap::empty(512);
        assert_eq!(bm.universe_size(), 512);
        assert_eq!(bm.levels(), 3);
        assert!(bm.data().is_empty());
    }

    #[test]
    fn test_build_universe8_bit0() {
        // universe=8, 1 level. Value 0 → treight [0x01]
        let bm = RawBitmap::from_sorted_iter([0].into_iter(), 8);
        assert_eq!(bm.data(), &[0x01]);
    }

    #[test]
    fn test_build_universe8_bit7() {
        let bm = RawBitmap::from_sorted_iter([7].into_iter(), 8);
        assert_eq!(bm.data(), &[0x80]);
    }

    #[test]
    fn test_build_universe8_all() {
        let bm = RawBitmap::from_sorted_iter(0..8, 8);
        assert_eq!(bm.data(), &[0xFF]);
    }

    #[test]
    fn test_build_universe16_insert_0_8() {
        // universe=16, 2 levels.
        // Serialize: root=0x03, child0=0x01, child1=0x01
        let bm = RawBitmap::from_sorted_iter([0, 8].into_iter(), 16);
        assert_eq!(bm.data(), &[0x03, 0x01, 0x01]);
    }

    #[test]
    fn test_build_universe64_insert_63() {
        // universe=64, 2 levels.
        // Serialize: root=0x80, leaf=0x80
        let bm = RawBitmap::from_sorted_iter([63].into_iter(), 64);
        assert_eq!(bm.data(), &[0x80, 0x80]);
    }

    #[test]
    fn test_build_universe9_insert_8() {
        // universe=9, 2 levels. Value 8 → root=0x02, leaf=0x01
        let bm = RawBitmap::from_sorted_iter([8].into_iter(), 9);
        assert_eq!(bm.levels(), 2);
        assert_eq!(bm.data(), &[0x02, 0x01]);
    }

    #[test]
    fn test_contains_universe8() {
        let bm = make_bitmap(8, &[0, 3, 7]);
        for i in 0..8 {
            assert_eq!(bm.contains(i), i == 0 || i == 3 || i == 7, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe16() {
        let bm = make_bitmap(16, &[0, 8]);
        for i in 0..16 {
            assert_eq!(bm.contains(i), i == 0 || i == 8, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe64_sparse() {
        let bm = make_bitmap(64, &[0, 31, 63]);
        for i in 0..64 {
            assert_eq!(bm.contains(i), i == 0 || i == 31 || i == 63, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe64_dense() {
        let values: Vec<u32> = (0..64).collect();
        let bm = make_bitmap(64, &values);
        for i in 0..64 {
            assert!(bm.contains(i), "value={i}");
        }
        assert!(!bm.contains(64));
    }

    #[test]
    fn test_contains_empty() {
        let bm = RawBitmap::empty(64);
        for i in 0..64 {
            assert!(!bm.contains(i));
        }
    }

    #[test]
    fn test_contains_universe9_insert_8() {
        let bm = make_bitmap(9, &[8]);
        for i in 0..9 {
            assert_eq!(bm.contains(i), i == 8, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe512() {
        let values = [0, 1, 7, 8, 63, 64, 255, 256, 511];
        let bm = make_bitmap(512, &values);
        assert_eq!(bm.levels(), 3);
        for i in 0..512 {
            assert_eq!(bm.contains(i), values.contains(&i), "value={i}");
        }
    }

    #[test]
    fn test_iter_ascending_order() {
        let bm = make_bitmap(512, &[511, 0, 255, 1, 63, 64, 8, 7, 256]);
        let result: Vec<u32> = bm.iter().collect();
        assert_eq!(result, vec![0, 1, 7, 8, 63, 64, 255, 256, 511]);
    }

    #[test]
    fn test_iter_empty() {
        let bm = RawBitmap::empty(64);
        let result: Vec<u32> = bm.iter().collect();
        assert!(result.is_empty());
    }

    #[test]
    fn test_iter_single_element() {
        let bm = make_bitmap(64, &[42]);
        let result: Vec<u32> = bm.iter().collect();
        assert_eq!(result, vec![42]);
    }

    #[test]
    fn test_iter_full() {
        let bm = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let result: Vec<u32> = bm.iter().collect();
        assert_eq!(result, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_len() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.len(), 0);

        let bm = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        assert_eq!(bm.len(), 9);
    }

    #[test]
    fn test_is_empty() {
        assert!(RawBitmap::empty(64).is_empty());
        assert!(!make_bitmap(8, &[0]).is_empty());
    }

    #[test]
    fn test_min_max() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.min(), None);
        assert_eq!(bm.max(), None);

        let bm = make_bitmap(512, &[42]);
        assert_eq!(bm.min(), Some(42));
        assert_eq!(bm.max(), Some(42));

        let bm = make_bitmap(512, &[0, 255, 511]);
        assert_eq!(bm.min(), Some(0));
        assert_eq!(bm.max(), Some(511));
    }

    #[test]
    fn test_min_max_universe8() {
        let bm = make_bitmap(8, &[3, 5]);
        assert_eq!(bm.min(), Some(3));
        assert_eq!(bm.max(), Some(5));
    }

    fn make_bitmap(universe_size: u32, values: &[u32]) -> RawBitmap {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        RawBitmap::from_sorted_iter(sorted.into_iter(), universe_size)
    }

    #[test]
    fn test_bitor_disjoint() {
        let a = make_bitmap(64, &[0, 1, 2]);
        let b = make_bitmap(64, &[60, 61, 62]);
        let c = &a | &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1, 2, 60, 61, 62]);
    }

    #[test]
    fn test_bitor_overlapping() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a | &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_bitor_empty() {
        let a = make_bitmap(64, &[1, 2, 3]);
        let b = RawBitmap::empty(64);
        let c = &a | &b;
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitor_single_level() {
        let a = make_bitmap(8, &[0, 2, 4]);
        let b = make_bitmap(8, &[1, 3, 5]);
        let c = &a | &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_bitor_single_level_overlapping() {
        let a = make_bitmap(8, &[0, 1, 2]);
        let b = make_bitmap(8, &[1, 2, 3]);
        let c = &a | &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_bitor_one_empty_both_directions() {
        let a = make_bitmap(512, &[0, 100, 511]);
        let b = RawBitmap::empty(512);
        assert_eq!(&a | &b, a);
        assert_eq!(&b | &a, a);
    }

    #[test]
    fn test_bitor_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        let c = &a | &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitor_with_self() {
        let a = make_bitmap(512, &[0, 7, 42, 100, 255, 511]);
        let c = &a | &a;
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitor_3level_disjoint_octants() {
        // Values in completely different top-level children — exercises copy_subtree
        let a = make_bitmap(512, &[0, 1, 2]);
        let b = make_bitmap(512, &[500, 510, 511]);
        let c = &a | &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 500, 510, 511]);
    }

    #[test]
    fn test_bitor_3level_partial_overlap() {
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a | &b;
        let mut expected = vec![0, 8, 9, 64, 65, 256, 300];
        expected.sort();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitor_4level() {
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a | &b;
        let mut expected = vec![0, 511, 512, 1000, 2000, 3000, 4095];
        expected.sort();
        expected.dedup();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitor_result_valid_contains() {
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a | &b;
        for v in c.iter() {
            assert!(
                a.contains(v) || b.contains(v),
                "result {v} not in either operand"
            );
        }
        for v in 0..512 {
            if a.contains(v) || b.contains(v) {
                assert!(c.contains(v), "value {v} missing from union");
            }
        }
    }

    #[test]
    fn test_bitand_overlapping() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a & &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![2, 3]);
    }

    #[test]
    fn test_bitand_disjoint() {
        let a = make_bitmap(64, &[0, 1]);
        let b = make_bitmap(64, &[2, 3]);
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_single_level() {
        // universe=8 → 1 level, leaf-only AND
        let a = make_bitmap(8, &[0, 1, 3, 5, 7]);
        let b = make_bitmap(8, &[1, 2, 3, 6, 7]);
        let c = &a & &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![1, 3, 7]);
    }

    #[test]
    fn test_bitand_single_level_disjoint() {
        let a = make_bitmap(8, &[0, 2, 4]);
        let b = make_bitmap(8, &[1, 3, 5]);
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_one_empty() {
        let a = make_bitmap(512, &[0, 100, 255, 511]);
        let b = RawBitmap::empty(512);
        assert!((&a & &b).is_empty());
        assert!((&b & &a).is_empty());
    }

    #[test]
    fn test_bitand_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        assert!((&a & &b).is_empty());
    }

    #[test]
    fn test_bitand_with_self() {
        let a = make_bitmap(512, &[0, 7, 42, 100, 255, 511]);
        let c = &a & &a;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 7, 42, 100, 255, 511]);
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitand_3level_sparse() {
        // 3-level tree (universe=512), values in completely different octants
        let a = make_bitmap(512, &[0, 1, 2]); // all in first leaf group
        let b = make_bitmap(512, &[500, 510, 511]); // all in last leaf group
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_3level_partial_overlap() {
        // Inner nodes overlap but only some leaves do
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a & &b;
        // Only value 0 is in both
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn test_bitand_inner_overlap_leaf_disjoint() {
        // Values share the same inner-node path but different leaf bits
        // In universe=64 (2 levels), values 0..7 share leaf byte 0.
        // a has bit 0, b has bit 7 — same inner child, different leaf bits.
        let a = make_bitmap(64, &[0]);
        let b = make_bitmap(64, &[7]);
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_4level() {
        // 4-level tree: universe=4096
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a & &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1000, 4095]);
    }

    #[test]
    fn test_bitand_dense() {
        // Both sides fully dense in a single-level tree
        let a = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let b = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let c = &a & &b;
        assert_eq!(c.len(), 8);
    }

    #[test]
    fn test_bitand_subset() {
        // a is a subset of b — AND should equal a
        let a = make_bitmap(512, &[10, 20, 30]);
        let b = make_bitmap(512, &[10, 15, 20, 25, 30, 35]);
        let c = &a & &b;
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitand_result_valid_contains() {
        // Verify every value in the result is in both operands,
        // and every value in both operands is in the result.
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a & &b;
        for v in c.iter() {
            assert!(a.contains(v), "result {v} not in a");
            assert!(b.contains(v), "result {v} not in b");
        }
        for v in 0..512 {
            if a.contains(v) && b.contains(v) {
                assert!(c.contains(v), "common value {v} missing from result");
            }
        }
    }

    #[test]
    fn test_sub() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a - &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1]);
    }

    #[test]
    fn test_sub_single_level() {
        let a = make_bitmap(8, &[0, 1, 2, 3, 4]);
        let b = make_bitmap(8, &[2, 3, 5]);
        let c = &a - &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 4]);
    }

    #[test]
    fn test_sub_one_empty() {
        let a = make_bitmap(512, &[0, 100, 511]);
        let b = RawBitmap::empty(512);
        assert_eq!(&a - &b, a);

        let c = &b - &a;
        assert!(c.is_empty());
    }

    #[test]
    fn test_sub_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        assert!((&a - &b).is_empty());
    }

    #[test]
    fn test_sub_with_self() {
        let a = make_bitmap(512, &[0, 42, 255, 511]);
        let c = &a - &a;
        assert!(c.is_empty());
    }

    #[test]
    fn test_sub_disjoint() {
        // Nothing to subtract — result equals a.
        let a = make_bitmap(512, &[0, 1, 2]);
        let b = make_bitmap(512, &[500, 510, 511]);
        let c = &a - &b;
        assert_eq!(c, a);
    }

    #[test]
    fn test_sub_3level_partial() {
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a - &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![8, 64, 256]);
    }

    #[test]
    fn test_sub_superset() {
        // b is a superset of a — result is empty.
        let a = make_bitmap(512, &[10, 20, 30]);
        let b = make_bitmap(512, &[10, 15, 20, 25, 30, 35]);
        let c = &a - &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_sub_4level() {
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a - &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![511, 2000]);
    }

    #[test]
    fn test_sub_result_valid() {
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a - &b;
        for v in c.iter() {
            assert!(a.contains(v), "result {v} not in a");
            assert!(!b.contains(v), "result {v} should not be in b");
        }
        for v in 0..512 {
            if a.contains(v) && !b.contains(v) {
                assert!(c.contains(v), "value {v} missing from difference");
            }
        }
    }

    #[test]
    fn test_bitxor() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a ^ &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1, 4, 5]);
    }

    #[test]
    fn test_bitxor_single_level() {
        let a = make_bitmap(8, &[0, 1, 2, 3]);
        let b = make_bitmap(8, &[2, 3, 4, 5]);
        let c = &a ^ &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 4, 5]);
    }

    #[test]
    fn test_bitxor_one_empty() {
        let a = make_bitmap(512, &[0, 100, 511]);
        let b = RawBitmap::empty(512);
        assert_eq!(&a ^ &b, a);
        assert_eq!(&b ^ &a, a);
    }

    #[test]
    fn test_bitxor_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        assert!((&a ^ &b).is_empty());
    }

    #[test]
    fn test_bitxor_with_self() {
        let a = make_bitmap(512, &[0, 42, 255, 511]);
        let c = &a ^ &a;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitxor_disjoint() {
        // Disjoint — XOR equals union.
        let a = make_bitmap(512, &[0, 1, 2]);
        let b = make_bitmap(512, &[500, 510, 511]);
        let c = &a ^ &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 500, 510, 511]);
    }

    #[test]
    fn test_bitxor_3level_partial() {
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a ^ &b;
        let mut expected = vec![8, 9, 64, 65, 256, 300];
        expected.sort();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitxor_4level() {
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a ^ &b;
        let mut expected = vec![511, 512, 2000, 3000];
        expected.sort();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitxor_result_valid() {
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a ^ &b;
        for v in c.iter() {
            assert!(
                a.contains(v) ^ b.contains(v),
                "result {v} should be in exactly one operand"
            );
        }
        for v in 0..512 {
            if a.contains(v) ^ b.contains(v) {
                assert!(c.contains(v), "value {v} missing from xor");
            }
        }
    }

    #[test]
    fn test_bitor_commutativity() {
        let a = make_bitmap(64, &[0, 10, 20]);
        let b = make_bitmap(64, &[5, 15, 25]);
        assert_eq!(&a | &b, &b | &a);
    }

    #[test]
    fn test_bitand_commutativity() {
        let a = make_bitmap(64, &[0, 10, 20, 30]);
        let b = make_bitmap(64, &[10, 20, 40, 50]);
        assert_eq!(&a & &b, &b & &a);
    }

    #[test]
    fn test_bitxor_commutativity() {
        let a = make_bitmap(64, &[0, 10, 20]);
        let b = make_bitmap(64, &[10, 20, 30]);
        assert_eq!(&a ^ &b, &b ^ &a);
    }

    #[test]
    fn test_intersection_subset_of_union() {
        let a = make_bitmap(512, &[0, 100, 200, 300, 400, 511]);
        let b = make_bitmap(512, &[50, 100, 250, 300, 450, 511]);
        let intersection = &a & &b;
        let union = &a | &b;
        for val in intersection.iter() {
            assert!(union.contains(val));
        }
    }

    #[test]
    fn test_bitor_full() {
        let a = make_bitmap(8, &[0, 1, 2, 3]);
        let b = make_bitmap(8, &[4, 5, 6, 7]);
        let c = &a | &b;
        assert_eq!(c.len(), 8);
    }

    #[test]
    fn test_assign_variants() {
        let mut a = make_bitmap(64, &[0, 1, 2]);
        let b = make_bitmap(64, &[2, 3, 4]);
        a |= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);

        let mut a = make_bitmap(64, &[0, 1, 2, 3]);
        a &= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![2, 3]);

        let mut a = make_bitmap(64, &[0, 1, 2, 3]);
        a -= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![0, 1]);

        let mut a = make_bitmap(64, &[0, 1, 2, 3]);
        a ^= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![0, 1, 4]);
    }

    #[test]
    #[should_panic(expected = "universe_size mismatch")]
    fn test_set_op_mismatched_universe() {
        let a = make_bitmap(64, &[0]);
        let b = make_bitmap(128, &[0]);
        let _ = &a | &b;
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let bm = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let mut buf = Vec::new();
        bm.serialize_into(&mut buf).unwrap();
        let bm2 = RawBitmap::deserialize_from(&buf[..]).unwrap();
        assert_eq!(bm, bm2);
    }

    #[test]
    fn test_serialize_deserialize_empty() {
        let bm = RawBitmap::empty(64);
        let mut buf = Vec::new();
        bm.serialize_into(&mut buf).unwrap();
        let bm2 = RawBitmap::deserialize_from(&buf[..]).unwrap();
        assert_eq!(bm, bm2);
    }

    #[test]
    fn test_serialized_size_matches() {
        let bm = make_bitmap(512, &[0, 100, 200, 300, 511]);
        let mut buf = Vec::new();
        bm.serialize_into(&mut buf).unwrap();
        assert_eq!(buf.len(), bm.serialized_size());
    }

    #[test]
    fn test_deserialize_truncated() {
        // Only 4 bytes (missing data_len and data).
        let buf = [1u8, 0, 0, 0];
        let result = RawBitmap::deserialize_from(&buf[..]);
        assert!(result.is_err());
    }

    #[test]
    fn test_heap_bytes() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.heap_bytes(), 0);

        let bm = make_bitmap(8, &[0]);
        assert_eq!(bm.heap_bytes(), bm.data().len());
    }

    #[test]
    fn test_insert_into_empty() {
        let mut bm = RawBitmap::empty(64);
        assert!(bm.is_empty());
        bm.insert(42);
        assert!(!bm.is_empty());
        assert!(bm.contains(42));
        assert_eq!(bm.len(), 1);
    }

    #[test]
    fn test_insert_duplicate_noop() {
        let mut bm = make_bitmap(64, &[10, 20]);
        let before = bm.clone();
        bm.insert(10);
        assert_eq!(bm, before);
    }

    #[test]
    fn test_remove_from_populated() {
        let mut bm = make_bitmap(64, &[10, 20, 30]);
        bm.remove(20);
        assert!(!bm.contains(20));
        assert!(bm.contains(10));
        assert!(bm.contains(30));
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn test_remove_absent_noop() {
        let mut bm = make_bitmap(64, &[10, 20]);
        let before = bm.clone();
        bm.remove(30);
        assert_eq!(bm, before);
    }

    #[test]
    fn test_clear() {
        let mut bm = make_bitmap(64, &[10, 20, 30]);
        bm.clear();
        assert!(bm.is_empty());
        assert_eq!(bm.len(), 0);
        for i in 0..64 {
            assert!(!bm.contains(i));
        }
    }

    #[test]
    fn test_mutation_sequence() {
        let mut bm = RawBitmap::empty(512);
        bm.insert(0);
        bm.insert(100);
        bm.insert(511);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 100, 511]);

        bm.remove(100);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 511]);

        bm.insert(200);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 200, 511]);

        bm.clear();
        assert!(bm.is_empty());

        bm.insert(42);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![42]);
    }

    #[test]
    fn test_remove_last_element() {
        let mut bm = make_bitmap(8, &[3]);
        bm.remove(3);
        assert!(bm.is_empty());
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn test_insert_out_of_bounds_mutation() {
        let mut bm = RawBitmap::empty(8);
        bm.insert(8);
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn test_remove_out_of_bounds() {
        let mut bm = RawBitmap::empty(8);
        bm.remove(8);
    }

    #[test]
    fn test_from_sorted_iter_correctness() {
        // Verify from_sorted_iter produces bitmaps with correct contains/iter.
        let cases: Vec<(u32, Vec<u32>)> = vec![
            (8, vec![0]),
            (8, vec![7]),
            (8, vec![0, 1, 2, 3, 4, 5, 6, 7]),
            (16, vec![0, 8]),
            (64, vec![63]),
            (64, vec![0, 31, 63]),
            (512, vec![0, 1, 7, 8, 63, 64, 255, 256, 511]),
            (9, vec![8]),
            (1_000_000, vec![950_000]),
            (1_000_000, vec![0, 500_000, 999_999]),
        ];
        for (universe_size, values) in &cases {
            let bm = RawBitmap::from_sorted_iter(values.iter().copied(), *universe_size);
            let result: Vec<u32> = bm.iter().collect();
            assert_eq!(
                result, *values,
                "universe={universe_size}, values={values:?}"
            );
            for &v in values {
                assert!(bm.contains(v), "universe={universe_size}, missing {v}");
            }
            assert_eq!(bm.len(), values.len() as u64);
        }
    }

    #[test]
    fn test_from_sorted_iter_empty() {
        let bm = RawBitmap::from_sorted_iter(std::iter::empty(), 64);
        assert!(bm.is_empty());
        assert_eq!(bm.universe_size(), 64);

        let bm = RawBitmap::from_sorted_iter(std::iter::empty(), 0);
        assert!(bm.is_empty());
        assert_eq!(bm.levels(), 0);
    }

    #[test]
    fn test_from_sorted_iter_single_level() {
        let bm = RawBitmap::from_sorted_iter([3, 5].into_iter(), 8);
        assert_eq!(bm.data(), &[0x28]); // bits 3 and 5
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![3, 5]);
    }

    #[test]
    fn test_from_sorted_iter_duplicates_tolerated() {
        let bm = RawBitmap::from_sorted_iter([10, 10, 20, 20, 20].into_iter(), 64);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![10, 20]);
    }

    #[test]
    fn test_from_sorted_iter_dense() {
        let values: Vec<u32> = (0..64).collect();
        let bm = RawBitmap::from_sorted_iter(values.iter().copied(), 64);
        assert_eq!(bm.len(), 64);
        for v in 0..64 {
            assert!(bm.contains(v));
        }
    }

    #[test]
    fn test_from_sorted_iter_large_universe() {
        let bm = RawBitmap::from_sorted_iter([0, 500_000, 999_999].into_iter(), 1_000_000);
        assert_eq!(bm.len(), 3);
        assert!(bm.contains(0));
        assert!(bm.contains(500_000));
        assert!(bm.contains(999_999));
        assert!(!bm.contains(1));
    }

    #[test]
    fn test_from_range_full() {
        let bm = RawBitmap::from_range(0..64, 64);
        assert_eq!(bm.len(), 64);
        for v in 0..64 {
            assert!(bm.contains(v));
        }
    }

    #[test]
    fn test_from_range_partial() {
        let bm = RawBitmap::from_range(10..20, 64);
        assert_eq!(bm.len(), 10);
        for v in 0..64 {
            assert_eq!(bm.contains(v), (10..20).contains(&v));
        }
    }

    #[test]
    fn test_from_range_inclusive() {
        let bm = RawBitmap::from_range(5..=10, 64);
        assert_eq!(bm.len(), 6);
        assert!(bm.contains(5));
        assert!(bm.contains(10));
        assert!(!bm.contains(4));
        assert!(!bm.contains(11));
    }

    #[test]
    fn test_from_range_unbounded() {
        let bm = RawBitmap::from_range(.., 64);
        assert_eq!(bm.len(), 64);

        let bm = RawBitmap::from_range(..32, 64);
        assert_eq!(bm.len(), 32);
        assert!(bm.contains(0));
        assert!(!bm.contains(32));

        let bm = RawBitmap::from_range(32.., 64);
        assert_eq!(bm.len(), 32);
        assert!(!bm.contains(31));
        assert!(bm.contains(32));
        assert!(bm.contains(63));
    }

    #[test]
    fn test_from_range_empty() {
        let bm = RawBitmap::from_range(10..10, 64);
        assert!(bm.is_empty());

        let bm = RawBitmap::from_range(20..10, 64);
        assert!(bm.is_empty());
    }

    #[test]
    fn test_from_range_clamped_to_universe() {
        let bm = RawBitmap::from_range(0..1000, 64);
        assert_eq!(bm.len(), 64);

        let bm = RawBitmap::from_range(60..1000, 64);
        assert_eq!(bm.len(), 4);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![60, 61, 62, 63]);
    }

    #[test]
    fn test_from_range_matches_from_sorted_iter() {
        for &universe in &[8, 64, 512, 4096] {
            let from_range = RawBitmap::from_range(0..universe, universe);
            let from_iter = RawBitmap::from_sorted_iter(0..universe, universe);
            assert_eq!(from_range, from_iter, "universe={universe}");

            let from_range = RawBitmap::from_range(universe / 4..universe * 3 / 4, universe);
            let start = universe / 4;
            let end = universe * 3 / 4;
            let from_iter = RawBitmap::from_sorted_iter(start..end, universe);
            assert_eq!(
                from_range, from_iter,
                "universe={universe} range={start}..{end}"
            );
        }
    }

    #[test]
    fn test_range_cardinality_full() {
        let bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        assert_eq!(bm.range_cardinality(..), 64);
        assert_eq!(bm.range_cardinality(0..64), 64);
        assert_eq!(bm.range_cardinality(0..=63), 64);
    }

    #[test]
    fn test_range_cardinality_equals_len() {
        let bm = make_bitmap(512, &[0, 7, 8, 63, 64, 255, 256, 511]);
        assert_eq!(bm.range_cardinality(..), bm.len());
        assert_eq!(bm.range_cardinality(0..512), bm.len());
    }

    #[test]
    fn test_range_cardinality_partial() {
        let bm = make_bitmap(64, &[0, 1, 2, 10, 20, 30, 40, 50, 60, 63]);
        assert_eq!(bm.range_cardinality(0..3), 3);
        assert_eq!(bm.range_cardinality(10..11), 1);
        assert_eq!(bm.range_cardinality(3..10), 0);
        assert_eq!(bm.range_cardinality(10..=20), 2);
        assert_eq!(bm.range_cardinality(..10), 3);
        assert_eq!(bm.range_cardinality(60..), 2);
    }

    #[test]
    fn test_range_cardinality_empty_bitmap() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.range_cardinality(..), 0);
        assert_eq!(bm.range_cardinality(0..64), 0);
    }

    #[test]
    fn test_range_cardinality_empty_range() {
        let bm = make_bitmap(64, &[0, 1, 2, 3]);
        assert_eq!(bm.range_cardinality(10..10), 0);
        assert_eq!(bm.range_cardinality(10..5), 0);
    }

    #[test]
    fn test_range_cardinality_multi_level() {
        let bm = make_bitmap(4096, &[0, 100, 500, 1000, 2000, 3000, 4095]);
        assert_eq!(bm.range_cardinality(0..101), 2);
        assert_eq!(bm.range_cardinality(100..1001), 3);
        assert_eq!(bm.range_cardinality(1000..4096), 4);
        assert_eq!(bm.range_cardinality(3000..=4095), 2);
    }

    #[test]
    fn test_range_cardinality_single_level() {
        let bm = make_bitmap(8, &[1, 3, 5, 7]);
        assert_eq!(bm.range_cardinality(0..4), 2);
        assert_eq!(bm.range_cardinality(4..8), 2);
        assert_eq!(bm.range_cardinality(0..8), 4);
        assert_eq!(bm.range_cardinality(1..=5), 3);
    }

    #[test]
    fn test_remove_range_all() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(..);
        assert!(bm.is_empty());
    }

    #[test]
    fn test_remove_range_prefix() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(..32);
        assert_eq!(bm.len(), 32);
        assert!(!bm.contains(31));
        assert!(bm.contains(32));
        assert!(bm.contains(63));
    }

    #[test]
    fn test_remove_range_suffix() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(32..);
        assert_eq!(bm.len(), 32);
        assert!(bm.contains(0));
        assert!(bm.contains(31));
        assert!(!bm.contains(32));
    }

    #[test]
    fn test_remove_range_middle() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(10..20);
        assert_eq!(bm.len(), 54);
        assert!(bm.contains(9));
        assert!(!bm.contains(10));
        assert!(!bm.contains(19));
        assert!(bm.contains(20));
    }

    #[test]
    fn test_remove_range_empty_range() {
        let vals: Vec<u32> = (0..64).collect();
        let mut bm = make_bitmap(64, &vals);
        let orig = bm.clone();
        bm.remove_range(10..10);
        assert_eq!(bm, orig);
    }

    #[test]
    fn test_remove_range_no_overlap() {
        let mut bm = make_bitmap(64, &[0, 1, 2, 60, 61, 62]);
        let orig = bm.clone();
        bm.remove_range(30..40);
        assert_eq!(bm, orig);
    }

    #[test]
    fn test_remove_range_single_level() {
        let mut bm = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        bm.remove_range(2..6);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 1, 6, 7]);
    }

    #[test]
    fn test_remove_range_multi_level() {
        let mut bm = make_bitmap(4096, &[0, 100, 500, 1000, 2000, 3000, 4095]);
        bm.remove_range(100..3000);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 3000, 4095]);
    }

    #[test]
    fn test_remove_range_inclusive() {
        let mut bm = make_bitmap(64, &[5, 10, 15, 20, 25]);
        bm.remove_range(10..=20);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![5, 25]);
    }

    #[test]
    fn test_remove_range_empty_bitmap() {
        let mut bm = RawBitmap::empty(64);
        bm.remove_range(0..64);
        assert!(bm.is_empty());
    }

    #[test]
    fn test_estimate_data_size_matches_actual() {
        // Verify estimate matches actual data().len() for various cases.
        let cases: Vec<(u32, Vec<u32>)> = vec![
            (8, vec![0]),
            (8, vec![7]),
            (8, vec![0, 1, 2, 3, 4, 5, 6, 7]),
            (16, vec![0, 8]),
            (64, vec![63]),
            (64, vec![0, 31, 63]),
            (512, vec![0, 1, 7, 8, 63, 64, 255, 256, 511]),
            (9, vec![8]),
            (1_000_000, vec![950_000]),
            (1_000_000, vec![0, 500_000, 999_999]),
        ];
        for (universe_size, values) in &cases {
            let bm = make_bitmap(*universe_size, values);
            let estimated = estimate_data_size(*universe_size, values.iter().copied());
            assert_eq!(
                estimated,
                bm.data().len(),
                "universe={universe_size}, values={values:?}"
            );
        }
    }

    #[test]
    fn test_estimate_data_size_empty() {
        assert_eq!(estimate_data_size(64, std::iter::empty()), 0);
        assert_eq!(estimate_data_size(0, std::iter::empty()), 0);
    }

    #[test]
    fn test_estimate_data_size_single_element_is_levels() {
        // A single element always costs exactly `levels` bytes.
        for &universe in &[8, 64, 512, 4096, 1_000_000] {
            let levels = ceil_log8(universe) as usize;
            let size = estimate_data_size(universe, std::iter::once(0));
            assert_eq!(size, levels, "universe={universe}");
        }
    }

    #[test]
    fn test_estimate_data_size_dense() {
        // Dense bitmap: all values set in universe=64.
        let values: Vec<u32> = (0..64).collect();
        let bm = make_bitmap(64, &values);
        let estimated = estimate_data_size(64, values.iter().copied());
        assert_eq!(estimated, bm.data().len());
    }

