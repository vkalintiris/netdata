//! `rrdeng_size_statistics()`: what `/api/v1/dbengine_stats` reports of a tier, from its v2 files as
//! `populate_v2_statistics()` walks them, each bounds-checked against its size as C checks its mapping and read through
//! a window of a MiB or one list, whichever is larger (C walks the mapping in the page cache).

use netdata_agent_text::c::double_to_u64;

use super::USEC_PER_SEC;
use super::collect::TIER_PAGE_SIZE;
use super::tier::TierData;
use crate::dbengine::format::ReadAt;
use crate::dbengine::format::descriptor::point_size;
use crate::dbengine::format::journal_v2::{
    self, EXTENT_SIZE, HEADER_SIZE, Header, METRIC_SIZE, PAGE_HEADER_SIZE, PAGE_SIZE, PageEntry,
    PageHeader,
};

/// `natural_alignment(sizeof(struct rrdengine_datafile)) + natural_alignment(sizeof(struct rrdengine_journalfile))` of
/// the 64-bit oracle build (D84.3).
pub const SIZEOF_DATAFILE: u64 = 408;

/// `RRDENG_SIZE_STATS` without its per-page-type totals, which nothing reports.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SizeStats {
    pub default_granularity_secs: u64,
    pub sizeof_datafile: u64,
    pub sizeof_page_in_cache: u64,
    pub sizeof_point_data: u64,
    pub sizeof_page_data: u64,
    pub pages_per_extent: u64,
    pub datafiles: u64,
    pub extents: u64,
    pub extents_pages: u64,
    pub points: u64,
    pub metrics: u64,
    pub metrics_pages: u64,
    pub extents_compressed_bytes: u64,
    pub pages_uncompressed_bytes: u64,
    pub pages_duration_secs: i64,
    pub single_point_pages: u64,
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub currently_collected_metrics: u64,
    pub estimated_concurrently_collected_metrics: u64,
    pub disk_space: u64,
    pub max_disk_space: u64,
    pub database_retention_secs: i64,
    pub average_compression_savings: f64,
    pub average_point_duration_secs: f64,
    pub average_metric_retention_secs: f64,
    pub ephemeral_metrics_per_day_percent: f64,
    pub average_page_size_bytes: f64,
}

/// The bytes a walk holds of a file at most, unless one list takes more.
const WINDOW: usize = 1 << 20;

/// A window over a v2 file, read as the walk asks: a walk in file order reads each byte once.
struct Window<'a, R: ReadAt + ?Sized> {
    file: &'a R,
    size: u64,
    at: u64,
    buf: Vec<u8>,
}

impl<'a, R: ReadAt + ?Sized> Window<'a, R> {
    fn new(file: &'a R, size: u64) -> Self {
        Window {
            file,
            size,
            at: 0,
            buf: Vec::new(),
        }
    }

    /// The `len` bytes at `offset`, which the caller checked are within the file; `None` when they cannot be read
    /// (a file shorter than its size, where C's walk would fault and stop: a window that cannot be read whole is
    /// read as asked).
    fn bytes(&mut self, offset: u64, len: usize) -> Option<&[u8]> {
        let end = offset + len as u64;
        if offset < self.at || end > self.at + self.buf.len() as u64 {
            self.at = offset;
            for want in [
                (len.max(WINDOW) as u64).min(self.size - offset) as usize,
                len,
            ] {
                self.buf.resize(want, 0);
                if self.file.read_exact_at(&mut self.buf, offset).is_ok() {
                    break;
                }
                self.buf.clear();
            }
            if self.buf.len() < len {
                return None;
            }
        }
        let start = (offset - self.at) as usize;
        Some(&self.buf[start..start + len])
    }
}

/// Entries of `entry` bytes read at a time: about a MiB.
fn chunk(entry: usize) -> u64 {
    (WINDOW / entry) as u64
}

impl SizeStats {
    /// One page of a metric's list; `start_s` is the file's start, `granularity_s` the update every of a page without
    /// two points.
    fn add_page(&mut self, start_s: i64, p: &PageEntry, point_size: u64, granularity_s: i64) {
        // the tier's page type is always known; an unknown one would count no points (C would divide by 0)
        let points = u64::from(p.page_length)
            .checked_div(point_size)
            .unwrap_or(0);
        let start = start_s + i64::from(p.delta_start_s);
        let end = start_s + i64::from(p.delta_end_s);
        let update_every_s = if points > 1 {
            // C divides a time_t by a size_t: unsigned
            (end.wrapping_sub(start) as u64 / (points - 1)) as i64
        } else {
            self.single_point_pages += 1;
            granularity_s
        };
        let duration_s = end.wrapping_sub(start).wrapping_add(update_every_s);
        self.pages_uncompressed_bytes += u64::from(p.page_length);
        self.pages_duration_secs = self.pages_duration_secs.wrapping_add(duration_s);
        self.points += points;
        let first = start.wrapping_sub(update_every_s);
        if self.first_time_s == 0 || first < self.first_time_s {
            self.first_time_s = first;
        }
        if self.last_time_s == 0 || end > self.last_time_s {
            self.last_time_s = end;
        }
    }

