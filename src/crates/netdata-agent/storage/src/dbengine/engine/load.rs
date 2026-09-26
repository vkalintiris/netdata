//! A tier's startup (`init_data_files()` and `scan_data_files()` in `datafile.c`, `journalfile_load()` and
//! `journalfile_v2_load()` in `journalfile.c`): the directory scan, the per-file decisions with their file effects
//! (orphan journals and invalid pairs deleted, v2 indexes built for replayed journals, a new empty pair), the v1
//! replay into the metric registry, and C's records. Brief `knowledge/brief-dbengine-s2.md` §1.4 and §4.4 in the
//! status repository; decisions D29 and D62.
//!
//! Where C loads the journal of a pair whose data file it is about to delete (a defect that leaves retention and
//! pages of deleted data behind), the data file is checked first and a bad pair's journal is never loaded (D29).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error, netdata_log_info};
use netdata_agent_text::size::size_to_string;

use super::io::{
    IoFile, align_ceiling, align_floor, check_file_properties, open_for_io, unlink, write_block,
};
use super::mrg::Mrg;
use crate::dbengine::format::descriptor::PAGE_TYPE_GORILLA_32BIT;
use crate::dbengine::format::journal_v1::{self, Event, Replay};
use crate::dbengine::format::journal_v2::{
    self, Builder, HEADER_SIZE, Invalid, Page, Verdict, open_cache_pages, write_in_place,
};
use crate::dbengine::format::superblock::{self, SuperblockError};
use crate::dbengine::format::{BLOCK_SIZE, FileKind, ReadAt, file_name};

/// `MAX_DATAFILES`.
const MAX_DATAFILES: usize = 65536 * 4;
/// `MIN_DATAFILE_SIZE`, `MAX_DATAFILE_SIZE`, `TARGET_DATAFILES`.
const MIN_DATAFILE_SIZE: u64 = 512 * 1024;
const MAX_DATAFILE_SIZE: u64 = 1024 * 1024 * 1024;
const TARGET_DATAFILES: u64 = 100;
/// A tier-0 journal whose newest page is older than this is indexed even when it is the last one.
const OLD_DATA_S: i64 = 86400;

/// What a tier's startup needs to know.
#[derive(Debug, Clone)]
pub struct TierConfig {
    pub tier: usize,
    /// `ctx->config.dbfiles_path`.
    pub path: PathBuf,
    /// `dbengine_use_direct_io`.
    pub direct_io: bool,
    /// `ctx->config.max_disk_space`, 0 for none.
    pub max_disk_space: u64,
    /// `db_engine_journal_check`: v2 files are checked whole at load.
    pub journal_check: bool,
}

impl TierConfig {
    /// `rrdeng_target_data_file_size()`.
    pub fn target_datafile_size(&self) -> u64 {
        let target = if self.max_disk_space != 0 {
            self.max_disk_space / TARGET_DATAFILES
        } else {
            MAX_DATAFILE_SIZE
        };
        target.clamp(MIN_DATAFILE_SIZE, MAX_DATAFILE_SIZE)
    }

    fn file(&self, kind: FileKind, fileno: u32) -> PathBuf {
        self.path.join(file_name(kind, 1, fileno))
    }
}

/// A journal v2 file in use.
#[derive(Debug)]
pub struct V2File {
    pub file: File,
    pub size: u64,
}

/// A data file pair of the tier (`struct rrdengine_datafile` with its journal).
#[derive(Debug)]
pub struct DataFile {
    pub fileno: u32,
    pub file: IoFile,
    /// `datafile->pos`: the size, rounded up to a block.
    pub pos: u64,
    /// `journalfile->unsafe.pos`.
    pub journal_pos: u64,
    /// The v2 index when the file has one (loaded, or built at this start).
    pub v2: Option<V2File>,
}

/// A tier after its startup.
#[derive(Debug)]
pub struct Tier {
    pub config: TierConfig,
    /// In file number order.
    pub files: Vec<DataFile>,
    /// `ctx->atomic.last_fileno`.
    pub last_fileno: u32,
    /// The open cache: the hot pages of replayed journals not indexed (the last file's, when reused).
    pub open_pages: Vec<(u32, Page)>,
    /// `ctx->atomic.transaction_id`: the next transaction's id.
    pub transaction_id: u64,
    /// `ctx->atomic.first_time_s`: the tier's oldest time, `i64::MAX` until known.
    pub first_time_s: i64,
    /// The v2 files that serve, by file number (a file whose population failed is left out).
    pub indexes: BTreeMap<u32, super::v2index::V2Index>,
}

