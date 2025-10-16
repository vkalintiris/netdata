use roaring::{NonSortedIntegers, RoaringBitmap};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bitmap(pub RoaringBitmap);

impl Bitmap {
    pub fn new() -> Self {
        Self(RoaringBitmap::new())
    }

    pub fn from_sorted_iter<I: IntoIterator<Item = u32>>(
        iterator: I,
    ) -> Result<Bitmap, NonSortedIntegers> {
        RoaringBitmap::from_sorted_iter(iterator).map(Bitmap)
    }
}

impl std::ops::Deref for Bitmap {
    type Target = RoaringBitmap;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Bitmap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<RoaringBitmap> for Bitmap {
    fn from(bitmap: RoaringBitmap) -> Self {
        Self(bitmap)
    }
}

impl From<Bitmap> for RoaringBitmap {
    fn from(wrapper: Bitmap) -> Self {
        wrapper.0
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
