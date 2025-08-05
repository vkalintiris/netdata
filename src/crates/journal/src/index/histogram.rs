use super::Bitmap;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

/// A [`Histogram::bucket_duration`] aligned bucket in the histogram.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Bucket {
    /// Start time of this bucket in seconds
    pub start_time: u32,
    /// Running count of items in this bucket since the first bucket
    /// of the histogram
    pub count: u32,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Histogram {
    /// The duration of each bucket in seconds
    pub bucket_duration: NonZeroU32,
    /// Sparse vector containing only bucket boundaries where changes occur.
    pub buckets: Vec<Bucket>,
}

impl Histogram {
    pub fn from_timestamp_offset_pairs(
        bucket_duration: std::num::NonZeroU32,
        timestamp_offset_pairs: &[(u64, std::num::NonZeroU64)],
    ) -> Histogram {
        debug_assert!(timestamp_offset_pairs.is_sorted());

        let mut buckets = Vec::new();
        let mut current_bucket = None;

        // Convert seconds to microseconds for bucket size
        let bucket_size_micros = bucket_duration.get() as u64 * 1_000_000;

        for (offset_index, &(timestamp_micros, _offset)) in
            timestamp_offset_pairs.iter().enumerate()
        {
            // Calculate which bucket this timestamp falls into
            let bucket = (timestamp_micros / bucket_size_micros) * bucket_duration.get() as u64;

            match current_bucket {
                None => {
                    // First entry - don't create bucket yet, just track the bucket
                    debug_assert_eq!(offset_index, 0);
                    current_bucket = Some(bucket);
                }
                Some(prev_bucket) if bucket > prev_bucket => {
                    // New bucket boundary - save the LAST index of the previous bucket
                    buckets.push(Bucket {
                        start_time: prev_bucket as u32,
                        count: offset_index as u32 - 1,
                    });
                    current_bucket = Some(bucket);
                }
                _ => {} // Same bucket, continue
            }
        }

        // Handle last bucket
        if let Some(last_bucket) = current_bucket {
            buckets.push(Bucket {
                start_time: last_bucket as u32,
                count: timestamp_offset_pairs.len() as u32 - 1,
            });
        }

        Histogram {
            bucket_duration,
            buckets,
        }
    }

    // Construct the buckets of a bitmap that contains entry indexes.
    pub fn bitmap_buckets(&self, bitmap: &Bitmap) -> Vec<Bucket> {
        let mut buckets = Vec::new();
        let mut iter = bitmap.iter().peekable();

        for bucket in &self.buckets {
            let mut count = 0;

            while let Some(&value) = iter.peek() {
                if value <= bucket.count {
                    count += 1;
                    iter.next();
                } else {
                    break;
                }
            }

            if count > 0 {
                buckets.push(Bucket {
                    start_time: bucket.start_time,
                    count,
                });
            }

            if iter.peek().is_none() {
                break;
            }
        }

        buckets
    }

    /// Get the start time of the histogram.
    pub fn start_time(&self) -> u32 {
        let first_bucket = self.buckets.first().expect("histogram to have buckets");
        first_bucket.start_time
    }

    /// Get the end time of the histogram.
    pub fn end_time(&self) -> u32 {
        let last_bucket = self.buckets.last().expect("histogram to have buckets");
        last_bucket.start_time + self.bucket_duration.get()
    }

    /// Get the time range covered by the histogram.
    pub fn time_range(&self) -> (u32, u32) {
        (self.start_time(), self.end_time())
    }

    /// Get the duration covered by this histogram.
    pub fn duration(&self) -> u32 {
        let (start, end) = self.time_range();
        end - start
    }

    /// Returns the number of buckets in the histogram.
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    /// Check if the file histogram is empty.
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// Get the total number of entries in the histogram.
    pub fn count(&self) -> usize {
        let last_bucket = self.buckets.last().expect("histogram to have buckets");
        // FIXME: Off-by-one error
        last_bucket.count as usize + 1
    }

