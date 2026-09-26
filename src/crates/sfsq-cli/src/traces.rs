//! `sfsq-cli trace` — the traces query engine from the terminal, without a
//! running agent: point it at a mix of sealed SFSTs and traces WALs and it
//! merges one trace through the shared combiner.
//!
//! WAL inputs are served as tail scans over the file's full frame range —
//! right for shut-down or recovered WALs (the dev case); an actively
//! written WAL should be queried through a live agent instead.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use anyhow::{Context, Result, bail};
use tokio_util::sync::CancellationToken;

use sfsq::traces::{
    QueryStatus, SourceId, TraceQuery, TraceSfstCandidate, TraceSource, TraceWalTail, WalCoverage,
    trace_by_id,
};

/// Reconstruct one trace across sealed SFSTs and traces WALs.
#[derive(Debug, clap::Args)]
pub struct TraceArgs {
    /// The trace id as hex (32 chars = 16 bytes; case-insensitive).
    #[arg(long)]
    pub trace_id: String,

    /// A sealed traces SFST file. Repeatable.
    #[arg(long = "sfst")]
    pub sfsts: Vec<PathBuf>,

    /// A flattened traces WAL file, scanned whole as a tail. Repeatable.
    #[arg(long = "wal")]
    pub wals: Vec<PathBuf>,

    /// Span cap override (default 65,536); 0 is rejected.
    #[arg(long)]
    pub span_cap: Option<usize>,
}

/// Parse a 32-hex-char trace id.
fn parse_trace_id(s: &str) -> Result<sfst::TraceId> {
    let s = s.trim();
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("--trace-id must be 32 hex chars (16 bytes), got {s:?}");
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        bytes[i] = u8::from_str_radix(std::str::from_utf8(chunk)?, 16)?;
    }
    Ok(sfst::TraceId::from(bytes))
}

/// Build the engine source set from CLI paths: sealed SFSTs by summary
/// read (maps header/TOC/SUMR pages only — the engine maps the file
/// separately for the actual work), WALs as whole-range tails.
fn build_sources(sfsts: &[PathBuf], wals: &[PathBuf]) -> Result<Vec<TraceSource>> {
    if sfsts.is_empty() && wals.is_empty() {
        bail!("provide at least one --sfst or --wal source");
    }
    // Source identity is the path STRING (SourceId, wal_id), so the same
    // file through two aliases (symlink, relative vs absolute) would pass
    // the engine's DuplicateSource check and be scanned twice, inflating
    // UNSET-span counts. Canonicalize so aliases collide and the duplicate
    // is rejected — BEST-EFFORT: a path canonicalize cannot resolve keeps
    // its user-supplied identity, so deleted-but-open files through
    // `/proc/<pid>/fd/N` (a forensic staple) still open, and nonexistent
    // paths surface the per-kind errors below instead of a generic one.
    let canonical = |path: &PathBuf| path.canonicalize().unwrap_or_else(|_| path.clone());
    let sfsts: Vec<PathBuf> = sfsts.iter().map(&canonical).collect();
    let wals: Vec<PathBuf> = wals.iter().map(&canonical).collect();
    let mut sources: Vec<TraceSource> = Vec::new();
    for path in &sfsts {
        let summary = sfst::read_summary_path(path)
            .with_context(|| format!("not a readable SFST: {}", path.display()))?;
        sources.push(TraceSource::Sfst(TraceSfstCandidate {
            source_id: SourceId::new(path.display().to_string()),
            summary,
            source: sfsq::Source::File(path.clone()),
            coverage: None,
        }));
    }
    for path in &wals {
        let len = std::fs::metadata(path)
            .with_context(|| format!("stat {}", path.display()))?
            .len();
        if len < wal::HEADER_SIZE as u64 {
            bail!(
                "{} is shorter than a WAL header ({len} bytes) — not a WAL file",
                path.display()
            );
        }
        // The bounded reader treats a frame crossing its end bound as an
        // expected torn tail — designed for `end = valid_up_to`, not a
        // physical file length. Scan the frame headers first so a
        // truncated tail is SURFACED and only complete frames are read
        // (the `discover` module's convention; corrupt data never drops
        // silently under a `complete` status).
        // Content corruption is a PER-SOURCE failure (warn + skip, the
        // discover convention) — one corrupt WAL must not abort a
        // multi-source query. Path-level problems (nonexistent, shorter
        // than a header) stay hard errors above: those are argument
        // typos, not data problems.
        let boundaries = match wal::scan_frame_boundaries(
            path,
            wal::FrameRange::new(wal::HEADER_SIZE as u64, len),
        ) {
            Ok(b) => b,
            // I/O errors (permissions, a directory, deleted mid-run) are
            // path-level like the stat above — fatal.
            Err(e @ wal::Error::Io(_)) => {
                return Err(e).with_context(|| format!("reading {}", path.display()));
            }
            Err(e) => {
                tracing::warn!("skipping WAL {}: {e}", path.display());
                continue;
            }
        };
        let valid_end = boundaries
            .last()
            .map_or(wal::HEADER_SIZE as u64, |b| b.end_offset);
        if valid_end != len {
            tracing::warn!(
                "{}: torn or truncated tail — complete frames end at byte {valid_end} of {len}; \
                 reading the intact prefix only",
                path.display()
            );
        }
        let range = wal::FrameRange::new(wal::HEADER_SIZE as u64, valid_end);
        sources.push(TraceSource::Tail(TraceWalTail {
            source_id: SourceId::new(path.display().to_string()),
            path: path.clone(),
            coverage: WalCoverage {
                wal_id: path.display().to_string().into(),
                range,
            },
        }));
    }
    Ok(sources)
}

