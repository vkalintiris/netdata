//! Deleting a tier's oldest pair (`datafile_delete()` and `database_rotate_tp_worker()` in `rrdengine.c`): the pair
//! waits for its users, the registry's first times are recalculated from the files that remain
//! (`update_metrics_first_time_s()`, with C's skip of a list's last entry fixed: D30, D75.3), then its files are
//! unlinked and the tier's space given back, with C's records. Briefs `knowledge/brief-dbengine-s5-engine.md` and
//! `knowledge/brief-dbengine-s5-commit34-map.md` in the status repository; decisions D75, D76.

use std::sync::Arc;

use netdata_agent_log::{
    ErrorLimit, Priority, Source, nd_log_limit, netdata_log_error, netdata_log_info,
};
use netdata_agent_text::size::size_to_string;

use super::mrg::Handle;
use super::query::Dbengine;
use super::tier::{DataFile, Reason, TierData};
use crate::dbengine::format::journal_v2::MetricEntry;
use crate::dbengine::format::{FileKind, pair_name};

/// The deletion's attempts at taking the file: C gives up after 30 retries, one second apart.
const DELETION_ATTEMPTS: usize = 30;

/// The pause between the deletion's attempts (none in tests).
fn deletion_pause() {
    #[cfg(not(test))]
    std::thread::sleep(std::time::Duration::from_secs(1));
}

/// `database_rotate_tp_worker()`: the tier's oldest pair deleted, the registry updated unless the tier is shutting
/// down, then the rotation hook, whether the deletion happened or gave up.
pub(crate) fn database_rotate(e: &Dbengine, tier: usize) {
    let td = &e.tiers[tier];
    if let Some(df) = td.first_file() {
        datafile_delete(e, tier, &df, !td.quiesced());
    }
    if let Some(hook) = &e.rotation {
        (hook.0)();
    }
}

/// The uses a deletion waits for, as C counts them (`datafile->users.lockers`, the open cache's included).
fn users_of(td: &TierData, df: &DataFile) -> u32 {
    df.lockers(None) + td.open_lockers(df)
}

/// `datafile_delete()`: once the pair can be taken (after at most 30 retries a second apart, else it stays pending
/// for the next rotation), the registry's first times are recalculated from the pairs after it, then the pair leaves
/// the tier and its v2 index, `.njf` and data file are unlinked, and the bytes removed are given back.
pub(crate) fn datafile_delete(
    e: &Dbengine,
    tier: usize,
    df: &Arc<DataFile>,
    update_retention: bool,
) {
    let td = &e.tiers[tier];
    let fileno = df.fileno;
    let name = pair_name(fileno);
    let mut acquired = td.acquire_for_deletion(df);
    let mut attempts = 0;
    while !acquired {
        acquired = td.acquire_for_deletion(df);
        if acquired {
            break;
        }
        attempts += 1;
        let users = users_of(td, df);
        if attempts >= DELETION_ATTEMPTS {
            netdata_log_error!(
                "DBENGINE: tier {tier}: {name} could not be acquired for deletion after {attempts} attempts \
                 ({users} lockers remain) - will retry on next rotation"
            );
            return;
        }
        netdata_log_info!(
            "DBENGINE: tier {tier}: waiting for {name} to be available for deletion, in use by {users} users."
        );
        deletion_pause();
    }
    if update_retention {
        update_metrics_first_time_s(e, tier, df, td.next_fileno(fileno));
    }
    netdata_log_info!("DBENGINE: tier {tier}: deleting {name} to maintain disk quota.");
    td.remove_file(fileno);
    let (data_bytes, journal_bytes) = (df.pos(), df.journal_pos());
    let expect_v2 = df.v2_available();
    let v2_bytes = td.remove_v2(fileno).map_or(0, |index| index.size);
    // journalfile_destroy_unsafe(): the v2 index, then the v1 journal; destroy_data_file_unsafe(): the data file
    let del_njfv2 = super::io::unlink_if_exists(&td.config.file(FileKind::JournalV2, fileno));
    let del_njf = super::io::unlink_if_exists(&td.config.file(FileKind::Journal, fileno));
    let del_ndf = super::io::unlink(&td.config.file(FileKind::Datafile, fileno));
    td.open_mut().remove_file(fileno);
    let reclaimed = u64::from(del_ndf) * data_bytes
        + u64::from(del_njf) * journal_bytes
        + u64::from(del_njfv2) * v2_bytes;
    td.sub_disk_space(reclaimed);
    let size = size_to_string(reclaimed, "B", false).unwrap_or_default();
    if del_ndf && del_njf && del_njfv2 {
        netdata_log_info!(
            "DBENGINE: tier {tier}: deleted {name} (.ndf, .njf, .njfv2), reclaimed {size}."
        );
    } else if del_ndf && del_njf && !expect_v2 {
        netdata_log_info!("DBENGINE: tier {tier}: deleted {name} (.ndf, .njf), reclaimed {size}.");
    } else if del_ndf || del_njf || del_njfv2 {
        let removed = [(del_ndf, ".ndf"), (del_njf, ".njf"), (del_njfv2, ".njfv2")]
            .iter()
            .filter(|(done, _)| *done)
            .map(|(_, ext)| *ext)
            .collect::<Vec<_>>()
            .join(", ");
        let failed = [
            (!del_ndf, ".ndf"),
            (!del_njf, ".njf"),
            (expect_v2 && !del_njfv2, ".njfv2"),
        ]
        .iter()
        .filter(|(missing, _)| *missing)
        .map(|(_, ext)| *ext)
        .collect::<Vec<_>>()
        .join(", ");
        if failed.is_empty() {
            netdata_log_info!(
                "DBENGINE: tier {tier}: deleted {name} ({removed}), reclaimed {size}."
            );
        } else {
            netdata_log_error!(
                "DBENGINE: tier {tier}: partial delete of {name} - removed: {removed}, failed: {failed}, reclaimed \
                 {size}."
            );
        }
    } else {
        netdata_log_error!(
            "DBENGINE: tier {tier}: failed to delete {name} to maintain disk quota."
        );
    }
}

