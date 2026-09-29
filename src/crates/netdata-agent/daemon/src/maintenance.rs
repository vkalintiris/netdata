//! The maintenance, ported from `run_maintenace()` (`src/daemon/service.c`), which METASYNC's store job runs first
//! and at most every 10 s: obsolete dimensions and charts are freed once quiet for `[db] cleanup obsolete charts
//! after`, the charts of a child gone that long are marked obsolete, then the orphan children are archived after
//! `cleanup orphan hosts after`, or freed when ephemeral past `cleanup ephemeral hosts after` or archived without
//! retention (D94).

use std::sync::atomic::{AtomicI64, Ordering};

use netdata_agent_log::netdata_log_info;
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_rrd::chart::{Chart, Dim, dim_flags, flags as chart_flags};
use netdata_agent_rrd::clock::now_realtime_ut;
use netdata_agent_rrd::host::{Host, Hosts, pending_flags};

use crate::meta_store;

/// `SERVICE_HEARTBEAT`: the least time between two runs.
const EVERY_S: i64 = 10;

/// `next_maintenance_check` of `start_metadata_hosts()`.
#[derive(Debug, Default)]
pub struct Schedule(AtomicI64);

impl Schedule {
    /// Runs the maintenance when `now_s` is past the next check, which then moves 10 s later.
    pub fn run_if_due(&self, hosts: &Hosts, meta: Option<&MetaDb>, now_s: i64) {
        if now_s > self.0.load(Ordering::Relaxed) {
            run(hosts, meta, now_s);
            self.0
                .store(now_s.saturating_add(EVERY_S), Ordering::Relaxed);
        }
    }
}

/// `run_maintenace()` at `now_s` (C reads the clock in each step; one run takes milliseconds).
pub fn run(hosts: &Hosts, meta: Option<&MetaDb>, now_s: i64) {
    cleanup_obsolete_charts_from_all_hosts(hosts, now_s);
    cleanup_orphan_hosts(hosts, meta, hosts.localhost(), now_s);
}

/// `svc_rrd_cleanup_obsolete_charts_from_all_hosts()`: each host's sweep, then the obsolete-all of a child gone; a
/// deep contexts pass is asked for when anything was archived, so that hosts without the dbengine drop the archived
/// entries too.
fn cleanup_obsolete_charts_from_all_hosts(hosts: &Hosts, now_s: i64) {
    let obsolete_s = hosts.storage().cleanup_times().obsolete_charts_s;
    let mut archived = 0;
    for host in hosts.all() {
        archived += cleanup_charts_marked_obsolete(&host, obsolete_s, now_s);
        if host.is_localhost() || host.is_virtual_host_os() {
            continue;
        }
        host.obsolete_all_if_gone(now_s, obsolete_s);
    }
    if archived > 0 {
        hosts
            .storage()
            .db_rotation()
            .request_full_gc(now_realtime_ut());
    }
}

/// `svc_rrdhost_cleanup_charts_marked_obsolete()`: of a host with pending obsolete work, the obsolete dimensions and
/// the quiet obsolete charts are freed; a chart replicating, or with a dimension still collected, waits, and the host
/// keeps its pending bit for the next sweep. The dimensions and charts freed.
fn cleanup_charts_marked_obsolete(host: &Host, obsolete_s: i64, now_s: i64) -> usize {
    if host.take_pending_flags() & (pending_flags::OBSOLETE_CHARTS | pending_flags::OBSOLETE_DIMENSIONS) == 0 {
        return 0;
    }
    let (mut full_candidates, mut full_archives) = (0, 0);
    let (mut partial_candidates, mut partial_archives) = (0, 0);
    let mut archived = 0;
    let charts = host.charts();
    for chart in charts.all() {
        let flags = chart.flags();
        let replicating = chart_flags::is_replicating(flags);
        if flags & chart_flags::OBSOLETE_DIMENSIONS != 0 {
            partial_candidates += 1;
            if !replicating {
                archived += archive_obsolete_dimensions(&chart, false, obsolete_s, now_s);
                if chart.flags() & chart_flags::OBSOLETE_DIMENSIONS == 0 {
                    partial_archives += 1;
                }
            }
        }
        if flags & chart_flags::OBSOLETE != 0 {
            full_candidates += 1;
            if !replicating && quiet(&chart, obsolete_s, now_s) {
                archived += archive_obsolete_dimensions(&chart, true, obsolete_s, now_s);
                if chart.flags() & chart_flags::OBSOLETE_DIMENSIONS == 0
                    && charts.free_if(&chart, |c| quiet(c, obsolete_s, now_s))
                {
                    full_archives += 1;
                    archived += 1;
                }
            }
        }
    }
    if partial_archives != partial_candidates {
        host.raise_pending_flags(pending_flags::OBSOLETE_DIMENSIONS);
    }
    if full_archives != full_candidates {
        host.raise_pending_flags(pending_flags::OBSOLETE_CHARTS);
    }
    archived
}

