//! Which rows the agent's traces store holds, grouped the way the explorer
//! reads them: a sealed file, a chunk of the live WAL, or the WAL's tail.
//!
//! This is the one module allowed to read the store, and only its format
//! layer: a sealed file's time, id and duration columns, and a WAL's frames
//! decoded to the same four values per span. The grouping itself is written
//! here from the documented rule, not taken from the engine:
//!
//! - a WAL whose (machine, instance, seq) already has a sealed file is read
//!   from the sealed file only;
//! - a WAL's frames, in file order, add their entry counts to a running
//!   total; a chunk closes at the frame that brings the total to the minimum
//!   or more, and the count starts again; the frames after the last chunk are
//!   the tail;
//! - a unit's seconds are its rows' start times truncated to whole seconds.
//!
//! A WAL can only be read up to its end of file, while the engine reads only
//! what the agent has synced; the two agree once writes have settled, which
//! the caller ensures (a read error here means "not settled yet").

use std::collections::BTreeMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::model::OracleSpan;

/// The chunk minimum the lab agent runs with (the calculator's own copy).
pub const DEFAULT_CHUNK_ENTRIES: u32 = 16_384;

/// A stored row, as far as membership can tell: its ids (all zero when unset),
/// start and duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowKey {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub start_ns: i64,
    pub duration_ns: i64,
}

impl RowKey {
    pub fn of(span: &OracleSpan) -> RowKey {
        RowKey {
            trace_id: span.trace_id.unwrap_or_default(),
            span_id: span.span_id.unwrap_or_default(),
            start_ns: span.start_ns,
            duration_ns: span.duration_ns,
        }
    }
}

/// A store file name without its extension:
/// `<machine>-<instance>-<pipeline>-<seq>-<part key>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stem {
    pub machine: String,
    pub instance: String,
    pub pipeline: u16,
    pub seq: u64,
    pub part_key: u64,
}

impl Stem {
    pub fn parse(stem: &str) -> Option<Stem> {
        let parts: Vec<&str> = stem.split('-').collect();
        let [machine, instance, pipeline, seq, part_key] = parts.as_slice() else {
            return None;
        };
        let hex = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit());
        let digits = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
        if !hex(machine, 32) || !hex(instance, 32) || !digits(pipeline, 5) || !digits(seq, 10) {
            return None;
        }
        if !hex(part_key, 16) {
            return None;
        }
        Some(Stem {
            machine: machine.to_string(),
            instance: instance.to_string(),
            pipeline: pipeline.parse().ok()?,
            seq: seq.parse().ok()?,
            part_key: u64::from_str_radix(part_key, 16).ok()?,
        })
    }

    /// What a WAL and the file sealed from it share.
    pub fn seq_key(&self) -> (&str, &str, u64) {
        (&self.machine, &self.instance, self.seq)
    }
}

/// Frame ranges: a WAL's chunks, then its tail.
pub type Folded = (Vec<Range<usize>>, Option<Range<usize>>);

/// The chunks (frame ranges) and the tail of a WAL whose frames hold
/// `entry_counts`, for a chunk minimum of `min_entries`.
pub fn fold_chunks(entry_counts: &[u32], min_entries: u32) -> Folded {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut total: u64 = 0;
    for (index, count) in entry_counts.iter().enumerate() {
        total += u64::from(*count);
        if total >= u64::from(min_entries) && total > 0 {
            chunks.push(start..index + 1);
            start = index + 1;
            total = 0;
        }
    }
    let tail_from = start;
    let tail = (tail_from < entry_counts.len()).then_some(tail_from..entry_counts.len());
    (chunks, tail)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitKind {
    Sealed,
    Chunk(usize),
    /// The frames after the last chunk, from this frame index.
    Tail(usize),
}

/// One unit the explorer reads, with every row it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub path: PathBuf,
    pub stem: Stem,
    pub kind: UnitKind,
    /// Start seconds of the oldest and newest rows; `None` when it has none.
    pub seconds: Option<(u32, u32)>,
    pub rows: Vec<RowKey>,
}

impl Unit {
    /// A name for reports; tails are named by frame index, not by the
    /// engine's byte offset.
    pub fn name(&self) -> String {
        let file = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        match self.kind {
            UnitKind::Sealed => file.to_string(),
            UnitKind::Chunk(index) => format!("{file}#chunk{index:06}"),
            UnitKind::Tail(frame) => format!("{file}#tail@frame{frame}"),
        }
    }
}

