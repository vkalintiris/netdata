//! Capture raw source events to a gzipped JSONL file for offline replay.
//!
//! Development/benchmarking tool: reuses the library's source modules
//! (`certstream::connect`, `jetstream::connect`) so every line written is
//! byte-identical to the raw `serde_json::Value` the OTLP pipeline would
//! consume in production — no parsing or conversion is duplicated here, and
//! no OTel export happens during capture.
//!
//! Output format (one JSON object per line, gzip-compressed):
//!
//! - `--source certstream` / `--source jetstream`: the raw source message,
//!   unchanged (certstream lines keep their `{"message_type":...,"data":...}`
//!   envelope; jetstream lines are the full event object).
//! - `--source mixed`: both sources connected concurrently; lines merged in
//!   arrival order at the writer and tagged as
//!   `{"_capture_src":"certstream"|"jetstream","event":{...raw...}}`.
//!
//! Disconnect tolerance: source connections are re-established with backoff
//! (the same policy `runner::run` applies to the OTLP bridges), so a source
//! restarting mid-capture does not end the run. Two failure modes remain:
//! a `--stall-timeout-secs` window with zero events aborts (source appears
//! dead), and a killed process leaves an unfinished `.tmp` file (gzip without
//! end-of-stream trailer). In both cases redo the capture; the final output
//! path only ever holds a cleanly finished gzip stream (write goes to
//! `<out>.tmp`, renamed on success).

use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Parser;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use otel_streams::args;
use otel_streams::{certstream, jetstream};

/// Which live source(s) to capture.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum SourceArg {
    Certstream,
    Jetstream,
    Mixed,
}

#[derive(Parser)]
#[command(name = "capture")]
#[command(about = "Capture raw certstream/jetstream events to gzipped JSONL for offline replay")]
struct Args {
    /// Source(s) to capture; `mixed` connects both concurrently, merges by
    /// arrival order and tags each line with its source
    #[arg(long)]
    source: SourceArg,

    /// Output file for the gzip-compressed JSONL capture
    #[arg(long)]
    out: String,

    /// Total number of events to capture before stopping
    #[arg(long)]
    max_events: usize,

    /// Per-source event cap for `--source mixed` (default: half of --max-events)
    #[arg(long)]
    per_source: Option<usize>,

    /// CertStream WebSocket endpoint
    #[arg(long, default_value = "ws://127.0.0.1:8080/")]
    certstream_url: String,

    /// Jetstream WebSocket endpoint, path included (query filters are appended)
    #[arg(long, default_value = "wss://jetstream2.us-east.bsky.network/subscribe")]
    jetstream_url: String,

    /// Comma-separated Jetstream collection filters, passed through to the URL
    #[arg(long)]
    collections: Option<String>,

    /// Abort when no event arrives for this many seconds (source permanently dead)
    #[arg(long, default_value_t = 300)]
    stall_timeout_secs: u64,

    /// Tracing log level
    #[arg(long, default_value = "info")]
    log_level: String,
}

/// Corpus line source tag (also the value of `_capture_src` for mixed output).
#[derive(Clone, Copy, PartialEq)]
enum Tag {
    Certstream,
    Jetstream,
}

impl Tag {
    fn as_str(self) -> &'static str {
        match self {
            Tag::Certstream => "certstream",
            Tag::Jetstream => "jetstream",
        }
    }
}

