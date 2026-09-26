//! Extent writes (`main_cache_flush_dirty_page_callback()` and `extent_write_tp_worker()` in `rrdengine.c`): a batch of
//! dirty pages becomes one extent at the position reserved in the tier's last pair (a new pair when it is full), its
//! journal transaction follows, and its pages join the open cache. A failed write marks the pair failed and moves the
//! extent to a new pair once; a second failure loses it. Brief `knowledge/brief-dbengine-s3-commit3-map.md` in the
//! status repository; decisions D65 and D66.

use std::sync::{Arc, PoisonError, mpsc};

use netdata_agent_evloop::work::on_worker;
use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log_limit, uv_strerror};

use super::cache::Batch;
use super::io::write_retrying;
use super::query::Dbengine;
use super::runtime::Cmd;
use super::tier::{DataFile, OpenPage, TierData};
use crate::dbengine::format::BLOCK_SIZE;
use crate::dbengine::format::descriptor::{PAGE_TYPE_GORILLA_32BIT, PageDescriptor};
use crate::dbengine::format::extent;
use crate::dbengine::format::journal_v1::{StoreData, encode_transaction};

const USEC_PER_SEC: u64 = 1_000_000;

/// `nd_log_limit_static_global_var(dbengine_rotate_erl, 10, 0)` and `(dbengine_erl, 10, 0)`.
static ROTATING: ErrorLimit = ErrorLimit::new(10, 0);
static LOST: ErrorLimit = ErrorLimit::new(10, 0);

/// A page of a written extent, as it joins the open cache: metric, start, end, update every.
type Written = ([u8; 16], i64, i64, u32);

fn errno(err: &std::io::Error) -> i32 {
    err.raw_os_error().unwrap_or(libc::EIO)
}

impl Dbengine {
    /// `pgc_flush_pages()` and its variants over the main cache: see `MainCache::flush_pages()`. Whether it stopped
    /// before the queues ran short.
    pub fn flush_pages(
        self: &Arc<Self>,
        max_flushes: usize,
        section: Option<usize>,
        wait: bool,
        all: bool,
    ) -> bool {
        self.main.flush_pages(
            max_flushes,
            section,
            wait,
            all,
            |tier| self.tiers[tier].extent_started(),
            |batch| self.save(batch),
        )
    }

    /// `flush_inline()` (the main cache's `max_flushes_inline` is 1): while the dirty pages outgrow the hot ones, up
    /// to two batches are flushed here, unless the cache is busy (D65.10).
    pub(crate) fn flush_inline(self: &Arc<Self>) {
        if self.main.flushing_critical() {
            self.flush_pages(1, None, false, false);
        }
    }

    /// `pgc_flush_dirty_pages()`: every dirty page of the tier.
    pub fn flush_dirty(self: &Arc<Self>, tier: usize) {
        self.flush_pages(0, Some(tier), true, true);
    }

    /// `pgc_flush_all_hot_and_dirty_pages()`: the tier's hot pages turn dirty, then every dirty page is flushed.
    pub fn flush_all_hot_and_dirty(self: &Arc<Self>, tier: usize) {
        self.main.all_hot_to_dirty(tier);
        self.flush_dirty(tier);
    }

    /// `main_cache_flush_dirty_page_callback()`: the extent is written on a pool thread, as C's `EXTENT_WRITE` runs
    /// on `UV_WORKER` (inline when already on one, or without a pool), and waited for.
    fn save(self: &Arc<Self>, batch: &Batch) {
        if let Some(pool) = self.pool.as_ref().filter(|_| !on_worker()) {
            let (tx, rx) = mpsc::channel();
            let (engine, job_batch) = (Arc::clone(self), batch.clone());
            let job = move || {
                engine.extent_write(&job_batch);
                let _ = tx.send(());
            };
            if pool.queue(job).is_ok() {
                let _ = rx.recv();
                return;
            }
        }
        self.extent_write(batch);
    }

