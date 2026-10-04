//! A host's alerts (`host->rrdcalc_root_index`, each chart's alert list, the name index) and its alert log:
//! `rrdcalc.c`, `health_log.c`.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
use netdata_agent_metadata::health_log::LoadedRow;
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;

use crate::alert::{Alert, Run, Status, run_flags};
use crate::config::HealthConfig;
use crate::entry::{Entry, Transition, entry_flags};
use crate::log::AlarmLog;
use crate::pass::{Env, PassCounts, elapsed, hysteresis};
use crate::prototype::Rule;
use crate::tables::ACTION_OPTION_NO_CLEAR_NOTIFICATION;
use crate::{Clock, journal, sql};

/// The saves an entry's addition owes, in C's order, each with whether it is asynchronous: the older entry the new
/// one replaced, then the new one. They are made once the store's lock and the alert's are released.
type Saves = Vec<(u32, bool)>;

/// `RRDCALC_MAX_KEY_SIZE`: a key is cut to one byte less.
const MAX_KEY_SIZE: usize = 1024;

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
    /// `host->health_log.next_alarm_id`: 0 until the host's first pass with a database, or until the first alarm
    /// without one.
    next_alarm_id: u32,
    /// `host->health_log`.
    log: AlarmLog,
}

impl Store {
    /// `health_create_alarm_entry()` and `health_alarm_log_add_entry()`: the entry of a status change, in the log.
    /// C makes the transition id before it reads the clock for the entry's `global_id`. The saves it owes (the
    /// entry it replaced, then itself) are added to `saves`.
    fn log_transition(
        &mut self,
        alert: &Alert,
        run: &mut Run,
        transition: &Transition,
        is_async: bool,
        env: &dyn Env,
        saves: &mut Saves,
    ) -> Entry {
        let transition_id = env.transition_id();
        let entry = Entry::create(alert, run, transition, env.now_usec(), transition_id);
        let (entry, replaced) = self.log.add(entry);
        saves.extend(replaced.map(|older| (older, is_async)));
        saves.push((entry.unique_id, is_async));
        entry
    }

    /// `rrdcalc_find_alarm_entry()`: the alarm id and the next event id of a rule on a chart, from the newest entry
    /// of the memory log with that name, chart and rule hash.
    fn alarm_id_in_log(&self, chart_id: &[u8], name: Option<&[u8]>, hash_id: &[u8; 16]) -> Option<(u32, u32)> {
        let known = self.log.entries.iter().find(|entry| {
            entry.name.as_deref() == name && entry.chart == chart_id && entry.config_hash_id == *hash_id
        });
        known.map(|entry| (entry.alarm_id, entry.alarm_event_id.wrapping_add(1)))
    }

    /// A new alarm's id: the host's counter, seeded with the clock at its first use.
    fn new_alarm_id(&mut self, clock: Clock) -> u32 {
        if self.next_alarm_id == 0 {
            self.next_alarm_id = clock() as u32;
        }
        let id = self.next_alarm_id;
        self.next_alarm_id = self.next_alarm_id.wrapping_add(1);
        id
    }

    /// Whether the alert is the one the store holds under its key.
    fn holds(&self, alert: &Arc<Alert>) -> bool {
        let stored = self.by_key.get(&alert.key).and_then(|seq| self.order.get(seq));
        stored.is_some_and(|stored| Arc::ptr_eq(stored, alert))
    }

    /// `rrdcalc_rrdset_acquire_linked()`, the store locked: the alert is in the store and its chart is still the
    /// host's chart of that id (obsolete or not).
    fn linked(&self, host: &Host, alert: &Arc<Alert>) -> bool {
        self.holds(alert) && chart_is_the_hosts(host, alert)
    }
}

fn chart_is_the_hosts(host: &Host, alert: &Arc<Alert>) -> bool {
    !alert.chart.is_freed()
        && host.charts().find(alert.chart.id(), true).is_some_and(|chart| Arc::ptr_eq(&chart, &alert.chart))
}

/// The flags an entry of a transition or of a repeat carries.
fn entry_flags_of(alert: &Alert, run: &Run) -> u32 {
    let mut flags = 0;
    if alert.config.alert_action_options & ACTION_OPTION_NO_CLEAR_NOTIFICATION != 0 {
        flags |= entry_flags::NO_CLEAR_NOTIFICATION;
    }
    if run.run_flags & run_flags::SILENCED != 0 {
        flags |= entry_flags::SILENCED;
    }
    if alert.is_repeating() {
        flags |= entry_flags::IS_REPEATING;
    }
    flags
}

/// A host's alerts.
#[derive(Default)]
pub struct HostAlerts {
    /// The host object these are the alerts of. A machine GUID can name another object: a host freed and created
    /// again, or one that lost the index to this one and is freed at once; what happens to that one is not this
    /// one's business.
    owner: Weak<Host>,
    /// These alerts themselves, for the metadata queue: a queued save names the host's alerts and the entry.
    me: Weak<HostAlerts>,
    /// `RRDHOST_FLAG_INITIALIZED_HEALTH`: set by the host's first pass and never cleared.
    initialized: AtomicBool,
    inner: Mutex<Store>,
    /// Held while one entry is read, saved and marked as saved, by HEALTH or by the metadata thread's store job:
    /// C has nothing here, and both can find an entry unsaved and insert it twice. Taken before the database and
    /// before the store, never after either.
    saving: Mutex<()>,
    /// `host->health.pending_transitions`: the saves the metadata queue holds; a pass is postponed while any is.
    pending_transitions: AtomicI32,
    /// `host->health_log.max`: how many entries the alert log's query answers, 0 until the host's first pass.
    log_max: AtomicU32,
    /// `host->health.alert_status_snapshot`: the counts of the last complete pass, and how often counts were
    /// published, times two (C's generation is odd while a writer is at it).
    counts: Mutex<(u64, Option<PassCounts>)>,
}

