#![allow(dead_code, unused_imports)]

use journal_file::Direction;
use journal_file::JournalFile;
use journal_file::JournalReader;
use journal_file::Location;
use journal_file::Mmap;
use journal_registry::{JournalRegistry, SortBy, SortOrder, SourceType};
use std::num::NonZeroU64;
use std::time::Instant;
use std::time::{Duration, SystemTime};
use tracing::{info, instrument, warn};

#[derive(Debug, Clone)]
struct MinuteBoundaryData {
    /// The indices into the offsets vector that represent minute boundaries
    boundary_indices: Vec<usize>,
    /// Total number of offsets in the file
    total_offsets: usize,
}

#[instrument(skip(files))]
fn sequential_collect(files: &[journal_registry::RegistryFile]) -> Vec<MinuteBoundaryData> {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut midx_count = 0;

    // Store boundary data for each file
    let mut all_boundary_data = Vec::new();

    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);

        // Collect minute boundary indices
        let mut boundary_indices = Vec::new();
        let mut current_minute = first_minute;
        boundary_indices.push(0); // First entry is always a boundary

        for (idx, &offset) in offsets.iter().enumerate().skip(1) {
            let entry = journal_file.entry_ref(offset).unwrap();
            let entry_minute = entry.header.realtime / (60 * 1_000_000);

            if entry_minute > current_minute {
                boundary_indices.push(idx);
                current_minute = entry_minute;
            }
        }

        midx_count += boundary_indices.len();
        count += offsets.len();

        // Store the boundary data for this file
        all_boundary_data.push(MinuteBoundaryData {
            boundary_indices,
            total_offsets: offsets.len(),
        });
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );

    all_boundary_data
}

#[instrument(skip(files, boundary_data))]
fn perfect_knowledge(
    files: &[journal_registry::RegistryFile],
    boundary_data: &[MinuteBoundaryData],
) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut num_resolved_entries = 0;
    let mut midx_count = 0;

    for (file_idx, file) in files.iter().rev().enumerate() {
        // Skip if we don't have boundary data for this file
        if file_idx >= boundary_data.len() {
            continue;
        }

        let boundaries = &boundary_data[file_idx];

        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        // Only resolve the entries at minute boundaries
        let mut minute_index = Vec::new();

        for &boundary_idx in &boundaries.boundary_indices {
            if boundary_idx < offsets.len() {
                let entry = journal_file.entry_ref(offsets[boundary_idx]).unwrap();
                let entry_minute = entry.header.realtime / (60 * 1_000_000);
                minute_index.push((entry_minute, boundary_idx));
                num_resolved_entries += 1;
            }
        }

        midx_count += minute_index.len();
        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "Perfect knowledge: {:#?} entry offsets in {:#?} msec (midx: {:#?}, resolved: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
        num_resolved_entries
    );
}

#[instrument(skip(files))]
fn baseline(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec",
        count,
        elapsed.as_millis()
    );
}

#[instrument(skip(files))]
fn sequential(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();

    let mut minute_index = Vec::new();
    let mut midx_count = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);

        // Build a compact minute index with only actual minute changes
        minute_index.clear();
        let mut current_minute = first_minute;
        // First minute starts at offset 0
        minute_index.push((current_minute, 0));

        for (idx, &offset) in offsets.iter().enumerate().skip(1) {
            let entry = journal_file.entry_ref(offset).unwrap();
            let entry_minute = entry.header.realtime / (60 * 1_000_000);

            if entry_minute > current_minute {
                // We've crossed into a new minute
                minute_index.push((entry_minute, idx));
                current_minute = entry_minute;
            }
        }

        midx_count += minute_index.len();

        // Print the compact minute index
        // println!("\n=== [A] Compact Minute Index ({:#?})", file.path);
        // println!(
        //     "{:<10} {:<15} {:<15} {:<30}",
        //     "Entry#", "Minute", "Offset Index", "Timestamp"
        // );
        // println!("{}", "-".repeat(70));

        // for (i, &(minute, offset_idx)) in minute_index.iter().take(10).enumerate() {
        //     if offset_idx < offsets.len() {
        //         let entry = journal_file.entry_ref(offsets[offset_idx]).unwrap();

        //         println!(
        //             "{:<10} {:<15} {:<15} {} (minute {})",
        //             i,
        //             minute - first_minute, // Relative minute for readability
        //             offset_idx,
        //             format_timestamp(entry.header.realtime),
        //             minute
        //         );
        //     }
        // }

        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );
}

