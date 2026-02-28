use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign};

use crate::raw::{Iter, RawBitmap};

/// A bitmap that may store its complement for better compression.
///
/// For sparse bitmaps (few bits set), the set bits are stored directly.
/// For dense bitmaps (most bits set), the *unset* bits are stored instead,
/// and the meaning is inverted. This keeps the underlying `RawBitmap` small
/// at both extremes of the density spectrum.
///
/// AND and OR operations between bitmaps with mixed representations are
/// handled transparently using De Morgan's laws, dispatching to the
/// appropriate `RawBitmap` operation (AND, OR, or SUB).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "allocative", derive(allocative::Allocative))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Bitmap {
    inner: RawBitmap,
    inverted: bool,
}

impl Bitmap {
    /// Create an empty bitmap (no bits set).
    pub fn empty(universe_size: u32) -> Self {
        Self {
            inner: RawBitmap::empty(universe_size),
            inverted: false,
        }
    }

    /// Create a full bitmap (all bits set).
    pub fn full(universe_size: u32) -> Self {
        Self {
            inner: RawBitmap::empty(universe_size),
            inverted: true,
        }
    }

    /// Build from a sorted iterator of **set** values.
    ///
    /// The resulting bitmap stores these values directly (normal representation).
    pub fn from_sorted_iter(iter: impl Iterator<Item = u32>, universe_size: u32) -> Self {
        Self {
            inner: RawBitmap::from_sorted_iter(iter, universe_size),
            inverted: false,
        }
    }

    /// Build from a sorted iterator of **unset** values (the complement).
    ///
    /// The resulting bitmap logically contains every value in `0..universe_size`
    /// that is NOT yielded by `complement_iter`.
    pub fn from_sorted_iter_complemented(
        complement_iter: impl Iterator<Item = u32>,
        universe_size: u32,
    ) -> Self {
        Self {
            inner: RawBitmap::from_sorted_iter(complement_iter, universe_size),
            inverted: true,
        }
    }

    /// The universe size (exclusive upper bound on values).
    pub fn universe_size(&self) -> u32 {
        self.inner.universe_size()
    }

    /// Test whether `value` is in the bitmap.
    ///
    /// Values outside `0..universe_size` always return `false`, regardless
    /// of whether the bitmap is inverted.
    pub fn contains(&self, value: u32) -> bool {
        if value >= self.inner.universe_size() {
            return false;
        }
        self.inner.contains(value) ^ self.inverted
    }

    /// Count the number of set bits.
    pub fn len(&self) -> u64 {
        if self.inverted {
            self.inner.universe_size() as u64 - self.inner.len()
        } else {
            self.inner.len()
        }
    }

    /// Returns `true` if no bits are set.
    pub fn is_empty(&self) -> bool {
        if self.inverted {
            self.inner.len() == self.inner.universe_size() as u64
        } else {
            self.inner.is_empty()
        }
    }

    /// Whether this bitmap uses inverted (complemented) representation.
    pub fn is_inverted(&self) -> bool {
        self.inverted
    }

    /// Access the underlying raw bitmap.
    pub fn raw(&self) -> &RawBitmap {
        &self.inner
    }

    /// The number of heap-allocated bytes used by this bitmap.
    pub fn heap_bytes(&self) -> usize {
        self.inner.heap_bytes()
    }

    /// Build a bitmap with all values in the given range set.
    ///
    /// Values outside `0..universe_size` are clamped/ignored.
    pub fn from_range(range: impl std::ops::RangeBounds<u32>, universe_size: u32) -> Self {
        use std::ops::Bound;

        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n.saturating_add(1),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n.saturating_add(1).min(universe_size),
            Bound::Excluded(&n) => n.min(universe_size),
            Bound::Unbounded => universe_size,
        };

        if start >= end {
            return Self::empty(universe_size);
        }

        let range_len = (end - start) as u64;
        let half_universe = universe_size as u64 / 2;

        if range_len > half_universe {
            // Dense: store the complement (values outside the range).
            let complement = (0..start).chain(end..universe_size);
            Self::from_sorted_iter_complemented(complement, universe_size)
        } else {
            Self::from_sorted_iter(start..end, universe_size)
        }
    }

    /// Iterate over set bits in ascending order.
    ///
    /// For normal bitmaps, delegates to the underlying `RawBitmap` iterator.
    /// For inverted bitmaps, yields all values in `0..universe_size` that are
    /// NOT in the underlying raw bitmap.
    pub fn iter(&self) -> BitmapIter<'_> {
        if self.inverted {
            BitmapIter::Complement(ComplementIter {
                raw_iter: self.inner.iter(),
                next_raw: None,
                current: 0,
                universe_size: self.inner.universe_size(),
                started: false,
            })
        } else {
            BitmapIter::Normal(self.inner.iter())
        }
    }

    /// Count the number of set bits within a range.
    ///
    /// For inverted bitmaps, computes `range_len - raw.range_cardinality(range)`.
    pub fn range_cardinality(&self, range: impl std::ops::RangeBounds<u32>) -> u64 {
        use std::ops::Bound;

        if self.inverted {
            let start = match range.start_bound() {
                Bound::Included(&n) => n,
                Bound::Excluded(&n) => n.saturating_add(1),
                Bound::Unbounded => 0,
            };
            let end = match range.end_bound() {
                Bound::Included(&n) => n.saturating_add(1),
                Bound::Excluded(&n) => n,
                Bound::Unbounded => self.inner.universe_size(),
            };
            let end = end.min(self.inner.universe_size());

            if start >= end {
                return 0;
            }

            let range_len = (end - start) as u64;
            let raw_count = self.inner.range_cardinality(start..end);
            range_len - raw_count
        } else {
            self.inner.range_cardinality(range)
        }
    }
}

