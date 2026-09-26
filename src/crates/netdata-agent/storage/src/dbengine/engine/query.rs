//! The dbengine's read path (`rrdeng_load_metric_init()` and friends in `rrdengineapi.c`, `get_page_list()`,
//! `list_has_time_gaps()` and `pg_cache_lookup_next()` in `pagecache.c`, the extent loads of `pdc.c`): a query's page
//! list from the main cache, the open pages and the v2 indexes, the gaps inside the metric's retention cached as empty
//! pages, the pages to read loaded from their extents, then the points one by one. Brief
//! `knowledge/brief-dbengine-s2.md` §1.8 and §4.6 in the status repository; decisions D62.5 and D62.7.
//!
//! The preparation (the page list and the extent reads) runs as one job on the `UV_WORKER` pool, which the query
//! waits for at its first point (C queues it for NORMAL priority and blocks at the first lookup).

use std::collections::{BTreeMap, HashMap, btree_map};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{
    ErrorLimit, Priority as LogPriority, Source, nd_log_limit, netdata_log_error,
};

use super::cache::{CachedPage, ExtentCache, MainCache, Search};
use super::io::IoFile;
use super::load::Tier;
use super::mrg::{Handle, Mrg};
use super::v2index::{PageListError, V2Index};
use crate::dbengine::format::descriptor::{
    PAGE_TYPE_ARRAY_32BIT, PAGE_TYPE_ARRAY_TIER1, PAGE_TYPE_GORILLA_32BIT, PageDescriptor,
    log_validation, uuid_text, validate_extent_page_descr,
};
use crate::dbengine::format::extent::{self, COMPRESSION_NONE, PageSlot};
use crate::dbengine::format::page::{DiskPage, EmptyPage};
use crate::dbengine::format::{BLOCK_SIZE, ReadAt};
use crate::storage_point::StoragePoint;

/// `STORAGE_PRIORITY`: how a query is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    High,
    Normal,
    Low,
    BestEffort,
    /// Prepared on the caller's thread.
    Synchronous,
}

/// A page the open cache holds: a replayed journal's page not indexed yet.
#[derive(Debug, Clone, Copy)]
struct OpenPage {
    end_time_s: i64,
    update_every_s: u32,
    fileno: u32,
    block: u64,
    bytes: u32,
}

/// A tier as queries read it.
#[derive(Debug)]
pub struct TierData {
    pub tier: usize,
    /// The data files, for extent reads.
    datafiles: HashMap<u32, IoFile>,
    /// `njfv2idx`: the serving v2 files by their end, then number.
    v2: BTreeMap<(i64, u32), V2Index>,
    open: HashMap<[u8; 16], BTreeMap<i64, OpenPage>>,
    /// `ctx->atomic.first_time_s`.
    pub first_time_s: i64,
    /// `ctx->quiesce.enabled`: the tier is shutting down, new queries get no preparation.
    quiesced: AtomicBool,
    /// `ctx->atomic.inflight_queries`: queries holding a preparation, which the tier's shutdown waits for.
    inflight: Arc<AtomicUsize>,
}

impl TierData {
    /// A tier after its startup and population.
    pub fn new(tier: Tier) -> TierData {
        let mut open: HashMap<[u8; 16], BTreeMap<i64, OpenPage>> = HashMap::new();
        for (fileno, p) in tier.open_pages {
            let page = OpenPage {
                end_time_s: p.end_time_s,
                update_every_s: p.update_every_s,
                fileno,
                block: p.block,
                bytes: p.extent_bytes,
            };
            // `pgc_open_add_hot_page()`: a page of another journal at the same start replaces the held one only if
            // it ends later
            match open.entry(p.uuid).or_default().entry(p.start_time_s) {
                btree_map::Entry::Vacant(v) => {
                    v.insert(page);
                }
                btree_map::Entry::Occupied(mut o) if page.end_time_s > o.get().end_time_s => {
                    o.insert(page);
                }
                btree_map::Entry::Occupied(_) => {}
            }
        }
        TierData {
            tier: tier.config.tier,
            datafiles: tier.files.into_iter().map(|f| (f.fileno, f.file)).collect(),
            v2: tier
                .indexes
                .into_values()
                .map(|index| ((index.end_time_s(), index.fileno), index))
                .collect(),
            open,
            first_time_s: tier.first_time_s,
            quiesced: AtomicBool::new(false),
            inflight: Arc::default(),
        }
    }

    /// `RRDENG_OPCODE_CTX_QUIESCE`: queries started from now on read nothing.
    pub fn quiesce(&self) {
        self.quiesced.store(true, Ordering::Release);
    }

    pub fn quiesced(&self) -> bool {
        self.quiesced.load(Ordering::Acquire)
    }

    /// The queries in flight.
    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::Acquire)
    }
}

