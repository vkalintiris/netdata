//! A host's alerts (`host->rrdcalc_root_index`, each chart's alert list, the name index): `rrdcalc.c`.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;

use crate::Clock;
use crate::alert::{Alert, Run, Status};
use crate::prototype::Rule;

/// `RRDCALC_MAX_KEY_SIZE`: a key is cut to one byte less.
const MAX_KEY_SIZE: usize = 1024;

/// `HEALTH_ENTRY_FLAG_IS_REPEATING`.
pub const ENTRY_FLAG_IS_REPEATING: u32 = 0x0000_0080;

/// What a link or an unlink logs (`health_create_alarm_entry()`'s arguments).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    pub when: i64,
    pub duration: i64,
    pub old_value: f64,
    pub new_value: f64,
    pub old_status: Status,
    pub new_status: Status,
    pub delay: i32,
    pub flags: u32,
}

#[derive(Default)]
struct Store {
    /// The dictionary: insertion order, which is the order the loop evaluates in. An alert linked again takes a
    /// new place at the end.
    order: BTreeMap<u64, Arc<Alert>>,
    by_key: HashMap<Vec<u8>, u64>,
    next_seq: u64,
    /// `st->alerts.base`: a chart's alerts in link order, by chart id.
    by_chart: HashMap<String, Vec<Arc<Alert>>>,
    /// `host->rrdcalc_by_name`: the alerts of a name, in link order.
    by_name: HashMap<Vec<u8>, Vec<Arc<Alert>>>,
    /// The dictionary's version: every insert and delete moves it.
    version: u64,
    /// `host->health_log.next_alarm_id`: seeded with the wall clock's seconds when first asked.
    next_alarm_id: u32,
    /// What links and unlinks logged, for the tests: the alert's key and the entry.
    #[cfg(test)]
    transitions: Vec<(Vec<u8>, Transition)>,
}

impl Store {
    /// The entry a link or an unlink writes (`health_create_alarm_entry()`, `health_alarm_log_add_entry()`,
    /// `health_log_alert()`). The alert log comes with the health loop: until then a transition only takes the
    /// alert's next event id, as the entry would.
    fn log_transition(&mut self, _alert: &Alert, run: &mut Run, _transition: &Transition) {
        run.next_event_id = run.next_event_id.wrapping_add(1);
        #[cfg(test)]
        self.transitions.push((_alert.key.clone(), *_transition));
    }
}

/// A host's alerts.
#[derive(Default)]
pub struct HostAlerts {
    /// The host object these are the alerts of. A machine GUID can name another object: a host freed and created
    /// again, or one that lost the index to this one and is freed at once; what happens to that one is not this
    /// one's business.
    owner: Weak<Host>,
    /// `RRDHOST_FLAG_INITIALIZED_HEALTH`: set by the host's first pass and never cleared.
    initialized: AtomicBool,
    inner: Mutex<Store>,
}

/// `rrdcalc_key()`: `NAME,on[CHART_ID]`, cut as `snprintfz()` cuts it.
fn key(chart_id: &str, name: &[u8]) -> Vec<u8> {
    let mut key = [name, b",on[", chart_id.as_bytes(), b"]"].concat();
    key.truncate(MAX_KEY_SIZE - 1);
    key
}

impl HostAlerts {
    /// The alerts of `host`, none yet.
    pub(crate) fn of(host: &Arc<Host>) -> HostAlerts {
        HostAlerts { owner: Arc::downgrade(host), ..HostAlerts::default() }
    }

