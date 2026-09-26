//! Live trace-query sources: the bridge between the traces pipeline's
//! [`TenantRegistries`] view and the `sfsq::traces` source types. Feeds
//! every one of the `otel-traces` Function's data modes.
//!
//! With remote storage enabled, files that local retention evicted are read
//! back: the remote planner selects the catalog entries the range needs
//! (catalogs read off the registry lock), they are downloaded once
//! through the shared download cache, and each entry becomes a sealed source
//! at its cache path — or, when it could not be obtained, an unavailable
//! source the engine reports as `remote_unavailable`. So is every catalog
//! that could not be read, over the time span its filename declares.
//!
//! Mirrors the logs handler's snapshot discipline
//! (`rpc/logs/handler.rs::resolve_wal`): descriptors captured under ONE
//! brief registry read lock (one `valid_up_to` per WAL — the whole
//! query sees one consistent durable prefix), chunk SFSTs built OFF the
//! lock through the shared singleflight [`ChunkCache`] (traces seal:
//! [`ng_index::build_sfst_traces_range`]). A WAL whose frames won't scan or
//! whose chunks won't build/parse is refused WHOLE for this snapshot (its
//! data returns via the sealed SFST after rotation, or on a later query —
//! build errors aren't cached) and becomes a [`TraceSource::Failed`], which
//! every operation reports as `source_failure`: never a silent gap.
//!
//! Source identity per `sfsq::traces::sources` docs, derived from the
//! file's full `FileId` and never from its directory: a sealed file is
//! `<stem>.sfst`; a WAL is `<stem>.wal`, its chunks
//! `<stem>.wal#chunk<index:06>` and its tail `<stem>.wal#tail<start>`,
//! with [`WalCoverage`] over the WAL's id so the engine's overlap
//! validation sees every WAL-derived byte range. One file therefore has
//! one name wherever its bytes are served from — the engine's duplicate
//! check sees a double inclusion, and its id-ordered reading does not
//! change with location. The
//! zero-padded chunk index keeps that order numeric.
//!
//! The supplier takes its window verbatim: canonicalizing the wire
//! request's window (the logs precedent defaults an unspecified
//! `after`/`before` to a recent window before querying) is the CALLER's
//! responsibility — the wire adapter of each data mode — not this
//! module's. A raw `0..0` here is an empty query, deliberately.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bridge::function::ProgressState;
use file_cache::CacheError;
use file_lifecycle::chunk::ChunkCache;
use file_lifecycle::query::{CatalogFile, RemotePlan};
use file_lifecycle::registry::{TenantRegistries, WalDesc};
use file_lifecycle::remote_read::RemoteRead;
use file_registry::{FileId, TenantId};
use sfsq::traces::{
    SourceId, TraceFailed, TraceSfstCandidate, TraceSource, TraceUnavailable, TraceWalTail,
    WalCoverage,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use wal::prefix::{chunk_boundaries, tail_start};

/// Filename extensions of the source-id derivation (the registries'
/// own on-disk extensions, which those crates keep private).
const SFST_EXT: &str = "sfst";
const WAL_EXT: &str = "wal";

/// One WAL resolved to buildable parts: its chunk images and tail range.
struct ResolvedWal {
    path: PathBuf,
    chunks: Vec<ResolvedChunk>,
    /// The trailing un-chunked byte range, when non-empty.
    tail: Option<wal::FrameRange>,
}

struct ResolvedChunk {
    index: u32,
    range: wal::FrameRange,
    summary: sfst::Summary,
    bytes: Arc<Vec<u8>>,
}

/// One capture: its sources, and the download-cache pins that keep captured
/// remote files in place. The pins MUST live until the engine has read the
/// sources — move them into the blocking closure that runs it.
pub(crate) struct Capture {
    pub(crate) sources: Vec<TraceSource>,
    pub(crate) pins: Vec<file_cache::CachedFile>,
}

/// Why a capture failed as a whole. Everything narrower degrades into the
/// captured sources instead (an unavailable remote file, a refused WAL).
#[derive(Debug)]
pub(crate) enum CaptureError {
    /// The query's remote files exceed the download cache's capacity;
    /// `at_least` is the footprint counted when planning stopped.
    TooLarge { at_least: u64, capacity: u64 },
    /// The download cache could not evict to make room: its directory is
    /// broken.
    EvictionFailed,
    /// The remote planning task panicked.
    Planning(tokio::task::JoinError),
}

pub(crate) struct TracesSourceSupplier {
    registries: Arc<RwLock<TenantRegistries>>,
    chunk_cache: Arc<ChunkCache>,
    min_entries: u64,
    /// Reading evicted files back; `None` when remote storage is disabled.
    remote: Option<RemoteRead>,
}

impl TracesSourceSupplier {
    pub(crate) fn new(
        registries: Arc<RwLock<TenantRegistries>>,
        chunk_cache: Arc<ChunkCache>,
        min_entries: u64,
        remote: Option<RemoteRead>,
    ) -> Self {
        Self {
            registries,
            chunk_cache,
            min_entries,
            remote,
        }
    }

    /// Capture one consistent snapshot of `tenant`'s sources overlapping
    /// `time_range` (unix seconds; window pruning is file-granular): the
    /// local snapshot and, with remote storage, which catalogs to read come
    /// from ONE registry read lock, so the query sees one `valid_up_to` per
    /// WAL.
    ///
    /// Remote files are planned first, so a query whose remote footprint
    /// exceeds the download cache fails ([`CaptureError::TooLarge`]) before
    /// any WAL work or download. The chunk-building phase can be the slow
    /// one, so it polls `cancel` between builds (the logs handler's
    /// discipline); a cancelled call returns no sources — the caller is
    /// about to discard the result anyway.
    ///
    /// Sets `progress`'s total before the downloads: one unit per source
    /// (the engine ticks once per source) plus one per planned download — an
    /// upper bound, as a cached file downloads nothing.
    pub(crate) async fn capture(
        &self,
        tenant: &TenantId,
        time_range: std::ops::Range<u32>,
        cancel: &CancellationToken,
        progress: &ProgressState,
    ) -> Result<Capture, CaptureError> {
        let cancelled = || Capture {
            sources: Vec::new(),
            pins: Vec::new(),
        };
        let query = file_registry::Query {
            time_range,
            partition_keys: Vec::new(),
        };
        let ((sealed, wal_descs), remote_input) = {
            let guard = self.registries.read().await;
            let snapshot = guard.query_snapshot(tenant, &query);
            let remote_input = self
                .remote
                .is_some()
                .then(|| guard.remote_plan_input(tenant, std::slice::from_ref(&query)));
            (snapshot, remote_input)
        };

        let RemotePlan {
            per_range,
            union,
            unreadable,
        } = match (&self.remote, remote_input) {
            (Some(remote), Some(input)) => {
                let capacity = remote.cache().capacity();
                tokio::task::spawn_blocking(move || input.plan_within(capacity))
                    .await
                    .map_err(CaptureError::Planning)?
                    .map_err(|too_large| CaptureError::TooLarge {
                        at_least: too_large.at_least,
                        capacity: too_large.capacity,
                    })?
            }
            _ => RemotePlan::default(),
        };
        let remote_entries = per_range.into_iter().next().unwrap_or_default();
        if cancel.is_cancelled() {
            return Ok(cancelled());
        }

        let mut resolved: HashMap<FileId, Result<ResolvedWal, String>> = HashMap::new();
        for wal in &wal_descs {
            let outcome = self.resolve_wal(wal, cancel).await;
            resolved.insert(wal.id, outcome);
        }
        if cancel.is_cancelled() {
            return Ok(cancelled());
        }

        let mut sources = local_sources(&sealed, &wal_descs, &resolved);
        // The unreadable catalogs the range overlaps: their entries are
        // unknown, so each is one unavailable source over its span.
        let catalogs: Vec<&CatalogFile> = unreadable
            .iter()
            .filter(|c| query.overlaps(c.min_timestamp_s, c.max_timestamp_s))
            .collect();
        progress.set_total(sources.len() + remote_entries.len() + catalogs.len() + union.len());

        let (downloaded, pins) = match &self.remote {
            Some(remote) if !union.is_empty() => {
                match remote.fetch(union, cancel, progress.done_counter()).await {
                    Ok(fetched) => (fetched.files, fetched.pins),
                    Err(CacheError::Cancelled) => return Ok(cancelled()),
                    Err(CacheError::TooLarge {
                        footprint,
                        capacity,
                    }) => {
                        return Err(CaptureError::TooLarge {
                            at_least: footprint,
                            capacity,
                        });
                    }
                    Err(CacheError::EvictionFailed) => return Err(CaptureError::EvictionFailed),
                }
            }
            _ => (Vec::new(), Vec::new()),
        };
        if cancel.is_cancelled() {
            return Ok(cancelled());
        }

        // Remote files are named like local sealed files (`<stem>.sfst`), so
        // one file has one name wherever its bytes come from.
        let downloaded: HashMap<FileId, &file_registry::SelectedFile> =
            downloaded.iter().map(|f| (f.id, f)).collect();
        for entry in remote_entries {
            let source_id = SourceId::new(entry.id.to_filename(SFST_EXT));
            sources.push(match downloaded.get(&entry.id) {
                Some(file) => TraceSource::Sfst(TraceSfstCandidate {
                    source_id,
                    summary: file.summary.clone(),
                    source: sfsq::Source::File(file.path.clone()),
                    coverage: None,
                }),
                None => TraceSource::Unavailable(TraceUnavailable {
                    source_id,
                    summary: entry.summary(),
                }),
            });
        }
        for catalog in catalogs {
            sources.push(TraceSource::Unavailable(TraceUnavailable {
                source_id: SourceId::new(catalog_source_id(catalog)),
                summary: sfst::Summary {
                    min_timestamp_s: catalog.min_timestamp_s,
                    max_timestamp_s: catalog.max_timestamp_s,
                    record_count: 0,
                    content_meta: Vec::new(),
                },
            }));
        }
        Ok(Capture { sources, pins })
    }

    /// Resolve one active WAL into chunk images + the tail range, or the
    /// reason to refuse the whole WAL (see the module docs). Polls `cancel`
    /// between chunk builds; a cancelled call returns an error too — the
    /// capture's result is discarded either way.
    async fn resolve_wal(
        &self,
        wal: &WalDesc,
        cancel: &CancellationToken,
    ) -> Result<ResolvedWal, String> {
        // Poll before the boundary scan too — it is a blocking file read
        // a cancelled call shouldn't pay for.
        if cancel.is_cancelled() {
            return Err("cancelled".to_string());
        }
        let header = wal::HEADER_SIZE as u64;
        let scan_path = wal.path.clone();
        let valid_up_to = wal.valid_up_to;
        let frames = match tokio::task::spawn_blocking(move || {
            wal::scan_frame_boundaries(&scan_path, wal::FrameRange::new(header, valid_up_to))
        })
        .await
        {
            Ok(Ok(frames)) => frames,
            Ok(Err(e)) => return Err(format!("WAL boundary scan failed: {e}")),
            Err(e) => return Err(format!("WAL boundary scan task failed: {e}")),
        };
        // The boundary scan is itself a blocking phase — poll on the way
        // out of it, then again before each chunk build.
        if cancel.is_cancelled() {
            return Err("cancelled".to_string());
        }

        let boundaries = chunk_boundaries(&frames, header, self.min_entries);
        let mut chunks = Vec::with_capacity(boundaries.len());
        for chunk in &boundaries {
            if cancel.is_cancelled() {
                return Err("cancelled".to_string());
            }
            let seq = wal.id.seq;
            let path = wal.path.clone();
            let (range, expected) = (chunk.range, chunk.entry_count);
            // The traces seal for the byte range; record-count
            // cross-check as in the logs path. Singleflighted through
            // the shared cache — (seq, index) keys never collide with
            // logs because seqs are process-global (shared highwater).
            let init = async move {
                match tokio::task::spawn_blocking(move || {
                    ng_index::build_sfst_traces_range(&path, range)
                })
                .await
                {
                    Ok(Ok((summary, bytes))) => {
                        if u64::from(summary.record_count) != expected {
                            Err(format!(
                                "chunk record count {} != expected {expected}",
                                summary.record_count
                            ))
                        } else {
                            Ok(Arc::new(bytes))
                        }
                    }
                    Ok(Err(e)) => Err(format!("build_sfst_traces_range: {e}")),
                    Err(e) => Err(format!("build task: {e}")),
                }
            };
            match self.chunk_cache.get_or_build(seq, chunk.index, init).await {
                Ok(bytes) => match sfst::IndexReader::open(&bytes[..]) {
                    Ok(reader) => chunks.push(ResolvedChunk {
                        index: chunk.index,
                        range: chunk.range,
                        summary: reader.summary().clone(),
                        bytes,
                    }),
                    Err(e) => {
                        return Err(format!("chunk {} parse failed: {e}", chunk.index));
                    }
                },
                Err(e) => return Err(format!("chunk {} build failed: {e}", chunk.index)),
            }
        }

        let tail_begin = tail_start(&boundaries, header);
        let tail = (tail_begin < wal.valid_up_to)
            .then(|| wal::FrameRange::new(tail_begin, wal.valid_up_to));
        Ok(ResolvedWal {
            path: wal.path.clone(),
            chunks,
            tail,
        })
    }
}

/// The range's local sources: its sealed files, then each resolved WAL's
/// chunks and tail; a refused WAL is one failed source.
fn local_sources(
    sealed: &[file_registry::SelectedFile],
    wal_descs: &[WalDesc],
    resolved: &HashMap<FileId, Result<ResolvedWal, String>>,
) -> Vec<TraceSource> {
    let mut sources: Vec<TraceSource> = Vec::new();
    for f in sealed {
        sources.push(TraceSource::Sfst(TraceSfstCandidate {
            source_id: SourceId::new(f.id.to_filename(SFST_EXT)),
            summary: f.summary.clone(),
            source: sfsq::Source::File(f.path.clone()),
            coverage: None,
        }));
    }
    for d in wal_descs {
        let wal_id: Arc<str> = d.id.to_filename(WAL_EXT).into();
        let w = match resolved.get(&d.id) {
            Some(Ok(w)) => w,
            Some(Err(error)) => {
                sources.push(TraceSource::Failed(TraceFailed {
                    source_id: SourceId::new(wal_id.to_string()),
                    error: error.clone(),
                }));
                continue;
            }
            None => continue,
        };
        for c in &w.chunks {
            sources.push(TraceSource::Sfst(TraceSfstCandidate {
                source_id: SourceId::new(format!("{wal_id}#chunk{:06}", c.index)),
                summary: c.summary.clone(),
                source: sfsq::Source::Memory(c.bytes.clone()),
                coverage: Some(WalCoverage {
                    wal_id: Arc::clone(&wal_id),
                    range: c.range,
                }),
            }));
        }
        if let Some(range) = w.tail {
            sources.push(TraceSource::Tail(TraceWalTail {
                source_id: SourceId::new(format!("{wal_id}#tail{}", range.start())),
                path: w.path.clone(),
                coverage: WalCoverage {
                    wal_id: Arc::clone(&wal_id),
                    range,
                },
            }));
        }
    }
    sources
}

/// An unreadable catalog's source id: its filename, which encodes its
/// identity, sequence and time span.
fn catalog_source_id(catalog: &CatalogFile) -> String {
    match catalog.path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => catalog.path.display().to_string(),
    }
}

#[cfg(test)]
mod tests;
