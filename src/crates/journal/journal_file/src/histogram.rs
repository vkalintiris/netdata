use crate::JournalFile;
use crate::Mmap;
use error::Result;

/// A single entry in the time histogram index representing a minute boundary.
#[derive(Debug, Clone, Copy)]
struct HistogramBucket {
    /// Index into the offsets vector where this minute's entries begin.
    offset_index: usize,
    /// The absolute minute value (microseconds / 60_000_000) since epoch.
    minute: u64,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
///
/// This structure stores minute boundaries and their corresponding offset indices,
/// enabling O(log n) lookups for time ranges and histogram generation with configurable
/// bucket sizes (1-minute, 10-minute, etc.).
#[derive(Clone)]
pub struct HistogramIndex {
    /// The first minute in the dataset, used as reference for relative calculations.
    base_minute: u64,
    /// Sparse vector containing only minute boundaries where changes occur.
    buckets: Vec<HistogramBucket>,
}

impl HistogramIndex {
    fn new(base_minute: u64) -> Self {
        Self {
            buckets: Vec::new(),
            base_minute,
        }
    }

    fn push(&mut self, minute: u64, offset_index: usize) {
        self.buckets.push(HistogramBucket {
            minute,
            offset_index,
        });
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
                i,
                entry.minute - self.base_minute,
                entry.offset_index,
                entry.minute
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

impl HistogramIndex {
    pub fn from(jf: &JournalFile<Mmap>) -> Result<Option<HistogramIndex>> {
        let Some(entry_list) = jf.entry_list() else {
            return Ok(None);
        };

        let mut offsets = Vec::new();
        entry_list.collect_offsets(jf, &mut offsets)?;

        let first_timestamp = offsets
            .first()
            .map(|eo| jf.entry_ref(*eo).map(|entry| entry.header.realtime))
            .transpose()?;

        let Some(first_timestamp) = first_timestamp else {
            return Ok(None);
        };

        let first_minute = first_timestamp / (60 * 1_000_000);

        let mut histogram_index = HistogramIndex::new(first_minute);
        let mut current_minute = first_minute;
        histogram_index.push(current_minute, 0);

        for (idx, &offset) in offsets.iter().enumerate().skip(1) {
            let entry = jf.entry_ref(offset).unwrap();
            let entry_minute = entry.header.realtime / (60 * 1_000_000);

            if entry_minute > current_minute {
                histogram_index.push(entry_minute, idx);
                current_minute = entry_minute;
            }
        }

        Ok(Some(histogram_index))
    }
}
