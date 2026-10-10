//! The analytics data `/api/v1/info` reports (`analytics_data` in `src/daemon/analytics.c`) and the ANALYTICS
//! thread that gathers it (`analytics_main()`). C's other fields are written for a submission no code reads, so they
//! are not kept (D251 F5); its thread runs whether anonymous statistics are on or off, as C's does.

use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_spawn::popen::Popen;
use netdata_agent_text::c::fgets_chunks;

use crate::heartbeat::{Phase, Thread};
use crate::{shutdown, v1_charts};

/// `ANALYTICS_MAX_DASHBOARD_HITS`.
const MAX_DASHBOARD_HITS: u64 = 255;
/// `ANALYTICS_INIT_SLEEP_SEC`, `ANALYTICS_INIT_IMMUTABLE_DATA_SEC` and `ANALYTICS_HEARTBEAT`, in ticks of a second.
const INIT_SLEEP_TICKS: u32 = 120;
const INIT_IMMUTABLE_DATA_TICKS: u32 = 10;
const HEARTBEAT_TICKS: u32 = 6 * 3600;
/// The `fgets()` buffer the script's output is read with: pieces of at most 199 bytes.
const METHODS_LINE: usize = 200;

/// What the ANALYTICS thread gathers and the requests count. The quoted texts are held as C stores them
/// (`analytics_set_data_str()` wraps the value in quotes; `None` is the bare `null` of `analytics_reset()`), so
/// `buffer_json_member_add_quoted_string()` prints them as C does.
#[derive(Debug, Default)]
pub struct Analytics {
    /// `netdata_exporting_connectors`.
    pub exporting_connectors: Mutex<Option<Vec<u8>>>,
    /// `netdata_notification_methods`.
    pub notification_methods: Mutex<Option<Vec<u8>>>,
    /// `charts_count` and `metrics_count`: localhost's, as the last mutable gather counted them.
    pub charts_count: AtomicU64,
    pub metrics_count: AtomicU64,
    /// `dashboard_hits`.
    pub dashboard_hits: AtomicU64,
}

/// Which of `analytics_main()`'s gathers is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gather {
    /// `analytics_gather_immutable_meta_data()`.
    Immutable,
    /// `analytics_gather_mutable_meta_data()`.
    Mutable,
}

impl Analytics {
    /// `analytics_log_dashboard()`: a dashboard's hello, counted up to 255 while anonymous statistics are on.
    pub fn log_dashboard(&self, statistics: bool) {
        if statistics {
            let _ = self
                .dashboard_hits
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| (n < MAX_DASHBOARD_HITS).then_some(n + 1));
        }
    }

    /// A quoted text under its lock; a panic while it was held does not make it unreadable.
    pub fn text(slot: &Mutex<Option<Vec<u8>>>) -> Option<Vec<u8>> {
        slot.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn store(slot: &Mutex<Option<Vec<u8>>>, text: Vec<u8>) {
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(text);
    }

    /// `analytics_gather_immutable_meta_data()`: of what it gathers only the exporting connectors are reported, and
    /// without an exporting engine they are none (`analytics_exporters()` stores the empty text).
    pub fn gather_immutable(&self) {
        Self::store(&self.exporting_connectors, b"\"\"".to_vec());
    }

    /// `analytics_gather_mutable_meta_data()`: of what it gathers, localhost's charts and metrics, then the
    /// notification methods (the counts are written before the script runs, as in C).
    pub fn gather_mutable(&self, localhost: &Host, plugins_dir: &str) {
        let (charts, metrics) = counts(localhost);
        self.charts_count.store(charts, Ordering::Relaxed);
        self.metrics_count.store(metrics, Ordering::Relaxed);
        if let Some(methods) = notification_methods(plugins_dir) {
            Self::store(&self.notification_methods, methods);
        }
    }
}

/// `analytics_charts()` and `analytics_metrics()`: the charts available for viewers and their dimensions that are
/// neither hidden nor obsolete.
fn counts(host: &Host) -> (u64, u64) {
    let (mut charts, mut metrics) = (0, 0);
    for st in host.charts().all() {
        if v1_charts::available_for_viewers(&st) {
            charts += 1;
            metrics += st.dims().iter().filter(|rd| v1_charts::dimension_visible(rd)).count() as u64;
        }
    }
    (charts, metrics)
}