/// What a journal's load left (`journalfile_load()`'s effects).
struct Journal {
    pos: u64,
    v2: Option<V2File>,
    /// The replayed journal's open pages, when they stay in the open cache.
    open_pages: Vec<Page>,
    /// Whether the last file was not reused, so that a new pair follows.
    create_new_pair: bool,
}

/// `sscanf(name, "<prefix>%1u-%10u")`: both numbers convert (after optional white space, digits only within their
/// widths); what follows is not checked. The file number is truncated to 32 bits, as `%u` stores it.
fn scan_numbers(name: &str, prefix: &str) -> Option<(u32, u32)> {
    let rest = name.strip_prefix(prefix)?.trim_start();
    let tier = rest.chars().next().filter(char::is_ascii_digit)?;
    let rest = rest[1..].strip_prefix('-')?.trim_start();
    let digits: String = rest
        .chars()
        .take(10)
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let fileno = digits.parse::<u64>().ok()? as u32;
    Some((tier.to_digit(10)?, fileno))
}

/// What the scan found in the tier's directory.
struct Scan {
    /// Datafile numbers, sorted.
    datafiles: BTreeSet<u32>,
}

/// `scan_data_files()` up to the loading loop: every entry in name order (as libuv's scandir sorts them), datafiles by
/// C's lax name match (only tier digit 1; the same number once), journals by their exact names, the rest reported;
/// then journals without a data file are deleted, when there is a data file at all.
fn scan(cfg: &TierConfig) -> io::Result<Scan> {
    let entries = match std::fs::read_dir(&cfg.path) {
        Ok(entries) => entries,
        Err(err) => {
            netdata_log_error!(
                "DBENGINE: uv_fs_scandir({}): {}",
                cfg.path.display(),
                netdata_agent_log::uv_strerror(err.raw_os_error().unwrap_or(0))
            );
            return Err(err);
        }
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    netdata_log_info!(
        "DBENGINE: tier {}: found {} files in path {}",
        cfg.tier,
        names.len(),
        cfg.path.display()
    );
    let mut datafiles = BTreeSet::new();
    let mut journals = BTreeSet::new();
    for name in &names {
        if datafiles.len() >= MAX_DATAFILES {
            break;
        }
        // a tier digit other than 1 is a fatal_assert() in C; it is reported as unknown here (D29)
        if let Some((1, fileno)) = scan_numbers(name, "datafile-") {
            datafiles.insert(fileno);
            continue;
        }
        let journal = scan_numbers(name, "journalfile-").filter(|&(_, fileno)| {
            *name == file_name(FileKind::Journal, 1, fileno)
                || *name == file_name(FileKind::JournalV2, 1, fileno)
        });
        match journal {
            Some((_, fileno)) => {
                journals.insert(fileno);
            }
            None => nd_log!(
                Source::Daemon,
                Priority::Warning,
                "Unknown file detected : \"{}/{name}\"",
                cfg.path.display()
            ),
        }
    }
    if datafiles.is_empty() {
        return Ok(Scan { datafiles });
    }
    if datafiles.len() == MAX_DATAFILES {
        netdata_log_error!(
            "DBENGINE: warning: hit maximum database engine file limit of {MAX_DATAFILES} files"
        );
    }
    let mut deleted = 0usize;
    for &fileno in journals.difference(&datafiles) {
        for kind in [FileKind::Journal, FileKind::JournalV2] {
            let path = cfg.file(kind, fileno);
            if unlink(&path) {
                netdata_log_info!(
                    "DBENGINE: deleting journal file without matching data file: {}",
                    path.display()
                );
                deleted += 1;
            }
        }
    }
    if deleted > 0 {
        netdata_log_info!("DBENGINE: deleted {deleted} journal files without matching data files");
    }
    Ok(Scan { datafiles })
}

/// `load_data_file()`: the data file open (direct I/O as configured), a regular file of at least a superblock, with a
/// valid superblock; its `pos` is its size rounded up to a block.
fn load_data_file(cfg: &TierConfig, fileno: u32) -> Option<(IoFile, u64)> {
    let path = cfg.file(FileKind::Datafile, fileno);
    let file = open_for_io(&path, false, cfg.direct_io).ok()?;
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: initializing data file \"{}\".",
        path.display()
    );
    let size = check_file_properties(&file.file, BLOCK_SIZE as u64)?;
    let size = align_ceiling(size);
    let mut sb = [0u8; BLOCK_SIZE];
    if let Err(err) = file.read_exact_at(&mut sb, 0) {
        netdata_log_error!(
            "DBENGINE: uv_fs_read: {}",
            netdata_agent_log::uv_strerror(err.raw_os_error().unwrap_or(libc::EIO))
        );
        return None;
    }
    if superblock::check_datafile(&sb).is_err() {
        netdata_log_error!("DBENGINE: file has invalid superblock.");
        return None;
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: data file \"{}\" initialized (size:{size}).",
        path.display()
    );
    Some((file, size))
}