/// A single entry in the time histogram index representing a minute boundary.
#[derive(Debug, Clone, Copy)]
struct BucketEntry {
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
struct TimeHistogramIndex {
    /// The first minute in the dataset, used as reference for relative calculations.
    base_minute: u64,
    /// Sparse vector containing only minute boundaries where changes occur.
    entries: Vec<BucketEntry>,
}

impl std::fmt::Debug for TimeHistogramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{:<10} {:<15} {:<15} {:<30}",
            "Entry#", "Minute", "Offset Index", "Timestamp"
        )?;
        writeln!(f, "{}", "-".repeat(70))?;

        for (i, entry) in self.entries.iter().take(10).enumerate() {
            writeln!(
                f,
                "{:<10} {:<15} {:<15} minute {}",
                i,
                entry.minute - self.base_minute,
                entry.offset_index,
                entry.minute
            )?;
        }

        if self.entries.len() > 10 {
            writeln!(f, "... and {} more entries", self.entries.len() - 10)?;
        }
        Ok(())
    }
}

impl std::fmt::Display for TimeHistogramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.entries.is_empty() {
            return write!(f, "Empty index");
        }

        writeln!(f, "Time ranges and entry counts:")?;
        writeln!(f, "{:<30} {:<10}", "Range", "Count")?;
        writeln!(f, "{}", "-".repeat(40))?;

        for window in self.entries.windows(2) {
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

impl TimeHistogramIndex {
    fn new(base_minute: u64) -> Self {
        Self {
            entries: Vec::new(),
            base_minute,
        }
    }

    fn push(&mut self, minute: u64, offset_index: usize) {
        self.entries.push(BucketEntry {
            minute,
            offset_index,
        });
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn iter(&self) -> impl Iterator<Item = &BucketEntry> {
        self.entries.iter()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

struct TimeHistogram {
    base_minute: u64,
    bucket_minutes: u64,
    counts: Vec<usize>,
}

impl TimeHistogram {
    fn from_index(index: &TimeHistogramIndex, bucket_minutes: u64) -> Vec<usize> {
        let mut histogram = Vec::new();
        // Align to bucket boundary
        let mut current_bucket_start = (index.base_minute / bucket_minutes) * bucket_minutes;
        let mut current_count = 0;
        let mut prev_offset = 0;

        for entry in &index.entries {
            while entry.minute >= current_bucket_start + bucket_minutes {
                histogram.push(current_count);
                current_bucket_start += bucket_minutes;
                current_count = 0;
            }
            // Add the entries since last boundary to current bucket
            current_count += entry.offset_index - prev_offset;
            prev_offset = entry.offset_index;
        }
        histogram.push(current_count);
        histogram
    }
}

#[instrument(skip(files))]
fn sequential_v1(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut midx_count = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);

        // Use the new type
        let mut minute_index = TimeHistogramIndex::new(first_minute);
        let mut current_minute = first_minute;
        minute_index.push(current_minute, 0);

        for (idx, &offset) in offsets.iter().enumerate().skip(1) {
            let entry = journal_file.entry_ref(offset).unwrap();
            let entry_minute = entry.header.realtime / (60 * 1_000_000);

            if entry_minute > current_minute {
                minute_index.push(entry_minute, idx);
                current_minute = entry_minute;
            }
        }

        midx_count += minute_index.len();

        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );
}

#[instrument(skip(files))]
fn partitioned(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut midx_count = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let last_timestamp = offsets
            .last()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(last_timestamp) = last_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);
        let last_minute = last_timestamp / (60 * 1_000_000);

        // Use the new type
        let mut minute_index = TimeHistogramIndex::new(first_minute);

        // For each minute in the range, find its starting index using binary search
        for minute in first_minute..=last_minute {
            let minute_start_us = minute * 60 * 1_000_000;

            // Find the first entry >= this minute's start time
            let idx = offsets.partition_point(|&offset| {
                journal_file
                    .entry_ref(offset)
                    .map(|entry| entry.header.realtime < minute_start_us)
                    .unwrap_or(false)
            });

            // Only add if we found entries for this minute
            if idx < offsets.len() {
                minute_index.push(minute, idx);
                midx_count += 1;
            }
        }

        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );
}

