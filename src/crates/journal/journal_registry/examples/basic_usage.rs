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
        let mut current_minute = first_timestamp / (60 * 1_000_000);
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
        println!("\n=== [A] Compact Minute Index ({:#?})", file.path);
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
                    minute - first_minute, // Relative minute for readability
                    offset_idx,
                    format_timestamp(entry.header.realtime),
                    minute
                );
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

    baseline(&files);
    sequential(&files);
    interpolation_minute_index(&files);

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}
