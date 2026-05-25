//! `sfsq` CLI — query a single SFST file.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde::Serialize;
use sfsq::{Resolution, Selection, SelectionStats};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    Debug,
    Ndjson,
}

#[derive(Debug, Parser)]
#[command(version, about = "Query a single SFST file.")]
struct Cli {
    /// Path to the .sfst file.
    file: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Debug)]
    format: Format,

    /// Selection of the form FIELD=VALUE. Repeatable; same field across
    /// multiple flags is OR'd, different fields are AND'd.
    #[arg(long = "select", value_name = "FIELD=VALUE")]
    selections: Vec<String>,
}

#[derive(Serialize)]
struct StreamView<'a> {
    namespace: &'a str,
    name: &'a str,
}

#[derive(Serialize)]
struct TimeRangeView {
    min_s: u32,
    max_s: u32,
    delta_s: u32,
}

#[derive(Serialize)]
struct SelectionView<'a> {
    field: &'a str,
    values: &'a [String],
    tier: String,
    chunk_index: Option<u16>,
    hits: u64,
}

#[derive(Serialize)]
struct OutputView<'a> {
    file: &'a str,
    stream: StreamView<'a>,
    time_range: TimeRangeView,
    total_logs: u32,
    chunks: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    selections: Option<Vec<SelectionView<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched: Option<u64>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let selections = sfsq::parse_selections(&cli.selections)?;
    let reader = sfsq::Reader::open(&cli.file)?;
    let summary = reader.summary();
    let stream = reader.stream();
    let delta_s = summary.max_timestamp_s.saturating_sub(summary.min_timestamp_s);
    let path_str = cli.file.display().to_string();

    let resolution: Option<Resolution> = if selections.is_empty() {
        None
    } else {
        Some(reader.select(&selections)?)
    };

    match cli.format {
        Format::Debug => print_debug(&path_str, summary, stream, delta_s, reader.chunk_count(),
                                     &selections, resolution.as_ref()),
        Format::Ndjson => print_ndjson(&path_str, summary, stream, delta_s, reader.chunk_count(),
                                       &selections, resolution.as_ref())?,
    }
    Ok(())
}

fn print_debug(
    path: &str,
    summary: &sfst::FileSummary,
    stream: &sfst::StreamEntry,
    delta_s: u32,
    chunks: u16,
    selections: &[Selection],
    resolution: Option<&Resolution>,
) {
    println!("File: {}", path);
    println!(
        "Stream: namespace={}, name={}",
        stream.namespace, stream.name
    );
    println!(
        "Time range: {} .. {}  ({}s)",
        summary.min_timestamp_s, summary.max_timestamp_s, delta_s
    );
    println!("Total logs: {}", summary.total_logs);
    println!("Chunks: {}", chunks);

    if let Some(res) = resolution {
        println!("Selections:");
        for (sel, stats) in selections.iter().zip(res.per_selection.iter()) {
            print_selection_line(sel, stats);
        }
        println!("Matched: {}", res.bitmap.cardinality());
    }
}

fn print_selection_line(sel: &Selection, stats: &SelectionStats) {
    let chunk_suffix = match stats.chunk_index {
        Some(idx) => format!("#{}", idx),
        None => String::new(),
    };
    println!(
        "  {}: {}  (tier={}{}, hits={})",
        sel.field,
        sel.values.join(" OR "),
        stats.tier,
        chunk_suffix,
        stats.hits
    );
}

fn print_ndjson(
    path: &str,
    summary: &sfst::FileSummary,
    stream: &sfst::StreamEntry,
    delta_s: u32,
    chunks: u16,
    selections: &[Selection],
    resolution: Option<&Resolution>,
) -> anyhow::Result<()> {
    let selection_views = resolution.map(|res| {
        selections
            .iter()
            .zip(res.per_selection.iter())
            .map(|(sel, stats)| SelectionView {
                field: &sel.field,
                values: &sel.values,
                tier: stats.tier.to_string(),
                chunk_index: stats.chunk_index,
                hits: stats.hits,
            })
            .collect::<Vec<_>>()
    });

    let view = OutputView {
        file: path,
        stream: StreamView {
            namespace: &stream.namespace,
            name: &stream.name,
        },
        time_range: TimeRangeView {
            min_s: summary.min_timestamp_s,
            max_s: summary.max_timestamp_s,
            delta_s,
        },
        total_logs: summary.total_logs,
        chunks,
        selections: selection_views,
        matched: resolution.map(|r| r.bitmap.cardinality()),
    };
    println!("{}", serde_json::to_string(&view)?);
    Ok(())
}