/// The record `journalfile_v2_validate()` leaves for a reason it refuses a file.
fn log_invalid(invalid: &Invalid) {
    match invalid {
        Invalid::TooShort | Invalid::Magic | Invalid::FileSize | Invalid::JournalSize => {}
        Invalid::HeaderCrc => netdata_log_error!("DBENGINE: file CRC32 check: FAILED"),
        Invalid::ExtentBounds => {
            netdata_log_error!("DBENGINE: extent list header offsets out of range")
        }
        Invalid::ExtentCrc => netdata_log_error!("DBENGINE: extent list CRC32 check: FAILED"),
        Invalid::MetricBounds => {
            netdata_log_error!("DBENGINE: metric list header offsets out of range")
        }
        Invalid::MetricCrc => netdata_log_error!("DBENGINE: metric list CRC32 check: FAILED"),
        Invalid::PageListOffset { index, offset } => netdata_log_info!(
            "DBENGINE: verification failed invalid page list header offset -- index {index} at offset {offset}"
        ),
        Invalid::PageListEntries {
            index,
            entries,
            offset,
        } => netdata_log_info!(
            "DBENGINE: verification failed invalid page list entries -- index {index} entries {entries} at offset \
             {offset}"
        ),
        Invalid::Unverified { total, verified } => netdata_log_info!(
            "DBENGINE: verification failed -- total entries {total}, verified {verified}"
        ),
    }
}

/// `journalfile_v2_load()`: the file's v2 index when it opens, validates (against the journal's size) and has
/// metrics; C's records otherwise. `None` without a record when there is no v2 file.
fn journal_v2_load(cfg: &TierConfig, fileno: u32) -> Option<V2File> {
    let v1_size =
        std::fs::metadata(cfg.file(FileKind::Journal, fileno)).map_or(0, |m| m.len() as u32);
    let path = cfg.file(FileKind::JournalV2, fileno);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => {
            nd_log!(Source::Daemon, Priority::Err, errno = err.raw_os_error().unwrap_or(0);
                "DBENGINE: failed to open \"{}\"", path.display());
            return None;
        }
    };
    let size = match file.metadata() {
        Ok(m) => m.len(),
        Err(_) => {
            netdata_log_error!(
                "DBENGINE: failed to get file information for \"{}\"",
                path.display()
            );
            return None;
        }
    };
    if size < HEADER_SIZE as u64 {
        netdata_log_error!("Invalid file \"{}\". Not the expected size", path.display());
        return None;
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: checking integrity of \"{}\"",
        path.display()
    );
    let started = Instant::now();
    let verdict = journal_v2::validate(&file, v1_size, cfg.journal_check);
    let reject = |what: &str| {
        netdata_log_error!("File \"{}\" {what}", path.display());
        None
    };
    match verdict {
        // a read failure is C's SIGBUS while validating
        Err(_) | Ok(Verdict::Rebuild) => reject("needs to be rebuilt"),
        Ok(Verdict::Skip) => reject("will be skipped"),
        Ok(Verdict::Invalid(invalid)) => {
            if cfg.journal_check
                && matches!(
                    invalid,
                    Invalid::PageListOffset { .. }
                        | Invalid::PageListEntries { .. }
                        | Invalid::Unverified { .. }
                )
            {
                log_checking_metrics(&file);
            }
            log_invalid(&invalid);
            reject("is invalid and it will be rebuilt")
        }
        Ok(Verdict::NoMetrics) => None,
        Ok(Verdict::Ok) => {
            let mut hb = [0u8; HEADER_SIZE];
            let metrics = match ReadAt::read_exact_at(&file, &mut hb, 0) {
                Ok(()) => journal_v2::Header::decode(&hb).metric_count,
                Err(_) => 0,
            };
            if cfg.journal_check {
                log_checking_metrics(&file);
            }
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "DBENGINE: journal v2 \"{}\" loaded, size: {:.2} MiB, metrics: {:.2} k, mmap: {:.2} ms, validate: \
                 {:.2} ms",
                path.display(),
                size as f64 / 1024.0 / 1024.0,
                f64::from(metrics) / 1000.0,
                0.0,
                started.elapsed().as_secs_f64() * 1000.0
            );
            Some(V2File { file, size })
        }
    }
}

