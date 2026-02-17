//! Compressed bitmap for efficient set operations on entry indices.

#[cfg(all(feature = "bitmap-roaring", feature = "bitmap-treight"))]
compile_error!("features `bitmap-roaring` and `bitmap-treight` are mutually exclusive");

#[cfg(not(any(feature = "bitmap-roaring", feature = "bitmap-treight")))]
compile_error!("exactly one of `bitmap-roaring` or `bitmap-treight` must be enabled");

// ---------------------------------------------------------------------------
// Roaring backend
// ---------------------------------------------------------------------------
#[cfg(feature = "bitmap-roaring")]
mod imp {
    use roaring::RoaringBitmap;
    use serde::{Deserialize, Serialize};

    /// A compressed bitmap representing a set of journal entry indices.
    ///
    /// Wraps [`RoaringBitmap`] and supports bitwise AND/OR operations for combining filters.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[cfg_attr(feature = "allocative", derive(allocative::Allocative))]
    #[serde(transparent)]
    pub struct Bitmap(pub RoaringBitmap);

    impl Bitmap {
        /// Create an empty bitmap.
        pub fn new() -> Self {
            Self(RoaringBitmap::new())
        }

        /// Create a bitmap from a sorted iterator of entry indices.
        ///
        /// The `_universe_size` parameter is ignored by the roaring backend.
        pub fn from_sorted_iter<I: IntoIterator<Item = u32>>(
            iterator: I,
            _universe_size: u32,
        ) -> Self {
            Bitmap(
                RoaringBitmap::from_sorted_iter(iterator)
                    .expect("bitmap input must be sorted"),
            )
        }

        /// Create a bitmap from a sorted iterator of the **complement** values
        /// (values NOT in the bitmap).
        ///
        /// The roaring backend ignores complement semantics and builds a full
        /// range bitmap, then removes the complement values. In practice this
        /// path is only taken under the treight backend.
        pub fn from_sorted_iter_complemented<I: IntoIterator<Item = u32>>(
            complement_iter: I,
            universe_size: u32,
        ) -> Self {
            let mut bm = Self::full(universe_size);
            for v in complement_iter {
                bm.0.remove(v);
            }
            bm
        }

        /// Create a bitmap containing all integers in `0..universe_size`.
        pub fn full(universe_size: u32) -> Self {
            let mut bitmap = Self::new();
            if universe_size > 0 {
                RoaringBitmap::insert_range(&mut bitmap.0, 0..universe_size);
            }
            bitmap
        }

        /// Optimize the internal representation for size/speed.
        pub fn optimize(&mut self) {
            self.0.optimize();
        }

        /// Count set bits (population count).
        pub fn len(&self) -> u64 {
            self.0.len()
        }

        /// Returns `true` if no bits are set.
        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        /// Test whether `value` is in the bitmap.
        pub fn contains(&self, value: u32) -> bool {
            self.0.contains(value)
        }

        /// Iterate over set bits in ascending order.
        pub fn iter(&self) -> roaring::bitmap::Iter<'_> {
            self.0.iter()
        }

        /// Count the number of set bits within a range.
        pub fn range_cardinality<R: std::ops::RangeBounds<u32>>(&self, range: R) -> u64 {
            self.0.range_cardinality(range)
        }
    }

    impl std::ops::BitAndAssign<&Bitmap> for Bitmap {
        fn bitand_assign(&mut self, rhs: &Bitmap) {
            self.0 &= &rhs.0;
        }
    }

    impl std::ops::BitAndAssign<Bitmap> for Bitmap {
        fn bitand_assign(&mut self, rhs: Bitmap) {
            self.0 &= rhs.0;
        }
    }

    impl std::ops::BitOrAssign<&Bitmap> for Bitmap {
        fn bitor_assign(&mut self, rhs: &Bitmap) {
            self.0 |= &rhs.0;
        }
    }

    impl std::ops::BitOrAssign<Bitmap> for Bitmap {
        fn bitor_assign(&mut self, rhs: Bitmap) {
            self.0 |= rhs.0;
        }
    }

    impl std::ops::BitAnd for &Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: &Bitmap) -> Bitmap {
            Bitmap(&self.0 & &rhs.0)
        }
    }

    impl std::ops::BitAnd<Bitmap> for &Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: Bitmap) -> Bitmap {
            Bitmap(&self.0 & rhs.0)
        }
    }

    impl std::ops::BitAnd<&Bitmap> for Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: &Bitmap) -> Bitmap {
            Bitmap(self.0 & &rhs.0)
        }
    }

    impl std::ops::BitAnd for Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: Bitmap) -> Bitmap {
            Bitmap(self.0 & rhs.0)
        }
    }
}

// ---------------------------------------------------------------------------
// Treight backend
// ---------------------------------------------------------------------------
#[cfg(feature = "bitmap-treight")]
mod imp {
    use serde::{Deserialize, Serialize};

    /// A compressed bitmap representing a set of journal entry indices.
    ///
    /// Wraps [`treight::Bitmap`] (8-way bit-tree with optional complement representation)
    /// and supports bitwise AND/OR operations for combining filters.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "allocative", derive(allocative::Allocative))]
    #[serde(transparent)]
    pub struct Bitmap(pub treight::Bitmap);

    impl Default for Bitmap {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Bitmap {
        /// Create an empty bitmap (universe_size = 0).
        pub fn new() -> Self {
            Self(treight::Bitmap::empty(0))
        }