/// A query's count in its tier's in-flight queries, held by the query and by its preparation job (C's pdc
/// reference count) and released with the last of them.
#[derive(Debug)]
struct Inflight(Arc<AtomicUsize>);

impl Inflight {
    fn new(count: &Arc<AtomicUsize>) -> Arc<Inflight> {
        count.fetch_add(1, Ordering::AcqRel);
        Arc::new(Inflight(Arc::clone(count)))
    }
}

impl Drop for Inflight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The engine's shared state for queries.
#[derive(Debug)]
pub struct Dbengine {
    pub mrg: Mrg,
    pub tiers: Vec<TierData>,
    pub main: MainCache,
    pub extents: ExtentCache,
    /// Where preparations run; without one they run on the caller's thread.
    pub pool: Option<WorkPool>,
    /// `nd_profile.update_every`.
    pub update_every_s: u32,
}

// `PDC_PAGE_*`.
const READY: u32 = 1 << 0;
const FAILED: u32 = 1 << 1;
const SKIP: u32 = 1 << 2;
const INVALID: u32 = 1 << 3;
const EMPTY: u32 = 1 << 4;
const PREPROCESSED: u32 = 1 << 5;
const PROCESSED: u32 = 1 << 6;
const RELEASED: u32 = 1 << 7;
const PRELOADED: u32 = 1 << 8;
const DISK_PENDING: u32 = 1 << 9;
const DATAFILE_ACQUIRED: u32 = 1 << 30;
/// `PDC_PAGE_QUERY_GLOBAL_SKIP_LIST`.
const GLOBAL_SKIP: u32 = FAILED | SKIP | INVALID | RELEASED;

/// `struct page_details`.
#[derive(Debug, Clone)]
struct Pd {
    first_time_s: i64,
    last_time_s: i64,
    update_every_s: u32,
    status: u32,
    page: Option<Arc<CachedPage>>,
    /// Where the page's extent is: data file, block and size.
    extent: Option<(u32, u64, u32)>,
}

/// `TIME_RANGE_COMPARE` of `is_page_in_time_range()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Range {
    Past,
    In,
    Future,
}

fn in_range(first_s: i64, last_s: i64, start_s: i64, end_s: i64) -> Range {
    if last_s < start_s {
        Range::Past
    } else if first_s > end_s {
        Range::Future
    } else {
        Range::In
    }
}

/// `pdc_find_page_for_time()`: the page for a time among the list's pages not marked `mode` or skipped, counting a
/// gap when the time is not inside the page returned.
fn find_page_for_time(
    list: &BTreeMap<i64, Pd>,
    t: i64,
    gaps: &mut usize,
    mode: u32,
    skip: u32,
) -> Option<i64> {
    let ignore = GLOBAL_SKIP | skip;
    let f = list
        .range(t..)
        .find(|(_, pd)| pd.status & (ignore | mode) == 0)
        .map(|(k, _)| *k);
    let mut l = None;
    for (k, pd) in list.range(..=t).rev() {
        if pd.status & mode != 0 {
            break;
        }
        if pd.status & ignore == 0 {
            l = Some(*k);
            break;
        }
    }
    let range = |k: Option<i64>, missing| {
        k.map_or(missing, |k| {
            let pd = &list[&k];
            in_range(pd.first_time_s, pd.last_time_s, t, t)
        })
    };
    let (rf, rl) = (range(f, Range::Future), range(l, Range::Past));
    if f.is_none() || f == l {
        *gaps += usize::from(rl != Range::In);
        return l;
    }
    let (Some(fk), Some(lk)) = (f, l) else {
        *gaps += usize::from(rf != Range::In);
        return f;
    };
    let (pf, pl) = (&list[&fk], &list[&lk]);
    if rf == rl {
        return match rf {
            Range::In => {
                // the finer resolution, else the one starting earlier
                if pf.update_every_s != 0 && pf.update_every_s < pl.update_every_s {
                    Some(fk)
                } else if (pl.update_every_s != 0 && pl.update_every_s < pf.update_every_s)
                    || pl.first_time_s < pf.first_time_s
                {
                    Some(lk)
                } else {
                    Some(fk)
                }
            }
            Range::Future => {
                *gaps += 1;
                Some(if pl.first_time_s < pf.first_time_s {
                    lk
                } else {
                    fk
                })
            }
            Range::Past => {
                *gaps += 1;
                None
            }
        };
    }
    if rf == Range::In {
        return Some(fk);
    }
    if rl == Range::In {
        return Some(lk);
    }
    *gaps += 1;
    if rf == Range::Future {
        return Some(fk);
    }
    if rl == Range::Future {
        return Some(lk);
    }
    None
}