fn seconds_of(rows: &[RowKey]) -> Option<(u32, u32)> {
    let min = rows.iter().map(|row| row.start_ns).min()?;
    let max = rows.iter().map(|row| row.start_ns).max()?;
    let second = |ns: i64| u32::try_from(ns / 1_000_000_000).unwrap_or(0);
    Some((second(min), second(max)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipError {
    /// A file name the store does not use.
    Name(PathBuf),
    /// A sealed file that cannot be read, or whose columns disagree with its
    /// record count; the engine reports such a file as a failed source.
    Sealed(PathBuf, String),
    /// A WAL that cannot be read to its end yet (still being written).
    NotSettled(PathBuf, String),
    /// A WAL frame whose entry count differs from its spans; the engine
    /// reports the WAL as a failed source.
    FrameCount {
        path: PathBuf,
        frame: usize,
        entries: u32,
        spans: usize,
    },
}

fn stem_of(path: &Path) -> Result<Stem, MembershipError> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(Stem::parse)
        .ok_or_else(|| MembershipError::Name(path.to_path_buf()))
}

/// Reads a sealed file's rows.
pub fn read_sealed(path: &Path) -> Result<Unit, MembershipError> {
    let stem = stem_of(path)?;
    let fail = |error: String| MembershipError::Sealed(path.to_path_buf(), error);
    let data = fs::read(path).map_err(|e| fail(e.to_string()))?;
    let reader = sfst::ChunkReader::open(&data).map_err(|e| fail(e.to_string()))?;
    let count = reader
        .summary()
        .map_err(|e| fail(e.to_string()))?
        .record_count as usize;
    let starts = reader.timestamps().map_err(|e| fail(e.to_string()))?;
    let trace_ids = reader.trace_ids().map_err(|e| fail(e.to_string()))?;
    let span_ids = reader.span_ids().map_err(|e| fail(e.to_string()))?;
    let durations = reader.durations().map_err(|e| fail(e.to_string()))?.0;

    let trace_ids: Vec<[u8; 16]> = trace_ids.iter().map(|id| *id.as_bytes()).collect();
    let span_ids: Vec<[u8; 8]> = span_ids.iter().map(|id| *id.as_bytes()).collect();
    let lengths = [
        starts.len(),
        trace_ids.len(),
        span_ids.len(),
        durations.len(),
    ];
    if lengths.iter().any(|len| *len != count) {
        return Err(fail(format!("columns {lengths:?} for {count} records")));
    }

    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        rows.push(RowKey {
            trace_id: trace_ids[i],
            span_id: span_ids[i],
            start_ns: starts[i],
            duration_ns: durations[i],
        });
    }
    Ok(Unit {
        path: path.to_path_buf(),
        stem,
        kind: UnitKind::Sealed,
        seconds: seconds_of(&rows),
        rows,
    })
}

/// What the runner watches of a WAL between two reads of the store: frames
/// added, a length that moved, or a first frame old enough that the agent is
/// about to seal the WAL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalInfo {
    pub path: PathBuf,
    /// Its length when read.
    pub bytes: u64,
    pub frames: usize,
    pub entries: u64,
    /// Ingestion times of its first and last frames, unix nanoseconds; `None`
    /// without frames.
    pub frame_times: Option<(u64, u64)>,
}