#[instrument(skip(files))]
fn probing(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut midx_count = 0;

    let mut num_resolved_entries = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let last_timestamp = offsets
            .last()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(last_timestamp) = last_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);
        let last_minute = last_timestamp / (60 * 1_000_000);

        let duration_minutes = (last_minute - first_minute + 1) as f64;

        // Calculate average logs per minute
        let logs_per_minute = offsets.len() as f64 / duration_minutes;
        // Use this as our probe step size (with some margin for variance)
        let probe_step = (logs_per_minute * 0.10) as usize;
        let probe_step = probe_step.max(1);

        // Use the new type
        let mut minute_index = TimeHistogramIndex::new(first_minute);
        let mut current_minute = first_minute;
        minute_index.push(current_minute, 0);

        // for (idx, &offset) in offsets.iter().enumerate().skip(1) {
        //     let entry = journal_file.entry_ref(offset).unwrap();
        //     let entry_minute = entry.header.realtime / (60 * 1_000_000);

        //     if entry_minute > current_minute {
        //         minute_index.push(entry_minute, idx);
        //         current_minute = entry_minute;
        //     }
        // }

        // Probing optimization for building the minute index
        // const PROBE_SKIP: usize = offsets.len() / ; // Adjust based on your typical entries-per-minute

        let mut idx = 1;
        while idx < offsets.len() {
            let entry_minute = {
                let entry = journal_file.entry_ref(offsets[idx]).unwrap();
                num_resolved_entries += 1;
                entry.header.realtime / (60 * 1_000_000)
            };

            if entry_minute > current_minute {
                // Found new minute boundary
                minute_index.push(entry_minute, idx);
                current_minute = entry_minute;
                idx += 1;
            } else {
                // Still in same minute, probe ahead
                let probe_idx = (idx + probe_step).min(offsets.len() - 1);
                let probe_minute = {
                    let probe_entry = journal_file.entry_ref(offsets[probe_idx]).unwrap();
                    num_resolved_entries += 1;
                    probe_entry.header.realtime / (60 * 1_000_000)
                };

                if probe_minute == current_minute {
                    // Still in same minute, skip to probe position
                    idx = probe_idx + 1;
                } else {
                    // Overshot into new minute(s), linear search to find exact boundary
                    idx += 1;
                    while idx <= probe_idx {
                        let entry_minute = {
                            let entry = journal_file.entry_ref(offsets[idx]).unwrap();
                            num_resolved_entries += 1;
                            entry.header.realtime / (60 * 1_000_000)
                        };
                        if entry_minute > current_minute {
                            minute_index.push(entry_minute, idx);
                            current_minute = entry_minute;
                        }
                        idx += 1;
                    }
                }
            }
        }

        midx_count += minute_index.len();

        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );
    info!("resolved entries: {:#?}", num_resolved_entries);
}

