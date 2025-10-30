use super::{Bitmap, Histogram};
use crate::collections::{HashMap, HashSet};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use bincode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FileIndexInner {
    // The journal file's histogram
    pub histogram: Histogram,

    // Set of fields in the file
    pub file_fields: HashSet<String>,

    // Set of fields that were requested to be indexed
    pub indexed_fields: HashSet<String>,

    // Bitmap for each indexed field
    pub bitmaps: HashMap<String, Bitmap>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FileIndex {
    pub inner: Arc<FileIndexInner>,
}

impl Serialize for FileIndex {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.inner.as_ref().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FileIndex {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let inner = FileIndexInner::deserialize(deserializer)?;
        Ok(FileIndex {
            inner: Arc::new(inner),
        })
    }
}

impl FileIndex {
    pub fn new(
        histogram: Histogram,
        fields: HashSet<String>,
        indexed_fields: HashSet<String>,
        bitmaps: HashMap<String, Bitmap>,
    ) -> Self {
        let inner = FileIndexInner {
            histogram,
            file_fields: fields,
            indexed_fields,
            bitmaps,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn bucket_duration(&self) -> u64 {
        self.inner.histogram.bucket_duration
    }

    pub fn histogram(&self) -> &Histogram {
        &self.inner.histogram
    }

    pub fn fields(&self) -> &HashSet<String> {
        &self.inner.file_fields
    }

    pub fn bitmaps(&self) -> &HashMap<String, Bitmap> {
        &self.inner.bitmaps
    }

    pub fn is_indexed(&self, field: &str) -> bool {
        self.inner.indexed_fields.contains(field)
    }

    pub fn count_bitmap_entries_in_range(
        &self,
        bitmap: &Bitmap,
        start_time: u64,
        end_time: u64,
    ) -> Option<usize> {
        self.histogram()
            .count_bitmap_entries_in_range(bitmap, start_time, end_time)
    }

    /// Compresses the bincode serialized representation of the entries_index field using lz4.
    /// Returns the compressed bytes on success.
    pub fn compress_entries_index(&self) -> Vec<u8> {
        // Serialize the entries_index to bincode format
        let serialized = bincode::serialize(&self.inner.bitmaps).unwrap();

        // Compress the serialized data using lz4
        lz4::block::compress(&serialized, None, false).unwrap()
    }

    pub fn memory_size(&self) -> usize {
        bincode::serialized_size(&*self.inner).unwrap() as usize
    }
}