/// Reads a WAL to its end and splits its rows into chunks and a tail.
pub fn read_wal(path: &Path, min_entries: u32) -> Result<(WalInfo, Vec<Unit>), MembershipError> {
    let stem = stem_of(path)?;
    let unsettled = |error: String| MembershipError::NotSettled(path.to_path_buf(), error);
    let bytes = fs::metadata(path)
        .map_err(|e| unsettled(e.to_string()))?
        .len();
    let mut reader = wal::Reader::open(path).map_err(|e| unsettled(e.to_string()))?;

    let mut frames: Vec<Vec<RowKey>> = Vec::new();
    let mut entry_counts = Vec::new();
    let mut frame_times: Option<(u64, u64)> = None;
    while let Some(frame) = reader.next_frame().map_err(|e| unsettled(e.to_string()))? {
        let at = frame.timestamp_ns.0;
        frame_times = Some(match frame_times {
            Some((first, _)) => (first, at),
            None => (at, at),
        });
        let request =
            ng_flatten::decode_trace_frame(frame.data).map_err(|e| unsettled(e.to_string()))?;
        let mut rows = Vec::new();
        for resource in &request.resources {
            for scope in &resource.scopes {
                for span in &scope.spans {
                    rows.push(RowKey {
                        trace_id: *span.trace_id.as_bytes(),
                        span_id: *span.span_id.as_bytes(),
                        start_ns: span.ts,
                        duration_ns: span.duration,
                    });
                }
            }
        }
        if rows.len() != frame.entry_count as usize {
            return Err(MembershipError::FrameCount {
                path: path.to_path_buf(),
                frame: frames.len(),
                entries: frame.entry_count,
                spans: rows.len(),
            });
        }
        entry_counts.push(frame.entry_count);
        frames.push(rows);
    }

    let info = WalInfo {
        path: path.to_path_buf(),
        bytes,
        frames: frames.len(),
        entries: entry_counts.iter().map(|count| u64::from(*count)).sum(),
        frame_times,
    };
    let (chunks, tail) = fold_chunks(&entry_counts, min_entries);
    let unit = |kind: UnitKind, range: Range<usize>| {
        let rows: Vec<RowKey> = frames[range].iter().flatten().copied().collect();
        Unit {
            path: path.to_path_buf(),
            stem: stem.clone(),
            kind,
            seconds: seconds_of(&rows),
            rows,
        }
    };
    let mut units = Vec::new();
    for (index, range) in chunks.into_iter().enumerate() {
        units.push(unit(UnitKind::Chunk(index), range));
    }
    if let Some(range) = tail {
        units.push(unit(UnitKind::Tail(range.start), range));
    }
    Ok((info, units))
}

/// A WAL without a sealed file from an older agent instance than the newest
/// WAL: the engine skips it until it is sealed, so windows that overlap it
/// cannot be judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleWal {
    pub path: PathBuf,
    /// Start seconds of its oldest and newest rows; `None` when it has none,
    /// or cannot be read (then it overlaps every window).
    pub seconds: Option<(u32, u32)>,
    pub readable: bool,
}

impl StaleWal {
    fn read(path: PathBuf, min_entries: u32) -> StaleWal {
        match read_wal(&path, min_entries) {
            Ok((_, units)) => {
                let rows: Vec<RowKey> = units.into_iter().flat_map(|unit| unit.rows).collect();
                StaleWal {
                    seconds: seconds_of(&rows),
                    path,
                    readable: true,
                }
            }
            Err(_) => StaleWal {
                path,
                seconds: None,
                readable: false,
            },
        }
    }

    /// Whether it may hold rows of the window `[after_s, before_s)`.
    pub fn overlaps(&self, after_s: u32, before_s: u32) -> bool {
        !self.readable
            || self
                .seconds
                .is_some_and(|(min, max)| max >= after_s && min < before_s)
    }
}

/// Every unit of a traces store directory (`<run>/lib/otel/traces`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Membership {
    pub units: Vec<Unit>,
    /// The WALs read as units, in the order read.
    pub wals: Vec<WalInfo>,
    pub stale_wals: Vec<StaleWal>,
}

/// Sealed files already read, by path and length. A sealed file does not
/// change, so a run reads each once however often it reads the store; a file
/// no longer listed is dropped.
#[derive(Debug, Clone, Default)]
pub struct SealedCache {
    units: BTreeMap<PathBuf, (u64, Unit)>,
}