#[instrument(skip(files))]
fn adaptive_probing(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut midx_count = 0;
    let mut num_resolved_entries = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);

        // Use the new type
        let mut minute_index = TimeHistogramIndex::new(first_minute);
        let mut current_minute = first_minute;
        minute_index.push(current_minute, 0);

        // Start with a conservative initial probe step
        // Can be based on file-wide average or just a reasonable default
        let initial_probe_step = ((offsets.len() / 1000).max(10)).min(100);
        let mut probe_step = initial_probe_step;

        let mut idx = 1;
        let mut last_minute_start_idx = 0;

        while idx < offsets.len() {
            let entry_minute = {
                let entry = journal_file.entry_ref(offsets[idx]).unwrap();
                num_resolved_entries += 1;
                entry.header.realtime / (60 * 1_000_000)
            };

            if entry_minute > current_minute {
                // Found new minute boundary
                minute_index.push(entry_minute, idx);

                // Adaptive step calculation:
                // Use half the entries from the previous minute as the new probe step
                let entries_in_last_minute = idx - last_minute_start_idx;
                probe_step = (entries_in_last_minute / 2).max(1);

                // Optional: Apply some bounds to avoid extreme values
                probe_step = probe_step.clamp(5, 1000);

                last_minute_start_idx = idx;
                current_minute = entry_minute;
                idx += 1;
            } else {
                // Still in same minute, probe ahead with adaptive step
                let probe_idx = (idx + probe_step).min(offsets.len() - 1);
                let probe_minute = {
                    let probe_entry = journal_file.entry_ref(offsets[probe_idx]).unwrap();
                    num_resolved_entries += 1;
                    probe_entry.header.realtime / (60 * 1_000_000)
                };

                if probe_minute == current_minute {
                    // Still in same minute, skip to probe position
                    idx = probe_idx + 1;
                } else {
                    // Overshot into new minute(s)
                    // Could use binary search here for the exact boundary
                    // instead of linear search, especially if probe_step is large

                    // Binary search for the minute boundary
                    let mut left = idx + 1;
                    let mut right = probe_idx;

                    while left < right {
                        let mid = left + (right - left) / 2;
                        let mid_minute = {
                            let entry = journal_file.entry_ref(offsets[mid]).unwrap();
                            num_resolved_entries += 1;
                            entry.header.realtime / (60 * 1_000_000)
                        };

                        if mid_minute > current_minute {
                            right = mid;
                        } else {
                            left = mid + 1;
                        }
                    }

                    // left now points to the first entry of the new minute
                    if left < offsets.len() {
                        let new_minute = {
                            let entry = journal_file.entry_ref(offsets[left]).unwrap();
                            num_resolved_entries += 1;
                            entry.header.realtime / (60 * 1_000_000)
                        };

                        minute_index.push(new_minute, left);

                        // Update adaptive probe step
                        let entries_in_last_minute = left - last_minute_start_idx;
                        probe_step = (entries_in_last_minute / 2).max(1).clamp(5, 1000);

                        last_minute_start_idx = left;
                        current_minute = new_minute;
                        idx = left + 1;
                    } else {
                        break;
                    }
                }
            }
        }

        midx_count += minute_index.len();
        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );
    info!("resolved entries: {:#?}", num_resolved_entries);
}