/// The integrity check's first record: how many metrics it checks.
fn log_checking_metrics(file: &File) {
    let mut hb = [0u8; HEADER_SIZE];
    if ReadAt::read_exact_at(file, &mut hb, 0).is_ok() {
        netdata_log_info!(
            "DBENGINE: checking {} metrics that exist in the journal",
            journal_v2::Header::decode(&hb).metric_count
        );
    }
}

/// The records of `journalfile_iterate_transactions()`: the replay's refusals, and each unknown page type once per
/// process (`page_error_map`), in the order C meets them.
fn log_replay(replay: &Replay) {
    use std::sync::Mutex;
    static UNKNOWN_TYPES: Mutex<[bool; 256]> = Mutex::new([false; 256]);
    for event in &replay.events {
        match event {
            Event::Corrupt { .. } => {
                netdata_log_error!("DBENGINE: corrupted transaction record, skipping.")
            }
            Event::CrcFailed { id, .. } => netdata_log_error!(
                "DBENGINE: transaction {id} was read from disk. CRC32 check: FAILED"
            ),
            Event::UnknownType { .. } => {
                netdata_log_error!("DBENGINE: unknown transaction type, skipping record.")
            }
            Event::CorruptPayload { .. } => {
                netdata_log_error!("DBENGINE: corrupted transaction payload.")
            }
            Event::StoreData { data, .. } => {
                for d in &data.descriptors {
                    if d.page_type > PAGE_TYPE_GORILLA_32BIT {
                        let mut seen = UNKNOWN_TYPES.lock().unwrap_or_else(|e| e.into_inner());
                        if !std::mem::replace(&mut seen[usize::from(d.page_type)], true) {
                            netdata_log_error!(
                                "DBENGINE: unknown page type {} encountered.",
                                d.page_type
                            );
                        }
                    }
                }
            }
        }
    }
}

