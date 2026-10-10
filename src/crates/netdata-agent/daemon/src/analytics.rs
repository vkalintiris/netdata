//! The analytics data `/api/v1/info` reports (`analytics_data` in `src/daemon/analytics.c`). C's other fields are
//! written for a submission no code reads, so they are not kept (D251 F5).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// `ANALYTICS_MAX_DASHBOARD_HITS`.
const MAX_DASHBOARD_HITS: u64 = 255;

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
        slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