/// A metric of the deleted file whose first time is recalculated (`struct uuid_first_time_s`).
struct Rotated {
    handle: Handle,
    first_time_s: i64,
    pages_found: u64,
    df_matched: u32,
}

/// `update_metrics_first_time_s()`: the deleted file's metrics that the registry holds get the first time the
/// remaining files give them (`find_uuid_first_time()`), or, found nowhere, what the main cache holds of them, and
/// leave the registry when that is nothing and nobody holds them; the tier's first time becomes the remaining files'
/// oldest start. Nothing without the deleted file's v2 index, and nothing more once the tier shuts down.
fn update_metrics_first_time_s(e: &Dbengine, tier: usize, deleted: &DataFile, next: Option<u32>) {
    let td = &e.tiers[tier];
    let Some(index) = td.v2_of(deleted.fileno) else {
        return;
    };
    let mut rotated = Vec::new();
    let walked = super::v2index::walk_metric_list(&index.file, &index.header, |_, m| {
        if let Some(handle) = e.mrg.get_and_acquire(&m.uuid, tier) {
            rotated.push(Rotated {
                handle,
                first_time_s: i64::MAX,
                pages_found: 0,
                df_matched: 0,
            });
        }
    });
    netdata_log_info!(
        "DBENGINE: tier {tier}: recalculating retention for {} metrics starting with datafile {}",
        index.header.metric_count,
        next.unwrap_or(0)
    );
    // an unreadable list (C's SIGBUS) releases what it took
    if walked.is_err() {
        return;
    }
    let global_first_time_s = find_uuid_first_time(td, next, &mut rotated);
    if td.quiesced() {
        return;
    }
    netdata_log_info!(
        "DBENGINE: tier {tier}: updating metrics registry retention for {} metrics",
        rotated.len()
    );
    for r in rotated {
        if td.quiesced() {
            continue;
        }
        if r.first_time_s != i64::MAX {
            let old = r.handle.first_time_s();
            if r.handle.set_first_time_s_if_bigger(r.first_time_s) {
                let samples = retention_samples_delta(
                    tier,
                    old,
                    r.first_time_s,
                    r.handle.update_every_s(),
                    "advancing metric first retention time",
                );
                td.sub_samples_saturating(samples, "advancing metric first retention time");
            }
        } else if !has_zero_disk_retention(e, &r.handle) {
            let samples = retention_samples_delta(
                tier,
                r.handle.first_time_s(),
                r.handle.latest_time_s(),
                r.handle.update_every_s(),
                "deleting a metric with zero disk retention",
            );
            td.sub_samples_saturating(samples, "deleting a metric with zero disk retention");
            r.handle.release();
        }
    }
    if td.quiesced() {
        return;
    }
    if global_first_time_s != i64::MAX {
        td.set_first_time_s(global_first_time_s);
    }
}

