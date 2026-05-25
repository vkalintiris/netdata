//! `sfsq` CLI — print the summary of a single SFST file.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde::Serialize;

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
struct SummaryView<'a> {
    file: &'a str,
    stream: StreamView<'a>,
    time_range: TimeRangeView,
    total_logs: u32,
    chunks: u16,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let reader = sfsq::Reader::open(&cli.file)?;
    let summary = reader.summary();
    let stream = reader.stream();
    let delta_s = summary.max_timestamp_s.saturating_sub(summary.min_timestamp_s);
    let path_str = cli.file.display().to_string();

    match cli.format {
        Format::Debug => {
            println!("File: {}", path_str);
            println!(
                "Stream: namespace={}, name={}",
                stream.namespace, stream.name
            );
            println!(
                "Time range: {} .. {}  ({}s)",
                summary.min_timestamp_s, summary.max_timestamp_s, delta_s
            );
            println!("Total logs: {}", summary.total_logs);
            println!("Chunks: {}", reader.chunk_count());
        }
        Format::Ndjson => {
            let view = SummaryView {
                file: &path_str,
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
                chunks: reader.chunk_count(),
            };
            println!("{}", serde_json::to_string(&view)?);
        }
    }
    Ok(())
}