    /// Count the number of bitmap entries that fall within a time range.
    ///
    /// This method efficiently counts entries by using the histogram's bucket structure
    /// rather than iterating through all bitmap entries. It only works when the time
    /// range is aligned to the histogram's bucket_duration.
    ///
    /// # Arguments
    ///
    /// * `bitmap` - The bitmap containing entry indices to count
    /// * `start_time` - Start of the time range (must be aligned to bucket_duration)
    /// * `end_time` - End of the time range (must be aligned to bucket_duration)
    ///
    /// # Returns
    ///
    /// * `Some(count)` - The number of bitmap entries in the time range
    /// * `None` - If the time range is not aligned to bucket_duration or if the range
    ///   is invalid (start >= end, or outside histogram bounds)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Histogram with 60-second buckets
    /// let histogram = Histogram::new(60, buckets);
    /// let bitmap = /* bitmap from filter evaluation */;
    ///
    /// // Count entries between 1000 and 1120 seconds (aligned to 60s buckets)
    /// if let Some(count) = histogram.count_bitmap_entries_in_range(&bitmap, 1000, 1120) {
    ///     println!("Found {} entries in time range", count);
    /// }
    /// ```
    pub fn count_bitmap_entries_in_range(
        &self,
        bitmap: &Bitmap,
        start_time: u32,
        end_time: u32,
    ) -> Option<usize> {
        // Validate inputs
        if start_time >= end_time {
            return None;
        }

        // Verify alignment to bucket_duration
        if !start_time.is_multiple_of(self.bucket_duration.get())
            || !end_time.is_multiple_of(self.bucket_duration.get())
        {
            return None;
        }

        // Handle empty histogram or bitmap
        if self.buckets.is_empty() || bitmap.is_empty() {
            return Some(0);
        }

        // Find the bucket indices for start and end times using binary search
        // partition_point returns the index of the first bucket with start_time >= start_time
        let start_bucket_idx = self.buckets.partition_point(|b| b.start_time < start_time);

        // If start_bucket_idx is beyond all buckets, no matches possible
        if start_bucket_idx >= self.buckets.len() {
            return Some(0);
        }

        // Find the last bucket that starts before end_time
        // partition_point returns the index of the first bucket with start_time >= end_time,
        // so we need to subtract 1 to get the last bucket before end_time
        let end_bucket_idx = self
            .buckets
            .partition_point(|b| b.start_time < end_time)
            .saturating_sub(1);

        // If start is after end, the range doesn't contain any buckets
        if start_bucket_idx > end_bucket_idx {
            return Some(0);
        }

        // Get the running count boundaries
        // For start: we want entries AFTER the previous bucket's running count
        let start_running_count = if start_bucket_idx == 0 {
            0
        } else {
            self.buckets[start_bucket_idx - 1].count + 1
        };

        // For end: we want entries UP TO AND INCLUDING this bucket's running count
        let end_running_count = self.buckets[end_bucket_idx].count;

        // Range is [start_running_count, end_running_count + 1) since range_cardinality is exclusive on the end
        let count = bitmap.range_cardinality(start_running_count..(end_running_count + 1));

        Some(count as usize)
    }
}

use chrono::{Local, TimeZone};