    /// Whether these are the alerts of that very host object.
    pub(crate) fn is_of(&self, host: &Host) -> bool {
        std::ptr::eq(self.owner.as_ptr(), host)
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the host's first pass ran; sets it.
    pub(crate) fn initialize(&self) -> bool {
        self.initialized.swap(true, Ordering::AcqRel)
    }

    /// The host's alerts in the dictionary's order: the order the loop evaluates them in.
    pub fn alerts(&self) -> Vec<Arc<Alert>> {
        self.store().order.values().cloned().collect()
    }

    /// The chart's alerts, in link order: those on that very chart object (C's list hangs on the chart), not on
    /// another one of its id.
    pub fn chart_alerts(&self, chart: &Chart) -> Vec<Arc<Alert>> {
        let mut alerts = self.store().by_chart.get(chart.id()).cloned().unwrap_or_default();
        alerts.retain(|alert| std::ptr::eq(Arc::as_ptr(&alert.chart), chart));
        alerts
    }

    /// The alerts of a name, in link order.
    pub fn by_name(&self, name: &[u8]) -> Vec<Arc<Alert>> {
        self.store().by_name.get(name).cloned().unwrap_or_default()
    }

    /// How many alerts the host has.
    pub fn count(&self) -> usize {
        self.store().order.len()
    }

    /// The dictionary's version.
    pub fn version(&self) -> u64 {
        self.store().version
    }

    /// `rrdcalc_add_from_prototype()`: the alert of `rule` on `chart`, created and linked, unless the chart has an
    /// alert of that name already (the first rule to give a key keeps it) or was freed meanwhile. True when linked.
    ///
    /// A key can still be held by an alert of an earlier chart object of that id, freed, whose free has not reached
    /// health yet. C's dictionary runs a chart's delete callback before a chart of that id can exist again, unless
    /// the old one is still referenced; here that alert goes now, as its chart's free would take it.
    pub(crate) fn add(&self, chart: &Arc<Chart>, rule: &Rule, clock: Clock) -> bool {
        let key = key(chart.id(), rule.config.name.as_deref().unwrap_or(b""));
        let mut store = self.store();
        if chart.is_freed() {
            return false;
        }
        if let Some(existing) = store.by_key.get(&key).and_then(|seq| store.order.get(seq)).cloned() {
            if Arc::ptr_eq(&existing.chart, chart) || !existing.chart.is_freed() {
                return false;
            }
            Self::unlink(&mut store, &existing, clock, false);
        }

        // C reads the clock for the alert's last status change, then for the alarm id's seed (below), then for
        // the link's entry
        let last_status_change = clock();

        // rrdcalc_get_unique_id(): the counter. The memory log's entry of this alert and the database's row go in
        // front of it with the loop and with SQLite.
        if store.next_alarm_id == 0 {
            store.next_alarm_id = clock() as u32;
        }
        let id = store.next_alarm_id;
        store.next_alarm_id = store.next_alarm_id.wrapping_add(1);

        let alert = Arc::new(Alert::new(key.clone(), chart, &rule.config, id, last_status_change));
        let seq = store.next_seq;
        store.next_seq += 1;
        store.order.insert(seq, Arc::clone(&alert));
        store.by_key.insert(key, seq);
        store.version += 1;
        store.by_name.entry(alert.name().to_vec()).or_default().push(Arc::clone(&alert));

        // rrdcalc_link_to_rrdset()
        store.by_chart.entry(chart.id().to_owned()).or_default().push(Arc::clone(&alert));
        let mut run = alert.run();
        let now = clock();
        let repeating = alert.config.warn_repeat_every > 0 || alert.config.crit_repeat_every > 0;
        let flags = if repeating { ENTRY_FLAG_IS_REPEATING } else { 0 };
        let transition = Transition {
            when: now,
            duration: now.saturating_sub(run.last_status_change).max(0),
            old_value: run.old_value,
            new_value: run.value,
            old_status: Status::Removed,
            new_status: run.status,
            delay: 0,
            flags,
        };
        store.log_transition(&alert, &mut run, &transition);
        true
    }

    /// `rrdcalc_unlink_and_delete()` of one alert, the store locked: out of the name index, its REMOVED entry
    /// unless it is REMOVED already or the agent is exiting, off its chart's list, out of the dictionary.
    fn unlink(store: &mut Store, alert: &Arc<Alert>, clock: Clock, exiting: bool) {
        if let Some(named) = store.by_name.get_mut(alert.name()) {
            named.retain(|other| !Arc::ptr_eq(other, alert));
            if named.is_empty() {
                store.by_name.remove(alert.name());
            }
        }

        if !exiting {
            let mut run = alert.run();
            if run.status != Status::Removed {
                let now = clock();
                let transition = Transition {
                    when: now,
                    duration: now.saturating_sub(run.last_status_change).max(0),
                    old_value: run.old_value,
                    new_value: run.value,
                    old_status: run.status,
                    new_status: Status::Removed,
                    delay: 0,
                    flags: 0,
                };
                store.log_transition(alert, &mut run, &transition);
            }
        }

        if let Some(linked) = store.by_chart.get_mut(alert.chart.id()) {
            linked.retain(|other| !Arc::ptr_eq(other, alert));
            if linked.is_empty() {
                store.by_chart.remove(alert.chart.id());
            }
        }
        if let Some(seq) = store.by_key.remove(&alert.key) {
            store.order.remove(&seq);
            store.version += 1;
        }
    }

    /// `rrdcalc_unlink_and_delete_all_rrdset_alerts()`: the alerts of that chart object go, in link order. Alerts
    /// on another chart object of its id (the chart defined again after this one was freed) stay.
    pub(crate) fn unlink_chart(&self, chart: &Chart, clock: Clock, exiting: bool) {
        let mut store = self.store();
        for alert in store.by_chart.get(chart.id()).cloned().unwrap_or_default() {
            if std::ptr::eq(Arc::as_ptr(&alert.chart), chart) {
                Self::unlink(&mut store, &alert, clock, exiting);
            }
        }
    }

    /// `rrdcalc_delete_all()`: every alert of the host goes, in the dictionary's order.
    pub(crate) fn delete_all(&self, clock: Clock, exiting: bool) {
        let mut store = self.store();
        for alert in store.order.values().cloned().collect::<Vec<_>>() {
            Self::unlink(&mut store, &alert, clock, exiting);
        }
    }

    /// `rrdcalc_rrdset_acquire_linked()`: the alert is in the store and its chart is still the host's chart of
    /// that id (obsolete or not).
    pub fn is_linked(&self, host: &Host, alert: &Arc<Alert>) -> bool {
        let stored = {
            let store = self.store();
            let stored = store.by_key.get(&alert.key).and_then(|seq| store.order.get(seq));
            stored.is_some_and(|stored| Arc::ptr_eq(stored, alert))
        };
        stored
            && !alert.chart.is_freed()
            && host.charts().find(alert.chart.id(), true).is_some_and(|chart| Arc::ptr_eq(&chart, &alert.chart))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{chart, health_with, host, named, pair, rule_text};

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn the_first_rule_to_give_a_key_keeps_it() {
        let rules = [("a", "units: first"), ("a", "units: second")].map(|(name, units)| rule_text("template", name, "t.ctx", &[units]));
        let health = health_with(&rules.concat());
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        let rules = prototypes.get(b"a").unwrap().rules();

        assert!(alerts.add(&c, &rules[0], &|| NOW));
        assert!(!alerts.add(&c, &rules[1], &|| NOW), "the chart has an alert of that name");
        let linked = alerts.chart_alerts(&c);
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].key, b"a,on[t.c]");
        assert_eq!(linked[0].config.units.as_deref(), Some(&b"first"[..]));
        assert_eq!(alerts.version(), 1);
    }