    /// `populate_v2_statistics()` over a v2 file of `size` bytes: its extents when their list is in bounds; then,
    /// when the metric list is, every metric (counted before its page list is checked) and the pages of each whose
    /// header and list are in bounds. A read that fails ends the walk with what it counted, as C's fault does.
    fn add_file<R: ReadAt + ?Sized>(
        &mut self,
        file: &R,
        size: u64,
        point_size: u64,
        granularity_s: i64,
    ) {
        let mut w = Window::new(file, size);
        let Some(h) = (size >= HEADER_SIZE as u64)
            .then(|| w.bytes(0, HEADER_SIZE))
            .flatten()
            .and_then(|b| b.first_chunk::<HEADER_SIZE>())
            .map(Header::decode)
        else {
            return;
        };
        let in_bounds = |offset: u32, count: u32, entry: usize| {
            let (offset, count) = (u64::from(offset), u64::from(count));
            offset <= size && count <= (size - offset) / entry as u64
        };
        if in_bounds(h.extent_offset, h.extent_count, EXTENT_SIZE) {
            self.extents += u64::from(h.extent_count);
            let (offset, count) = (u64::from(h.extent_offset), u64::from(h.extent_count));
            for first in (0..count).step_by(chunk(EXTENT_SIZE) as usize) {
                let n = (count - first).min(chunk(EXTENT_SIZE)) as usize;
                let at = offset + first * EXTENT_SIZE as u64;
                let Some(list) = w.bytes(at, n * EXTENT_SIZE) else {
                    return;
                };
                for e in journal_v2::extents(list) {
                    self.extents_compressed_bytes += u64::from(e.datafile_size);
                    self.extents_pages += u64::from(e.pages);
                }
            }
        }
        if !in_bounds(h.metric_offset, h.metric_count, METRIC_SIZE) {
            return;
        }
        let start_s = (h.start_time_ut / USEC_PER_SEC) as i64;
        self.metrics += u64::from(h.metric_count);
        let (offset, count) = (u64::from(h.metric_offset), u64::from(h.metric_count));
        for first in (0..count).step_by(chunk(METRIC_SIZE) as usize) {
            let n = (count - first).min(chunk(METRIC_SIZE)) as usize;
            let Some(list) = w.bytes(offset + first * METRIC_SIZE as u64, n * METRIC_SIZE) else {
                return;
            };
            let metrics: Vec<_> = journal_v2::metrics(list).collect();
            for m in metrics {
                let offset = u64::from(m.page_offset);
                if offset > size || size - offset < PAGE_HEADER_SIZE as u64 {
                    continue;
                }
                let Some(entries) = w
                    .bytes(offset, PAGE_HEADER_SIZE)
                    .and_then(|b| b.first_chunk::<PAGE_HEADER_SIZE>())
                    .map(|b| PageHeader::decode(b).entries)
                else {
                    return;
                };
                let room = size - offset - PAGE_HEADER_SIZE as u64;
                if u64::from(entries) > room / PAGE_SIZE as u64 {
                    continue;
                }
                self.metrics_pages += u64::from(entries);
                let at = offset + PAGE_HEADER_SIZE as u64;
                let Some(pages) = w.bytes(at, entries as usize * PAGE_SIZE) else {
                    return;
                };
                for p in journal_v2::pages(pages) {
                    self.add_page(start_s, &p, point_size, granularity_s);
                }
            }
        }
    }

    /// The figures derived from the totals, in C's double arithmetic (a zero divisor gives C's infinities).
    fn derive(&mut self) {
        self.database_retention_secs = self.last_time_s.wrapping_sub(self.first_time_s);
        if self.extents_pages != 0 {
            self.average_page_size_bytes =
                self.pages_uncompressed_bytes as f64 / self.extents_pages as f64;
        }
        if self.pages_uncompressed_bytes > 0 {
            self.average_compression_savings = 100.0
                - (self.extents_compressed_bytes as f64 * 100.0
                    / self.pages_uncompressed_bytes as f64);
        }
        if self.points != 0 {
            self.average_point_duration_secs = self.pages_duration_secs as f64 / self.points as f64;
        }
        if self.metrics != 0 {
            self.average_metric_retention_secs =
                self.pages_duration_secs as f64 / self.metrics as f64;
            if self.database_retention_secs != 0 {
                let coverage =
                    self.average_metric_retention_secs / self.database_retention_secs as f64;
                let days = self.database_retention_secs as f64 / 86400.0;
                self.estimated_concurrently_collected_metrics =
                    double_to_u64(self.metrics as f64 * coverage);
                self.ephemeral_metrics_per_day_percent = (self.metrics as f64 * 100.0
                    / self.estimated_concurrently_collected_metrics as f64
                    - 100.0)
                    / days;
            }
        }
    }
}

impl TierData {
    /// `rrdeng_size_statistics()`: every pair counts as a data file; each whose v2 index serves it adds its v2 file.
    /// `granularity_s` is the tier's update every (`nd_profile.update_every` times `get_tier_grouping()`).
    pub fn size_statistics(&self, granularity_s: u64, pages_per_extent: usize) -> SizeStats {
        let point_size = point_size(self.config.page_type) as u64;
        let mut stats = SizeStats::default();
        for (fileno, df) in self.files_snapshot() {
            stats.datafiles += 1;
            let Some(index) = self.v2_of(fileno).filter(|_| df.v2_available()) else {
                continue;
            };
            stats.add_file(&index.file, index.size, point_size, granularity_s as i64);
        }
        stats.currently_collected_metrics = self.collectors_running() as u64;
        stats.disk_space = self.current_disk_space();
        stats.max_disk_space = self.config.max_disk_space;
        stats.derive();
        stats.sizeof_datafile = SIZEOF_DATAFILE;
        stats.sizeof_point_data = point_size;
        stats.sizeof_page_data = TIER_PAGE_SIZE[self.tier()] as u64;
        stats.pages_per_extent = pages_per_extent as u64;
        stats.default_granularity_secs = granularity_s;
        stats
    }
}

#[cfg(test)]
mod tests;