/// `analytics_alarms_notifications()`: the methods `alarm-notify.sh dump_methods` prints, as C stores them; none when
/// the script cannot be read, so the stored text stays as it was. A spawn that fails, or a child without an output,
/// stores the empty text.
fn notification_methods(plugins_dir: &str) -> Option<Vec<u8>> {
    let script = format!("{plugins_dir}/alarm-notify.sh");
    if let Err(errno) = nix::unistd::access(script.as_str(), nix::unistd::AccessFlags::R_OK) {
        nd_log!(Source::Daemon, Priority::Info, errno = errno as i32; "Alarm notify script {script} not found.");
        return None;
    }
    let mut stdout = Vec::new();
    if let Some(mut child) = Popen::run_shell(&format!("{script} dump_methods")) {
        let read = match child.stdout() {
            Some(out) => {
                let _ = out.read_to_end(&mut stdout);
                true
            }
            None => false,
        };
        // the exit code is not looked at
        if read {
            let _ = child.wait();
        } else {
            let _ = child.kill(0, &|| false);
        }
    }
    Some(stored_methods(&stdout))
}

/// The text C stores of the script's output: `fgets()`'s pieces, each cut at its newline or at a NUL, joined by `|`,
/// in quotes. A line of 199 bytes or more arrives in pieces (one of exactly 199 then gives an empty one).
fn stored_methods(stdout: &[u8]) -> Vec<u8> {
    let mut text = vec![b'"'];
    for (i, piece) in fgets_chunks(stdout, METHODS_LINE).enumerate() {
        if i > 0 {
            text.push(b'|');
        }
        let end = piece.iter().position(|&c| c == b'\n' || c == 0).unwrap_or(piece.len());
        text.extend_from_slice(&piece[..end]);
    }
    text.push(b'"');
    text
}

/// `analytics_main()`'s two loops over a tick source: the immutable gather after the 10th tick, the mutable one after
/// the 121st and then after every 21,600 more. The first loop tests `running` before each tick, the second after it;
/// a stop ends both.
pub fn run(running: impl Fn() -> bool, mut tick: impl FnMut(), mut gather: impl FnMut(Gather)) {
    let mut ticks = 0;
    while running() && ticks <= INIT_SLEEP_TICKS {
        tick();
        ticks += 1;
        if ticks == INIT_IMMUTABLE_DATA_TICKS {
            gather(Gather::Immutable);
        }
    }
    if !running() {
        return;
    }
    gather(Gather::Mutable);
    ticks = 0;
    loop {
        tick();
        ticks += 1;
        if !running() {
            break;
        }
        if ticks < HEARTBEAT_TICKS {
            continue;
        }
        gather(Gather::Mutable);
        ticks = 0;
    }
}