/// `journalfile_migrate_to_v2_callback()` at startup: the replayed pages indexed and written in place, with C's
/// records; `None` when there are no metrics (nothing is written) or the write failed (the file is removed and
/// skipped).
fn build_v2(cfg: &TierConfig, fileno: u32, journal_pos: u64, pages: &[Page]) -> Option<V2File> {
    let mut b = Builder::new(journal_pos as u32);
    let (mut metrics, mut extents) = (BTreeSet::new(), BTreeSet::new());
    for p in pages {
        metrics.insert(p.uuid);
        extents.insert(p.block);
        b.page(*p);
    }
    if metrics.is_empty() {
        return None;
    }
    netdata_log_info!(
        "DBENGINE: tier {}: indexing {}: extents {}, metrics {}, pages {}",
        cfg.tier,
        file_name(FileKind::JournalV2, 1, fileno),
        extents.len(),
        metrics.len(),
        pages.len()
    );
    let path = cfg.file(FileKind::JournalV2, fileno);
    let image = b.build()?;
    let written = write_in_place(&path, &image).and_then(|()| File::open(&path));
    match written {
        Ok(file) => {
            netdata_log_info!(
                "DBENGINE: tier {}: migrated {}, {}",
                cfg.tier,
                file_name(FileKind::JournalV2, 1, fileno),
                size_to_string(image.len() as u64, "B", false).unwrap_or_default()
            );
            Some(V2File {
                file,
                size: image.len() as u64,
            })
        }
        Err(_) => {
            netdata_log_info!(
                "DBENGINE: failed to build index \"{}\", file will be skipped",
                path.display()
            );
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// `journalfile_load()`: the v2 index of a file that is not the last one, else the v1 journal replayed into the
/// registry (`now_s` + 1 is the newest acceptable time); a replayed journal gets its v2 built, except the last file
/// when it is small and (on tier 0) recent, whose pages stay open. `None` makes the pair invalid.
fn journal_load(
    cfg: &TierConfig,
    fileno: u32,
    last_fileno: u32,
    datafile_pos: u64,
    mrg: &Mrg,
    now_s: i64,
    transaction_id: &mut u64,
) -> Option<Journal> {
    let v2 = if fileno != last_fileno {
        journal_v2_load(cfg, fileno)
    } else {
        None
    };
    let path = cfg.file(FileKind::Journal, fileno);
    let Ok(file) = open_for_io(&path, false, cfg.direct_io) else {
        return v2.map(|v2| Journal {
            pos: 0,
            v2: Some(v2),
            open_pages: Vec::new(),
            create_new_pair: false,
        });
    };
    let size = check_file_properties(&file.file, BLOCK_SIZE as u64)?;
    if v2.is_some() {
        return Some(Journal {
            pos: size,
            v2,
            open_pages: Vec::new(),
            create_new_pair: false,
        });
    }
    let size = align_floor(size);
    let mut sb = [0u8; BLOCK_SIZE];
    let sb_ok = match file.read_exact_at(&mut sb, 0) {
        Ok(()) => match superblock::check_journal(&sb) {
            Ok(()) => true,
            Err(SuperblockError::Magic | SuperblockError::Version | SuperblockError::Tier) => {
                netdata_log_error!("DBENGINE: File has invalid superblock.");
                false
            }
            Err(SuperblockError::TooShort) => false,
        },
        Err(err) => {
            netdata_log_error!(
                "DBENGINE: uv_fs_read: {}",
                netdata_agent_log::uv_strerror(err.raw_os_error().unwrap_or(libc::EIO))
            );
            false
        }
    };
    if !sb_ok {
        netdata_log_info!(
            "DBENGINE: invalid journal file \"{}\" ; superblock check failed.",
            path.display()
        );
        return None;
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: loading journal file \"{}\"",
        path.display()
    );
    let replay = journal_v1::replay(&file, size).unwrap_or(Replay {
        events: Vec::new(),
        max_id: 1,
        read_error: true,
    });
    log_replay(&replay);
    let open = open_cache_pages(&replay, now_s + 1, &mut mrg.tier(cfg.tier));
    *transaction_id = (*transaction_id).max(replay.max_id + 1);
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: journal file \"{}\" loaded (size:{size}).",
        path.display()
    );
    let is_last = fileno == last_fileno;
    let has_old_data =
        cfg.tier == 0 && open.last_time_s > 0 && now_s - open.last_time_s > OLD_DATA_S;
    if is_last && datafile_pos <= cfg.target_datafile_size() / 3 && !has_old_data {
        return Some(Journal {
            pos: size,
            v2: None,
            open_pages: open.pages,
            create_new_pair: false,
        });
    }
    let v2 = build_v2(cfg, fileno, size, &open.pages);
    // pages a failed or empty build did not index stay open
    let open_pages = if v2.is_some() { Vec::new() } else { open.pages };
    Some(Journal {
        pos: size,
        v2,
        open_pages,
        create_new_pair: is_last,
    })
}

/// `create_data_file()` or `journalfile_create()`: the file created with its superblock (a data file's `tier` byte
/// is 1); its size.
fn create_file(
    path: &Path,
    direct: bool,
    superblock: &[u8; BLOCK_SIZE],
    what: &str,
) -> Option<IoFile> {
    let file = open_for_io(path, true, direct).ok()?;
    if write_block(&file, superblock, 0).is_err() {
        netdata_log_error!("DBENGINE: Failed to create {what} {}", path.display());
        let _ = std::fs::remove_file(path);
        return None;
    }
    Some(file)
}

/// `create_new_datafile_pair()`: the next file number's data file and journal, with C's record.
fn create_new_pair(cfg: &TierConfig, tier: &mut Tier) -> bool {
    let fileno = tier.last_fileno + 1;
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: creating new data and journal files in path \"{}\"",
        cfg.path.display()
    );
    let data_path = cfg.file(FileKind::Datafile, fileno);
    let Some(file) = create_file(
        &data_path,
        cfg.direct_io,
        &superblock::encode_datafile(),
        "datafile",
    ) else {
        return false;
    };
    let journal_path = cfg.file(FileKind::Journal, fileno);
    if create_file(
        &journal_path,
        cfg.direct_io,
        &superblock::encode_journal(),
        "journal file",
    )
    .is_none()
    {
        let _ = std::fs::remove_file(&data_path);
        return false;
    }
    netdata_log_info!(
        "DBENGINE: tier {}: created {} (.ndf, .njf).",
        cfg.tier,
        file_name(FileKind::Datafile, 1, fileno).trim_end_matches(".ndf")
    );
    tier.files.push(DataFile {
        fileno,
        file,
        pos: BLOCK_SIZE as u64,
        journal_pos: BLOCK_SIZE as u64,
        v2: None,
    });
    tier.last_fileno = fileno;
    true
}

/// `init_data_files()`: the tier's files scanned and loaded, invalid pairs deleted, and a new pair created when the
/// directory had none or the last file was not reused. `now_s` is the wall clock the decisions use.
pub fn load(cfg: TierConfig, mrg: &Mrg, now_s: i64) -> io::Result<Tier> {
    let scanned = match scan(&cfg) {
        Ok(scan) => scan,
        Err(err) => {
            netdata_log_error!("DBENGINE: failed to scan path \"{}\".", cfg.path.display());
            return Err(err);
        }
    };
    let mut tier = Tier {
        files: Vec::new(),
        last_fileno: scanned.datafiles.last().copied().unwrap_or(0),
        open_pages: Vec::new(),
        transaction_id: 1,
        first_time_s: i64::MAX,
        indexes: BTreeMap::new(),
        config: cfg.clone(),
    };
    let mut create = false;
    if !scanned.datafiles.is_empty() {
        netdata_log_info!(
            "DBENGINE: tier {}: loading {} data/journal files...",
            cfg.tier,
            scanned.datafiles.len()
        );
    }
    let mut loaded: BTreeMap<u32, DataFile> = BTreeMap::new();
    for &fileno in &scanned.datafiles {
        let data = load_data_file(&cfg, fileno);
        let journal = data.as_ref().and_then(|&(_, pos)| {
            journal_load(
                &cfg,
                fileno,
                tier.last_fileno,
                pos,
                mrg,
                now_s,
                &mut tier.transaction_id,
            )
        });
        match (data, journal) {
            (Some((file, pos)), Some(journal)) => {
                create |= journal.create_new_pair;
                tier.open_pages
                    .extend(journal.open_pages.into_iter().map(|p| (fileno, p)));
                loaded.insert(
                    fileno,
                    DataFile {
                        fileno,
                        file,
                        pos,
                        journal_pos: journal.pos,
                        v2: journal.v2,
                    },
                );
            }
            _ => {
                netdata_log_error!("DBENGINE: deleting invalid data and journal file pair.");
                let journal = cfg.file(FileKind::Journal, fileno);
                if unlink(&journal) {
                    netdata_log_info!("DBENGINE: deleted journal file \"{}\".", journal.display());
                }
                let data = cfg.file(FileKind::Datafile, fileno);
                if unlink(&data) {
                    netdata_log_info!("DBENGINE: deleted data file \"{}\".", data.display());
                }
            }
        }
    }
    tier.files = loaded.into_values().collect();
    // scan_data_files() counts what loaded: no pair left is as no pair found
    if tier.files.is_empty() {
        netdata_log_info!(
            "DBENGINE: data files not found, creating in path \"{}\".",
            cfg.path.display()
        );
        tier.last_fileno = 0;
        if !create_new_pair(&cfg, &mut tier) {
            netdata_log_error!(
                "DBENGINE: failed to create data and journal files in path \"{}\".",
                cfg.path.display()
            );
            return Err(io::Error::other("no data files"));
        }
    } else if create {
        create_new_pair(&cfg, &mut tier);
    }
    Ok(tier)
}

#[cfg(test)]
mod tests;
