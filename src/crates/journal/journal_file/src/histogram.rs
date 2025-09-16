use crate::JournalFile;
use crate::Mmap;
use error::Result;

/// A minute-aligned bucket in the histogram index.
#[derive(Debug, Clone, Copy)]
struct HistogramBucket {
    /// Minute-aligned seconds since EPOCH.
    minute: u64,
    /// Index into the global entry offsets array.
    offset_index: usize,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
///
/// This structure stores minute boundaries and their corresponding offset indices,
/// enabling O(log n) lookups for time ranges and histogram generation with configurable
/// bucket sizes (1-minute, 10-minute, etc.).
#[derive(Clone)]
pub struct HistogramIndex {
    /// Sparse vector containing only minute boundaries where changes occur.
    buckets: Vec<HistogramBucket>,
}

impl HistogramIndex {
    pub fn from(jf: &JournalFile<Mmap>) -> Result<Option<HistogramIndex>> {
        let Some(entry_list) = jf.entry_list() else {
            return Ok(None);
        };

        let mut offsets = Vec::new();
        entry_list.collect_offsets(jf, &mut offsets)?;

        if offsets.is_empty() {
            return Ok(None);
        }

        let mut buckets = Vec::new();
        let mut current_minute = None;

        for (offset_index, &offset) in offsets.iter().enumerate() {
            let entry = jf.entry_ref(offset)?;
            let minute = entry.header.realtime / (60 * 1_000_000);

            match current_minute {
                None => {
                    // First entry
                    debug_assert_eq!(offset_index, 0);

                    buckets.push(HistogramBucket {
                        minute,
                        offset_index: 0,
                    });
                    current_minute = Some(minute);
                }
                Some(prev_minute) if minute > prev_minute => {
                    // New minute boundary
                    buckets.push(HistogramBucket {
                        minute,
                        offset_index,
                    });
                    current_minute = Some(minute);
                }
                _ => {} // Same minute, skip
            }
        }

        Ok(Some(HistogramIndex { buckets }))
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

impl std::fmt::Debug for HistogramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{:<10} {:<15} {:<15} {:<30}",
            "Entry#", "Minute", "Offset Index", "Timestamp"
        )?;
        writeln!(f, "{}", "-".repeat(70))?;

        for (i, entry) in self.buckets.iter().take(10).enumerate() {
            writeln!(
                f,
                "{:<10} {:<15} {:<15} minute {}",
                i, entry.minute, entry.offset_index, entry.minute
            )?;
        }

        if self.buckets.len() > 10 {
            writeln!(f, "... and {} more entries", self.buckets.len() - 10)?;
        }
        Ok(())
    }
}

impl std::fmt::Display for HistogramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.buckets.is_empty() {
            return write!(f, "Empty index");
        }

        writeln!(f, "Time ranges and entry counts:")?;
        writeln!(f, "{:<30} {:<10}", "Range", "Count")?;
        writeln!(f, "{}", "-".repeat(40))?;

        for window in self.buckets.windows(2) {
            let start_minute = window[0].minute;
            let end_minute = window[1].minute;
            let count = window[1].offset_index - window[0].offset_index;

            writeln!(
                f,
                "{:02}:{:02} - {:02}:{:02} ({}m)          {}",
                (start_minute % (24 * 60)) / 60, // hours
                start_minute % 60,               // minutes
                (end_minute % (24 * 60)) / 60,
                end_minute % 60,
                end_minute - start_minute,
                count
            )?;
        }
        Ok(())
    }
}