        /// Create a bitmap from a sorted iterator of entry indices.
        pub fn from_sorted_iter<I: IntoIterator<Item = u32>>(
            iterator: I,
            universe_size: u32,
        ) -> Self {
            Bitmap(treight::Bitmap::from_sorted_iter(
                iterator.into_iter(),
                universe_size,
            ))
        }

        /// Create a bitmap from a sorted iterator of the **complement** values
        /// (values NOT in the bitmap).
        pub fn from_sorted_iter_complemented<I: IntoIterator<Item = u32>>(
            complement_iter: I,
            universe_size: u32,
        ) -> Self {
            Bitmap(treight::Bitmap::from_sorted_iter_complemented(
                complement_iter.into_iter(),
                universe_size,
            ))
        }

        /// Create a bitmap containing all integers in `0..universe_size`.
        pub fn full(universe_size: u32) -> Self {
            Bitmap(treight::Bitmap::full(universe_size))
        }

        /// No-op under the treight backend (treight has no `optimize()`).
        pub fn optimize(&mut self) {}

        /// Count set bits (population count).
        pub fn len(&self) -> u64 {
            self.0.len()
        }

        /// Returns `true` if no bits are set.
        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        /// Test whether `value` is in the bitmap.
        pub fn contains(&self, value: u32) -> bool {
            self.0.contains(value)
        }

        /// Iterate over set bits in ascending order.
        pub fn iter(&self) -> treight::BitmapIter<'_> {
            self.0.iter()
        }

        /// Count the number of set bits within a range.
        pub fn range_cardinality<R: std::ops::RangeBounds<u32>>(&self, range: R) -> u64 {
            self.0.range_cardinality(range)
        }
    }

    impl std::ops::BitAndAssign<&Bitmap> for Bitmap {
        fn bitand_assign(&mut self, rhs: &Bitmap) {
            self.0 &= &rhs.0;
        }
    }

    impl std::ops::BitAndAssign<Bitmap> for Bitmap {
        fn bitand_assign(&mut self, rhs: Bitmap) {
            self.0 &= &rhs.0;
        }
    }

    impl std::ops::BitOrAssign<&Bitmap> for Bitmap {
        fn bitor_assign(&mut self, rhs: &Bitmap) {
            self.0 |= &rhs.0;
        }
    }

    impl std::ops::BitOrAssign<Bitmap> for Bitmap {
        fn bitor_assign(&mut self, rhs: Bitmap) {
            self.0 |= &rhs.0;
        }
    }

    impl std::ops::BitAnd for &Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: &Bitmap) -> Bitmap {
            Bitmap(&self.0 & &rhs.0)
        }
    }

    impl std::ops::BitAnd<Bitmap> for &Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: Bitmap) -> Bitmap {
            Bitmap(&self.0 & &rhs.0)
        }
    }

    impl std::ops::BitAnd<&Bitmap> for Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: &Bitmap) -> Bitmap {
            Bitmap(&self.0 & &rhs.0)
        }
    }

    impl std::ops::BitAnd for Bitmap {
        type Output = Bitmap;

        fn bitand(self, rhs: Bitmap) -> Bitmap {
            Bitmap(&self.0 & &rhs.0)
        }
    }
}

pub use imp::Bitmap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_sorted_iter() {
        let bitmap = Bitmap::from_sorted_iter([0, 5, 10, 15], 20);

        assert_eq!(bitmap.len(), 4);
        assert!(bitmap.contains(5));
        assert!(!bitmap.contains(6));
    }

    #[test]
    fn test_full() {
        let bitmap = Bitmap::full(5);

        assert_eq!(bitmap.len(), 5);
        assert!(bitmap.contains(0));
        assert!(bitmap.contains(4));
        assert!(!bitmap.contains(5));
    }

    #[test]
    fn test_bitwise_operations() {
        let bitmap1 = Bitmap::from_sorted_iter([1, 2, 3], 5);
        let bitmap2 = Bitmap::from_sorted_iter([2, 3, 4], 5);

        let intersection = &bitmap1 & &bitmap2;
        assert_eq!(intersection.len(), 2);

        let mut union = bitmap1.clone();
        union |= bitmap2;
        assert_eq!(union.len(), 4);
    }

    #[test]
    fn test_empty_or_assign() {
        let mut empty = Bitmap::new();
        let bitmap = Bitmap::from_sorted_iter([1, 2, 3], 5);

        empty |= &bitmap;
        assert_eq!(empty.len(), 3);
        assert!(empty.contains(1));
        assert!(empty.contains(2));
        assert!(empty.contains(3));
    }

    #[test]
    fn test_empty_and_assign() {
        let mut empty = Bitmap::new();
        let bitmap = Bitmap::from_sorted_iter([1, 2, 3], 5);

        empty &= &bitmap;
        assert!(empty.is_empty());
    }

    #[test]
    fn test_range_cardinality() {
        let bitmap = Bitmap::from_sorted_iter([0, 1, 2, 5, 6, 7, 8, 9], 10);

        assert_eq!(bitmap.range_cardinality(0..3), 3);
        assert_eq!(bitmap.range_cardinality(5..10), 5);
        assert_eq!(bitmap.range_cardinality(3..5), 0);
    }

    #[test]
    fn test_iter() {
        let bitmap = Bitmap::from_sorted_iter([0, 5, 10, 15], 20);
        let values: Vec<u32> = bitmap.iter().collect();
        assert_eq!(values, vec![0, 5, 10, 15]);
    }
}