/// Starts `ANALYTICS`, which gathers into `analytics` on a tick a second until the exit starts; the mutable gather
/// reads `hosts`' localhost and runs the script of `plugins_dir`, the primary plugins directory.
pub fn spawn(
    analytics: Arc<Analytics>,
    hosts: Arc<Hosts>,
    plugins_dir: String,
    stack_size: usize,
) -> io::Result<Thread> {
    Thread::spawn("ANALYTICS", stack_size, Duration::from_secs(1), Phase::Randomized, move |ticker| {
        // service_running(SERVICE_ANALYTICS): false once the exit starts
        let running = || ticker.running() && !shutdown::exiting();
        let tick = || {
            let _ = ticker.next();
        };
        run(running, tick, |gather| match gather {
            Gather::Immutable => analytics.gather_immutable(),
            Gather::Mutable => analytics.gather_mutable(hosts.localhost(), &plugins_dir),
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// The counter stops at 255 and counts nothing while anonymous statistics are off.
    #[test]
    fn the_dashboard_counter_stops_at_its_cap_and_follows_the_flag() {
        let a = Analytics::default();
        a.log_dashboard(false);
        assert_eq!(a.dashboard_hits.load(Ordering::Relaxed), 0);
        for _ in 0..256 {
            a.log_dashboard(true);
        }
        assert_eq!(a.dashboard_hits.load(Ordering::Relaxed), 255);
        a.log_dashboard(false);
        assert_eq!(a.dashboard_hits.load(Ordering::Relaxed), 255);
    }

    /// The gathers `run()` makes, with the tick after which each came, when it stops after `stop_after` ticks.
    fn gathers(stop_after: u32) -> Vec<(u32, Gather)> {
        let ticks = Cell::new(0u32);
        let mut seen = Vec::new();
        run(|| ticks.get() < stop_after, || ticks.set(ticks.get() + 1), |g| seen.push((ticks.get(), g)));
        seen
    }

    /// `analytics_main()`'s schedule: the immutable gather after exactly 10 ticks, the first mutable one after
    /// exactly 121 (none when the stop comes with the 121st), the next after exactly 21,600 more.
    #[test]
    fn the_gathers_follow_c_s_schedule() {
        use Gather::{Immutable, Mutable};
        assert!(gathers(9).is_empty());
        assert_eq!(gathers(10), [(10, Immutable)]);
        assert_eq!(gathers(121), [(10, Immutable)]);
        assert_eq!(gathers(122), [(10, Immutable), (121, Mutable)]);
        assert_eq!(gathers(121 + 21_600), [(10, Immutable), (121, Mutable)]);
        assert_eq!(gathers(121 + 21_600 + 1), [(10, Immutable), (121, Mutable), (21_721, Mutable)]);
    }

    /// The counts take the charts available for viewers and, of theirs, the dimensions neither hidden nor obsolete.
    #[test]
    fn the_counts_follow_the_viewers_predicates() {
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType, dim_flags, flags as chart_flags};
        use netdata_agent_rrd::mode::DbMode;
        let host = Host::new("0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e", true, crate::testing::host_info("box"));
        let chart = |id: &str| {
            let (st, _) = host.charts().create(&ChartSpec {
                type_: "t",
                id,
                name: None,
                family: None,
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
            });
            for d in ["a", "b", "c"] {
                st.dim_add(d, None, 1, 1, Algorithm::Absolute);
            }
            st
        };
        let shown = chart("shown");
        let dims = shown.dims();
        dims[0].update_meta(|m| m.flags |= dim_flags::HIDDEN);
        dims[1].update_meta(|m| m.flags |= dim_flags::OBSOLETE);
        chart("whole");
        chart("hidden").flags_set_and_clear(chart_flags::HIDDEN, 0);
        let a = Analytics::default();
        a.gather_mutable(&host, "/nonexistent-plugins-dir");
        assert_eq!((a.charts_count.load(Ordering::Relaxed), a.metrics_count.load(Ordering::Relaxed)), (2, 4));
    }

    /// The text stored of the script's output: lines joined by `|`, none giving the empty text, a line of 199 bytes
    /// or more in pieces of 199, an empty line as an empty piece, a NUL cutting its piece; a line `null` is a text.
    #[test]
    fn the_methods_are_stored_as_fgets_reads_them() {
        let stored = |out: &[u8]| String::from_utf8(stored_methods(out)).unwrap();
        assert_eq!(stored(b""), "\"\"");
        assert_eq!(stored(b"email\nslack\n"), "\"email|slack\"");
        assert_eq!(stored(b"email\nslack"), "\"email|slack\"");
        assert_eq!(stored(b"a\n\nb\n"), "\"a||b\"");
        assert_eq!(stored(b"ab\0cd\nef\n"), "\"ab|ef\"");
        assert_eq!(stored(b"null\n"), "\"null\"");
        let (a198, a199, a250) = ("a".repeat(198), "a".repeat(199), "a".repeat(250));
        assert_eq!(stored(format!("{a198}\nb\n").as_bytes()), format!("\"{a198}|b\""));
        assert_eq!(stored(format!("{a199}\nb\n").as_bytes()), format!("\"{a199}||b\""));
        assert_eq!(stored(format!("{a250}\n").as_bytes()), format!("\"{a199}|{}\"", "a".repeat(51)));
    }

    /// A directory without the script leaves the stored text as it was and writes C's info record, with the errno.
    #[test]
    fn a_missing_script_keeps_the_stored_methods() {
        let host = Host::new("0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e", true, crate::testing::host_info("box"));
        let a = Analytics::default();
        let ((), records) = netdata_agent_log::capture(|| a.gather_mutable(&host, "/nonexistent-plugins-dir"));
        assert_eq!(Analytics::text(&a.notification_methods), None);
        let records: Vec<_> = records.iter().map(|r| (r.priority, r.errno, r.message.clone())).collect();
        let message = "Alarm notify script /nonexistent-plugins-dir/alarm-notify.sh not found.";
        assert_eq!(records, [(Priority::Info, nix::errno::Errno::ENOENT as i32, Some(message.to_owned()))]);
        Analytics::store(&a.notification_methods, b"\"email\"".to_vec());
        let ((), _) = netdata_agent_log::capture(|| a.gather_mutable(&host, "/nonexistent-plugins-dir"));
        assert_eq!(Analytics::text(&a.notification_methods), Some(b"\"email\"".to_vec()));
    }

    /// The immutable gather sets the exporting connectors to the empty text, which they are without an engine.
    #[test]
    fn the_immutable_gather_sets_the_empty_connectors() {
        let a = Analytics::default();
        assert_eq!(Analytics::text(&a.exporting_connectors), None);
        a.gather_immutable();
        assert_eq!(Analytics::text(&a.exporting_connectors), Some(b"\"\"".to_vec()));
    }
}
