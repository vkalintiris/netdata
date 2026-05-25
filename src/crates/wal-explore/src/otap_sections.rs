//! The `sections` subcommand — prints section sizes of an .sfst file.

use std::path::PathBuf;

use sfst::FieldTier;
use log_index::reader::IndexReader;

pub fn run(path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read(path)?;
    let file_size = data.len();
    let reader = IndexReader::open(&data)?;
    let fields = reader.field_table()?;
    let sfst = sfst::Reader::open(&data)?;

    let mut total_sections = 0usize;

    // META
    if let Ok(raw) = sfst.metadata_raw() {
        print_section("META", raw.len(), file_size);
        total_sections += raw.len();
    }

    // FLDS
    if let Ok(raw) = sfst.fields_raw() {
        print_section("FLDS", raw.len(), file_size);
        total_sections += raw.len();
        let mut sorted_fields = fields.to_vec();
        sorted_fields.sort_by(|a, b| a.cardinality.cmp(&b.cardinality));
        let max_name = sorted_fields
            .iter()
            .map(|f| f.name.len())
            .max()
            .unwrap_or(0);
        for f in &sorted_fields {
            let tier = match f.tier {
                FieldTier::Low => "low",
                FieldTier::Mid => "mid",
                FieldTier::High => "high",
            };
            println!(
                "  {:<width$}  {:>6}  {tier}",
                f.name,
                f.cardinality,
                width = max_name
            );
        }
    }

    // PRIM
    let mut prim_size = 0usize;
    if let Ok(raw) = sfst.primary_raw() {
        prim_size = raw.len();
        print_section("PRIM", prim_size, file_size);
        total_sections += prim_size;
    }

    // Secondary chunks: field chunks (mid/high), then stream chunks
    let mut field_total = 0usize;
    let mut mid_total = 0usize;
    let mut high_total = 0usize;
    let mut mid_idx = 0u16;
    let mut high_idx = 0u16;
    for field in fields {
        match field.tier {
            FieldTier::Low => continue,
            FieldTier::Mid => {
                if let Ok(raw) = sfst.mid_field_raw(mid_idx) {
                    print_section(
                        &format!("MF[{mid_idx}] mid: {}", field.name),
                        raw.len(),
                        file_size,
                    );
                    mid_total += raw.len();
                    field_total += raw.len();
                }
                mid_idx += 1;
            }
            FieldTier::High => {
                if let Ok(raw) = sfst.high_field_raw(high_idx) {
                    print_section(
                        &format!("HF[{high_idx}] high: {}", field.name),
                        raw.len(),
                        file_size,
                    );
                    high_total += raw.len();
                    field_total += raw.len();
                }
                high_idx += 1;
            }
        }
    }

    // The single stream chunk.
    let stream = reader.stream();
    let mut stream_total = 0usize;
    if let Ok(raw) = sfst.stream_entries_raw() {
        print_section(
            &format!("STREAM {}/{}", stream.namespace, stream.name),
            raw.len(),
            file_size,
        );
        stream_total += raw.len();
    }

    total_sections += field_total + stream_total;

    println!();
    print_section("low tier (PRIM)", prim_size, file_size);
    print_section("mid tier chunks", mid_total, file_size);
    print_section("high tier chunks", high_total, file_size);
    println!();
    print_section("field chunks total", field_total, file_size);
    print_section("stream chunks total", stream_total, file_size);
    print_section("sections total", total_sections, file_size);
    println!("{:<40} {:>10}", "file size", format_size(file_size),);
    let overhead = file_size.saturating_sub(total_sections);
    print_section("overhead (header + TOC)", overhead, file_size);

    Ok(())
}

fn print_section(name: &str, size: usize, total: usize) {
    let pct = if total > 0 {
        size as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    println!("{:<40} {:>10}  ({:5.1}%)", name, format_size(size), pct,);
}

fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
