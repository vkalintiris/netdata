//! The `dump-index` subcommand — reconstructs log entries from an .sfst file.

use std::path::PathBuf;
use std::time::Instant;

use sfst::IndexReader;

pub fn run(path: &PathBuf, limit: Option<u32>) -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read(path)?;
    let reader = IndexReader::open(&data)?;

    let t = Instant::now();
    let fields = reader.field_table();
    let string_table = reader.build_string_table(&fields)?;
    eprintln!(
        "string table: {} entries ({:.0}ms)",
        string_table.len(),
        t.elapsed().as_secs_f64() * 1000.0,
    );

    let mut total_printed = 0u32;
    let stream = reader.stream();
    let t = Instant::now();
    let entries = reader.load_stream_entries()?;
    let timestamps = reader.load_timestamps()?;
    eprintln!(
        "stream {}/{}: {} entries ({:.0}ms)",
        stream.namespace,
        stream.name,
        entries.len(),
        t.elapsed().as_secs_f64() * 1000.0,
    );

    if timestamps.len() != entries.len() {
        eprintln!(
            "warning: timestamps ({}) and stream-entries ({}) lengths differ",
            timestamps.len(),
            entries.len(),
        );
    }

    for (pos, kv_ids) in entries.iter().enumerate() {
        if let Some(max) = limit {
            if total_printed >= max {
                return Ok(());
            }
        }

        let ts = timestamps.get(pos).copied().unwrap_or(0);
        println!("--- log {total_printed} (pos {pos}, t={ts}ns)");
        for id in kv_ids {
            let idx = id.0 as usize;
            if idx < string_table.len() {
                println!("  {}", string_table[idx]);
            } else {
                println!("  <unknown KvId({})>", id.0);
            }
        }

        total_printed += 1;
    }

    eprintln!("{total_printed} log entries");
    Ok(())
}