/// Iterator over set bits of a [`Bitmap`].
///
/// Handles both normal and inverted (complement) representations.
pub enum BitmapIter<'a> {
    /// Normal: yields values present in the raw bitmap.
    Normal(Iter<'a>),
    /// Complement: yields values in `0..universe_size` NOT in the raw bitmap.
    Complement(ComplementIter<'a>),
}

impl Iterator for BitmapIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        match self {
            BitmapIter::Normal(iter) => iter.next(),
            BitmapIter::Complement(iter) => iter.next(),
        }
    }
}

/// Iterator that yields values in `0..universe_size` that are NOT in the raw bitmap.
pub struct ComplementIter<'a> {
    raw_iter: Iter<'a>,
    next_raw: Option<u32>,
    current: u32,
    universe_size: u32,
    started: bool,
}

impl Iterator for ComplementIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if !self.started {
            self.next_raw = self.raw_iter.next();
            self.started = true;
        }

        loop {
            if self.current >= self.universe_size {
                return None;
            }

            let val = self.current;
            self.current += 1;

            match self.next_raw {
                Some(raw_val) if raw_val == val => {
                    // This value is in the raw bitmap (i.e., unset in the logical bitmap).
                    // Skip it and advance the raw iterator.
                    self.next_raw = self.raw_iter.next();
                    continue;
                }
                _ => return Some(val),
            }
        }
    }
}

impl<'a> IntoIterator for &'a Bitmap {
    type Item = u32;
    type IntoIter = BitmapIter<'a>;

    fn into_iter(self) -> BitmapIter<'a> {
        self.iter()
    }
}

impl BitAnd for &Bitmap {
    type Output = Bitmap;

    /// Intersection using De Morgan's dispatch:
    ///
    /// - N ∩ N → A & B (normal)
    /// - N ∩ I → A - B (normal)
    /// - I ∩ N → B - A (normal)
    /// - I ∩ I → A | B (inverted)
    ///
    /// Short-circuits when either operand is logically empty (annihilator)
    /// or logically full (identity).
    fn bitand(self, rhs: Self) -> Bitmap {
        // Short-circuit: empty AND anything = empty.
        if self.is_empty() {
            return self.clone();
        }
        if rhs.is_empty() {
            return rhs.clone();
        }

        // Short-circuit: full AND anything = anything.
        if self.inverted && self.inner.is_empty() {
            return rhs.clone();
        }
        if rhs.inverted && rhs.inner.is_empty() {
            return self.clone();
        }

        debug_assert_eq!(
            self.inner.universe_size(),
            rhs.inner.universe_size(),
            "universe_size mismatch: {} vs {}",
            self.inner.universe_size(),
            rhs.inner.universe_size()
        );

        let (inner, inverted) = match (self.inverted, rhs.inverted) {
            (false, false) => (&self.inner & &rhs.inner, false),
            (false, true) => (&self.inner - &rhs.inner, false),
            (true, false) => (&rhs.inner - &self.inner, false),
            (true, true) => (&self.inner | &rhs.inner, true),
        };

        Bitmap { inner, inverted }
    }
}

impl BitOr for &Bitmap {
    type Output = Bitmap;

    /// Union using De Morgan's dispatch:
    ///
    /// - N ∪ N → A | B (normal)
    /// - N ∪ I → B - A (inverted)
    /// - I ∪ N → A - B (inverted)
    /// - I ∪ I → A & B (inverted)
    ///
    /// Short-circuits when either operand is logically empty (identity)
    /// or logically full (annihilator).
    fn bitor(self, rhs: Self) -> Bitmap {
        // Short-circuit: empty OR anything = anything.
        if self.is_empty() {
            return rhs.clone();
        }
        if rhs.is_empty() {
            return self.clone();
        }

        // Short-circuit: full OR anything = full.
        if self.inverted && self.inner.is_empty() {
            return self.clone();
        }
        if rhs.inverted && rhs.inner.is_empty() {
            return rhs.clone();
        }

        debug_assert_eq!(
            self.inner.universe_size(),
            rhs.inner.universe_size(),
            "universe_size mismatch: {} vs {}",
            self.inner.universe_size(),
            rhs.inner.universe_size()
        );

        let (inner, inverted) = match (self.inverted, rhs.inverted) {
            (false, false) => (&self.inner | &rhs.inner, false),
            (false, true) => (&rhs.inner - &self.inner, true),
            (true, false) => (&self.inner - &rhs.inner, true),
            (true, true) => (&self.inner & &rhs.inner, true),
        };

        Bitmap { inner, inverted }
    }
}

impl BitAndAssign<&Bitmap> for Bitmap {
    fn bitand_assign(&mut self, rhs: &Bitmap) {
        *self = &*self & rhs;
    }
}

impl BitOrAssign<&Bitmap> for Bitmap {
    fn bitor_assign(&mut self, rhs: &Bitmap) {
        *self = &*self | rhs;
    }
}