pub fn run_trace(args: &TraceArgs, out: &mut dyn std::io::Write) -> Result<()> {
    let trace_id = parse_trace_id(&args.trace_id)?;
    let sources = build_sources(&args.sfsts, &args.wals)?;

    let mut query = TraceQuery::new(trace_id);
    if let Some(cap) = args.span_cap {
        query = query.span_cap(cap);
    }
    let data = trace_by_id(
        sources,
        query,
        CancellationToken::new(),
        Arc::new(AtomicUsize::new(0)),
    )?;

    let t = &data.trace;
    let status = match &data.status {
        QueryStatus::Complete => "complete".to_string(),
        QueryStatus::Partial(reasons) => format!("PARTIAL {reasons:?}"),
    };
    let typed = data.field_kinds.fields.len()
        + data.field_kinds.event_attributes.len()
        + data.field_kinds.link_attributes.len();
    writeln!(
        out,
        "trace {}: {} span(s), {} root(s), status {status}, {typed} typed field(s)",
        args.trace_id,
        t.spans.len(),
        t.roots.len(),
    )?;

    // Iterative revisit-guarded DFS (cycle edges survive in `children`).
    // The summary root is computed ONCE (it scans the span list).
    let summary_root_idx = t.summary_root();
    let mut visited = vec![false; t.spans.len()];
    let mut stack: Vec<(usize, usize)> = t.roots.iter().rev().map(|&i| (i, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        if std::mem::replace(&mut visited[i], true) {
            continue;
        }
        let s = &t.spans[i];
        let name = s
            .fields
            .iter()
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v.as_str())
            .unwrap_or("<unnamed>");
        let summary_root = (summary_root_idx == Some(i)).then_some(" [summary root]");
        writeln!(
            out,
            "{:indent$}{} kind={} start={} dur={}ns ev={} lk={}{}",
            "",
            name,
            s.kind,
            s.start_ns,
            s.duration_ns,
            s.events.len(),
            s.links.len(),
            summary_root.unwrap_or(""),
            indent = depth * 2,
        )?;
        for &c in t.children[i].iter().rev() {
            stack.push((c, depth + 1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_wal_is_skipped_per_source() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("garbage.wal");
        std::fs::write(&bad, vec![0xFFu8; wal::HEADER_SIZE * 2]).unwrap();
        let good = dir.path().join("good");
        std::fs::create_dir_all(&good).unwrap();
        let seq = std::sync::Arc::new(wal::SeqAllocator::ephemeral(0));
        let mut writer = wal::Writer::new(
            &good,
            wal::Config::default(),
            seq,
            wal::FileStamp {
                pipeline_id: 1,
                payload_format: ng_flatten::TRACE_FRAME_PAYLOAD_FORMAT,
            },
            wal::test_identity(),
        )
        .unwrap();
        writer
            .write_frame(
                0,
                b"",
                &[7u8; 64],
                wal::FrameMeta {
                    entry_count: 1,
                    ingestion_ns: file_registry::TimestampNs(1),
                    log_ts_range: None,
                },
            )
            .unwrap();
        writer.shutdown_all().unwrap();
        let good_wal = std::fs::read_dir(&good)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "wal"))
            .expect("one WAL");

        let sources = build_sources(&[], &[bad, good_wal.clone()]).unwrap();
        assert_eq!(sources.len(), 1, "the corrupt WAL is skipped, not fatal");
        assert!(matches!(
            &sources[0],
            TraceSource::Tail(t) if t.path == good_wal.canonicalize().unwrap()
        ));
    }

    /// The torn-tail clamp: a WAL truncated mid-frame serves only the
    /// intact prefix, and a header-only file yields an empty range —
    /// never a bounded scan that relabels truncation as "complete".
    #[test]
    fn wal_sources_clamp_to_the_last_complete_frame() {
        let dir = tempfile::tempdir().unwrap();
        let seq = std::sync::Arc::new(wal::SeqAllocator::ephemeral(0));
        let mut writer = wal::Writer::new(
            dir.path(),
            wal::Config::default(),
            seq,
            wal::FileStamp {
                pipeline_id: 1,
                payload_format: ng_flatten::TRACE_FRAME_PAYLOAD_FORMAT,
            },
            wal::test_identity(),
        )
        .unwrap();
        writer
            .write_frame(
                0,
                b"",
                &[7u8; 200],
                wal::FrameMeta {
                    entry_count: 1,
                    ingestion_ns: file_registry::TimestampNs(1),
                    log_ts_range: None,
                },
            )
            .unwrap();
        writer.shutdown_all().unwrap();
        let path = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "wal"))
            .expect("one WAL");

        let range_of = |p: &PathBuf| -> wal::FrameRange {
            match build_sources(&[], std::slice::from_ref(p)).unwrap().pop().unwrap() {
                TraceSource::Tail(t) => t.coverage.range,
                _ => panic!("expected a tail source"),
            }
        };

        // Intact: the range reaches EOF.
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!(range_of(&path).end(), len);

        // Truncated mid-frame: clamp to the last complete boundary.
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(len - 1).unwrap();
        let clamped = range_of(&path);
        assert!(clamped.end() < len - 1, "tail dropped, prefix kept");
        assert_eq!(clamped.end(), wal::HEADER_SIZE as u64, "one-frame file: prefix is empty");

        // Header-only: empty range, no error.
        f.set_len(wal::HEADER_SIZE as u64).unwrap();
        let empty = range_of(&path);
        assert_eq!((empty.start(), empty.end()), (wal::HEADER_SIZE as u64, wal::HEADER_SIZE as u64));
    }
}