#[instrument(skip(files))]
fn adaptive_probing_linear(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut midx_count = 0;
    let mut num_resolved_entries = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);

        // Use the new type
        let mut minute_index = TimeHistogramIndex::new(first_minute);
        let mut current_minute = first_minute;
        minute_index.push(current_minute, 0);

        // Start with a conservative initial probe step
        // Can be based on file-wide average or just a reasonable default
        let initial_probe_step = ((offsets.len() / 1000).max(10)).min(100);
        let mut probe_step = initial_probe_step;

        let mut idx = 1;
        let mut last_minute_start_idx = 0;

        while idx < offsets.len() {
            let entry_minute = {
                let entry = journal_file.entry_ref(offsets[idx]).unwrap();
                num_resolved_entries += 1;
                entry.header.realtime / (60 * 1_000_000)
            };

            if entry_minute > current_minute {
                // Found new minute boundary
                minute_index.push(entry_minute, idx);

                // Adaptive step calculation:
                // Use half the entries from the previous minute as the new probe step
                let entries_in_last_minute = idx - last_minute_start_idx;
                probe_step = (entries_in_last_minute / 2).max(1);

                // Optional: Apply some bounds to avoid extreme values
                probe_step = probe_step.clamp(5, 1000);

                last_minute_start_idx = idx;
                current_minute = entry_minute;
                idx += 1;
            } else {
                // Still in same minute, probe ahead with adaptive step
                let probe_idx = (idx + probe_step).min(offsets.len() - 1);
                let probe_minute = {
                    let probe_entry = journal_file.entry_ref(offsets[probe_idx]).unwrap();
                    num_resolved_entries += 1;
                    probe_entry.header.realtime / (60 * 1_000_000)
                };

                if probe_minute == current_minute {
                    // Still in same minute, skip to probe position
                    idx = probe_idx + 1;
                } else {
                    // Overshot into new minute(s), linear search to find exact boundary
                    idx += 1;
                    while idx <= probe_idx {
                        let entry_minute = {
                            let entry = journal_file.entry_ref(offsets[idx]).unwrap();
                            num_resolved_entries += 1;
                            entry.header.realtime / (60 * 1_000_000)
                        };

                        if entry_minute > current_minute {
                            // Found the boundary
                            minute_index.push(entry_minute, idx);

                            // Update adaptive probe step
                            let entries_in_last_minute = idx - last_minute_start_idx;
                            probe_step = (entries_in_last_minute / 2).max(1).clamp(5, 1000);

                            last_minute_start_idx = idx;
                            current_minute = entry_minute;
                            break;
                        }
                        idx += 1;
                    }
                    idx += 1;
                }
            }
        }

        midx_count += minute_index.len();
        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );
    info!("resolved entries: {:#?}", num_resolved_entries);
}
#[instrument(skip(files))]
fn interpolation_minute_index(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut minute_index = Vec::new();
    let mut midx_count = 0;
    let mut total_lookups = 0;

    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        if offsets.is_empty() {
            continue;
        }

        // Get time boundaries (2 lookups)
        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };
        let last_timestamp = offsets
            .last()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(last_timestamp) = last_timestamp else {
            continue;
        };

        total_lookups += 2;

        let start_minute = first_timestamp / (60 * 1_000_000);
        let end_minute = last_timestamp / (60 * 1_000_000);

        if start_minute == end_minute {
            // Only one minute in this file
            minute_index.push((start_minute, 0));
            midx_count += 1;
            count += offsets.len();
            continue;
        }

        let total_minutes = (end_minute - start_minute + 1) as usize;

        // Sample X equally-spaced points (one per expected minute)
        let mut sampled_points = Vec::with_capacity(total_minutes + 1);
        for i in 0..=total_minutes {
            let offset_idx = (i * (offsets.len() - 1)) / total_minutes;

            let entry = journal_file.entry_ref(offsets[offset_idx]).unwrap();
            let timestamp = entry.header.realtime;
            let minute = timestamp / (60 * 1_000_000);
            sampled_points.push((offset_idx, timestamp, minute));
            total_lookups += 1;
        }

        // Build minute index using interpolation search between sampled points
        minute_index.clear();
        let mut processed_minutes = std::collections::HashSet::new();

        for i in 0..sampled_points.len() - 1 {
            let (left_idx, left_ts, left_minute) = sampled_points[i];
            let (right_idx, right_ts, right_minute) = sampled_points[i + 1];

            // Find all minute boundaries in this segment
            for target_minute in left_minute..=right_minute {
                if processed_minutes.contains(&target_minute) {
                    continue;
                }

                let minute_idx = find_minute_boundary_interpolation(
                    &journal_file,
                    &offsets,
                    left_idx,
                    right_idx,
                    left_ts,
                    right_ts,
                    target_minute,
                    &mut total_lookups,
                );

                if let Some(idx) = minute_idx {
                    minute_index.push((target_minute, idx));
                    processed_minutes.insert(target_minute);
                }
            }
        }

        // Sort by minute (in case we found them out of order)
        minute_index.sort_by_key(|&(minute, _)| minute);

        // Print the compact minute index
        println!("\n=== [B] Compact Minute Index ({:#?})", file.path);
        println!(
            "{:<10} {:<15} {:<15} {:<30}",
            "Entry#", "Minute", "Offset Index", "Timestamp"
        );
        println!("{}", "-".repeat(70));

        for (i, &(minute, offset_idx)) in minute_index.iter().take(10).enumerate() {
            if offset_idx < offsets.len() {
                let entry = journal_file.entry_ref(offsets[offset_idx]).unwrap();

                println!(
                    "{:<10} {:<15} {:<15} {} (minute {})",
                    i,
                    minute - start_minute, // Relative minute for readability
                    offset_idx,
                    format_timestamp(entry.header.realtime),
                    minute
                );
            }
        }

        midx_count += minute_index.len();
        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "Interpolation: {} entry offsets, {} minute indices in {} ms ({} timestamp lookups vs {} for sequential)",
        count,
        midx_count,
        elapsed.as_millis(),
        total_lookups,
        count
    );
}

