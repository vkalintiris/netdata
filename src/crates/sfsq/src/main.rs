//! Single-file SFST query CLI.
//!
//! Run with `cargo run -p sfsq -- <file.sfst> [...]` or as the installed
//! `sfsq` binary. See `--help` for full flag list.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{Parser, ValueEnum};
use sfsq::{Anchor, Direction, Filter, LogQuery, LogQueryParamsBuilder, ResolvedLog};

#[derive(Parser)]
#[command(name = "sfsq", about = "Query a single SFST log index")]
struct Cli {
    /// Path to the .sfst file.
    file: PathBuf,

    /// Field=value selection, repeatable; combined with AND.
    #[arg(short, long = "filter", value_parser = parse_kv)]
    filters: Vec<(String, String)>,

    /// Lower time bound (inclusive). RFC3339 (`2026-05-26T10:00:00Z`)
    /// or integer nanoseconds since epoch.
    #[arg(long, value_parser = parse_ts)]
    after: Option<i64>,

    /// Upper time bound (exclusive). Same format as `--after`.
    #[arg(long, value_parser = parse_ts)]
    before: Option<i64>,

    /// Maximum number of entries to return.
    #[arg(short = 'n', long, default_value_t = 200)]
    limit: usize,

    /// Iteration direction.
    #[arg(long, value_enum, default_value_t = DirectionArg::Backward)]
    direction: DirectionArg,

    /// Where to start: `latest`, `earliest`, RFC3339 timestamp, or
    /// integer nanoseconds. Defaults: `latest` for backward, `earliest`
    /// for forward.
    #[arg(long)]
    anchor: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = FormatArg::Text)]
    format: FormatArg,
}

#[derive(Clone, Copy, ValueEnum)]
enum DirectionArg {
    Forward,
    Backward,
}

impl From<DirectionArg> for Direction {
    fn from(d: DirectionArg) -> Self {
        match d {
            DirectionArg::Forward => Direction::Forward,
            DirectionArg::Backward => Direction::Backward,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Text,
    Ndjson,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let direction = Direction::from(cli.direction);

    let anchor = match cli.anchor.as_deref() {
        Some("latest") => Anchor::Latest,
        Some("earliest") => Anchor::Earliest,
        Some(s) => Anchor::At(parse_ts(s)?),
        None => match direction {
            Direction::Backward => Anchor::Latest,
            Direction::Forward => Anchor::Earliest,
        },
    };

    let mut builder = LogQueryParamsBuilder::new(anchor, direction).with_limit(cli.limit);
    if !cli.filters.is_empty() {
        let mut filter = Filter::new();
        for (field, value) in cli.filters {
            filter = filter.select(field, value);
        }
        builder = builder.with_filter(filter);
    }
    if let Some(after) = cli.after {
        builder = builder.with_after(after);
    }
    if let Some(before) = cli.before {
        builder = builder.with_before(before);
    }
    let params = builder.build()?;

    let data = std::fs::read(&cli.file)?;
    let reader = sfst::IndexReader::open(&data)?;
    let mut query = LogQuery::new(&reader, params);
    let logs = query.run()?;

    match cli.format {
        FormatArg::Text => print_text(&logs),
        FormatArg::Ndjson => print_ndjson(&logs)?,
    }
    eprintln!("{} logs returned", logs.len());
    Ok(())
}

fn print_text(logs: &[ResolvedLog]) {
    for log in logs {
        let ts = format_ts(log.timestamp_ns);
        println!("--- pos {} t={} ({})", log.position, log.timestamp_ns, ts);
        for (key, value) in &log.attrs {
            println!("  {key}={value}");
        }
    }
}

fn print_ndjson(logs: &[ResolvedLog]) -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::{Map, Value, json};
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for log in logs {
        let mut attrs = Map::new();
        for (k, v) in &log.attrs {
            attrs.insert(k.clone(), Value::String(v.clone()));
        }
        let obj = json!({
            "position": log.position,
            "timestamp_ns": log.timestamp_ns,
            "attrs": Value::Object(attrs),
        });
        serde_json::to_writer(&mut out, &obj)?;
        std::io::Write::write_all(&mut out, b"\n")?;
    }
    Ok(())
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("expected `field=value`, got `{s}`"))
}

/// Accept either integer nanoseconds since epoch or RFC3339.
fn parse_ts(s: &str) -> Result<i64, String> {
    if let Ok(n) = s.parse::<i64>() {
        return Ok(n);
    }
    let dt: DateTime<Utc> = s
        .parse()
        .map_err(|e| format!("not RFC3339 or integer nanoseconds: {e}"))?;
    Ok(dt.timestamp_nanos_opt().ok_or("timestamp out of range")?)
}

fn format_ts(ns: i64) -> String {
    // `DateTime::from_timestamp` rejects out-of-range values cleanly;
    // pair an Option fallback with a clear marker so we never panic on
    // corrupted or adversarial timestamps from disk.
    let secs = ns.div_euclid(1_000_000_000);
    let sub_nanos = ns.rem_euclid(1_000_000_000) as u32;
    DateTime::<Utc>::from_timestamp(secs, sub_nanos)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| format!("(out-of-range: {ns}ns)"))
}