    /// The key names the chart by its id, also for an alarm that matched it by its name.
    #[test]
    fn the_key_holds_the_chart_s_id() {
        let health = health_with(&rule_text("alarm", "by_name", "t.named", &[]));
        let host = host(&[]);
        let c = chart(&host, "t.c", Some("named"), "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        assert!(alerts.add(&c, &prototypes.get(b"by_name").unwrap().rules()[0], &|| NOW));
        assert_eq!(alerts.chart_alerts(&c)[0].key, b"by_name,on[t.c]");
    }

    #[test]
    fn a_new_alert_is_uninitialized_with_c_s_first_values() {
        let with_units = rule_text("template", "with_units", "t.ctx", &["units: mine", "warn: $this > 1"]);
        let health = health_with(&(with_units + &rule_text("template", "bare", "t.ctx", &[])));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        for (name, _) in [("with_units", 0), ("bare", 1)] {
            assert!(alerts.add(&c, &prototypes.get(name.as_bytes()).unwrap().rules()[0], &|| NOW));
        }
        let linked = alerts.chart_alerts(&c);

        // the alarm ids: the counter, seeded with the clock's seconds
        assert_eq!((linked[0].id, linked[1].id), (NOW as u32, NOW as u32 + 1));
        // a rule without units takes its chart's
        assert_eq!(linked[0].config.units.as_deref(), Some(&b"mine"[..]));
        assert_eq!(linked[1].config.units.as_deref(), Some(&b"units"[..]));

        let run = linked[0].run();
        assert!(run.value.is_nan() && run.old_value.is_nan() && run.last_status_change_value.is_nan());
        assert_eq!((run.status, run.old_status), (Status::Uninitialized, Status::Uninitialized));
        assert_eq!((run.last_status_change, run.db_after, run.db_before), (NOW, 0, 0));
        // the link's entry took the first event id
        assert_eq!(run.next_event_id, 2);
        // the alert evaluates its own copies of the rule's expressions
        assert_eq!(run.calculation.as_ref().map(|e| e.source().to_vec()), Some(b"1".to_vec()));
        assert_eq!(run.warning.as_ref().map(|e| e.source().to_vec()), Some(b"$this > 1".to_vec()));
        assert!(run.critical.is_none());
        assert!(linked[0].config.calculation.is_none() && linked[0].config.warning.is_none());
        drop(run);

        let snapshot = linked[0].snapshot();
        assert!(snapshot.value.is_nan());
        assert_eq!((snapshot.status, snapshot.last_status_change), (Status::Uninitialized, NOW));
    }

    #[test]
    fn the_key_is_cut_as_c_cuts_it() {
        let name = "n".repeat(1100);
        let health = health_with(&rule_text("template", &name, "t.ctx", &[]));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        let (stored_name, prototype) = prototypes.iter().next().unwrap();
        assert!(alerts.add(&c, &prototype.rules()[0], &|| NOW));
        let key = &alerts.chart_alerts(&c)[0].key;
        assert_eq!(key.len(), 1023);
        assert!(key.starts_with(&stored_name[..1000]));
    }

    #[test]
    fn the_three_indexes_follow_links_and_unlinks() {
        let health = health_with(&(rule_text("template", "a", "t.ctx", &[]) + &rule_text("template", "b", "t.ctx", &[])));
        let host = host(&[]);
        let (c1, c2) = (chart(&host, "t.c1", None, "t.ctx", &[]), chart(&host, "t.c2", None, "t.ctx", &[]));
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        let (a, b) = (&prototypes.get(b"a").unwrap().rules()[0], &prototypes.get(b"b").unwrap().rules()[0]);
        for (chart, rule) in [(&c1, a), (&c1, b), (&c2, a)] {
            assert!(alerts.add(chart, rule, &|| NOW));
        }
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c1"), pair("b", "t.c1"), pair("a", "t.c2")]);
        assert_eq!(named(&alerts.chart_alerts(&c1)), [pair("a", "t.c1"), pair("b", "t.c1")]);
        assert_eq!(named(&alerts.by_name(b"a")), [pair("a", "t.c1"), pair("a", "t.c2")]);
        assert_eq!(alerts.version(), 3);
        let first = Arc::clone(&alerts.chart_alerts(&c1)[0]);
        assert!(alerts.is_linked(&host, &first));

        alerts.unlink_chart(&c1, &|| NOW + 5, false);
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c2")]);
        assert!(alerts.chart_alerts(&c1).is_empty());
        assert_eq!(named(&alerts.by_name(b"a")), [pair("a", "t.c2")]);
        assert!(alerts.by_name(b"b").is_empty());
        assert_eq!(alerts.version(), 5);
        assert!(!alerts.is_linked(&host, &first));

        // linked again, an alert goes to the end: of the dictionary and of its name's list
        assert!(alerts.add(&c1, a, &|| NOW + 6));
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c2"), pair("a", "t.c1")]);
        assert_eq!(named(&alerts.by_name(b"a")), [pair("a", "t.c2"), pair("a", "t.c1")]);
        // a new alert with the next alarm id (the memory log would give the old one back: the loop's commit)
        assert_eq!(alerts.chart_alerts(&c1)[0].id, first.id + 3);

        alerts.delete_all(&|| NOW + 7, false);
        assert!(alerts.alerts().is_empty() && alerts.by_name(b"a").is_empty() && alerts.chart_alerts(&c2).is_empty());
    }

    /// An unlink writes its REMOVED entry unless the alert is REMOVED already or the agent is exiting.
    #[test]
    fn an_unlink_logs_unless_removed_or_exiting() {
        let health = health_with(&rule_text("template", "a", "t.ctx", &[]));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let prototypes = health.prototypes();
        let rule = &prototypes.get(b"a").unwrap().rules()[0];
        let event_id_after_unlink = |exiting: bool, removed: bool| {
            let alerts = HostAlerts::default();
            assert!(alerts.add(&c, rule, &|| NOW));
            let alert = Arc::clone(&alerts.chart_alerts(&c)[0]);
            if removed {
                alert.run().status = Status::Removed;
            }
            alerts.unlink_chart(&c, &|| NOW + 1, exiting);
            assert!(alerts.alerts().is_empty());
            alert.run().next_event_id
        };
        assert_eq!(event_id_after_unlink(false, false), 3, "the link's entry and the unlink's");
        assert_eq!(event_id_after_unlink(true, false), 2, "exiting");
        assert_eq!(event_id_after_unlink(false, true), 2, "already REMOVED");
    }

    /// A transition's when, duration, old and new value, old and new status, delay and flags.
    type Fields = (i64, i64, Option<f64>, Option<f64>, Status, Status, i32, u32);

    /// What a transition holds, with a NaN as `None`.
    fn fields(t: &Transition) -> Fields {
        let value = |v: f64| (!v.is_nan()).then_some(v);
        (t.when, t.duration, value(t.old_value), value(t.new_value), t.old_status, t.new_status, t.delay, t.flags)
    }

    /// The entry of a link (from REMOVED to the alert's status, flagged when the rule repeats) and of an unlink (to
    /// REMOVED), and the three clock reads of a host's first link in C's order: the alert's last status change,
    /// the alarm id's seed, the entry.
    #[test]
    fn a_link_and_an_unlink_log_their_entries() {
        let rules = rule_text("template", "plain", "t.ctx", &[])
            + &rule_text("template", "repeating", "t.ctx", &["repeat: warning 7s critical 11s"]);
        let health = health_with(&rules);
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        let reads = std::cell::Cell::new(0);
        let clock = || {
            reads.set(reads.get() + 1);
            NOW + reads.get() - 1
        };

        assert!(alerts.add(&c, &prototypes.get(b"plain").unwrap().rules()[0], &clock));
        let plain = Arc::clone(&alerts.chart_alerts(&c)[0]);
        assert_eq!((plain.run().last_status_change, i64::from(plain.id)), (NOW, NOW + 1));
        // two reads for the second alert: the seed is set
        assert!(alerts.add(&c, &prototypes.get(b"repeating").unwrap().rules()[0], &clock));
        alerts.unlink_chart(&c, &|| NOW + 60, false);

        let logged: Vec<_> = alerts.store().transitions.iter().map(|(key, t)| (key.clone(), fields(t))).collect();
        let (uninitialized, removed, repeats) = (Status::Uninitialized, Status::Removed, ENTRY_FLAG_IS_REPEATING);
        let (plain_key, repeating_key) = (b"plain,on[t.c]".to_vec(), b"repeating,on[t.c]".to_vec());
        assert_eq!(
            logged,
            [
                (plain_key.clone(), (NOW + 2, 2, None, None, removed, uninitialized, 0, 0)),
                (repeating_key.clone(), (NOW + 4, 1, None, None, removed, uninitialized, 0, repeats)),
                (plain_key, (NOW + 60, 60, None, None, uninitialized, removed, 0, 0)),
                (repeating_key, (NOW + 60, 57, None, None, uninitialized, removed, 0, 0)),
            ]
        );
    }

    /// A key still held by an alert of a freed chart (the chart was defined again before its free reached health)
    /// is given to the new chart; the old chart's free then finds nothing of its own to unlink.
    #[test]
    fn a_new_chart_of_a_freed_chart_s_id_takes_its_key() {
        let health = health_with(&rule_text("template", "a", "t.ctx", &[]));
        let host = host(&[]);
        let old = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        let rule = &prototypes.get(b"a").unwrap().rules()[0];
        assert!(alerts.add(&old, rule, &|| NOW));
        let old_alert = Arc::clone(&alerts.chart_alerts(&old)[0]);

        assert!(host.charts().free_if(&old, |_| true));
        let new = chart(&host, "t.c", None, "t.ctx", &[]);
        assert!(!Arc::ptr_eq(&old, &new));
        assert!(alerts.add(&new, rule, &|| NOW + 1));
        let new_alert = Arc::clone(&alerts.chart_alerts(&new)[0]);
        assert!(!Arc::ptr_eq(&old_alert, &new_alert) && Arc::ptr_eq(&new_alert.chart, &new));
        assert!(alerts.chart_alerts(&old).is_empty(), "a chart's alerts are those on that chart object");
        assert_eq!(old_alert.run().next_event_id, 3, "the old alert was unlinked, with its entry");
        assert_eq!(alerts.count(), 1);

        alerts.unlink_chart(&old, &|| NOW + 2, false);
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c")]);
        assert!(alerts.is_linked(&host, &new_alert));
        // a live chart keeps its key against a second rule, as before
        assert!(!alerts.add(&new, rule, &|| NOW + 3));
    }

    #[test]
    fn a_freed_chart_takes_no_alert() {
        let health = health_with(&rule_text("template", "a", "t.ctx", &[]));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        assert!(host.charts().free_if(&c, |_| true));
        assert!(!alerts.add(&c, &prototypes.get(b"a").unwrap().rules()[0], &|| NOW));
        assert!(alerts.alerts().is_empty());
        assert_eq!(alerts.version(), 0);
    }
}

