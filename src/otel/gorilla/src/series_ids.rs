#![allow(dead_code)]

use std::convert::TryFrom;
use std::ops::Deref;

use zerocopy_derive::*;

use error::NdError;

/// A non-zero series identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SeriesId(u32);

impl SeriesId {
    /// Creates a new SeriesId if the value is non-zero.
    pub fn new(value: u32) -> Option<Self> {
        if value != 0 {
            Some(SeriesId(value))
        } else {
            None
        }
    }

    /// Creates a new SeriesId without checking if the value is non-zero.
    ///
    /// # Safety
    /// The value must be non-zero.
    pub unsafe fn new_unchecked(value: u32) -> Self {
        debug_assert!(value != 0);
        SeriesId(value)
    }

    /// Returns the underlying u32 value.
    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for SeriesId {
    type Error = NdError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(NdError::InvalidSeriesId(value))
    }
}

impl From<SeriesId> for u32 {
    fn from(id: SeriesId) -> Self {
        id.0
    }
}

/// A sorted, non-empty slice of non-zero series IDs.
#[derive(Debug, Clone)]
pub struct SeriesIdSlice<'a> {
    inner: &'a [u32],
}

impl<'a> SeriesIdSlice<'a> {
    /// Creates a new SeriesIdSlice from a raw slice of u32s.
    /// Returns None if the slice is empty, contains zeros, is unsorted, or
    /// contains duplicates.
    pub fn new(slice: &'a [u32]) -> Option<Self> {
        if slice.is_empty()
            || !slice.is_sorted()
            || slice.windows(2).any(|w| w[0] >= w[1])
            || slice[0] == 0
        {
            None
        } else {
            Some(SeriesIdSlice { inner: slice })
        }
    }

    /// Creates a new SeriesIdSlice without validation.
    ///
    /// # Safety
    /// The slice must be:
    /// - non-empty
    /// - contain no zeros
    /// - sorted in ascending order
    pub unsafe fn new_unchecked(slice: &'a [u32]) -> Self {
        debug_assert!(!slice.is_empty());
        debug_assert!(slice.is_sorted());
        debug_assert!(slice.windows(2).all(|w| w[0] < w[1]));
        debug_assert!(slice[0] != 0);
        SeriesIdSlice { inner: slice }
    }
}

impl Deref for SeriesIdSlice<'_> {
    type Target = [u32];

    fn deref(&self) -> &Self::Target {
        self.inner
    }
}

/// A fixed-size array of series IDs that can be used with zerocopy.
#[derive(Clone, IntoBytes, FromBytes)]
#[repr(C)]
pub struct SeriesIdArray<const N: usize> {
    inner: [u32; N],
}

impl<const N: usize> SeriesIdArray<N> {
    /// Creates a new SeriesIdArray from a slice of raw u32 IDs.
    /// Returns None if the slice is too large or contains invalid IDs.
    pub fn new(series_ids: &[u32]) -> Option<Self> {
        if series_ids.is_empty()
            || !series_ids.is_sorted()
            || series_ids.windows(2).any(|w| w[0] >= w[1])
            || series_ids[0] == 0
            || series_ids.len() > N
        {
            None
        } else {
            let mut inner = [0; N];
            inner[..series_ids.len()].copy_from_slice(series_ids);
            Some(SeriesIdArray { inner })
        }
    }

    /// Creates a new SeriesIdSlice without validation.
    ///
    /// # Safety
    /// The slice must be:
    /// - non-empty
    /// - contain no zeros
    /// - sorted in ascending order
    pub unsafe fn new_unchecked(series_ids: &[u32]) -> Self {
        debug_assert!(!series_ids.is_empty());
        debug_assert!(series_ids.is_sorted());
        debug_assert!(series_ids[0] != 0);
        debug_assert!(series_ids.windows(2).all(|w| w[0] < w[1]));

        let mut inner = [0; N];
        inner[..series_ids.len()].copy_from_slice(series_ids);
        Self { inner }
    }

    /// Creates a new SeriesIdArray from a SeriesIdSlice.
    pub fn from_slice(slice: &SeriesIdSlice) -> Self {
        // SAFETY:
        //  - Invariants verified at the construction time of the series id slice.
        unsafe { Self::new_unchecked(slice) }
    }