/// `svc_rrdset_lock_for_deletion()`'s test: an obsolete chart not accessed, updated or collected for `obsolete_s`.
fn quiet(chart: &Chart, obsolete_s: i64, now_s: i64) -> bool {
    let collection = chart.collection();
    chart.last_accessed_s().saturating_add(obsolete_s) < now_s
        && collection.last_updated.0.saturating_add(obsolete_s) < now_s
        && collection.last_collected.0.saturating_add(obsolete_s) < now_s
        && chart.flags() & chart_flags::OBSOLETE != 0
}

/// `svc_rrdset_archive_obsolete_dimensions()`: the chart's obsolete dimensions (every one, for an obsolete chart) not
/// collected for `obsolete_s` are freed; `OBSOLETE_DIMENSIONS` is raised again while a candidate remains. The number
/// freed.
fn archive_obsolete_dimensions(chart: &Chart, chart_obsolete: bool, obsolete_s: i64, now_s: i64) -> usize {
    if !chart.take_obsolete_dimensions() && !chart_obsolete {
        return 0;
    }
    let candidate = |dim: &Dim| chart_obsolete || dim.meta().flags & dim_flags::OBSOLETE != 0;
    let uncollected = |dim: &Dim| dim.collection().last_collected_time.0.saturating_add(obsolete_s) < now_s;
    // C's destroy_lock, held across its loop: rechecked under the dimensions' lock, which a revive takes too
    let still = |dim: &Dim| {
        uncollected(dim)
            && (dim.meta().flags & dim_flags::OBSOLETE != 0
                || chart_obsolete
                    && chart.flags() & chart_flags::OBSOLETE != 0
                    && chart.last_accessed_s().saturating_add(obsolete_s) < now_s)
    };
    let (mut candidates, mut archives) = (0, 0);
    for dim in chart.dims() {
        if !candidate(&dim) {
            continue;
        }
        candidates += 1;
        if uncollected(&dim) && chart.free_dim_if(&dim, still) {
            archives += 1;
        }
    }
    if archives != candidates {
        chart.raise_obsolete_dimensions();
    }
    archives
}

