//! Journal v2 indexing (`journalfile_migrate_to_v2_callback()` in `journalfile.c`, and at run time
//! `journal_v2_indexing_tp_worker()` in `rrdengine.c` with `pgc_open_cache_to_journal_v2()` in `cache.c`): a file's
//! open pages written as its v2 index, which then serves its queries in their place. Brief
//! `knowledge/brief-dbengine-s3-commit45-map.md` in the status repository; decisions D65 and D67.

use std::fs::File;

use netdata_agent_log::{Priority, Source, errno_of, nd_log, netdata_log_info};
use netdata_agent_text::size::size_to_string;

use super::load::TierConfig;
use super::query::Dbengine;
use super::tier::{DataFile, TierData};
use super::v2index::V2Index;
use crate::dbengine::format::journal_v2::{Builder, Layout, Page, WriteError};
use crate::dbengine::format::{FileKind, file_name, pair_name};

/// `journalfile_migrate_to_v2_callback()`: the pages indexed and the v2 file written, with C's records; `None` when
/// there is no metric (nothing is written, no record) or the file could not be written. `v1_size` is the journal's
/// position, which the header stores. The records come from the calling thread.
pub(crate) fn write_v2(
    cfg: &TierConfig,
    fileno: u32,
    v1_size: u64,
    pages: impl IntoIterator<Item = Page>,
) -> Option<(File, Layout)> {
    let mut builder = Builder::new(v1_size as u32);
    for page in pages {
        builder.page(page);
    }
    let layout = builder.layout()?;
    let name = file_name(FileKind::JournalV2, 1, fileno);
    let h = layout.header();
    netdata_log_info!(
        "DBENGINE: tier {}: indexing {name}: extents {}, metrics {}, pages {}",
        cfg.tier,
        h.extent_count,
        h.metric_count,
        h.page_count
    );
    let path = cfg.file(FileKind::JournalV2, fileno);
    let size = layout.size();
    let failed_allocation = || {
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "DBENGINE: Failed to allocate {size} bytes of memory for journal file \"{}\". Will retry later",
            path.display()
        );
    };
    match layout.write(&path) {
        Ok(file) => {
            netdata_log_info!(
                "DBENGINE: tier {}: migrated {name}, {}",
                cfg.tier,
                size_to_string(size as u64, "B", false).unwrap_or_default()
            );
            Some((file, layout))
        }
        Err(WriteError::Create(err)) => {
            nd_log!(Source::Daemon, Priority::Err, errno = errno_of(&err);
                "Cannot create/open file '{}'.", path.display());
            failed_allocation();
            None
        }
        Err(WriteError::Size(err)) => {
            nd_log!(Source::Daemon, Priority::Err, errno = errno_of(&err);
                "Cannot truncate file '{}' to size {size}.", path.display());
            failed_allocation();
            None
        }
        Err(WriteError::Write(_)) => {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DBENGINE: failed to write journal file \"{}\" (SIGBUS)",
                path.display()
            );
            netdata_log_info!(
                "DBENGINE: failed to build index \"{}\", file will be skipped",
                path.display()
            );
            None
        }
    }
}

/// `journal_v2_indexing_tp_worker()`: the tier's files that are neither the last one nor the one extents last went to,
/// and have no v2 index, indexed in order; a file with writers still on it is skipped for now, and after the first
/// file a tier over its quota stops, leaving the rest to the next run. Nothing while the tier is shutting down. How
/// many files it indexed.
pub fn journal_index(engine: &Dbengine, tier: usize) -> u32 {
    let td = &engine.tiers[tier];
    if td.quiesced() {
        return 0;
    }
    let (mut count, mut after) = (0, None);
    while let Some(df) = td.next_for_indexing(after) {
        after = Some(df.fileno);
        let name = pair_name(df.fileno);
        if df.writers_running() != (0, 0) {
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "DBENGINE: tier {tier}: {name} needs to be indexed, but it has writers working on it - skipping it \
                 for now"
            );
            continue;
        }
        if count > 0 && td.cap_exceeded(engine.now_s()) {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "DBENGINE: tier {tier}: reached quota limit, stopping journal indexing"
            );
            // C asks for another run here, which the deletion of the oldest files it schedules then lets progress;
            // until S5 deletes, the next rotation starts the next run (D69)
            break;
        }
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "DBENGINE: tier {tier}: {name} is ready to be indexed"
        );
        index_file(td, &df);
        count += 1;
        if td.quiesced() {
            break;
        }
    }
    if count > 0 {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "DBENGINE: tier {tier}: journal indexing done; {count} files processed"
        );
    }
    count
}

/// `pgc_open_cache_to_journal_v2()` of one file: its open pages written as its v2 index, which serves them from then
/// on (it is registered before they leave the open cache, so a query finds each page in one or the other). A file
/// that could not be written keeps its pages open for the next run.
fn index_file(td: &TierData, df: &DataFile) {
    let pages = td.open().file_pages(df.fileno);
    let indexed = !pages.is_empty();
    let Some((file, layout)) = write_v2(&td.config, df.fileno, df.journal_pos(), pages) else {
        return;
    };
    let index = V2Index::from_layout(df.fileno, file, &layout);
    td.add_disk_space(layout.size() as u64);
    df.set_times(index.start_time_s(), index.end_time_s());
    td.add_v2(df, index);
    td.open_mut().remove_file(df.fileno);
    // C keeps the indexed pages in its open cache as clean pages of the file (D76.1)
    if indexed {
        df.mark_clean_open();
    }
}

#[cfg(test)]
mod tests;
