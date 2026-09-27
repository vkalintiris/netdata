//! The `RRDCONTEXT` thread, ported from `rrdcontext_main()` (`src/database/contexts/rrdcontext-worker.c`): once a
//! second, the deep pass a dbengine rotation armed, then every host's contexts post-processed.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use netdata_agent_inicfg::Config;
use netdata_agent_rrd::contexts;
use netdata_agent_rrd::host::{Host, Hosts};

use crate::heartbeat::{Phase, Thread};

/// `RRDCONTEXT_WORKER_THREAD_HEARTBEAT_USEC`.
const HEARTBEAT: Duration = Duration::from_secs(1);

/// `now_realtime_usec()`.
pub fn now_realtime_ut() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// `rrdcontext_main()`'s `[db] extreme cardinality protection` (on by default with more than one tier in dbengine
/// mode), `keep instances` (1000, 1 to 1,000,000) and `min ephemerality` (50, 0 to 100), out-of-range values written
/// back with C's record.
pub fn extreme_cardinality_settings(c: &mut Config, default_on: bool) -> (bool, usize, usize) {
    let enabled = c.get_boolean("db", "extreme cardinality protection", default_on);
    let keep = c.get_number_range(
        "db",
        "extreme cardinality keep instances",
        1000,
        1,
        1_000_000,
    );
    let min = c.get_number_range("db", "extreme cardinality min ephemerality", 50, 0, 100);
    (enabled, keep as usize, min as usize)
}

/// Starts `RRDCONTEXT`. `settings` runs first on the thread (C reads the extreme cardinality keys there);
/// `delete_from_sql` removes a context the deep pass collected from the host's context database
/// (`rrdcontext_delete_from_sql_unsafe()`).
pub fn spawn(
    hosts: Arc<Hosts>,
    stack_size: usize,
    settings: impl FnOnce() + Send + 'static,
    delete_from_sql: impl Fn(&Host, &str, u64) + Send + 'static,
) -> std::io::Result<Thread> {
    Thread::spawn(
        "RRDCONTEXT",
        stack_size,
        HEARTBEAT,
        Phase::Randomized,
        move |ticker| {
            settings();
            let running = || ticker.running();
            while ticker.next() {
                contexts::deep_pass(&hosts, now_realtime_ut(), &running, |host, id, version| {
                    delete_from_sql(host, id, version)
                });
                // a host whose contexts are still loading waits for the load, as in C
                for host in hosts.all().iter().filter(|h| !h.is_pending_context_load()) {
                    if !running() {
                        break;
                    }
                    host.contexts().worker_cycle_while(&running);
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys' defaults (the protection on only as the caller says), and C's clamps with their records.
    #[test]
    fn extreme_cardinality_keys_read_as_c() {
        assert_eq!(
            extreme_cardinality_settings(&mut Config::new(), false),
            (false, 1000, 50)
        );
        assert_eq!(
            extreme_cardinality_settings(&mut Config::new(), true),
            (true, 1000, 50)
        );
        let mut c = Config::new();
        c.set("db", "extreme cardinality protection", "no");
        c.set("db", "extreme cardinality keep instances", "0");
        c.set("db", "extreme cardinality min ephemerality", "101");
        let (settings, records) =
            netdata_agent_log::capture(|| extreme_cardinality_settings(&mut c, true));
        assert_eq!(settings, (false, 1, 100));
        assert_eq!(
            records
                .into_iter()
                .filter_map(|r| r.message)
                .collect::<Vec<_>>(),
            [
                "CONFIG: out of range [db].extreme cardinality keep instances = 0. Acceptable values: 1 to 1000000 \
                 inclusive. Setting it to 1",
                "CONFIG: out of range [db].extreme cardinality min ephemerality = 101. Acceptable values: 0 to 100 \
                 inclusive. Setting it to 100"
            ]
        );
    }
}