/// `svc_rrdhost_cleanup_orphan_hosts()`: a host that should be cleaned up is archived, or freed when it is ephemeral
/// past its time or archived without retention; a host whose metadata is being stored waits for the next run. The
/// hosts' write lock is held throughout, as `rrd_wrlock()`.
fn cleanup_orphan_hosts(hosts: &Hosts, meta: Option<&MetaDb>, protected: &Host, now_s: i64) {
    let ephemeral_s = hosts.storage().cleanup_times().ephemeral_hosts_s;
    let mut locked = hosts.write();
    for host in locked.all() {
        if !host.should_be_cleaned_up(protected, now_s) {
            continue;
        }
        let mut delete = ephemeral_s != 0
            && now_s - host.receiver_last_disconnected_s() > ephemeral_s
            && host.is_ephemeral();
        if !delete && host.is_archived() {
            // archived already: freed once it has no retention left
            if host.contexts().retention() != (0, 0) {
                continue;
            }
            delete = true;
        }
        let Some(mut freed) = host.metadata_try_write() else {
            continue;
        };
        if delete {
            netdata_log_info!(
                "Host '{}' with machine guid '{}' is archived, ephemeral clean up.",
                host.hostname(),
                host.machine_guid()
            );
            // the node info and update sends: the agent is never claimed (D61.3); unregister_node() runs here, as
            // ACLKSYNC is not ported
            if let (Some(meta), Some(id)) = (meta, meta_store::host_id(&host)) {
                meta.unregister_node(&id);
            }
            *freed = true;
            drop(freed);
            locked.free(&host);
        } else {
            host.cleanup_data_collection();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
    use netdata_agent_rrd::clock::now_realtime_s;
    use netdata_agent_rrd::host::{Attach, HostInfo, ReceiverLink, ReceiverSlot};
    use netdata_agent_rrd::mode::DbMode;
    use netdata_agent_rrd::storage::CleanupTimes;

    use super::*;

    const LOCALHOST: &str = "5a1e0000-0000-4000-8000-0000000000aa";
    const CHILD: &str = "5a1e0000-0000-4000-8000-00000000c005";

    fn info(hostname: &str) -> HostInfo {
        HostInfo {
            hostname: hostname.into(),
            registry_hostname: hostname.into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 3600,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        }
    }

    fn spec(id: &str) -> ChartSpec<'_> {
        ChartSpec {
            type_: "t",
            id,
            name: None,
            family: Some("f"),
            context: Some("t.ctx"),
            title: "T",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 5,
            page_size: 4096,
        }
    }

    /// The hosts with a child, and the cleanup times at 10 s (the minimum).
    fn hosts() -> (Hosts, Arc<Host>) {
        let hosts = Hosts::new(Host::new(LOCALHOST, true, info("parent")));
        hosts.storage().set_cleanup_times(CleanupTimes {
            obsolete_charts_s: 10,
            orphan_hosts_s: 10,
            ephemeral_hosts_s: 0,
        });
        let child = hosts.find_or_create(CHILD, DbMode::Ram, || info("child"), |_| {});
        (hosts, child)
    }

    /// An obsolete dimension not collected for the time is freed at once; an obsolete chart waits until it was not
    /// accessed for the time either, the host keeping its pending bit meanwhile; a free asks for a deep pass.
    #[test]
    fn obsolete_dimensions_and_charts_are_freed_once_quiet() {
        let (hosts, child) = hosts();
        // connected: no obsolete-all
        let slot = Arc::new(ReceiverSlot::new(0, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(child.set_receiver(slot), Attach::Attached);
        let (chart, _) = child.charts().create(&spec("c"));
        let (d1, _) = chart.dim_add("d1", None, 1, 1, Algorithm::Absolute);
        chart.dim_add("d2", None, 1, 1, Algorithm::Absolute);
        let now = now_realtime_s();
        chart.dim_is_obsolete(&d1);
        run(&hosts, None, now);
        assert!(d1.is_freed() && chart.dim("d1").is_none() && chart.dim("d2").is_some());
        assert_eq!(child.pending_flags(), 0, "all done");
        assert!(hosts.storage().db_rotation().due(u64::MAX).is_some(), "a deep pass asked for");
        chart.is_obsolete(&child);
        run(&hosts, None, now);
        assert!(!chart.is_freed(), "accessed within the time");
        assert_eq!(child.pending_flags(), pending_flags::OBSOLETE_CHARTS, "kept for the next sweep");
        run(&hosts, None, now + 11);
        assert!(chart.is_freed() && child.charts().find("t.c", true).is_none());
        assert_eq!(child.pending_flags(), 0);
    }

    /// A sweep that found the chart obsolete frees none of its dimensions once a revive cleared the flag (a CHART
    /// line meanwhile): the free rechecks under the dimensions' lock, as C's destroy_lock holds the loop (D95.2).
    #[test]
    fn a_revive_during_the_sweep_keeps_the_dimensions() {
        let (_hosts, child) = hosts();
        let (chart, _) = child.charts().create(&spec("c"));
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        let now = now_realtime_s();
        assert_eq!(archive_obsolete_dimensions(&chart, true, 10, now + 11), 0, "not obsolete");
        chart.is_obsolete(&child);
        chart.isnot_obsolete();
        assert_eq!(archive_obsolete_dimensions(&chart, true, 10, now + 11), 0, "revived");
        assert!(!dim.is_freed() && chart.dim("d").is_some());
        chart.is_obsolete(&child);
        assert_eq!(archive_obsolete_dimensions(&chart, true, 10, now), 0, "accessed within the time");
        assert_eq!(archive_obsolete_dimensions(&chart, true, 10, now + 11), 1);
        assert!(dim.is_freed());
    }

    /// A streamed vnode gone for the obsolete time keeps its charts: C skips virtual hosts' obsolete-all.
    #[test]
    fn a_vnode_gone_keeps_its_charts() {
        let (hosts, _) = hosts();
        let vnode = hosts.find_or_create(
            "5a1e0000-0000-4000-8000-00000000c006",
            DbMode::Ram,
            || HostInfo {
                os: netdata_agent_rrd::host::VIRTUAL_HOST_OS.into(),
                ..info("vnode")
            },
            |_| {},
        );
        let (chart, _) = vnode.charts().create(&spec("c"));
        let slot = Arc::new(ReceiverSlot::new(0, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(vnode.set_receiver(Arc::clone(&slot)), Attach::Attached);
        vnode.clear_receiver(&slot);
        run(&hosts, None, now_realtime_s() + 11);
        assert_eq!(chart.flags() & chart_flags::OBSOLETE, 0);
    }

    /// A child gone for the obsolete time has its charts marked obsolete; after the orphan time and more than 10 HEALTH
    /// passes it is archived (its charts freed), and, archived without retention, freed at the next run.
    #[test]
    fn a_child_gone_is_archived_then_freed() {
        let (hosts, child) = hosts();
        let (chart, _) = child.charts().create(&spec("c"));
        let slot = Arc::new(ReceiverSlot::new(0, Default::default(), ReceiverLink::default(), Box::new(|| {})));
        assert_eq!(child.set_receiver(Arc::clone(&slot)), Attach::Attached);
        child.clear_receiver(&slot);
        let now = now_realtime_s();
        run(&hosts, None, now + 11);
        assert!(chart.flags() & chart_flags::OBSOLETE != 0, "obsolete-all of a child gone");
        assert!(!child.is_archived(), "not before 10 HEALTH passes");
        for _ in 0..11 {
            hosts.storage().next_health_iteration();
        }
        assert!(child.should_be_cleaned_up(hosts.localhost(), now + 11));
        assert!(!child.should_be_cleaned_up(&child, now + 11), "the protected host");
        run(&hosts, None, now + 11);
        assert!(child.is_archived() && child.charts().all().is_empty());
        assert!(hosts.find_by_guid(CHILD).is_some(), "archived, still indexed");
        run(&hosts, None, now + 12);
        assert!(hosts.find_by_guid(CHILD).is_none(), "freed without retention");
    }
}