/// `pgc_page_get_and_acquire()` over a metric's open pages.
fn search_open(pages: &BTreeMap<i64, OpenPage>, t: i64, mode: Search) -> Option<(i64, OpenPage)> {
    let next = || pages.range(t + 1..).next().map(|(k, p)| (*k, *p));
    match mode {
        Search::Exact => pages.get(&t).map(|p| (t, *p)),
        Search::Next => next(),
        Search::Closest => pages
            .get(&t)
            .map(|p| (t, *p))
            .or_else(|| {
                pages
                    .range(..t)
                    .next_back()
                    .filter(|(_, p)| t <= p.end_time_s)
                    .map(|(k, p)| (*k, *p))
            })
            .or_else(next),
    }
}

/// Global once-a-second limits of the read path's records (`nd_log_limit_static_global_var(erl, 1, 0)`).
static EXTENT_ERRORS: ErrorLimit = ErrorLimit::new(1, 0);
static EXTENT_SIZE: ErrorLimit = ErrorLimit::new(1, 0);

thread_local! {
    /// `nd_log_limit_static_thread_var(erl, 60, 0)` of each v2 structure error: the page list header, the page list,
    /// the extent index.
    static V2_HEADER_ERRORS: ErrorLimit = const { ErrorLimit::new(60, 0) };
    static V2_LIST_ERRORS: ErrorLimit = const { ErrorLimit::new(60, 0) };
    static V2_EXTENT_ERRORS: ErrorLimit = const { ErrorLimit::new(60, 0) };
}

/// `log_date()`: local time as `%Y-%m-%d %H:%M:%S`, empty for time 0.
fn log_date(t: i64) -> String {
    if t == 0 {
        return String::new();
    }
    netdata_agent_sys::localtime(t).map_or_else(String::new, |tm| {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.year,
            tm.month0 + 1,
            tm.mday,
            tm.hour,
            tm.min,
            tm.sec
        )
    })
}