/// `rrdcalc_key()`: `NAME,on[CHART_ID]`, cut as `snprintfz()` cuts it.
fn key(chart_id: &str, name: &[u8]) -> Vec<u8> {
    let mut key = [name, b",on[", chart_id.as_bytes(), b"]"].concat();
    key.truncate(MAX_KEY_SIZE - 1);
    key
}

impl HostAlerts {
    /// The alerts of `host`, none yet.
    pub(crate) fn new(host: &Arc<Host>) -> Arc<HostAlerts> {
        Arc::new_cyclic(|me| HostAlerts { owner: Arc::downgrade(host), me: me.clone(), ..HostAlerts::default() })
    }

    /// Whether these are the alerts of that very host object.
    pub(crate) fn is_of(&self, host: &Host) -> bool {
        std::ptr::eq(self.owner.as_ptr(), host)
    }

    /// The host these are the alerts of, while it exists.
    pub fn host(&self) -> Option<Arc<Host>> {
        self.owner.upgrade()
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The records of entries made under the store's lock, written once it is released, in the order made.
    fn log_records(&self, entries: &[Entry]) {
        if entries.is_empty() {
            return;
        }
        let hostname = self.owner.upgrade().map(|host| host.hostname()).unwrap_or_default();
        for entry in entries {
            journal::log_alert(&hostname, entry);
        }
    }

    /// `health_alarm_log_save()`: an entry's save. An asynchronous one (a link's, an unlink's) is offered to the
    /// metadata queue, which counts it on the host and on the entry before it answers; refused, it is made at once
    /// only on the HEALTH thread while the service runs, else not at all. A synchronous one is made at once.
    fn save(&self, env: &dyn Env, unique_id: u32, is_async: bool) {
        if is_async {
            // metadata_queue_ae_save(): both counters go up before the queue is asked, and back when it refuses
            self.pending_transitions.fetch_add(1, Ordering::Relaxed);
            self.with_entry(unique_id, |entry| entry.pending_save_count = entry.pending_save_count.saturating_add(1));
            if self.me.upgrade().is_some_and(|me| env.queue_save(&me, unique_id)) {
                return;
            }
            self.pending_transitions.fetch_sub(1, Ordering::Relaxed);
            self.with_entry(unique_id, |entry| entry.pending_save_count = entry.pending_save_count.saturating_sub(1));
            if !(env.is_health_thread() && env.service_running()) {
                return;
            }
        }
        if let Some(host) = self.owner.upgrade() {
            self.save_now(unique_id, &|entry| env.sql_save(&host, entry));
        }
    }

    /// `sql_health_alarm_log_save()` of the entry as it stands now, in the log or among those that left it with a
    /// save queued: `sql` inserts or updates its row and says whether it inserted, which marks the entry as saved.
    fn save_now(&self, unique_id: u32, sql: &dyn Fn(&Entry) -> bool) {
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(copy) = self.with_entry(unique_id, |entry| entry.clone()) else {
            return;
        };
        if sql(&copy) {
            self.with_entry(unique_id, |entry| entry.flags |= entry_flags::SAVED);
        }
    }

    /// The metadata thread's store job, for one queued save (`store_alert_transitions()`): the entry is saved as
    /// it stands by now, and the two counters are taken back, whatever the save did.
    pub fn save_queued(&self, unique_id: u32, sql: &dyn Fn(&Entry) -> bool) {
        self.save_now(unique_id, sql);
        self.store().log.save_done(unique_id);
        self.pending_transitions.fetch_sub(1, Ordering::Relaxed);
    }

    /// `host->health.pending_transitions`.
    pub fn pending_transitions(&self) -> i32 {
        self.pending_transitions.load(Ordering::Relaxed)
    }

    fn with_entry<T>(&self, unique_id: u32, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        self.store().log.entry_mut(unique_id).map(f)
    }

    /// The saves of entries made under the store's lock, once it is released, in the order made.
    fn run_saves(&self, env: &dyn Env, saves: &Saves) {
        for &(unique_id, is_async) in saves {
            self.save(env, unique_id, is_async);
        }
    }

    /// Whether the host's first pass ran.
    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// `health_initialize_rrdhost()` up to its flag: the log's limit and retention from the configuration, the ids
    /// the host's log starts from, and the load of what the table has. C takes the clock's second for the next log
    /// id; without a database that stays, and the alarm ids are seeded by the first alarm.
    pub(crate) fn initialize(
        &self,
        host: &Host,
        config: &HealthConfig,
        env: &dyn Env,
        clock: Clock,
        running: &dyn Fn() -> bool,
    ) {
        self.log_max.store(config.health_log_entries_max, Ordering::Relaxed);
        host.set_health_log_retention_s(config.health_log_retention_s);
        {
            let mut store = self.store();
            store.log.next_log_id = clock() as u32;
            store.next_alarm_id = 0;
        }
        if let Some(rows) = env.load(host) {
            self.load(host, rows, clock, running);
        }
        self.initialized.store(true, Ordering::Release);
    }

    /// `sql_health_alarm_log_load()` once the table gave its rows: each row that can be an entry goes in front of
    /// the log (so the log is in reverse row order), marked as saved; the others are recorded and counted. The
    /// next ids go on from the highest loaded ones; with nothing loaded the clock's second is read for each, so
    /// both are one more than it. Then the record, and the memory log's retention cleanup.
    ///
    /// C also asks, per row, whether the host has a repeating alert named as the row's chart, to hand it the row's
    /// last repeat and skip the row. No alert exists before the host's first pass (every path that makes one
    /// looks for `RRDHOST_FLAG_INITIALIZED_HEALTH`, which this pass sets after the load and nothing clears), so
    /// that look finds nothing and is not made here.
    fn load(&self, host: &Host, rows: Vec<LoadedRow>, clock: Clock, running: &dyn Fn() -> bool) {
        let hostname = host.hostname();
        let (mut loaded, mut errored) = (0usize, 0usize);
        let (mut max_unique_id, mut max_alarm_id) = (0u32, 0u32);
        let mut store = self.store();
        let mut rows = rows.into_iter();
        // C looks at the service before it steps to each row, the step that finds no more rows included
        while running() {
            let Some(row) = rows.next() else {
                break;
            };
            let entry = match sql::row_lacks(&row).map_or_else(|| sql::entry_of(row), Err) {
                Ok(entry) => entry,
                Err(wrong) => {
                    netdata_log_error!("HEALTH [{hostname}]: {wrong}");
                    errored += 1;
                    continue;
                }
            };
            max_unique_id = max_unique_id.max(entry.unique_id);
            max_alarm_id = max_alarm_id.max(entry.alarm_id);
            store.log.entries.push_front(entry);
            loaded += 1;
        }
        if max_unique_id == 0 {
            max_unique_id = clock() as u32;
        }
        if max_alarm_id == 0 {
            max_alarm_id = clock() as u32;
        }
        store.log.next_log_id = max_unique_id.wrapping_add(1);
        if store.next_alarm_id == 0 || store.next_alarm_id <= max_alarm_id {
            store.next_alarm_id = max_alarm_id.wrapping_add(1);
        }
        drop(store);
        let priority = if errored != 0 { Priority::Warning } else { Priority::Debug };
        let text = format_args!("loaded {loaded} alarm entries, errors in {errored} entries.");
        nd_log!(Source::Daemon, priority, "[{hostname}]: Table health_log, {text}");
        self.log_cleanup(host.health_log_retention_s(), clock);
    }

    /// `host->health_log.max`.
    pub fn log_max(&self) -> u32 {
        self.log_max.load(Ordering::Relaxed)
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

    /// `host->health_transitions`: the entries the host's log took so far.
    pub fn transitions(&self) -> u64 {
        self.store().log.transitions
    }

    /// `host->health_log.next_log_id - 1`, 0 before the host's first pass: what the API shows as the latest unique
    /// id. Repeats take ids too.
    pub fn latest_log_unique_id(&self) -> u32 {
        self.store().log.next_log_id.saturating_sub(1)
    }

    /// The log's entries, newest first.
    pub fn log_entries(&self) -> Vec<Entry> {
        self.store().log.entries.iter().cloned().collect()
    }

    /// The host's three counters around the log: the next log id, the next alarm id, the lowest id that waits.
    pub fn log_counters(&self) -> (u32, u32, u32) {
        let store = self.store();
        (store.log.next_log_id, store.next_alarm_id, store.log.last_processed_id)
    }

    /// The status counts of the last complete pass; none before the first.
    pub fn pass_counts(&self) -> Option<PassCounts> {
        self.counts.lock().unwrap_or_else(PoisonError::into_inner).1
    }

    /// The generation of the status counts: two more at every publication.
    pub fn pass_counts_generation(&self) -> u64 {
        self.counts.lock().unwrap_or_else(PoisonError::into_inner).0
    }

    pub(crate) fn publish_counts(&self, counts: PassCounts) {
        let mut published = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        *published = (published.0 + 2, Some(counts));
    }

    /// `rrdcalc_add_from_prototype()`: the alert of `rule` on `chart`, created and linked, unless the chart has an
    /// alert of that name already (the first rule to give a key keeps it) or was freed meanwhile. True when linked.
    ///
    /// A key can still be held by an alert of an earlier chart object of that id, freed, whose free has not reached
    /// health yet. C's dictionary runs a chart's delete callback before a chart of that id can exist again, unless
    /// the old one is still referenced; here that alert goes now, as its chart's free would take it.
    pub(crate) fn add(&self, chart: &Arc<Chart>, rule: &Rule, env: &dyn Env, clock: Clock) -> bool {
        let key = key(chart.id(), rule.config.name.as_deref().unwrap_or(b""));
        let (mut entries, mut saves) = (Vec::new(), Saves::new());
        {
            let mut store = self.store();
            if chart.is_freed() {
                return false;
            }
            if let Some(existing) = store.by_key.get(&key).and_then(|seq| store.order.get(seq)).cloned() {
                if Arc::ptr_eq(&existing.chart, chart) || !existing.chart.is_freed() {
                    return false;
                }
                Self::unlink(&mut store, &existing, env, clock, &mut entries, &mut saves);
            }

            // C reads the clock for the alert's last status change, then for the alarm id's seed when it has none
            // yet, then for the link's entry
            let last_status_change = clock();
            let name = rule.config.name.as_deref();
            let (chart_id, hash_id) = (chart.id().as_bytes(), &rule.config.hash_id);
            // rrdcalc_get_unique_id(): the memory log; else the table is asked with no lock held, then the
            // memory log again, then the table's answer, then the host's counter
            let (id, next_event_id) = match store.alarm_id_in_log(chart_id, name, hash_id) {
                Some(known) => known,
                None => {
                    drop(store);
                    let in_table = self.owner.upgrade().and_then(|host| env.sql_alarm_id(&host, chart_id, name));
                    store = self.store();
                    // nothing else links, but the chart can have been freed meanwhile
                    if chart.is_freed() || store.by_key.contains_key(&key) {
                        drop(store);
                        self.run_saves(env, &saves);
                        self.log_records(&entries);
                        return false;
                    }
                    match store.alarm_id_in_log(chart_id, name, hash_id) {
                        Some(known) => known,
                        None => in_table.unwrap_or_else(|| (store.new_alarm_id(clock), 1)),
                    }
                }
            };

            let alert = Alert::new(key.clone(), chart, &rule.config, id, next_event_id, last_status_change);
            let alert = Arc::new(alert);
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
            let transition = Transition {
                when: now,
                duration: elapsed(now, run.last_status_change),
                old_value: run.old_value,
                new_value: run.value,
                old_status: Status::Removed,
                new_status: run.status,
                delay: 0,
                flags: if alert.is_repeating() { entry_flags::IS_REPEATING } else { 0 },
            };
            let entry = store.log_transition(&alert, &mut run, &transition, true, env, &mut saves);
            alert.publish(&run, Some((entry.global_id, entry.transition_id)));
            entries.push(entry);
        }
        self.run_saves(env, &saves);
        self.log_records(&entries);
        true
    }

    /// `rrdcalc_unlink_and_delete()` of one alert, the store locked: out of the name index, its REMOVED entry
    /// unless it is REMOVED already or the agent is exiting, off its chart's list, out of the dictionary.
    fn unlink(
        store: &mut Store,
        alert: &Arc<Alert>,
        env: &dyn Env,
        clock: Clock,
        entries: &mut Vec<Entry>,
        saves: &mut Saves,
    ) {
        if let Some(named) = store.by_name.get_mut(alert.name()) {
            named.retain(|other| !Arc::ptr_eq(other, alert));
            if named.is_empty() {
                store.by_name.remove(alert.name());
            }
        }

        if !env.exiting() {
            // C reads the clock before it looks at the status
            let now = clock();
            let mut run = alert.run();
            if run.status != Status::Removed {
                let transition = Transition {
                    when: now,
                    duration: elapsed(now, run.last_status_change),
                    old_value: run.old_value,
                    new_value: run.value,
                    old_status: run.status,
                    new_status: Status::Removed,
                    delay: 0,
                    flags: 0,
                };
                entries.push(store.log_transition(alert, &mut run, &transition, true, env, saves));
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
    pub(crate) fn unlink_chart(&self, chart: &Chart, env: &dyn Env, clock: Clock) {
        let (mut entries, mut saves) = (Vec::new(), Saves::new());
        {
            let mut store = self.store();
            for alert in store.by_chart.get(chart.id()).cloned().unwrap_or_default() {
                if std::ptr::eq(Arc::as_ptr(&alert.chart), chart) {
                    Self::unlink(&mut store, &alert, env, clock, &mut entries, &mut saves);
                }
            }
        }
        self.run_saves(env, &saves);
        self.log_records(&entries);
    }

    /// `rrdcalc_delete_all()`: every alert of the host goes, in the dictionary's order.
    pub(crate) fn delete_all(&self, env: &dyn Env, clock: Clock) {
        let (mut entries, mut saves) = (Vec::new(), Saves::new());
        {
            let mut store = self.store();
            for alert in store.order.values().cloned().collect::<Vec<_>>() {
                Self::unlink(&mut store, &alert, env, clock, &mut entries, &mut saves);
            }
        }
        self.run_saves(env, &saves);
        self.log_records(&entries);
    }

    /// `rrdcalc_rrdset_acquire_linked()`: the alert is in the store and its chart is still the host's chart of
    /// that id (obsolete or not).
    pub fn is_linked(&self, host: &Host, alert: &Arc<Alert>) -> bool {
        let stored = self.store().holds(alert);
        stored && chart_is_the_hosts(host, alert)
    }

    /// The obsolete rule of the pass's first walk: the alert of a chart that is obsolete and no longer collected
    /// becomes REMOVED, with its entry. The clock is read for the entry. Returns the entry and the status the alert
    /// left; nothing when the alert was unlinked meanwhile.
    pub(crate) fn obsolete_removed(
        &self,
        host: &Host,
        alert: &Arc<Alert>,
        env: &dyn Env,
        clock: Clock,
    ) -> Option<(Entry, Status)> {
        let mut saves = Saves::new();
        let removed = {
            let mut store = self.store();
            if !store.linked(host, alert) {
                return None;
            }
            let mut run = alert.run();
            let when = clock();
            let old_status = run.status;
            let transition = Transition {
                when,
                duration: elapsed(when, run.last_status_change),
                old_value: run.value,
                new_value: f64::NAN,
                old_status,
                new_status: Status::Removed,
                delay: 0,
                flags: 0,
            };
            let entry = store.log_transition(alert, &mut run, &transition, false, env, &mut saves);
            run.old_status = run.status;
            run.status = Status::Removed;
            run.last_status_change = when;
            run.last_status_change_value = run.value;
            run.last_updated = when;
            run.value = f64::NAN;
            (entry, old_status)
        };
        self.run_saves(env, &saves);
        Some(removed)
    }

    /// A status change of the pass's second walk: the hysteresis, the entry, then the alert's new status. Nothing
    /// when the alert was unlinked meanwhile.
    pub(crate) fn transition(
        &self,
        host: &Host,
        alert: &Arc<Alert>,
        status: Status,
        now: i64,
        env: &dyn Env,
    ) -> Option<Entry> {
        let mut saves = Saves::new();
        let entry = {
            let mut store = self.store();
            if !store.linked(host, alert) {
                return None;
            }
            let mut run = alert.run();
            let delay = hysteresis(&mut run, &alert.config, status, now);
            let transition = Transition {
                when: now,
                duration: elapsed(now, run.last_status_change),
                old_value: run.old_value,
                new_value: run.value,
                old_status: run.status,
                new_status: status,
                delay,
                flags: entry_flags_of(alert, &run),
            };
            let entry = store.log_transition(alert, &mut run, &transition, false, env, &mut saves);
            run.last_status_change_value = run.value;
            run.last_status_change = now;
            run.old_status = run.status;
            run.status = status;
            if alert.is_repeating() {
                run.last_repeat = now;
                if status == Status::Clear {
                    run.run_flags |= run_flags::RUN_ONCE;
                }
            }
            entry
        };
        self.run_saves(env, &saves);
        Some(entry)
    }

    /// A repeat of the pass's third walk: an entry that takes a unique id and is in no log. The alert counts the
    /// repeat and is marked as having run once. Nothing when the alert was unlinked meanwhile.
    pub(crate) fn repeat(&self, host: &Host, alert: &Arc<Alert>, now: i64, env: &dyn Env) -> Option<Entry> {
        let mut store = self.store();
        if !store.linked(host, alert) {
            return None;
        }
        let mut run = alert.run();
        run.last_repeat = now;
        run.times_repeat = run.times_repeat.saturating_add(1);
        let transition = Transition {
            when: now,
            duration: elapsed(now, run.last_status_change),
            old_value: run.old_value,
            new_value: run.value,
            old_status: run.old_status,
            new_status: run.status,
            delay: run.delay_last,
            flags: entry_flags_of(alert, &run),
        };
        let transition_id = env.transition_id();
        let mut entry = Entry::create(alert, &mut run, &transition, env.now_usec(), transition_id);
        store.log.assign_unique_id(&mut entry);
        entry.last_repeat = run.last_repeat;
        if run.run_flags & run_flags::RUN_ONCE == 0 && run.status == Status::Clear {
            entry.flags |= entry_flags::RUN_ONCE;
        }
        run.run_flags |= run_flags::RUN_ONCE;
        alert.publish_repeat_state(&run);
        Some(entry)
    }

    /// `health_send_notification()` as far as the log goes, for a repeat's entry, which is in no log: it is offered
    /// for a notification, marked as processed and saved at once (never queued). What a notification is comes with
    /// its own commit.
    pub(crate) fn notify_repeat(&self, host: &Host, entry: &mut Entry, env: &dyn Env) {
        env.notify(entry);
        entry.flags |= entry_flags::PROCESSED;
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        if env.sql_save(host, entry) {
            entry.flags |= entry_flags::SAVED;
        }
    }

    /// The pass's last step (`health_alarm_log_process_to_send_notifications()`): every entry that is due gets its
    /// notification, is marked as processed and saved; then the log is trimmed. The due entries are found under
    /// the store's lock; each is notified and saved with no lock held, where C holds the log's read lock: an entry
    /// that another thread takes out of the log in between is skipped.
    pub(crate) fn process_log(&self, env: &dyn Env, clock: Clock) {
        let due = self.store().log.scan(clock);
        for unique_id in due {
            let Some(mut copy) = self.with_entry(unique_id, |entry| entry.clone()) else {
                continue;
            };
            env.notify(&mut copy);
            self.with_entry(unique_id, |entry| entry.flags |= entry_flags::PROCESSED);
            self.save(env, unique_id, false);
        }
        self.store().log.trim(clock);
    }

    /// `health_apply_prototypes_to_host()`'s walk of the log, between its delete of every alert and its relink.
    pub(crate) fn mark_log_updated(&self) {
        self.store().log.mark_updated();
    }

    /// `health_alarm_log_cleanup()`: the memory log's cleanup by the host's retention, after a load and after the
    /// table's hourly cleanup. An empty log reads no clock. C gives up when the log's lock is taken; here the
    /// store's lock is waited for: nothing is held while it is asked for, and what the cleanup leaves shows nowhere
    /// but in memory.
    pub fn log_cleanup(&self, retention_s: u32, clock: Clock) {
        let mut store = self.store();
        if !store.log.entries.is_empty() {
            store.log.cleanup(retention_s, clock());
        }
    }

    /// `rrdhost_cleanup_data_collection_and_health()` once the host's charts are gone: C destroys the alert
    /// dictionary, so its version starts from 0 again when the host comes back, and frees the log; the id counters
    /// and the count of transitions stay.
    pub(crate) fn charts_flushed(&self) {
        let mut store = self.store();
        store.version = 0;
        store.log.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::Idle;
    use crate::testing::{Scripted, chart, health_with, host, named, pair, rule_text};
    use netdata_agent_metadata::health_log::Uuid;
    use netdata_agent_text::units::format_value_and_unit;

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn the_first_rule_to_give_a_key_keeps_it() {
        let rules = [("a", "units: first"), ("a", "units: second")]
            .map(|(name, units)| rule_text("template", name, "t.ctx", &[units]));
        let health = health_with(&rules.concat());
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        let rules = prototypes.get(b"a").unwrap().rules();

        assert!(alerts.add(&c, &rules[0], &Idle, &|| NOW));
        assert!(!alerts.add(&c, &rules[1], &Idle, &|| NOW), "the chart has an alert of that name");
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
        assert!(alerts.add(&c, &prototypes.get(b"by_name").unwrap().rules()[0], &Idle, &|| NOW));
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
            assert!(alerts.add(&c, &prototypes.get(name.as_bytes()).unwrap().rules()[0], &Idle, &|| NOW));
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
        let expressions = linked[0].expressions();
        assert_eq!(expressions.calculation.as_ref().map(|e| e.source().to_vec()), Some(b"1".to_vec()));
        assert_eq!(expressions.warning.as_ref().map(|e| e.source().to_vec()), Some(b"$this > 1".to_vec()));
        assert!(expressions.critical.is_none());
        let sources = linked[0].texts.each_ref().map(|text| text.as_ref().map(|text| text.source.clone()));
        assert_eq!(sources, [Some(b"1".to_vec()), Some(b"$this > 1".to_vec()), None]);
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
        assert!(alerts.add(&c, &prototype.rules()[0], &Idle, &|| NOW));
        let key = &alerts.chart_alerts(&c)[0].key;
        assert_eq!(key.len(), 1023);
        assert!(key.starts_with(&stored_name[..1000]));
    }

    #[test]
    fn the_three_indexes_follow_links_and_unlinks() {
        let rules = rule_text("template", "a", "t.ctx", &[]) + &rule_text("template", "b", "t.ctx", &[]);
        let health = health_with(&rules);
        let host = host(&[]);
        let (c1, c2) = (chart(&host, "t.c1", None, "t.ctx", &[]), chart(&host, "t.c2", None, "t.ctx", &[]));
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        let (a, b) = (&prototypes.get(b"a").unwrap().rules()[0], &prototypes.get(b"b").unwrap().rules()[0]);
        for (chart, rule) in [(&c1, a), (&c1, b), (&c2, a)] {
            assert!(alerts.add(chart, rule, &Idle, &|| NOW));
        }
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c1"), pair("b", "t.c1"), pair("a", "t.c2")]);
        assert_eq!(named(&alerts.chart_alerts(&c1)), [pair("a", "t.c1"), pair("b", "t.c1")]);
        assert_eq!(named(&alerts.by_name(b"a")), [pair("a", "t.c1"), pair("a", "t.c2")]);
        assert_eq!(alerts.version(), 3);
        let first = Arc::clone(&alerts.chart_alerts(&c1)[0]);
        assert!(alerts.is_linked(&host, &first));

        alerts.unlink_chart(&c1, &Idle, &|| NOW + 5);
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c2")]);
        assert!(alerts.chart_alerts(&c1).is_empty());
        assert_eq!(named(&alerts.by_name(b"a")), [pair("a", "t.c2")]);
        assert!(alerts.by_name(b"b").is_empty());
        assert_eq!(alerts.version(), 5);
        assert!(!alerts.is_linked(&host, &first));

        // linked again, an alert goes to the end: of the dictionary and of its name's list
        assert!(alerts.add(&c1, a, &Idle, &|| NOW + 6));
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c2"), pair("a", "t.c1")]);
        assert_eq!(named(&alerts.by_name(b"a")), [pair("a", "t.c2"), pair("a", "t.c1")]);
        // a new alert object with the alarm id the log holds for its name, chart and rule, and the next event id
        let again = Arc::clone(&alerts.chart_alerts(&c1)[0]);
        assert!(!Arc::ptr_eq(&again, &first));
        assert_eq!(again.id, first.id);
        // the first alert's link and unlink took the event ids 1 and 2, the new link 3
        assert_eq!(again.run().next_event_id, 4);

        alerts.delete_all(&Idle, &|| NOW + 7);
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
            assert!(alerts.add(&c, rule, &Idle, &|| NOW));
            let alert = Arc::clone(&alerts.chart_alerts(&c)[0]);
            if removed {
                alert.run().status = Status::Removed;
            }
            alerts.unlink_chart(&c, &Scripted { exiting, ..Scripted::default() }, &|| NOW + 1);
            assert!(alerts.alerts().is_empty());
            alert.run().next_event_id
        };
        assert_eq!(event_id_after_unlink(false, false), 3, "the link's entry and the unlink's");
        assert_eq!(event_id_after_unlink(true, false), 2, "exiting");
        assert_eq!(event_id_after_unlink(false, true), 2, "already REMOVED");
    }

    /// An entry's name, when, duration, old and new value, old and new status, delay and whether it is flagged as
    /// its alert repeating.
    type Fields = (Vec<u8>, i64, i64, Option<f64>, Option<f64>, Status, Status, i32, bool);

    /// What an entry holds of its transition, with a NaN as `None`.
    fn fields(e: &Entry) -> Fields {
        let value = |v: f64| (!v.is_nan()).then_some(v);
        let repeating = e.flags & entry_flags::IS_REPEATING != 0;
        let name = e.name.clone().unwrap_or_default();
        let values = (value(e.old_value), value(e.new_value));
        (name, e.when, e.duration, values.0, values.1, e.old_status, e.new_status, e.delay, repeating)
    }

    /// The entry of a link (from REMOVED to the alert's status, flagged when the rule repeats) and of an unlink (to
    /// REMOVED), and the three clock reads of a host's first link without a database, in C's order: the alert's
    /// last status change, the alarm id's seed, the entry. Each unlink's entry replaces its alert's link entry.
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

        assert!(alerts.add(&c, &prototypes.get(b"plain").unwrap().rules()[0], &Idle, &clock));
        let plain = Arc::clone(&alerts.chart_alerts(&c)[0]);
        assert_eq!((plain.run().last_status_change, i64::from(plain.id)), (NOW, NOW + 1));
        // two reads for the second alert: the seed is set
        assert!(alerts.add(&c, &prototypes.get(b"repeating").unwrap().rules()[0], &Idle, &clock));
        alerts.unlink_chart(&c, &Idle, &|| NOW + 60);

        let mut entries = alerts.log_entries();
        entries.reverse();
        let logged: Vec<Fields> = entries.iter().map(fields).collect();
        let (uninitialized, removed) = (Status::Uninitialized, Status::Removed);
        let (plain, repeating) = (b"plain".to_vec(), b"repeating".to_vec());
        assert_eq!(
            logged,
            [
                (plain.clone(), NOW + 2, 2, None, None, removed, uninitialized, 0, false),
                (repeating.clone(), NOW + 4, 1, None, None, removed, uninitialized, 0, true),
                (plain, NOW + 60, 60, None, None, uninitialized, removed, 0, false),
                (repeating, NOW + 60, 57, None, None, uninitialized, removed, 0, false),
            ]
        );
        // without a host pass the log's ids start at 0: the first entry takes none
        let links: Vec<_> = entries.iter().map(|e| (e.alarm_event_id, e.updates_id, e.updated_by_id)).collect();
        let ids: Vec<u32> = entries.iter().map(|e| e.unique_id).collect();
        assert_eq!(links, [(1, 0, ids[2]), (1, 0, ids[3]), (2, ids[0], 0), (2, ids[1], 0)]);
        assert_eq!(alerts.transitions(), 4);
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
        assert!(alerts.add(&old, rule, &Idle, &|| NOW));
        let old_alert = Arc::clone(&alerts.chart_alerts(&old)[0]);

        assert!(host.charts().free_if(&old, |_| true));
        let new = chart(&host, "t.c", None, "t.ctx", &[]);
        assert!(!Arc::ptr_eq(&old, &new));
        assert!(alerts.add(&new, rule, &Idle, &|| NOW + 1));
        let new_alert = Arc::clone(&alerts.chart_alerts(&new)[0]);
        assert!(!Arc::ptr_eq(&old_alert, &new_alert) && Arc::ptr_eq(&new_alert.chart, &new));
        assert!(alerts.chart_alerts(&old).is_empty(), "a chart's alerts are those on that chart object");
        assert_eq!(old_alert.run().next_event_id, 3, "the old alert was unlinked, with its entry");
        assert_eq!(alerts.count(), 1);

        alerts.unlink_chart(&old, &Idle, &|| NOW + 2);
        assert_eq!(named(&alerts.alerts()), [pair("a", "t.c")]);
        assert!(alerts.is_linked(&host, &new_alert));
        // a live chart keeps its key against a second rule, as before
        assert!(!alerts.add(&new, rule, &Idle, &|| NOW + 3));
    }

    #[test]
    fn a_freed_chart_takes_no_alert() {
        let health = health_with(&rule_text("template", "a", "t.ctx", &[]));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        assert!(host.charts().free_if(&c, |_| true));
        assert!(!alerts.add(&c, &prototypes.get(b"a").unwrap().rules()[0], &Idle, &|| NOW));
        assert!(alerts.alerts().is_empty());
        assert_eq!(alerts.version(), 0);
    }

    /// A clock that counts its reads: the first gives NOW, each later one a second more.
    fn counting(reads: &std::cell::Cell<i64>) -> impl Fn() -> i64 + '_ {
        move || {
            reads.set(reads.get() + 1);
            NOW + reads.get() - 1
        }
    }

    /// `health_initialize_rrdhost()`: C takes the clock for the next log id; with a database the load of an empty
    /// alert log reads it twice more, and both counters start one above what it read. Without a database the log's
    /// ids start at the second itself and the alarm ids are seeded by the first alarm. The log's limit and its
    /// retention are the configuration's from then on.
    #[test]
    fn a_host_s_first_pass_seeds_the_log_s_ids() {
        let (host, config) = (host(&[]), HealthConfig::default());
        let reads = std::cell::Cell::new(0);
        let alerts = HostAlerts::default();
        assert!(!alerts.is_initialized());
        assert_eq!(alerts.latest_log_unique_id(), 0);
        assert_eq!((alerts.log_max(), host.health_log_retention_s()), (0, 0));
        let empty = Scripted { table: Some(Vec::new()), ..Scripted::default() };
        alerts.initialize(&host, &config, &empty, &counting(&reads), &|| true);
        assert!(alerts.is_initialized());
        assert_eq!(reads.get(), 3);
        assert_eq!(alerts.log_counters(), ((NOW + 2) as u32, (NOW + 3) as u32, 0));
        assert_eq!(alerts.latest_log_unique_id(), (NOW + 1) as u32);
        assert_eq!((alerts.log_max(), host.health_log_retention_s()), (1000, 5 * 86400));

        let reads = std::cell::Cell::new(0);
        let alerts = HostAlerts::default();
        alerts.initialize(&host, &config, &Scripted::default(), &counting(&reads), &|| true);
        assert_eq!(reads.get(), 1);
        assert_eq!(alerts.log_counters(), (NOW as u32, 0, 0));
    }

    /// A row of the alert log's table: the entry of that unique id for that alarm, CLEAR since `NOW`.
    fn row(unique_id: u32, alarm_id: u32) -> LoadedRow {
        LoadedRow {
            unique_id,
            alarm_id,
            alarm_event_id: 3,
            config_hash_id: Uuid::Valid([7; 16]),
            updated_by_id: 0,
            updates_id: 0,
            when: NOW,
            duration: 0,
            non_clear_duration: 0,
            flags: entry_flags::PROCESSED,
            exec_run_timestamp: 0,
            delay_up_to_timestamp: NOW,
            name: Some(b"a".to_vec()),
            chart: Some(b"t.c".to_vec()),
            exec: None,
            recipient: Some(Vec::new()),
            source: None,
            units: Some(b"%".to_vec()),
            info: None,
            exec_code: 0,
            new_status: 1,
            old_status: 0,
            delay: 0,
            new_value: 12.0,
            old_value: 0.0,
            last_repeat: 0,
            classification: None,
            component: None,
            r#type: None,
            chart_context: Some(b"t.ctx".to_vec()),
            transition_id: Uuid::Valid([unique_id as u8; 16]),
            global_id: None,
            chart_name: Some(b"t.c".to_vec()),
            summary: None,
        }
    }

    /// `sql_health_alarm_log_load()`: the rows that can be entries go in front of the log one after the other, so
    /// the log is in reverse row order; each is marked as saved and gets its value strings again; a text that is
    /// empty is no text. A row that cannot be an entry is recorded and counted. The ids go on from the highest
    /// loaded ones, and the clock is read only for the log's first id and for the retention cleanup, which takes
    /// what is older than the retention and replaced.
    #[test]
    fn a_first_pass_loads_what_the_table_has() {
        let (host, config) = (host(&[]), HealthConfig::default());
        let old = LoadedRow { when: NOW - 6 * 86400, flags: entry_flags::UPDATED, ..row(900, 77) };
        let table = vec![
            row(500, 40),
            LoadedRow { unique_id: 0, ..row(501, 41) },
            LoadedRow { alarm_id: 0, ..row(502, 42) },
            LoadedRow { name: None, ..row(503, 43) },
            LoadedRow { chart: None, ..row(504, 44) },
            LoadedRow { transition_id: Uuid::Invalid, ..row(505, 45) },
            LoadedRow { config_hash_id: Uuid::Invalid, ..row(506, 46) },
            old,
            LoadedRow { transition_id: Uuid::Null, config_hash_id: Uuid::Null, global_id: Some(9), ..row(507, 47) },
        ];
        let reads = std::cell::Cell::new(0);
        let alerts = HostAlerts::default();
        let env = Scripted { table: Some(table), ..Scripted::default() };
        let ((), records) = netdata_agent_log::capture(|| {
            alerts.initialize(&host, &config, &env, &counting(&reads), &|| true);
        });
        assert_eq!(reads.get(), 2, "the log's first id, and the cleanup's now");
        assert_eq!(alerts.log_counters(), (901, 78, 0));

        let entries = alerts.log_entries();
        let ids: Vec<u32> = entries.iter().map(|entry| entry.unique_id).collect();
        assert_eq!(ids, [507, 500], "reverse row order, without the old replaced entry");
        let (last, first) = (&entries[0], &entries[1]);
        assert_eq!(first.flags, entry_flags::PROCESSED | entry_flags::SAVED);
        assert_eq!((first.new_status, first.old_status), (Status::Clear, Status::Uninitialized));
        assert_eq!((first.alarm_id, first.alarm_event_id, first.global_id), (40, 3, 0));
        assert_eq!((first.name.as_deref(), &first.chart[..]), (Some(&b"a"[..]), &b"t.c"[..]));
        assert_eq!((&first.chart_context[..], &first.chart_name[..]), (&b"t.ctx"[..], &b"t.c"[..]));
        assert_eq!(first.recipient, None, "an empty text is no text");
        assert_eq!(first.new_value_string, format_value_and_unit(12.0, b"%"));
        assert_eq!(first.old_value_string, format_value_and_unit(0.0, b"%"));
        assert_eq!((first.transition_id, first.config_hash_id), ([244; 16], [7; 16]));
        assert_eq!((last.transition_id, last.config_hash_id, last.global_id), ([0; 16], [0; 16], 9));

        let messages: Vec<String> = records.iter().map(|record| record.message.clone().unwrap_or_default()).collect();
        let hostname = host.hostname();
        let wrong = [
            "Got invalid unique id. Ignoring it.",
            "Got invalid alarm id. Ignoring it.",
            "Got null name field. Ignoring it.",
            "Got null chart field. Ignoring it.",
            "Got invalid transition id. Ignoring entry.",
            "Got invalid config hash id. Ignoring entry.",
        ];
        let mut expected: Vec<String> = wrong.iter().map(|wrong| format!("HEALTH [{hostname}]: {wrong}")).collect();
        expected.push(format!("[{hostname}]: Table health_log, loaded 3 alarm entries, errors in 6 entries."));
        assert_eq!(messages, expected);
        assert_eq!(records.last().map(|record| record.priority), Some(Priority::Warning));
    }

    /// The load looks at the service before each row, the one after the last too: a service that stops leaves the
    /// rest of the table unread, and the host is initialized all the same.
    #[test]
    fn a_stopping_service_cuts_the_load_short() {
        let (host, config) = (host(&[]), HealthConfig::default());
        for (looks_allowed, loaded, looks_made) in [(0, 0, 1), (1, 1, 2), (2, 2, 3), (3, 2, 3)] {
            let looks = std::cell::Cell::new(0);
            let running = || {
                looks.set(looks.get() + 1);
                looks.get() <= looks_allowed
            };
            let alerts = HostAlerts::default();
            let env = Scripted { table: Some(vec![row(500, 40), row(501, 41)]), ..Scripted::default() };
            alerts.initialize(&host, &config, &env, &|| NOW, &running);
            assert!(alerts.is_initialized());
            assert_eq!(alerts.log_entries().len(), loaded, "{looks_allowed} looks allowed");
            assert_eq!(looks.get(), looks_made, "{looks_allowed} looks allowed");
        }
    }

    /// `rrdcalc_unlink_from_rrdset()`: unless the agent is exiting the clock is read before the alert's status is
    /// looked at, so also for an alert that is REMOVED and logs nothing.
    #[test]
    fn an_unlink_reads_the_clock_before_it_looks_at_the_status() {
        let health = health_with(&rule_text("template", "a", "t.ctx", &[]));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let prototypes = health.prototypes();
        let rule = &prototypes.get(b"a").unwrap().rules()[0];
        let reads_of_an_unlink = |exiting: bool, removed: bool| {
            let alerts = HostAlerts::default();
            assert!(alerts.add(&c, rule, &Idle, &|| NOW));
            if removed {
                alerts.chart_alerts(&c)[0].run().status = Status::Removed;
            }
            let reads = std::cell::Cell::new(0);
            alerts.unlink_chart(&c, &Scripted { exiting, ..Scripted::default() }, &counting(&reads));
            (reads.get(), alerts.log_entries().len())
        };
        assert_eq!(reads_of_an_unlink(false, false), (1, 2), "the link's entry and the unlink's");
        assert_eq!(reads_of_an_unlink(false, true), (1, 1), "REMOVED already: the clock is read, nothing logged");
        assert_eq!(reads_of_an_unlink(true, false), (0, 1), "exiting");
    }

    /// A status change, a repeat and the obsolete rule look again, under the store's lock, whether the alert is
    /// still linked: a chart freed on another thread since the evaluation logs nothing.
    #[test]
    fn a_change_of_an_alert_whose_chart_was_freed_logs_nothing() {
        let health = health_with(&rule_text("template", "a", "t.ctx", &["repeat: warning 2s critical 2s"]));
        let host = host(&[]);
        let c = chart(&host, "t.c", None, "t.ctx", &[]);
        let alerts = HostAlerts::default();
        let prototypes = health.prototypes();
        assert!(alerts.add(&c, &prototypes.get(b"a").unwrap().rules()[0], &Idle, &|| NOW));
        let alert = Arc::clone(&alerts.chart_alerts(&c)[0]);

        let entry = alerts.transition(&host, &alert, Status::Warning, NOW + 1, &Idle).expect("linked");
        assert_eq!((entry.old_status, entry.new_status), (Status::Uninitialized, Status::Warning));
        assert_eq!((alerts.log_entries().len(), alert.run().status), (2, Status::Warning));

        // the chart leaves the host's index; its free has not reached health yet
        assert!(host.charts().free_if(&c, |_| true));
        assert!(alerts.transition(&host, &alert, Status::Critical, NOW + 2, &Idle).is_none());
        assert!(alerts.repeat(&host, &alert, NOW + 3, &Idle).is_none());
        assert!(alerts.obsolete_removed(&host, &alert, &Idle, &|| NOW + 3).is_none());
        let run = alert.run();
        assert_eq!((alerts.log_entries().len(), run.status, run.times_repeat), (2, Status::Warning, 0));
    }
}