    /// `extent_write_tp_worker()`: the extent built, placed, written with its transaction, and its pages added to
    /// the open cache; the tier's in-flight count, taken when the batch was, released.
    fn extent_write(&self, batch: &Batch) {
        let td = &self.tiers[batch.tier];
        let mut descriptors = Vec::with_capacity(batch.pages.len());
        let mut payloads = Vec::with_capacity(batch.pages.len());
        let mut written: Vec<Written> = Vec::with_capacity(batch.pages.len());
        for (uuid, page) in &batch.pages {
            let Some((page_type, entries, bytes)) = page.extent_data() else {
                continue;
            };
            let (start_s, end_s) = (page.start_time_s, page.end_time_s());
            let (start_ut, end_ut) = (start_s as u64 * USEC_PER_SEC, end_s as u64 * USEC_PER_SEC);
            let length = bytes.len() as u32;
            descriptors.push(if page_type == PAGE_TYPE_GORILLA_32BIT {
                let delta_s = (end_ut.saturating_sub(start_ut) / USEC_PER_SEC) as u32;
                PageDescriptor::gorilla(*uuid, length, start_ut, entries as u32, delta_s)
            } else {
                PageDescriptor::array(page_type, *uuid, length, start_ut, end_ut)
            });
            payloads.push(bytes);
            written.push((*uuid, start_s, end_s, page.update_every_s()));
        }
        if descriptors.is_empty() {
            td.extent_finished();
            return;
        }
        let pages: Vec<(PageDescriptor, &[u8])> = descriptors
            .iter()
            .copied()
            .zip(payloads.iter().map(Vec::as_slice))
            .collect();
        let encoded = extent::encode(&pages, td.config.compression);
        let real = encoded.bytes.len() as u64;
        let size_bytes = encoded.size_bytes as u32;

        let (mut df, pos) = self.reserve(td, real, None);
        let mut pos = pos.expect("a reserve with no file to avoid places the extent");
        let mut id = td.next_transaction_id();
        td.flushed_to(df.fileno);
        let mut moved = false;
        let result = loop {
            let result = write_retrying(&df.file, &encoded.bytes, pos).and_then(|()| {
                td.add_disk_space(real);
                let store = StoreData {
                    extent_offset: pos,
                    extent_size: size_bytes,
                    descriptors: descriptors.clone(),
                };
                df.append_journal(&encode_transaction(id, &store))
            });
            let Err(err) = &result else {
                td.add_disk_space(BLOCK_SIZE as u64);
                break result;
            };
            // the pair dropped a write: no more extents go to it
            df.mark_failed();
            if moved {
                break result;
            }
            moved = true;
            nd_log_limit!(
                &ROTATING,
                Source::Daemon,
                Priority::Err,
                "DBENGINE: tier {} datafile {} write failed ({}) - rotating to a new datafile and retrying the \
                 extent, to prevent data loss",
                td.tier(),
                df.fileno,
                uv_strerror(errno(err))
            );
            // `extent_move_to_new_datafile()`
            df.writer_finished();
            let (next, next_pos) = self.reserve(td, real, Some(&df));
            df = next;
            let Some(next_pos) = next_pos else {
                break result;
            };
            pos = next_pos;
            id = td.next_transaction_id();
            td.flushed_to(df.fileno);
        };
        if let Err(err) = &result {
            nd_log_limit!(
                &LOST,
                Source::Daemon,
                Priority::Err,
                "DBENGINE: tier {} datafile {} write failed ({}) - the extent is lost",
                td.tier(),
                df.fileno,
                uv_strerror(errno(err))
            );
        }
        df.writer_flushing_to_open();
        let journal_pos = result.as_ref().map_or(0, |&at| at);
        flush_to_open(td, &df, pos, size_bytes, journal_pos, &written, result.is_ok());
        td.extent_finished();
        // `after_extent_write()`: a rotation may have left a file to index
        if let Some(events) = self.events.get() {
            let _ = events.send(Cmd::ExtentWritten(batch.tier));
        }
    }

    /// `get_datafile_to_write_extent()` under the engine's reserve lock, the position taken inside it (D65.3): the
    /// last pair with a writer counted on it, a new pair first when it is full (C writes past the target when the pair
    /// cannot be created). With `avoid`, the pair of a failed write: when it comes back its position is not taken.
    fn reserve(
        &self,
        td: &TierData,
        size: u64,
        avoid: Option<&Arc<DataFile>>,
    ) -> (Arc<DataFile>, Option<u64>) {
        let _reserve = self
            .reserve
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut df = td.last_file();
        df.writer_started();
        if df.is_full(size, td.config.target_datafile_size()) {
            let old = df;
            if td.create_pair() {
                td.set_needs_indexing();
            }
            df = td.last_file();
            df.writer_started();
            old.writer_finished();
        }
        if avoid.is_some_and(|a| Arc::ptr_eq(a, &df)) {
            return (df, None);
        }
        let pos = df.advance(size);
        (df, Some(pos))
    }
}

/// `extent_flush_to_open()`: a written extent's pages join the open cache unless the tier is shutting down, and a
/// write to a file other than the last one leaves it to index.
/// The pages join in extent order, under the key of the extent's transaction (`journal_pos`); the file's last time
/// moves to the newest page, while quiescing too.
fn flush_to_open(
    td: &TierData,
    df: &DataFile,
    pos: u64,
    size_bytes: u32,
    journal_pos: u64,
    pages: &[Written],
    ok: bool,
) {
    let still_running = !td.quiesced();
    if still_running && ok {
        let mut open = td.open_mut();
        for (i, &(uuid, start_s, end_s, update_every_s)) in (0u32..).zip(pages) {
            let page = OpenPage {
                end_time_s: end_s,
                update_every_s,
                fileno: df.fileno,
                block: pos / BLOCK_SIZE as u64,
                bytes: size_bytes,
            };
            open.add(uuid, start_s, page, (journal_pos, i));
        }
    }
    if ok {
        let newest = pages.iter().map(|p| p.2).max().unwrap_or(0);
        if newest > 0 {
            df.extend_last_time(newest);
        }
    }
    df.flushed_to_open();
    if df.fileno != td.last_fileno() && still_running {
        td.set_needs_indexing();
    }
}

#[cfg(test)]
mod tests;