/// What an extent error record names (`epdl_extent_loading_error_log()`).
enum Subject<'a> {
    /// The first page the extent was read for.
    Pd(&'a [u8; 16], i64, i64),
    /// The descriptor of a page in the extent.
    Descr(&'a PageDescriptor),
}

fn extent_error(
    tier: usize,
    fileno: u32,
    block: u64,
    bytes: u32,
    subject: Subject<'_>,
    msg: &str,
    priority: LogPriority,
) {
    let (what, uuid, start, end) = match subject {
        Subject::Pd(uuid, start, end) => ("to extract page (PD)", uuid, start, end),
        Subject::Descr(d) => {
            let start = (d.start_time_ut / 1_000_000) as i64;
            let end = match d.page_type {
                PAGE_TYPE_ARRAY_32BIT | PAGE_TYPE_ARRAY_TIER1 => {
                    (d.end_time_ut() / 1_000_000) as i64
                }
                PAGE_TYPE_GORILLA_32BIT => start + i64::from(d.gorilla_delta_s()),
                _ => 0,
            };
            ("expected page (DESCR)", &d.uuid, start, end)
        }
    };
    nd_log_limit!(
        &EXTENT_ERRORS,
        Source::Daemon,
        priority,
        "DBENGINE: error while reading extent from datafile {fileno} of tier {tier}, at offset {} ({bytes} bytes) {what} \
         from {start} ({}) to {end} ({})  of metric {}: {msg}",
        block * BLOCK_SIZE as u64,
        log_date(start),
        log_date(end),
        uuid_text(uuid)
    );
}

/// A query's page list and what the preparation found (`struct page_details_control`).
#[derive(Debug, Default)]
struct Prep {
    list: BTreeMap<i64, Pd>,
    optimal_end_time_s: i64,
}

impl Dbengine {
    /// `mrg_metric_get_update_every_s()` or the profile's.
    fn metric_dt(&self, metric: &Handle) -> i64 {
        match metric.update_every_s() {
            0 => i64::from(self.update_every_s),
            ue => i64::from(ue),
        }
    }

    /// `get_page_list_from_pgc()` over the main cache (`open` false) or the open pages: the pages from the one at the
    /// start on, added to the list unless it holds their start; gaps between them counted. How many were added.
    fn pages_from_cache(
        &self,
        metric: &Handle,
        start_s: i64,
        end_s: i64,
        list: &mut BTreeMap<i64, Pd>,
        cache_gaps: &mut usize,
        open: bool,
    ) -> usize {
        let tier = metric.tier();
        let data = &self.tiers[tier];
        let open_pages = data.open.get(metric.uuid());
        let mut found = 0;
        let mut now_s = start_s;
        let mut dt_s = self.metric_dt(metric);
        let mut previous_end_s = now_s - dt_s;
        let mut mode = Search::Closest;
        loop {
            let page = if open {
                open_pages
                    .and_then(|p| search_open(p, now_s, mode))
                    .map(|(start, p)| (start, p.end_time_s, p.update_every_s, None, Some(p)))
            } else {
                self.main.search(tier, metric.uuid(), now_s, mode).map(|p| {
                    (
                        p.start_time_s,
                        p.end_time_s(),
                        p.update_every_s(),
                        Some(p),
                        None,
                    )
                })
            };
            mode = Search::Next;
            let Some((page_start, page_end, ue, cached, open_page)) = page else {
                if previous_end_s < end_s {
                    *cache_gaps += 1;
                }
                break;
            };
            let ue = if ue == 0 { dt_s as u32 } else { ue };
            if in_range(page_start, page_end, start_s, end_s) != Range::In {
                if previous_end_s < end_s {
                    *cache_gaps += 1;
                }
                break;
            }
            if page_start - previous_end_s > dt_s {
                *cache_gaps += 1;
            }
            if let std::collections::btree_map::Entry::Vacant(v) = list.entry(page_start) {
                let mut pd = Pd {
                    first_time_s: page_start,
                    last_time_s: page_end,
                    update_every_s: ue,
                    status: 0,
                    page: None,
                    extent: None,
                };
                if let Some(page) = cached {
                    pd.status |= READY | PRELOADED;
                    if page.is_empty() {
                        pd.status |= EMPTY;
                    }
                    pd.page = Some(page);
                }
                if let Some(p) = open_page {
                    if data.datafiles.contains_key(&p.fileno) {
                        pd.extent = Some((p.fileno, p.block, p.bytes));
                        pd.status |= DATAFILE_ACQUIRED | DISK_PENDING;
                    } else {
                        pd.status |= FAILED;
                    }
                }
                v.insert(pd);
                found += 1;
            }
            previous_end_s = page_end;
            if ue > 0 {
                dt_s = i64::from(ue);
            }
            now_s = page_start;
            if now_s > end_s {
                break;
            }
        }
        found
    }

    /// `get_page_list_from_journal_v2()`: the metric's pages in the v2 files overlapping the window, in the files'
    /// order, each added unless the list holds its start. How many were found.
    fn pages_from_v2(
        &self,
        metric: &Handle,
        start_s: i64,
        end_s: i64,
        list: &mut BTreeMap<i64, Pd>,
    ) -> usize {
        let data = &self.tiers[metric.tier()];
        let mut found = 0;
        // JudyLPrev() of the start: the file ending last before it, then every later one
        let first = data.v2.range(..(start_s, 0)).next_back().map(|(k, _)| *k);
        let files = match first {
            Some(k) => data.v2.range(k..),
            None => data.v2.range(..),
        };
        for index in files.map(|(_, v)| v) {
            match in_range(index.start_time_s(), index.end_time_s(), start_s, end_s) {
                Range::Past => continue,
                Range::Future => break,
                Range::In => {}
            }
            let pages = match index.find(metric.uuid()) {
                Ok(Some(entry)) => index.pages(&entry),
                Ok(None) | Err(_) => continue,
            };
            let pages = match pages {
                Ok(pages) => pages,
                Err(err) => {
                    let limit = match err {
                        PageListError::Header => &V2_HEADER_ERRORS,
                        PageListError::List => &V2_LIST_ERRORS,
                    };
                    limit.with(|erl| {
                        nd_log_limit!(
                            erl,
                            Source::Daemon,
                            LogPriority::Err,
                            "{}",
                            err.record(index.fileno, data.tier)
                        );
                    });
                    continue;
                }
            };
            if !data.datafiles.contains_key(&index.fileno) {
                continue;
            }
            let base = index.start_time_s();
            for p in pages {
                let first_s = base + i64::from(p.delta_start_s);
                let last_s = base + i64::from(p.delta_end_s);
                match in_range(first_s, last_s, start_s, end_s) {
                    Range::Past => continue,
                    Range::Future => break,
                    Range::In => {}
                }
                let Some(extent) = index.extents.get(p.extent_index as usize) else {
                    V2_EXTENT_ERRORS.with(|erl| {
                        nd_log_limit!(
                            erl,
                            Source::Daemon,
                            LogPriority::Err,
                            "DBENGINE: Invalid extent index in journalfile {}",
                            index.fileno
                        );
                    });
                    break;
                };
                list.entry(first_s).or_insert(Pd {
                    first_time_s: first_s,
                    last_time_s: last_s,
                    update_every_s: p.update_every_s,
                    status: DISK_PENDING | DATAFILE_ACQUIRED,
                    page: None,
                    extent: Some((
                        index.fileno,
                        extent.datafile_offset / BLOCK_SIZE as u64,
                        extent.datafile_size,
                    )),
                });
                found += 1;
            }
        }
        found
    }

    /// `pgc_inject_gap()`: an empty page for the time between `start_s` and `end_s` inside the metric's retention.
    fn inject_gap(&self, metric: &Handle, start_s: i64, end_s: i64) {
        let r = metric.retention();
        if in_range(start_s, end_s, r.first_time_s, r.last_time_s) != Range::In {
            return;
        }
        let (start_s, end_s) = (start_s.max(r.first_time_s), end_s.min(r.last_time_s));
        if start_s >= end_s {
            return;
        }
        self.main.add(
            metric.tier(),
            metric.uuid(),
            CachedPage::new(start_s, end_s, 0, None),
        );
    }

    /// `list_has_time_gaps()`: the pages a query would use, walked from the start; the others skipped; pages to use
    /// looked up once more in the main cache; with `populate`, the time no page covers cached as gaps. The gaps met,
    /// the pages to use, and the pages to load from disk.
    fn time_gaps(
        &self,
        metric: &Handle,
        prep: &mut Prep,
        start_s: i64,
        end_s: i64,
        populate: bool,
    ) -> (usize, usize, usize) {
        let list = &mut prep.list;
        prep.optimal_end_time_s = 0;
        for pd in list.values_mut() {
            pd.status &= !(SKIP | PREPROCESSED);
        }
        let mut gaps = 0;
        let mut now_s = start_s;
        let mut dt_s = self.metric_dt(metric);
        let mut total = 0;
        while let Some(k) = find_page_for_time(list, now_s, &mut gaps, PREPROCESSED, 0) {
            let pd = list.get_mut(&k).expect("found in the list");
            pd.status |= PREPROCESSED;
            total += 1;
            if pd.update_every_s != 0 {
                dt_s = i64::from(pd.update_every_s);
            }
            let (first_s, last_s) = (pd.first_time_s, pd.last_time_s);
            if populate && first_s > now_s {
                self.inject_gap(metric, now_s, first_s);
            }
            now_s = last_s + dt_s;
            if now_s > end_s {
                prep.optimal_end_time_s = last_s;
                break;
            }
        }
        if populate && now_s < end_s {
            self.inject_gap(metric, now_s, end_s);
        }
        let mut to_load = 0;
        for pd in list.values_mut() {
            if pd.status & PREPROCESSED == 0 {
                pd.status |= SKIP;
                pd.status &= !(READY | DISK_PENDING);
                continue;
            }
            if pd.page.is_none() {
                if let Some(page) =
                    self.main
                        .search(metric.tier(), metric.uuid(), pd.first_time_s, Search::Exact)
                {
                    pd.status &= !DISK_PENDING;
                    pd.status |= READY | PRELOADED;
                    if page.is_empty() {
                        pd.status |= EMPTY;
                    }
                    pd.page = Some(page);
                } else if pd.status & FAILED == 0 && pd.status & DATAFILE_ACQUIRED != 0 {
                    to_load += 1;
                    pd.status |= DISK_PENDING;
                }
            } else {
                pd.status &= !DISK_PENDING;
                pd.status |= READY | PRELOADED;
            }
        }
        (gaps, total, to_load)
    }

    /// `get_page_list()`: the main cache, then the open pages, then the v2 files, stopping at the first pass whose
    /// pages cover the window without gaps; the last pass caches the gaps. The pages to load from disk.
    fn page_list(&self, metric: &Handle, start_s: i64, end_s: i64) -> (Prep, usize) {
        let mut prep = Prep::default();
        let mut cache_gaps = 0;
        let found = self.pages_from_cache(
            metric,
            start_s,
            end_s,
            &mut prep.list,
            &mut cache_gaps,
            false,
        );
        if found > 0 && cache_gaps == 0 {
            let (gaps, total, to_load) = self.time_gaps(metric, &mut prep, start_s, end_s, false);
            if total > 0 && gaps == 0 {
                return (prep, to_load);
            }
        }
        let found = self.pages_from_cache(
            metric,
            start_s,
            end_s,
            &mut prep.list,
            &mut cache_gaps,
            true,
        );
        if found > 0 {
            let (gaps, total, to_load) = self.time_gaps(metric, &mut prep, start_s, end_s, false);
            if total > 0 && gaps == 0 {
                return (prep, to_load);
            }
        }
        self.pages_from_v2(metric, start_s, end_s, &mut prep.list);
        let (_, _, to_load) = self.time_gaps(metric, &mut prep, start_s, end_s, true);
        (prep, to_load)
    }

    /// `datafile_extent_read()`: the extent's bytes, read in whole blocks.
    fn read_extent(
        &self,
        tier: usize,
        fileno: u32,
        block: u64,
        bytes: u32,
    ) -> Option<Arc<Vec<u8>>> {
        if let Some(cached) = self.extents.get((tier, fileno, block)) {
            return Some(cached);
        }
        if !extent::valid_disk_size(bytes) {
            nd_log_limit!(
                &EXTENT_SIZE,
                Source::Daemon,
                LogPriority::Err,
                "DBENGINE: refusing to read extent at offset {} with invalid size {bytes}",
                block * BLOCK_SIZE as u64
            );
            return None;
        }
        let file = self.tiers[tier].datafiles.get(&fileno)?;
        let len = u64::from(bytes).div_ceil(BLOCK_SIZE as u64) * BLOCK_SIZE as u64;
        let mut buf = vec![0u8; len as usize];
        file.read_exact_at(&mut buf, block * BLOCK_SIZE as u64)
            .ok()?;
        buf.truncate(bytes as usize);
        Some(self.extents.add((tier, fileno, block), buf))
    }

    /// `epdl_find_extent_and_populate_pages()` for one extent: its requested pages validated, decoded and cached
    /// (invalid ones as empty pages); pages it did not give are failed.
    fn load_extent(
        &self,
        metric: &Handle,
        list: &mut BTreeMap<i64, Pd>,
        keys: &[i64],
        extent_at: (u32, u64, u32),
        now_s: i64,
    ) {
        let (fileno, block, bytes) = extent_at;
        let tier = metric.tier();
        let first_pd = &list[&keys[0]];
        let pd_subject = (first_pd.first_time_s, first_pd.last_time_s);
        let decoded = self
            .read_extent(tier, fileno, block, bytes)
            .map(|data| extent::decode(&data));
        match decoded {
            Some(Ok(extent)) => {
                if !extent.crc_ok {
                    extent_error(
                        tier,
                        fileno,
                        block,
                        bytes,
                        Subject::Pd(metric.uuid(), pd_subject.0, pd_subject.1),
                        "CRC32 checksum FAILED",
                        LogPriority::Err,
                    );
                }
                let count = extent.descriptors.len();
                let mut offset = 0u32;
                for (i, d) in extent.descriptors.iter().enumerate() {
                    let page_offset = offset;
                    offset = offset.wrapping_add(d.page_length);
                    let slot = extent.page(i);
                    if slot == PageSlot::Skipped {
                        extent_error(
                            tier,
                            fileno,
                            block,
                            bytes,
                            Subject::Descr(d),
                            &format!("page {i} (out of {count}) is EMPTY"),
                            LogPriority::Err,
                        );
                        continue;
                    }
                    if self.mrg.get_and_acquire(&d.uuid, tier).is_none() {
                        extent_error(
                            tier,
                            fileno,
                            block,
                            bytes,
                            Subject::Descr(d),
                            &format!("page {i} (out of {count}) has unknown UUID"),
                            LogPriority::Debug,
                        );
                        continue;
                    }
                    let start_s = (d.start_time_ut / 1_000_000) as i64;
                    if d.uuid != *metric.uuid() || !keys.contains(&start_s) {
                        continue;
                    }
                    let pd_ue = list[&start_s].update_every_s;
                    let vd = validate_extent_page_descr(d, now_s + 1, pd_ue, extent.read_error);
                    log_validation(&d.uuid, &vd, now_s + 1, "loaded");
                    let data = if !vd.valid {
                        None
                    } else {
                        match slot {
                            PageSlot::Page(b) => {
                                match DiskPage::load(d.page_type, &b[..vd.page_length.min(b.len())])
                                {
                                    Ok(page) => Some(page),
                                    Err(EmptyPage::InvalidChain) => {
                                        netdata_log_error!(
                                            "DBENGINE: invalid gorilla disk page chain."
                                        );
                                        None
                                    }
                                    Err(EmptyPage::Unfit) => None,
                                }
                            }
                            _ => {
                                let msg = if extent.compression == COMPRESSION_NONE {
                                    format!(
                                        "page {i} (out of {count}) offset {page_offset} + page length {}, exceeds \
                                             the payload size {}",
                                        vd.page_length,
                                        extent.payload_len()
                                    )
                                } else {
                                    format!(
                                        "page {i} (out of {count}) offset {page_offset} + page length {}, exceeds \
                                             the uncompressed buffer size {}",
                                        vd.page_length,
                                        extent.payload_len()
                                    )
                                };
                                extent_error(
                                    tier,
                                    fileno,
                                    block,
                                    bytes,
                                    Subject::Descr(d),
                                    &msg,
                                    LogPriority::Err,
                                );
                                None
                            }
                        }
                    };
                    let page = self.main.add(
                        tier,
                        metric.uuid(),
                        CachedPage::new(vd.start_time_s, vd.end_time_s, vd.update_every_s, data),
                    );
                    if let Some(pd) = list.get_mut(&start_s) {
                        pd.status |= READY;
                        if page.is_empty() {
                            pd.status |= EMPTY;
                        }
                        pd.status &= !DISK_PENDING;
                        pd.page = Some(page);
                    }
                }
            }
            Some(Err(_)) => extent_error(
                tier,
                fileno,
                block,
                bytes,
                Subject::Pd(metric.uuid(), pd_subject.0, pd_subject.1),
                "header is INVALID",
                LogPriority::Err,
            ),
            None => {}
        }
        // epdl_mark_all_not_loaded_pages_as_failed()
        for k in keys {
            if let Some(pd) = list.get_mut(k) {
                if pd.page.is_none() {
                    pd.status |= FAILED;
                    pd.status &= !DISK_PENDING;
                }
            }
        }
    }

    /// `rrdeng_prep_query()`: the page list, then the pages to read loaded, extent by extent in file order.
    fn prepare(&self, metric: &Handle, start_s: i64, end_s: i64, now_s: i64) -> Prep {
        let (mut prep, to_load) = self.page_list(metric, start_s, end_s);
        if to_load > 0 {
            let mut by_extent: BTreeMap<(u32, u64, u32), Vec<i64>> = BTreeMap::new();
            for (k, pd) in &prep.list {
                if pd.status & DISK_PENDING != 0 {
                    if let Some(at) = pd.extent {
                        by_extent.entry(at).or_default().push(*k);
                    }
                }
            }
            for (at, keys) in by_extent {
                self.load_extent(metric, &mut prep.list, &keys, at, now_s);
            }
        }
        prep
    }

    /// `rrdeng_load_metric_init()`: a query of `metric` from `start_s` to `end_s`, clamped to its retention; outside
    /// it the query gives no point. `now_s` is the wall clock page validation uses.
    pub fn query(
        self: &Arc<Self>,
        metric: &Handle,
        start_s: i64,
        end_s: i64,
        priority: Priority,
        now_s: i64,
    ) -> Query {
        let r = metric.retention();
        let mut q = Query {
            engine: Arc::clone(self),
            metric: metric.dup(),
            start_time_s: start_s,
            end_time_s: 0,
            now_s: start_s,
            dt_s: i64::from(r.update_every_s),
            prep: None,
            pending: None,
            page: None,
            _inflight: None,
        };
        if in_range(start_s, end_s, r.first_time_s, r.last_time_s) != Range::In {
            return q;
        }
        q.start_time_s = start_s.max(r.first_time_s);
        q.end_time_s = end_s.min(r.last_time_s);
        q.now_s = q.start_time_s;
        if q.dt_s == 0 {
            q.dt_s = i64::from(self.update_every_s);
            metric.set_update_every_s_if_zero(self.update_every_s);
        }
        // pg_cache_preload()
        let data = &self.tiers[metric.tier()];
        let inflight = Inflight::new(&data.inflight);
        q._inflight = Some(Arc::clone(&inflight));
        if data.quiesced.load(Ordering::Acquire) {
            q.prep = Some(Prep {
                list: BTreeMap::new(),
                optimal_end_time_s: q.end_time_s,
            });
            return q;
        }
        let (engine, job_metric, s, e) =
            (Arc::clone(self), metric.dup(), q.start_time_s, q.end_time_s);
        match (&self.pool, priority) {
            (Some(pool), p) if p != Priority::Synchronous => {
                let (tx, rx) = mpsc::channel();
                if pool
                    .queue(move || {
                        let _inflight = inflight;
                        let _ = tx.send(engine.prepare(&job_metric, s, e, now_s));
                    })
                    .is_ok()
                {
                    q.pending = Some(rx);
                } else {
                    q.prep = Some(self.prepare(metric, s, e, now_s));
                }
            }
            _ => q.prep = Some(self.prepare(metric, s, e, now_s)),
        }
        q
    }
}

/// The page a query reads: its points from the position it starts at.
#[derive(Debug)]
struct Current {
    entries: usize,
    position: usize,
    points: Vec<(bool, StoragePoint)>,
    first_position: usize,
}

/// A running query (`struct rrdeng_query_handle` with its `storage_engine_query_handle`).
#[derive(Debug)]
pub struct Query {
    engine: Arc<Dbengine>,
    metric: Handle,
    pub start_time_s: i64,
    /// 0 when the window is outside the metric's retention: no point is ever read.
    pub end_time_s: i64,
    now_s: i64,
    dt_s: i64,
    prep: Option<Prep>,
    pending: Option<mpsc::Receiver<Prep>>,
    page: Option<Current>,
    /// Set once the query has a preparation.
    _inflight: Option<Arc<Inflight>>,
}

impl Query {
    /// `rrdeng_prep_wait()`.
    fn prep_wait(&mut self) {
        if let Some(rx) = self.pending.take() {
            self.prep = Some(rx.recv().unwrap_or_default());
        }
    }

    /// `pg_cache_lookup_next()`: the next usable page for the current time, and its entries by time.
    fn lookup_next(&mut self) -> Option<(Arc<CachedPage>, usize)> {
        self.prep_wait();
        let last_ue = self.dt_s as u32;
        let now_s = self.now_s;
        let prep = self.prep.as_mut()?;
        let mut gaps = 0;
        loop {
            let k = find_page_for_time(&prep.list, now_s, &mut gaps, PROCESSED, EMPTY)?;
            let pd = prep.list.get_mut(&k).expect("found in the list");
            let Some(page) = pd.page.clone() else {
                pd.status |= FAILED;
                continue;
            };
            if page.is_empty() {
                pd.status |= EMPTY;
            }
            if pd.status & (GLOBAL_SKIP | EMPTY) != 0 {
                continue;
            }
            let (start_s, mut end_s) = (page.start_time_s, page.end_time_s());
            if start_s == 0 || end_s == 0 {
                pd.status |= INVALID | RELEASED;
                pd.page = None;
                continue;
            }
            let mut ue = page.update_every_s();
            if ue == 0 {
                ue = page.fix_update_every(last_ue);
                pd.update_every_s = ue;
            }
            let by_size = page.data.as_ref().map_or(0, DiskPage::slots_used);
            let ue64 = i64::from(ue);
            let mut by_time = if ue != 0 {
                ((end_s - (start_s - ue64)) / ue64) as usize
            } else {
                1
            };
            if by_size < by_time {
                let fixed = start_s + (by_size as i64 - 1) * ue64;
                end_s = page.fix_end_time_s(fixed);
                pd.last_time_s = end_s;
                by_time = ((end_s - (start_s - ue64)) / ue64) as usize;
            }
            if end_s < now_s {
                pd.status |= SKIP | RELEASED;
                pd.page = None;
                continue;
            }
            // the query holds the page until it moves on (C releases the list's reference to the handle)
            pd.page = None;
            pd.status |= RELEASED | PROCESSED;
            return Some((page, by_time));
        }
    }

    /// `rrdeng_load_page_next()`: the next page, positioned at the first point not before the current time.
    fn load_page_next(&mut self) -> bool {
        self.page = None;
        if self.now_s > self.end_time_s {
            return false;
        }
        let Some((page, entries)) = self.lookup_next() else {
            return false;
        };
        if page.is_empty() || entries == 0 {
            return false;
        }
        let (start_s, end_s, ue) = (
            page.start_time_s,
            page.end_time_s(),
            i64::from(page.update_every_s()),
        );
        let position;
        if self.now_s >= start_s && self.now_s <= end_s {
            if entries == 1 || start_s == end_s || ue == 0 {
                position = 0;
                self.now_s = start_s;
            } else {
                let mut p = ((self.now_s - start_s) as u64 * (entries as u64 - 1)
                    / (end_s - start_s) as u64) as usize;
                let mut point_end_s = start_s + p as i64 * ue;
                while point_end_s < self.now_s && p + 1 < entries {
                    p += 1;
                    point_end_s = start_s + p as i64 * ue;
                }
                position = p;
                self.now_s = point_end_s;
            }
        } else if self.now_s < start_s {
            self.now_s = start_s;
            position = 0;
        } else {
            self.now_s = end_s;
            position = entries - 1;
        }
        self.dt_s = ue;
        let points = match &page.data {
            Some(data) => {
                let mut c = data.cursor(position);
                (position..entries).map(|_| c.next_point()).collect()
            }
            None => Vec::new(),
        };
        self.page = Some(Current {
            entries,
            position,
            points,
            first_position: position,
        });
        true
    }

    /// `rrdeng_load_metric_next()`: the next point; empty ones outside the pages, and one trailing empty point when
    /// they run out.
    pub fn next_metric(&mut self) -> StoragePoint {
        let sp = if self.now_s > self.end_time_s {
            StoragePoint::empty(self.now_s - self.dt_s, self.now_s)
        } else {
            let need = self.page.as_ref().is_none_or(|c| c.position >= c.entries);
            if need && !self.load_page_next() {
                self.now_s = self.end_time_s;
                let sp = StoragePoint::empty(self.now_s - self.dt_s, self.now_s);
                self.now_s += self.dt_s;
                return sp;
            }
            let c = self.page.as_ref().expect("a page is loaded");
            let (_, mut point) = c
                .points
                .get(c.position - c.first_position)
                .copied()
                .unwrap_or((false, StoragePoint::empty(0, 0)));
            point.start_time_s = self.now_s - self.dt_s;
            point.end_time_s = self.now_s;
            point
        };
        self.now_s += self.dt_s;
        if let Some(c) = self.page.as_mut() {
            c.position += 1;
        }
        sp
    }

    /// `rrdeng_load_metric_is_finished()`.
    pub fn is_finished(&self) -> bool {
        self.now_s > self.end_time_s
    }

    /// `rrdeng_load_align_to_optimal_before()`: the end the page list reached, when later than the query's.
    pub fn align_to_optimal_before(&mut self) -> i64 {
        self.prep_wait();
        if let Some(prep) = &self.prep {
            if prep.optimal_end_time_s > self.end_time_s {
                self.end_time_s = prep.optimal_end_time_s;
            }
        }
        self.end_time_s
    }

    /// The metric queried.
    pub fn metric(&self) -> &Handle {
        &self.metric
    }

    /// The engine the query reads.
    pub fn engine(&self) -> &Arc<Dbengine> {
        &self.engine
    }
}

#[cfg(test)]
mod tests;