impl std::fmt::Display for Histogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.buckets.is_empty() {
            return writeln!(f, "Empty histogram");
        }

        writeln!(f, "Histogram (bucket_duration: {}s)", self.bucket_duration)?;
        writeln!(f, "{:<18} {:<12} {:<12}", "Start Time", "Count", "Running")?;
        writeln!(f, "{:-<42}", "")?;

        let mut prev_running = 0;
        for (i, bucket) in self.buckets.iter().enumerate() {
            let count = if i == 0 {
                bucket.count + 1
            } else {
                bucket.count - prev_running
            };

            // Convert seconds to datetime with format: dd/mm HH:MM:SS
            let datetime = Local
                .timestamp_opt(bucket.start_time as i64, 0)
                .single()
                .map(|dt| dt.format("%d/%m %H:%M:%S").to_string())
                .expect("a local datetime from bucket's start time");

            writeln!(f, "{:<18} {:<12} {:<12}", datetime, count, bucket.count)?;
            prev_running = bucket.count;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test histogram with known buckets
    ///
    /// Creates a histogram with:
    /// - bucket_duration: 60 seconds
    /// - Entries at indices 0-4 in bucket starting at time 0
    /// - Entries at indices 5-9 in bucket starting at time 60
    /// - Entries at indices 10-14 in bucket starting at time 120
    /// - Entries at indices 15-19 in bucket starting at time 180
    fn create_test_histogram() -> Histogram {
        let buckets = vec![
            Bucket {
                start_time: 0,
                count: 4, // entries 0-4 (5 entries)
            },
            Bucket {
                start_time: 60,
                count: 9, // entries 5-9 (5 entries)
            },
            Bucket {
                start_time: 120,
                count: 14, // entries 10-14 (5 entries)
            },
            Bucket {
                start_time: 180,
                count: 19, // entries 15-19 (5 entries)
            },
        ];

        Histogram {
            bucket_duration: NonZeroU32::new(60).unwrap(),
            buckets,
        }
    }

    #[test]
    fn test_count_bitmap_entries_in_range_full_bucket() {
        let histogram = create_test_histogram();
        // Bitmap contains entries 5, 6, 7, 8, 9 (all in bucket starting at 60)
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7, 8, 9]).unwrap();

        // Query for the full bucket from 60 to 120
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 120);
        assert_eq!(count, Some(5));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_partial_match() {
        let histogram = create_test_histogram();
        // Bitmap contains some entries in bucket 60-120 and some in 120-180
        let bitmap = Bitmap::from_sorted_iter([7, 8, 9, 10, 11]).unwrap();

        // Query for bucket 60-120 should only count entries 7, 8, 9
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 120);
        assert_eq!(count, Some(3));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_multiple_buckets() {
        let histogram = create_test_histogram();
        // Bitmap spans multiple buckets
        let bitmap = Bitmap::from_sorted_iter([5, 6, 10, 11, 15, 16]).unwrap();

        // Query for buckets 60-180 (includes buckets at 60 and 120)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 180);
        assert_eq!(count, Some(4)); // 5, 6, 10, 11
    }

    #[test]
    fn test_count_bitmap_entries_in_range_no_matches() {
        let histogram = create_test_histogram();
        // Bitmap contains entries in bucket 0-60
        let bitmap = Bitmap::from_sorted_iter([0, 1, 2]).unwrap();

        // Query for bucket 120-180 should find no matches
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 120, 180);
        assert_eq!(count, Some(0));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_empty_bitmap() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::new();

        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 60);
        assert_eq!(count, Some(0));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_unaligned_start() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // Start time not aligned to bucket_duration (60)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 30, 120);
        assert_eq!(count, None);
    }

    #[test]
    fn test_count_bitmap_entries_in_range_unaligned_end() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // End time not aligned to bucket_duration (60)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 100);
        assert_eq!(count, None);
    }

    #[test]
    fn test_count_bitmap_entries_in_range_invalid_range() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // start >= end
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 120, 60);
        assert_eq!(count, None);

        // start == end
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 60);
        assert_eq!(count, None);
    }

    #[test]
    fn test_count_bitmap_entries_in_range_outside_histogram() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // Range completely before histogram
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 60);
        // This will actually work since 0-60 is the first bucket
        assert!(count.is_some());

        // Range completely after histogram (histogram ends at 240)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 240, 300);
        assert_eq!(count, Some(0));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_first_bucket() {
        let histogram = create_test_histogram();
        // Entries in first bucket (0-60)
        let bitmap = Bitmap::from_sorted_iter([0, 1, 2, 3, 4]).unwrap();

        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 60);
        assert_eq!(count, Some(5));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_last_bucket() {
        let histogram = create_test_histogram();
        // Entries in last bucket (180-240)
        let bitmap = Bitmap::from_sorted_iter([15, 16, 17, 18, 19]).unwrap();

        let count = histogram.count_bitmap_entries_in_range(&bitmap, 180, 240);
        assert_eq!(count, Some(5));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_all_buckets() {
        let histogram = create_test_histogram();
        // Entries spanning all buckets
        let bitmap = Bitmap::from_sorted_iter([0, 5, 10, 15]).unwrap();

        // Query for entire histogram range
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 240);
        assert_eq!(count, Some(4));
    }

    #[test]
    fn test_histogram_properties() {
        let histogram = create_test_histogram();

        assert_eq!(histogram.start_time(), 0);
        assert_eq!(histogram.end_time(), 240);
        assert_eq!(histogram.time_range(), (0, 240));
        assert_eq!(histogram.duration(), 240);
        assert_eq!(histogram.len(), 4);
        assert!(!histogram.is_empty());
        assert_eq!(histogram.count(), 20);
    }
}
