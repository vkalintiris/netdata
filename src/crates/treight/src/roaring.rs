use crate::raw::RawBitmap;
use roaring::RoaringBitmap;

impl RawBitmap {
    /// Build from a `RoaringBitmap` with an explicit universe size.
    ///
    /// Use this when the universe size is known and may be larger than `max + 1`.
    pub fn from_roaring(rb: &RoaringBitmap, universe_size: u32) -> Self {
        Self::from_sorted_iter(rb.iter(), universe_size)
    }
}

impl From<&RoaringBitmap> for RawBitmap {
    /// Build from a `RoaringBitmap`, using `max + 1` as the universe size.
    fn from(rb: &RoaringBitmap) -> Self {
        let universe_size = rb.max().map_or(0, |m| m + 1);
        Self::from_roaring(rb, universe_size)
    }
}

impl From<&RawBitmap> for RoaringBitmap {
    fn from(bm: &RawBitmap) -> Self {
        RoaringBitmap::from_sorted_iter(bm.iter()).unwrap()
    }
}