    pub fn as_slice(&self) -> SeriesIdSlice {
        // Safe because SeriesIdArray maintains the required invariants:
        // - non-empty (we always have at least one valid ID if constructed successfully)
        // - non-zero values (checked during construction)
        // - sorted (checked during construction)
        unsafe { SeriesIdSlice::new_unchecked(&self.inner[..self.len()]) }
    }

    /// Returns the number of valid (non-zero) series IDs.
    pub fn len(&self) -> usize {
        self.inner.iter().position(|&x| x == 0).unwrap_or(N)
    }

    /// Returns true if there are no valid (non-zero) series IDs.
    pub fn is_empty(&self) -> bool {
        self.inner[0] == 0
    }
}

impl<const N: usize> std::fmt::Debug for SeriesIdArray<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Convert to SeriesIdSlice which contains only the valid portion
        self.as_slice().fmt(f)
    }
}

#[derive(Debug)]
pub enum LeftRight<T> {
    Left(T),
    Right(T),
}

/// Iterator that merges two sorted series ID slices.
#[derive(Debug, Clone)]
pub struct MergedSeriesIds<'a, 'b> {
    left: std::slice::Iter<'a, u32>,
    right: std::slice::Iter<'b, u32>,
    left_next: Option<u32>,
    right_next: Option<u32>,
}

impl<'a, 'b> MergedSeriesIds<'a, 'b> {
    pub fn new(left: &'a SeriesIdSlice<'a>, right: &'b SeriesIdSlice<'b>) -> Self {
        Self {
            left: left.iter(),
            right: right.iter(),
            left_next: None,
            right_next: None,
        }
    }
}

impl Iterator for MergedSeriesIds<'_, '_> {
    type Item = LeftRight<SeriesId>;