fn files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == extension) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Every WAL of a traces store directory with its length (one removed while
/// listing is left out): what the runner polls while writes settle.
pub fn wal_lengths(store: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    for path in files(&store.join("wal/default"), "wal") {
        if let Ok(meta) = fs::metadata(&path) {
            out.push((path, meta.len()));
        }
    }
    out
}

pub fn read_store(store: &Path, min_entries: u32) -> Result<Membership, MembershipError> {
    read_store_with(store, min_entries, &mut SealedCache::default())
}

/// [`read_store`], taking sealed files from `cache` when their length is the
/// one read before.
pub fn read_store_with(
    store: &Path,
    min_entries: u32,
    cache: &mut SealedCache,
) -> Result<Membership, MembershipError> {
    let sealed_paths = files(&store.join("index/default"), "sfst");
    let wal_paths = files(&store.join("wal/default"), "wal");

    cache.units.retain(|path, _| sealed_paths.contains(path));
    let mut out = Membership::default();
    let mut sealed_keys = Vec::new();
    for path in &sealed_paths {
        let bytes = fs::metadata(path)
            .map_err(|e| MembershipError::Sealed(path.clone(), e.to_string()))?
            .len();
        let unit = match cache.units.get(path) {
            Some((cached, unit)) if *cached == bytes => unit.clone(),
            _ => {
                let unit = read_sealed(path)?;
                cache.units.insert(path.clone(), (bytes, unit.clone()));
                unit
            }
        };
        sealed_keys.push(unit.stem.clone());
        out.units.push(unit);
    }

    let mut live = Vec::new();
    for path in wal_paths {
        let stem = stem_of(&path)?;
        let sealed = sealed_keys.iter().any(|s| s.seq_key() == stem.seq_key());
        if !sealed {
            let modified = fs::metadata(&path).and_then(|m| m.modified()).ok();
            live.push((modified, stem, path));
        }
    }
    let newest_instance = live
        .iter()
        .max_by_key(|(modified, _, _)| *modified)
        .map(|(_, stem, _)| stem.instance.clone());
    for (_, stem, path) in live {
        if newest_instance.as_deref() != Some(stem.instance.as_str()) {
            out.stale_wals.push(StaleWal::read(path, min_entries));
            continue;
        }
        let (info, units) = read_wal(&path, min_entries)?;
        out.wals.push(info);
        out.units.extend(units);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &str = "baf93e178ba34a37bdf778e397883163";
    const INSTANCE: &str = "d5439cb5a64e4ceb89c297cbcd8ee6f2";

    // A list holding one frame range is what these cases mean.
    #[allow(clippy::single_range_in_vec_init)]
    #[test]
    fn folds_frames_into_chunks_and_a_tail() {
        let cases: [(&str, &[u32], Folded); 8] = [
            (
                "two chunks, no tail",
                &[8, 8, 8, 8],
                (vec![0..2, 2..4], None),
            ),
            ("one chunk that overshoots", &[5, 3, 7], (vec![0..3], None)),
            ("a tail only", &[5, 3], (vec![], Some(0..2))),
            ("nothing", &[], (vec![], None)),
            ("exactly the minimum", &[10], (vec![0..1], None)),
            ("a chunk and a tail", &[9, 1, 1], (vec![0..2], Some(2..3))),
            (
                "empty frames never close a chunk",
                &[0, 10, 0, 0],
                (vec![0..2], Some(2..4)),
            ),
            (
                "empty frames alone stay the tail",
                &[0, 0],
                (vec![], Some(0..2)),
            ),
        ];
        for (name, counts, expected) in cases {
            assert_eq!(fold_chunks(counts, 10), expected, "{name}");
        }
    }

    #[test]
    fn a_longer_wal_only_adds_chunks() {
        let short = fold_chunks(&[8, 8, 8], 10);
        let long = fold_chunks(&[8, 8, 8, 8, 8], 10);
        assert_eq!(long.0[..short.0.len()], short.0[..]);
    }

    #[test]
    fn parses_store_names_and_pairs_a_wal_with_its_sealed_file() {
        let wal = Stem::parse(&format!(
            "{MACHINE}-{INSTANCE}-00001-0000000079-0000000000000000"
        ))
        .unwrap();
        let sealed = Stem::parse(&format!(
            "{MACHINE}-{INSTANCE}-00001-0000000079-00000000000000ff"
        ))
        .unwrap();
        let other = Stem::parse(&format!(
            "{MACHINE}-{INSTANCE}-00001-0000000080-0000000000000000"
        ))
        .unwrap();

        assert_eq!((wal.pipeline, wal.seq, sealed.part_key), (1, 79, 255));
        assert_eq!(wal.seq_key(), sealed.seq_key());
        assert_ne!(wal.seq_key(), other.seq_key());
        for bad in [
            "short",
            &format!("{MACHINE}-{INSTANCE}-1-0000000079-0000000000000000"),
            &format!("{MACHINE}-{INSTANCE}-00001-0000000079"),
            &format!("{MACHINE}-{INSTANCE}-00001-000000007x-0000000000000000"),
        ] {
            assert_eq!(Stem::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn keys_an_unset_id_as_zeros() {
        let mut span = crate::model::OracleSpan {
            trace_id: None,
            span_id: Some([2; 8]),
            parent_span_id: None,
            start_ns: 7,
            duration_ns: 3,
            fields: Default::default(),
            unit: 0,
        };
        assert_eq!(
            RowKey::of(&span),
            RowKey {
                trace_id: [0; 16],
                span_id: [2; 8],
                start_ns: 7,
                duration_ns: 3
            }
        );
        span.trace_id = Some([1; 16]);
        assert_eq!(RowKey::of(&span).trace_id, [1; 16]);
    }
}