/// Find the first entry of a target minute using interpolation search
#[allow(clippy::too_many_arguments)]
fn find_minute_boundary_interpolation(
    journal_file: &JournalFile<Mmap>,
    offsets: &[NonZeroU64],
    mut left: usize,
    mut right: usize,
    left_ts: u64,
    right_ts: u64,
    target_minute: u64,
    lookups: &mut usize,
) -> Option<usize> {
    let target_ts_start = target_minute * 60 * 1_000_000;
    let target_ts_end = (target_minute + 1) * 60 * 1_000_000;

    // First, check if the target minute exists in this range
    if left_ts >= target_ts_end || right_ts < target_ts_start {
        return None;
    }

    // Use interpolation search to find an entry within the target minute
    let mut found_in_minute = None;

    while left <= right {
        let mid = if right_ts > left_ts {
            // Interpolate position based on timestamp distribution
            let fraction =
                (target_ts_start.saturating_sub(left_ts)) as f64 / (right_ts - left_ts) as f64;
            let estimated = left + ((right - left) as f64 * fraction) as usize;
            estimated.min(right).max(left)
        } else {
            (left + right) / 2
        };

        let entry = journal_file.entry_ref(offsets[mid]).unwrap();
        let timestamp = entry.header.realtime;
        *lookups += 1;

        if timestamp >= target_ts_start && timestamp < target_ts_end {
            // Found an entry in the target minute
            found_in_minute = Some(mid);
            break;
        } else if timestamp < target_ts_start {
            left = mid + 1;
        } else {
            right = mid.saturating_sub(1);
        }

        if left > right {
            break;
        }
    }

    // If we found an entry in the minute, binary search backwards to find the first one
    if let Some(idx) = found_in_minute {
        let mut first_idx = idx;

        // Check backwards for the first entry of this minute
        let mut check_idx = idx.saturating_sub(1);
        while check_idx < idx {
            let entry = journal_file.entry_ref(offsets[check_idx]).unwrap();
            let timestamp = entry.header.realtime;
            *lookups += 1;

            if timestamp >= target_ts_start && timestamp < target_ts_end {
                first_idx = check_idx;
                if check_idx == 0 {
                    break;
                }
                check_idx = check_idx.saturating_sub(1);
            } else {
                break;
            }
        }

        Some(first_idx)
    } else {
        None
    }
}

fn format_timestamp(timestamp_us: u64) -> String {
    use chrono::{DateTime, Local, TimeZone};

    let seconds = (timestamp_us / 1_000_000) as i64;
    let nanoseconds = ((timestamp_us % 1_000_000) * 1000) as u32;

    // Assuming the timestamp is Unix epoch in microseconds
    if let Some(dt) = Local.timestamp_opt(seconds, nanoseconds).single() {
        dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
    } else {
        format!("{}μs", timestamp_us)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let registry = JournalRegistry::new()?;
    info!("Journal registry initialized");

    for dir in ["/var/log/journal", "/run/log/journal"] {
        match registry.add_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    let mut files = registry.query().execute();
    files.sort_by_key(|x| x.path.clone());
    files.sort_by_key(|x| x.size);
    files.reverse();
    // files.truncate(5);

    baseline(&files);
    sequential(&files);
    adaptive_probing(&files);

    let boundary_data = sequential_collect(&files);
    perfect_knowledge(&files, &boundary_data);

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}