    fn next(&mut self) -> Option<Self::Item> {
        // Get next items if needed
        if self.left_next.is_none() {
            self.left_next = self.left.next().copied();
        }
        if self.right_next.is_none() {
            self.right_next = self.right.next().copied();
        }

        match (self.left_next, self.right_next) {
            (Some(left), Some(right)) => match left.cmp(&right) {
                std::cmp::Ordering::Less => {
                    self.left_next = None;
                    // Safe because SeriesIdSlice guarantees non-zero values
                    unsafe { Some(LeftRight::Left(SeriesId::new_unchecked(left))) }
                }
                std::cmp::Ordering::Greater => {
                    self.right_next = None;
                    unsafe { Some(LeftRight::Right(SeriesId::new_unchecked(right))) }
                }
                std::cmp::Ordering::Equal => {
                    panic!("Found same id twice: {}", left);
                }
            },
            (Some(left), None) => {
                self.left_next = None;
                unsafe { Some(LeftRight::Left(SeriesId::new_unchecked(left))) }
            }
            (None, Some(right)) => {
                self.right_next = None;
                unsafe { Some(LeftRight::Right(SeriesId::new_unchecked(right))) }
            }
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod series_id {
        use super::*;

        #[test]
        fn tests() {
            let id = SeriesId::new(0);
            assert!(id.is_none());

            let id = SeriesId::new(42);
            assert!(id.is_some());
            assert_eq!(id.unwrap().get(), 42);

            let id = SeriesId::try_from(42);
            assert!(id.is_ok());
            assert_eq!(id.unwrap().get(), 42);

            let id = SeriesId::try_from(0);
            assert!(matches!(id, Err(NdError::InvalidSeriesId(0))));

            let id = SeriesId::new(42).unwrap();
            let value: u32 = id.into();
            assert_eq!(value, 42);
        }
    }

    mod series_id_slice {
        use super::*;

        #[test]
        fn tests() {
            let slice = SeriesIdSlice::new(&[1, 2, 3]);
            assert!(slice.is_some());
            assert_eq!(slice.unwrap().len(), 3);

            let slice = SeriesIdSlice::new(&[]);
            assert!(slice.is_none());

            let slice = SeriesIdSlice::new(&[3, 1, 2]);
            assert!(slice.is_none());

            let slice = SeriesIdSlice::new(&[0, 1, 2]);
            assert!(slice.is_none());

            let slice = SeriesIdSlice::new(&[1, 2, 3]).unwrap();
            assert_eq!(&*slice, &[1, 2, 3]);
        }
    }

    mod series_id_array {
        use super::*;

        #[test]
        fn tests() {
            let array: SeriesIdArray<4> = SeriesIdArray::new(&[1, 2, 3]).unwrap();
            assert_eq!(array.len(), 3);
            assert_eq!(array.as_slice().as_ref(), &[1, 2, 3]);

            let array: Option<SeriesIdArray<4>> = SeriesIdArray::new(&[]);
            assert!(array.is_none());

            let array: Option<SeriesIdArray<2>> = SeriesIdArray::new(&[1, 2, 3]);
            assert!(array.is_none());

            let array: Option<SeriesIdArray<4>> = SeriesIdArray::new(&[3, 1, 2]);
            assert!(array.is_none());

            let array: Option<SeriesIdArray<4>> = SeriesIdArray::new(&[0, 1, 2]);
            assert!(array.is_none());

            let slice = SeriesIdSlice::new(&[1, 2, 3]).unwrap();
            let array: SeriesIdArray<4> = SeriesIdArray::from_slice(&slice);
            assert_eq!(array.len(), 3);
            assert_eq!(array.as_slice().as_ref(), &[1, 2, 3]);

            let array: SeriesIdArray<4> = SeriesIdArray::new(&[1, 2]).unwrap();
            assert_eq!(array.len(), 2);
            assert!(!array.is_empty());
        }
    }

    mod merged_series_ids {
        use super::*;

        #[test]
        fn tests() {
            // disjoint
            {
                let left = SeriesIdSlice::new(&[1, 3, 5]).unwrap();
                let right = SeriesIdSlice::new(&[2, 4, 6]).unwrap();
                let merged: Vec<_> = MergedSeriesIds::new(&left, &right).collect();

                assert_eq!(merged.len(), 6);
                let expected = vec![
                    LeftRight::Left(SeriesId::new(1).unwrap()),
                    LeftRight::Right(SeriesId::new(2).unwrap()),
                    LeftRight::Left(SeriesId::new(3).unwrap()),
                    LeftRight::Right(SeriesId::new(4).unwrap()),
                    LeftRight::Left(SeriesId::new(5).unwrap()),
                    LeftRight::Right(SeriesId::new(6).unwrap()),
                ];
                assert_eq!(format!("{:?}", merged), format!("{:?}", expected));
            }

            // left only
            {
                let left = SeriesIdSlice::new(&[1, 2, 3]).unwrap();
                let right = SeriesIdSlice::new(&[4, 5, 6]).unwrap();
                let merged: Vec<_> = MergedSeriesIds::new(&left, &right)
                    .take_while(|x| matches!(x, LeftRight::Left(_)))
                    .collect();

                assert_eq!(merged.len(), 3);
                let expected = vec![
                    LeftRight::Left(SeriesId::new(1).unwrap()),
                    LeftRight::Left(SeriesId::new(2).unwrap()),
                    LeftRight::Left(SeriesId::new(3).unwrap()),
                ];
                assert_eq!(format!("{:?}", merged), format!("{:?}", expected));
            }

            // right only
            {
                let left = SeriesIdSlice::new(&[4, 5, 6]).unwrap();
                let right = SeriesIdSlice::new(&[1, 2, 3]).unwrap();
                let merged: Vec<_> = MergedSeriesIds::new(&left, &right)
                    .take_while(|x| matches!(x, LeftRight::Right(_)))
                    .collect();

                assert_eq!(merged.len(), 3);
                let expected = vec![
                    LeftRight::Right(SeriesId::new(1).unwrap()),
                    LeftRight::Right(SeriesId::new(2).unwrap()),
                    LeftRight::Right(SeriesId::new(3).unwrap()),
                ];
                assert_eq!(format!("{:?}", merged), format!("{:?}", expected));
            }
        }

        #[test]
        #[should_panic(expected = "Found same id twice: 2")]
        fn test_merge_duplicate_ids_panic() {
            let left = SeriesIdSlice::new(&[1, 2, 3]).unwrap();
            let right = SeriesIdSlice::new(&[2, 4, 6]).unwrap();
            let _merged: Vec<_> = MergedSeriesIds::new(&left, &right).collect();
        }
    }
}