/// Envelope written for `--source mixed`; field order is part of the corpus
/// format (`_capture_src` first, then the untouched raw event).
#[derive(Serialize)]
struct TaggedLine<'a> {
    _capture_src: &'a str,
    event: &'a serde_json::Value,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args::init_tls_and_logging(&args.log_level);

    let jetstream_ws_url = jetstream::build_jetstream_url(&args.jetstream_url, args.collections.as_deref())
        .context("building jetstream URL")?;

    let (certstream_cap, jetstream_cap) = match args.source {
        SourceArg::Certstream => (args.max_events, 0),
        SourceArg::Jetstream => (0, args.max_events),
        SourceArg::Mixed => {
            let per_source = args.per_source.unwrap_or(args.max_events.div_ceil(2));
            (per_source, per_source)
        }
    };

    if let Some(parent) = Path::new(&args.out).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory for {}", args.out))?;
    }
    let tmp_out = format!("{}.tmp", args.out);

    let started = Instant::now();
    info!(
        source = ?args.source,
        out = %args.out,
        max_events = args.max_events,
        certstream_cap,
        jetstream_cap,
        certstream_url = %args.certstream_url,
        jetstream_url = %jetstream_ws_url,
        "starting capture"
    );

    let done = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::channel::<(Tag, serde_json::Value)>(4096);

    let mut pumps = tokio::task::JoinSet::new();
    if certstream_cap > 0 {
        let url = args.certstream_url.clone();
        let tx = tx.clone();
        let done = done.clone();
        pumps.spawn(pump(
            "CertStream",
            Tag::Certstream,
            certstream_cap,
            done,
            tx,
            move |event_tx| {
                let url = url.clone();
                async move { certstream::connect(&url, event_tx).await }
            },
        ));
    }
    if jetstream_cap > 0 {
        let url = jetstream_ws_url.clone();
        let tx = tx.clone();
        let done = done.clone();
        pumps.spawn(pump(
            "Jetstream",
            Tag::Jetstream,
            jetstream_cap,
            done,
            tx,
            move |event_tx| {
                let url = url.clone();
                async move { jetstream::connect(&url, event_tx).await }
            },
        ));
    }
    drop(tx);

    let file = std::fs::File::create(&tmp_out).with_context(|| format!("creating {tmp_out}"))?;
    let mut gz = GzEncoder::new(std::io::BufWriter::new(file), Compression::new(6));

    let stall = Duration::from_secs(args.stall_timeout_secs);
    let tagged_output = matches!(args.source, SourceArg::Mixed);
    let mut counts = [0usize; 2];
    let mut total = 0usize;
    let mut raw_bytes = 0u64;

    while total < args.max_events {
        let received = tokio::time::timeout(stall, rx.recv()).await;
        let (tag, raw_json) = match received {
            Err(_) => bail!(
                "no event for {}s: source appears dead, capture aborted \
                 (unfinished output stays at {tmp_out} and must be discarded)",
                args.stall_timeout_secs
            ),
            Ok(None) => bail!(
                "all sources stopped after {total}/{} events \
                 (unfinished output stays at {tmp_out} and must be discarded)",
                args.max_events
            ),
            Ok(Some(pair)) => pair,
        };

        let line = if tagged_output {
            serde_json::to_vec(&TaggedLine {
                _capture_src: tag.as_str(),
                event: &raw_json,
            })?
        } else {
            serde_json::to_vec(&raw_json)?
        };
        gz.write_all(&line).with_context(|| format!("writing to {tmp_out}"))?;
        gz.write_all(b"\n").with_context(|| format!("writing to {tmp_out}"))?;
        raw_bytes += line.len() as u64 + 1;

        counts[tag as usize] += 1;
        total += 1;
        if total.is_multiple_of(10_000) {
            info!(
                total,
                certstream = counts[Tag::Certstream as usize],
                jetstream = counts[Tag::Jetstream as usize],
                elapsed_secs = started.elapsed().as_secs(),
                "capture progress"
            );
        }
    }

    done.store(true, Ordering::Relaxed);
    drop(rx);
    pumps.abort_all();

    gz.finish()
        .with_context(|| format!("finishing gzip stream in {tmp_out}"))?
        .into_inner()
        .map_err(|e| anyhow::anyhow!("flushing {tmp_out}: {e}"))?;
    std::fs::rename(&tmp_out, &args.out)
        .with_context(|| format!("renaming {tmp_out} to {}", args.out))?;
    let compressed_bytes = std::fs::metadata(&args.out)?.len();

    info!(
        out = %args.out,
        total,
        certstream = counts[Tag::Certstream as usize],
        jetstream = counts[Tag::Jetstream as usize],
        elapsed_secs = started.elapsed().as_secs(),
        events_per_sec = (total as f64 / started.elapsed().as_secs().max(1) as f64) as u64,
        raw_bytes,
        compressed_bytes,
        "capture complete"
    );

    Ok(())
}

/// Read one source through its library `connect`, forwarding raw events into
/// the shared capture channel until `cap` events were sent, the capture is
/// done, or the writer went away. Reconnects with backoff between attempts,
/// mirroring `runner::run`'s disconnect policy.
async fn pump<E, F, Fut>(
    name: &'static str,
    tag: Tag,
    cap: usize,
    done: Arc<AtomicBool>,
    out: mpsc::Sender<(Tag, serde_json::Value)>,
    connect: F,
) where
    E: Send + 'static,
    F: Fn(mpsc::Sender<(E, serde_json::Value)>) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let sent = Arc::new(AtomicUsize::new(0));
    let mut backoff = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);

    loop {
        if done.load(Ordering::Relaxed) {
            break;
        }

        let (event_tx, mut event_rx) = mpsc::channel::<(E, serde_json::Value)>(1000);
        let forwarder = {
            let out = out.clone();
            let sent = sent.clone();
            tokio::spawn(async move {
                while let Some((_event, raw_json)) = event_rx.recv().await {
                    if out.send((tag, raw_json)).await.is_err() {
                        break;
                    }
                    if sent.fetch_add(1, Ordering::Relaxed) + 1 >= cap {
                        break;
                    }
                }
            })
        };

        match connect(event_tx).await {
            Ok(()) => {
                backoff = Duration::from_secs(1);
                warn!("{name}: connection closed");
            }
            Err(e) => {
                error!("{name}: connection error: {e}; retrying in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
        let _ = forwarder.await;

        if sent.load(Ordering::Relaxed) >= cap {
            info!("{name}: reached per-source cap of {cap} events");
            break;
        }
        if done.load(Ordering::Relaxed) {
            break;
        }
        warn!("{name}: reconnecting");
    }
}