/// `find_uuid_first_time()`: the files from `next` on, each taken for the retention walk, give each rotated metric
/// the earliest start of its pages in the first four files that hold it (while it has fewer than six pages there),
/// and the open cache its first page; the oldest file start (`global_first_time_s`), `i64::MAX` without one. C's
/// walk skips a metric that is the last entry of a file's list, which here is recorded (D30).
fn find_uuid_first_time(td: &TierData, next: Option<u32>, rotated: &mut [Rotated]) -> i64 {
    let mut global_first_time_s = i64::MAX;
    let Some(mut file) = next.and_then(|n| td.acquire_from(n, Reason::Retention)) else {
        return global_first_time_s;
    };
    loop {
        let index = td.v2_of(file.fileno).filter(|_| file.v2_available());
        let mut any_matching = true;
        let mut listed = true;
        if let Some(index) = index {
            let start_s = index.start_time_s();
            if start_s > 0 && start_s < global_first_time_s {
                global_first_time_s = start_s;
            }
            let mut list: Vec<MetricEntry> = Vec::with_capacity(index.header.metric_count as usize);
            listed =
                super::v2index::walk_metric_list(&index.file, &index.header, |_, m| list.push(*m))
                    .is_ok();
            if listed {
                any_matching = false;
                let mut search_start = 0;
                for r in rotated.iter_mut() {
                    if r.df_matched > 3 || r.pages_found > 5 {
                        continue;
                    }
                    any_matching = true;
                    if search_start >= list.len() {
                        break;
                    }
                    let uuid = r.handle.uuid();
                    // C's direct compare, else its bsearch of the rest: a miss leaves the search where it was
                    let at =
                        search_start + list[search_start..].partition_point(|m| &m.uuid < uuid);
                    let Some(m) = list.get(at).filter(|m| &m.uuid == uuid) else {
                        continue;
                    };
                    search_start = at + 1;
                    r.pages_found += u64::from(m.entries);
                    r.df_matched += 1;
                    r.first_time_s = r.first_time_s.min(i64::from(m.delta_start_s) + start_s);
                    if td.quiesced() {
                        return global_first_time_s;
                    }
                }
            }
        }
        let following = td.acquire_from(file.fileno + 1, Reason::Retention);
        drop(file);
        // a file without an index, or whose list could not be read, does not end the walk
        if index_ends_walk(listed, any_matching) {
            break;
        }
        match following {
            Some(f) => file = f,
            None => break,
        }
    }
    // the open cache: each metric's first page (PGC_SEARCH_FIRST)
    let open = td.open();
    for r in rotated.iter_mut() {
        if let Some((&start, _)) = open
            .pages(r.handle.uuid())
            .and_then(|p| p.first_key_value())
        {
            r.first_time_s = r.first_time_s.min(start);
        }
    }
    global_first_time_s
}

/// A walked file whose list gave no metric still to look for ends the walk.
fn index_ends_walk(listed: bool, any_matching: bool) -> bool {
    listed && !any_matching
}

/// `mrg_metric_has_zero_disk_retention()`: a metric the remaining files do not hold takes its retention from the
/// main cache (its hot and dirty pages' earliest start, its dirty pages' latest end), looking again up to five times
/// while it has none and is being collected; whether that leaves it any retention.
fn has_zero_disk_retention(e: &Dbengine, handle: &Handle) -> bool {
    let mut countdown = 5;
    loop {
        let (first, end) = e.main.metric_span(handle.tier(), handle.uuid());
        countdown -= 1;
        if countdown != 0 && first == 0 && handle.latest_hot_time_s() != 0 {
            continue;
        }
        handle.set_disk_retention(first, end);
        break;
    }
    let r = handle.retention();
    r.first_time_s != 0 && r.last_time_s != 0 && r.first_time_s < r.last_time_s
}

/// `rrdeng_retention_samples_delta()`: the intervals of `update_every_s` from `first` to `last`; 0 for an empty or
/// unknown interval, and for an inverted one, which is recorded (at most once a minute).
pub(crate) fn retention_samples_delta(
    tier: usize,
    first: i64,
    last: i64,
    update_every_s: u32,
    reason: &str,
) -> u64 {
    static INVERTED: ErrorLimit = ErrorLimit::new(60, 0);
    if update_every_s == 0 || first == 0 || last == 0 || first == last {
        return 0;
    }
    if first > last {
        nd_log_limit!(
            &INVERTED,
            Source::Daemon,
            Priority::Err,
            "DBENGINE: tier {tier}: invalid retention interval while {reason} (first={first}, last={last}, \
             update_every={update_every_s}); not updating sample counter"
        );
        return 0;
    }
    ((last - first) / i64::from(update_every_s)) as u64
}

#[cfg(test)]
mod tests;
