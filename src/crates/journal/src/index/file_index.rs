use super::{field_types::{FieldName, FieldValuePair}, Bitmap, Histogram};
use crate::collections::{HashMap, HashSet};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use bincode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FileIndex {
    // The journal file's histogram
    pub histogram: Histogram,

    // Set of fields in the file
    pub file_fields: HashSet<FieldName>,

    // Set of fields that were requested to be indexed
    pub indexed_fields: HashSet<FieldName>,

    // Bitmap for each indexed field=value pair
    pub bitmaps: HashMap<FieldValuePair, Bitmap>,
}

impl FileIndex {
    pub fn new(
        histogram: Histogram,
        fields: HashSet<FieldName>,
        indexed_fields: HashSet<FieldName>,
        bitmaps: HashMap<FieldValuePair, Bitmap>,
    ) -> Self {
        Self {
            histogram,
            file_fields: fields,
            indexed_fields,
            bitmaps,
        }
    }

    pub fn bucket_duration(&self) -> u32 {
        self.histogram.bucket_duration.get()
    }

    pub fn histogram(&self) -> &Histogram {
        &self.histogram
    }

    /// Get all field names present in this file.
    pub fn fields(&self) -> &HashSet<FieldName> {
        &self.file_fields
    }

    /// Get all indexed field=value pairs with their bitmaps.
    pub fn bitmaps(&self) -> &HashMap<FieldValuePair, Bitmap> {
        &self.bitmaps
    }

    /// Check if a field is indexed.
    pub fn is_indexed(&self, field: &FieldName) -> bool {
        self.indexed_fields.contains(field)
    }

    pub fn count_bitmap_entries_in_range(
        &self,
        bitmap: &Bitmap,
        start_time: u32,
        end_time: u32,
    ) -> Option<usize> {
        self.histogram()
            .count_bitmap_entries_in_range(bitmap, start_time, end_time)
    }

    /// Compresses the bincode serialized representation of the entries_index field using lz4.
    /// Returns the compressed bytes on success.
    pub fn compress_entries_index(&self) -> Vec<u8> {
        // Serialize the entries_index to bincode format
        let serialized = bincode::serialize(&self.bitmaps).unwrap();

        // Compress the serialized data using lz4
        lz4::block::compress(&serialized, None, false).unwrap()
    }

    pub fn memory_size(&self) -> usize {
        bincode::serialized_size(self).unwrap() as usize
    }
}
