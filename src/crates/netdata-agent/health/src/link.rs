//! Linking: which alerts a host's charts get, and when (`health_prototypes.c` `health_apply_prototypes_to_host()`
//! and its helpers; `health_event_loop.c` `health_initialize_rrdhost()`,
//! `health_execute_delayed_initializations()`), and a host's pass (`health_event_loop_for_host()`) around them.
//!
//! The collecting threads link nothing: they only raise flags on a chart and its host, and a host's pass takes
//! them before it evaluates anything. Beside the health loop, a DynCfg change of one alert's rules links and
//! unlinks that name's alerts on the thread of the change ([`crate::dyncfg`]); unlinking also happens where a chart
//! is freed and where a host is cleaned up.

use std::sync::Arc;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::chart::{Chart, flags};
use netdata_agent_rrd::host::{Host, pending_flags};

use crate::alerts::HostAlerts;
use crate::dyncfg::Ctx;
use crate::matching::{ChartKey, prototype_rules_for_chart};
use crate::notify::Executing;
use crate::pass::{Env, Idle, Pass};
use crate::{Clock, Health};

impl Health {
    /// The host's alerts; `None` until its first health pass, and for a host object that only shares its GUID
    /// with the one the alerts are of.
    pub fn host(&self, host: &Host) -> Option<Arc<HostAlerts>> {
        self.host_by_guid(host.machine_guid()).filter(|alerts| alerts.is_of(host))
    }

    fn host_by_guid(&self, guid: &str) -> Option<Arc<HostAlerts>> {
        self.hosts().get(guid).cloned()
    }

    /// `host->health.default_exec` and `host->health.default_recipient`: the configuration's from the host's first
    /// health pass on (set with the log's limit, before the log is loaded), and no text before it (a host health
    /// never ran for has none).
    pub fn host_defaults(&self, host: &Host) -> (&[u8], &[u8]) {
        match self.host(host).is_some_and(|alerts| alerts.has_defaults()) {
            true => (&self.config.default_exec, &self.config.default_recipient),
            false => (b"", b""),
        }
    }

    /// The host's alerts, made at its first pass. Alerts another host object of this GUID left behind are dropped:
    /// this one starts with its first pass.
    fn host_alerts(&self, host: &Arc<Host>) -> Arc<HostAlerts> {
        let mut hosts = self.hosts();
        match hosts.get(host.machine_guid()) {
            Some(alerts) if alerts.is_of(host) => Arc::clone(alerts),
            _ => {
                let alerts = HostAlerts::new(host);
                hosts.insert(host.machine_guid().to_owned(), Arc::clone(&alerts));
                alerts
            }
        }
    }

    /// `health_prototype_alerts_for_rrdset_incrementally()`: every stored rule that matches the chart is linked to
    /// it; a name the chart already has an alert of keeps that alert. The HEALTH thread looks for a stopping
    /// service before each prototype of the store, whether it matches or not.
    fn alerts_for_chart_incrementally(
        &self,
        host: &Host,
        alerts: &HostAlerts,
        chart: &Arc<Chart>,
        env: &dyn Env,
        clock: Clock,
        running: &dyn Fn() -> bool,
    ) {
        let host_labels = host.labels();
        let meta = chart.meta();
        let key = ChartKey {
            id: chart.id().as_bytes(),
            name: meta.name.as_deref().unwrap_or(chart.id()).as_bytes(),
            context: meta.context.as_bytes(),
            labels: Some(&meta.labels),
        };
        let prototypes = self.prototypes();
        let enabled_alerts = &self.config().enabled_alerts;
        for (_, prototype) in prototypes.iter() {
            if !running() {
                break;
            }
            for (_, rule) in prototype_rules_for_chart(prototype, enabled_alerts, Some(&host_labels), &key) {
                alerts.add(chart, rule, env, clock);
            }
        }
    }

    /// `health_prototype_reset_alerts_for_rrdset()`: the chart's alerts go, then every rule is applied again.
    fn reset_alerts_for_chart(
        &self,
        host: &Host,
        alerts: &HostAlerts,
        chart: &Arc<Chart>,
        env: &dyn Env,
        clock: Clock,
        running: &dyn Fn() -> bool,
    ) {
        alerts.unlink_chart(chart, env, clock);
        self.alerts_for_chart_incrementally(host, alerts, chart, env, clock, running);
    }

    /// `health_apply_prototypes_to_host()`: every alert of the host goes, the log's entries that are no removals
    /// count as replaced, then every chart gets its alerts again, in the host's chart order.
    pub fn apply_prototypes_to_host(&self, host: &Host, env: &dyn Env, clock: Clock, running: &dyn Fn() -> bool) {
        // C tests both before it touches anything: the host's health is enabled, and ran once
        if !host.health_enabled() {
            return;
        }
        let Some(alerts) = self.host(host) else {
            return;
        };
        if !alerts.is_initialized() {
            return;
        }
        alerts.delete_all(env, clock);
        alerts.mark_log_updated();
        for chart in host.charts().all() {
            if !running() {
                break;
            }
            self.reset_alerts_for_chart(host, &alerts, &chart, env, clock, running);
        }
    }

    /// `health_execute_delayed_initializations()`: the host's two pending flags are taken in one step; with neither
    /// nothing is done. Else every chart's two flags are taken: a recheck (the host's or the chart's) unlinks the
    /// chart's alerts and links again; an initialization alone links what is missing.
    fn delayed_initializations(
        &self,
        host: &Host,
        alerts: &HostAlerts,
        env: &dyn Env,
        clock: Clock,
        running: &dyn Fn() -> bool,
    ) {
        let pending = host.take_health_pending();
        if pending == 0 {
            return;
        }
        let host_recheck = pending & pending_flags::LABEL_RECHECK != 0;
        for chart in host.charts().all() {
            let chart_pending = chart.take_health_pending();
            let needs_init = chart_pending & flags::PENDING_HEALTH_INITIALIZATION != 0;
            let needs_recheck = host_recheck || chart_pending & flags::PENDING_LABEL_RECHECK != 0;
            if needs_recheck {
                self.reset_alerts_for_chart(host, alerts, &chart, env, clock, running);
            } else if needs_init {
                self.alerts_for_chart_incrementally(host, alerts, &chart, env, clock, running);
            } else {
                continue;
            }
            if !running() {
                break;
            }
        }
    }

    /// `health_initialize_rrdhost()` and `health_execute_delayed_initializations()`: a host's first pass seeds its
    /// log's ids and links every chart, and every pass takes the pending flags. A stopping service is looked for
    /// where C looks: before the host is initialized, and again before its charts are linked.
    fn link(&self, host: &Arc<Host>, alerts: &HostAlerts, env: &dyn Env, clock: Clock, running: &dyn Fn() -> bool) {
        if !alerts.is_initialized() && running() {
            alerts.initialize(host, &self.config, env, clock, running);
            if running() {
                self.apply_prototypes_to_host(host, env, clock, running);
            }
        }
        self.delayed_initializations(host, alerts, env, clock, running);
    }

    /// The linking steps of a host's pass alone, for a caller that evaluates nothing.
    pub fn host_link(&self, host: &Arc<Host>, clock: Clock, running: &dyn Fn() -> bool) {
        let alerts = self.host_alerts(host);
        self.link(host, &alerts, &Idle, clock, running);
    }

    /// `health_event_loop_for_host()`: a host that may run health is stamped with the loop's iteration, linked,
    /// and, unless its health is postponed, evaluated. A pass that carries the hibernation flag postpones the
    /// host; a host that a connecting child's receiver postponed is left alone until its delay is over.
    pub fn host_pass(&self, host: &Arc<Host>, mut pass: Pass, env: &dyn Env, clock: Clock, running: &dyn Fn() -> bool) {
        if !(pass.gate)() {
            return;
        }
        host.stamp_health_iteration();

        let alerts = self.host_alerts(host);
        if alerts.pending_transitions() != 0 {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "Host \"{}\" has pending alert transitions to save, postponing health checks",
                host.hostname()
            );
            return;
        }
        self.link(host, &alerts, env, clock, running);

        let hostname = host.hostname();
        if pass.apply_hibernation_delay {
            let postpone = self.config().postpone_s;
            nd_log!(Source::Daemon, Priority::Debug, "[{hostname}]: Postponing health checks for {postpone} seconds.");
            host.set_health_delay_up_to(pass.now.saturating_add(i64::from(postpone)));
        }
        if host.health_delay_up_to() != 0 {
            if pass.now < host.health_delay_up_to() {
                return;
            }
            nd_log!(Source::Daemon, Priority::Debug, "[{hostname}]: Resuming health checks after delay.");
            host.set_health_delay_up_to(0);
        }

        self.evaluate_host(host, &alerts, &mut pass, env, clock, running);
    }

    /// `health_apply_prototypes_to_all_hosts()`: every host of the index in its order, each as
    /// [`Health::apply_prototypes_to_host`] takes or passes it. A stopping service ends the walk on the HEALTH
    /// thread only.
    pub fn apply_prototypes_to_all_hosts(&self, ctx: &Ctx<'_>) {
        let running = || !ctx.env.is_health_thread() || ctx.env.service_running();
        for host in ctx.hosts.all() {
            self.apply_prototypes_to_host(&host, ctx.env, ctx.clock, &running);
        }
    }

    /// `rrdcalc_child_disconnected()`: a child's receiver detaches. The host's alerts go, each with its REMOVED
    /// entry; then every chart of the host, and last the host, are flagged for the alerts to be linked again by the
    /// first pass after the child's return (a returning child sends the same charts, which raises no flag by
    /// itself, and a host's health is initialized once). The flags are raised for a host health never ran for too.
    pub fn child_disconnected(&self, host: &Host, env: &dyn Env, clock: Clock) {
        if let Some(alerts) = self.host(host) {
            alerts.delete_all(env, clock);
        }
        for chart in host.charts().all() {
            chart.flags_set_and_clear(flags::PENDING_HEALTH_INITIALIZATION, 0);
        }
        host.raise_pending_flags(pending_flags::HEALTH_INITIALIZATION);
    }

    /// `rrdset_delete_callback()`: the alerts on a freed chart go, on the thread that frees it. A chart index knows
    /// its host by machine GUID only; the alerts are found by the chart object, so another host object of that
    /// GUID, or another chart of that id, loses nothing.
    pub fn chart_freed(&self, host_guid: &str, chart: &Chart, env: &dyn Env, clock: Clock) {
        if let Some(alerts) = self.host_by_guid(host_guid) {
            alerts.unlink_chart(chart, env, clock);
        }
    }

    /// `rrdhost_cleanup_data_collection_and_health()`: every alert of that host object goes, before its charts do.
    /// The host stays initialized, as C never clears that flag.
    pub fn host_cleanup(&self, host: &Host, env: &dyn Env, clock: Clock) {
        if let Some(alerts) = self.host(host) {
            alerts.delete_all(env, clock);
        }
    }

    /// The same cleanup after the host's charts were freed: its alert index and its log go; its id counters stay.
    pub fn host_charts_flushed(&self, host: &Host) {
        if let Some(alerts) = self.host(host) {
            // C frees the log's entries, and one whose notification runs has its command killed, with no grace
            // (`health_alarm_log_free_one_nochecks_nounlink()`). C leaves such an entry on the list HEALTH waits
            // on and later walks freed memory there: here the item leaves the queue with its entry
            let of_host = |item: &Executing| std::ptr::eq(item.alerts.as_ptr(), Arc::as_ptr(&alerts));
            let killed: Vec<Executing> = {
                let mut executing = self.executing();
                let (killed, kept): (Vec<_>, Vec<_>) = executing.drain(..).partition(of_host);
                executing.extend(kept);
                killed
            };
            for item in killed {
                item.execution.kill(0);
            }
            alerts.charts_flushed();
        }
    }

    /// `rrdhost_free_unlinked()`, after the cleanup: that host object is forgotten, so that a new host of its GUID
    /// starts with its first pass.
    pub fn host_freed(&self, host: &Host) {
        let mut hosts = self.hosts();
        if hosts.get(host.machine_guid()).is_some_and(|alerts| alerts.is_of(host)) {
            hosts.remove(host.machine_guid());
        }
    }
}

#[cfg(test)]
mod tests {
    use netdata_agent_rrd::host::Host;

    use super::*;
    use crate::alert::Alert;
    use crate::testing::{chart, health_with, host, named, pair, rule_text};

    const NOW: i64 = 1_700_000_000;

    fn running() -> bool {
        true
    }

    /// The host's alerts as (name, chart id), in evaluation order.
    fn linked(health: &Health, host: &Host) -> Vec<(String, String)> {
        named(&health.host(host).map(|alerts| alerts.alerts()).unwrap_or_default())
    }

    fn alert_of(health: &Health, host: &Host, chart: &Chart, name: &[u8]) -> Arc<Alert> {
        let alerts = health.host(host).expect("the host's alerts").chart_alerts(chart);
        Arc::clone(alerts.iter().find(|alert| alert.name() == name).expect("the alert"))
    }

    /// The rules of the tests: a template of each context, an alarm on a chart by its name, and a template only
    /// for hosts of another region.
    fn rules() -> String {
        rule_text("template", "on_a", "ctx.a", &[])
            + &rule_text("template", "on_b", "ctx.b", &[])
            + &rule_text("alarm", "by_name", "t.named", &[])
            + &rule_text("template", "elsewhere", "ctx.a", &["host labels: region=us"])
    }

    /// A host's first pass links every chart, in the host's chart order and per chart in the store's order, alarms
    /// before templates within a name.
    #[test]
    fn the_first_pass_links_every_chart() {
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        let a1 = chart(&host, "t.a1", Some("named"), "ctx.a", &[]);
        chart(&host, "t.b1", None, "ctx.b", &[]);
        chart(&host, "t.other", None, "ctx.other", &[]);
        assert!(health.host(&host).is_none(), "no alerts before the first pass");

        health.host_link(&host, &|| NOW, &running);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1"), pair("by_name", "t.a1"), pair("on_b", "t.b1")]);
        // one link each: the charts' pending initialization found every key in place
        assert_eq!(alert_of(&health, &host, &a1, b"on_a").run().next_event_id, 2);
        assert_eq!(host.take_health_pending(), 0, "the pass took the host's flags");
        assert_eq!(a1.take_health_pending(), 0, "and each chart's");
    }

    /// Localhost starts with a label recheck pending (its labels are loaded after its charts' host exists): its
    /// first pass links, unlinks and links again.
    #[test]
    fn a_pending_host_recheck_relinks_after_the_first_link() {
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        let a1 = chart(&host, "t.a1", None, "ctx.a", &[]);
        host.raise_label_recheck();
        health.host_link(&host, &|| NOW, &running);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1")]);
        // the alert linked again takes its alarm id back from the log, and the link is its third entry
        let alert = alert_of(&health, &host, &a1, b"on_a");
        assert_eq!((alert.id, alert.run().next_event_id), (NOW as u32, 4));
    }

    /// `health_execute_delayed_initializations()`'s table: what each combination of the host's and a chart's flags
    /// does to the chart's alerts.
    #[test]
    fn the_pending_flags_decide_what_a_pass_does_to_a_chart() {
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        let a1 = chart(&host, "t.a1", None, "ctx.a", &[]);
        let a2 = chart(&host, "t.a2", None, "ctx.a", &[]);
        health.host_link(&host, &|| NOW, &running);
        let before = |c: &Arc<Chart>| alert_of(&health, &host, c, b"on_a");
        let (first1, first2) = (before(&a1), before(&a2));
        let same = |c: &Arc<Chart>, old: &Arc<Alert>| Arc::ptr_eq(&alert_of(&health, &host, c, b"on_a"), old);

        // nothing pending: nothing happens
        health.host_link(&host, &|| NOW + 1, &running);
        assert!(same(&a1, &first1) && same(&a2, &first2));

        // a chart's flag without its host's: the pass does not look at the charts
        a1.flags_set_and_clear(flags::PENDING_LABEL_RECHECK, 0);
        health.host_link(&host, &|| NOW + 2, &running);
        assert!(same(&a1, &first1));
        assert_eq!(a1.take_health_pending(), flags::PENDING_LABEL_RECHECK, "left for a pass that looks");

        // a chart's initialization: what is missing is linked, what is there stays
        a1.raise_health_init();
        health.host_link(&host, &|| NOW + 3, &running);
        assert!(same(&a1, &first1) && same(&a2, &first2));

        // a chart's recheck: its alerts are made again; the other chart's stay
        a1.raise_label_recheck();
        health.host_link(&host, &|| NOW + 4, &running);
        assert!(!same(&a1, &first1) && same(&a2, &first2));
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a2"), pair("on_a", "t.a1")], "linked again: at the end");

        // the host's recheck: every chart's alerts are made again, and the new labels count
        let second1 = before(&a1);
        host.update_labels(|labels| labels.add(b"region", b"us", netdata_agent_rrd::labels::SRC_CONFIG));
        host.raise_label_recheck();
        health.host_link(&host, &|| NOW + 5, &running);
        assert!(!same(&a1, &second1) && !same(&a2, &first2));
        assert_eq!(
            linked(&health, &host),
            [pair("on_a", "t.a1"), pair("elsewhere", "t.a1"), pair("on_a", "t.a2"), pair("elsewhere", "t.a2")]
        );
    }

    /// A flag raised while a pass works on its chart is kept for the next pass: the flags are taken in one step,
    /// before the work.
    #[test]
    fn a_flag_raised_during_a_pass_is_kept_for_the_next() {
        use netdata_agent_rrd::host::pending_flags;
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        let a1 = chart(&host, "t.a1", None, "ctx.a", &[]);
        health.host_link(&host, &|| NOW, &running);

        a1.raise_label_recheck();
        // the collector raises both again while the pass makes the chart's alerts again (the clock is read there)
        let raised = std::cell::Cell::new(false);
        let clock = || {
            if !raised.replace(true) {
                a1.raise_label_recheck();
                host.raise_label_recheck();
            }
            NOW + 1
        };
        health.host_link(&host, &clock, &running);
        assert!(raised.get());
        assert_eq!(a1.take_health_pending(), flags::PENDING_LABEL_RECHECK);
        assert_eq!(host.take_health_pending(), pending_flags::HEALTH_INITIALIZATION | pending_flags::LABEL_RECHECK);
    }

    /// Alerts belong to a host object. Another object of the same machine GUID (one that found the GUID taken in
    /// the index and is freed at once) has none, and its cleanup and its free leave the owner's alone. Two hosts
    /// with a chart of one id each keep their own.
    #[test]
    fn a_host_s_alerts_are_its_own() {
        use crate::testing::host_of;
        let health = health_with(&rules());
        let owner = host(&[("region", "eu")]);
        let a1 = chart(&owner, "t.a1", None, "ctx.a", &[]);
        health.host_link(&owner, &|| NOW, &running);
        let one = [pair("on_a", "t.a1")];
        assert_eq!(linked(&health, &owner), one);

        let twin = host(&[("region", "eu")]);
        assert_eq!(twin.machine_guid(), owner.machine_guid());
        assert!(health.host(&twin).is_none());
        health.host_cleanup(&twin, &Idle, &|| NOW + 1);
        health.host_freed(&twin);
        assert_eq!(linked(&health, &owner), one);

        // another host with a chart of the same id
        let other = host_of("99999999-2222-4333-8444-555555555555", &[("region", "eu")]);
        let other_a1 = chart(&other, "t.a1", None, "ctx.a", &[]);
        health.host_link(&other, &|| NOW + 2, &running);
        assert_eq!(linked(&health, &other), one);
        // a chart's free names its host by GUID and the chart by its object
        health.chart_freed(other.machine_guid(), &a1, &Idle, &|| NOW + 3);
        assert_eq!((linked(&health, &owner), linked(&health, &other)), (one.to_vec(), one.to_vec()));
        health.chart_freed(owner.machine_guid(), &a1, &Idle, &|| NOW + 3);
        assert!(linked(&health, &owner).is_empty());
        assert_eq!(linked(&health, &other), one);
        health.chart_freed(other.machine_guid(), &other_a1, &Idle, &|| NOW + 4);
        assert!(linked(&health, &other).is_empty());

        // a host object that takes over a GUID starts with its first pass
        chart(&twin, "t.a1", None, "ctx.a", &[]);
        twin.take_health_pending();
        health.host_link(&twin, &|| NOW + 5, &running);
        assert_eq!(linked(&health, &twin), one);
        assert!(health.host(&owner).is_none());
    }

    /// A new chart is linked by the pass after its definition; a freed one loses its alerts where it is freed.
    #[test]
    fn a_new_chart_is_linked_and_a_freed_one_unlinked() {
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        chart(&host, "t.a1", None, "ctx.a", &[]);
        health.host_link(&host, &|| NOW, &running);

        let a2 = chart(&host, "t.a2", None, "ctx.a", &[]);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1")], "not before a pass");
        health.host_link(&host, &|| NOW + 1, &running);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1"), pair("on_a", "t.a2")]);

        let alert = alert_of(&health, &host, &a2, b"on_a");
        assert!(host.charts().free_if(&a2, |_| true));
        health.chart_freed(host.machine_guid(), &a2, &Idle, &|| NOW + 2);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1")]);
        assert_eq!(alert.run().next_event_id, 3, "the unlink logged");

        health.host_cleanup(&host, &Idle, &|| NOW + 3);
        assert!(linked(&health, &host).is_empty());
        // cleaned, and still initialized: a pass with nothing pending links nothing
        health.host_link(&host, &|| NOW + 4, &running);
        assert!(linked(&health, &host).is_empty());

        // freed: forgotten, and a host of that GUID starts with its first pass
        health.host_freed(&host);
        assert!(health.host(&host).is_none());
        health.host_link(&host, &|| NOW + 5, &running);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1")]);
    }

    /// While the service is stopping a host is not initialized, and nothing is linked: the delayed initializations
    /// take the host's flags and the first chart's, find the service stopping before the first rule, and stop
    /// after that chart, as C does.
    #[test]
    fn a_stopping_service_does_not_initialize_a_host() {
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        let a1 = chart(&host, "t.a1", None, "ctx.a", &[]);
        let a2 = chart(&host, "t.a2", None, "ctx.a", &[]);

        health.host_link(&host, &|| NOW, &|| false);
        assert!(linked(&health, &host).is_empty());
        assert!(!health.host(&host).expect("the host's alerts").is_initialized());
        assert_eq!(host.take_health_pending(), 0, "the pass took the host's flags");
        assert_eq!(a1.take_health_pending(), 0, "and the first chart's");
        assert_ne!(a2.take_health_pending(), 0, "and stopped before the second");

        // still not initialized: the next pass is the host's first, and links every chart whatever its flags
        health.host_link(&host, &|| NOW + 1, &running);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1"), pair("on_a", "t.a2")]);
    }

    /// A host's first pass stops between charts, and between the rule names of a chart, when the service stops:
    /// it is looked for twice by the initialization, then before each chart and before each name of the store
    /// (four here), whether the name has a rule for the chart or not.
    #[test]
    fn an_initialization_stops_between_charts() {
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        chart(&host, "t.a1", Some("named"), "ctx.a", &[]);
        chart(&host, "t.a2", None, "ctx.a", &[]);

        // running for the pass's two looks, for the first chart and for its four names
        let calls = std::cell::Cell::new(0);
        let for_one_chart = || {
            calls.set(calls.get() + 1);
            calls.get() <= 7
        };
        health.host_link(&host, &|| NOW, &for_one_chart);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1"), pair("by_name", "t.a1")]);

        // running for the first two names of the first chart: its third name is not linked
        let health = health_with(&rules());
        let calls = std::cell::Cell::new(0);
        let for_two_names = || {
            calls.set(calls.get() + 1);
            calls.get() <= 5
        };
        health.host_link(&host, &|| NOW, &for_two_names);
        assert_eq!(linked(&health, &host), [pair("on_a", "t.a1")]);

        // stopping between the pass's two looks: initialized, and nothing linked by the initialization
        let health = health_with(&rules());
        let calls = std::cell::Cell::new(0);
        let for_the_first_look = || {
            calls.set(calls.get() + 1);
            calls.get() <= 1
        };
        health.host_link(&host, &|| NOW, &for_the_first_look);
        assert!(linked(&health, &host).is_empty(), "the charts' flags were taken by the pass above");
        health.host_link(&host, &|| NOW + 1, &running);
        assert!(linked(&health, &host).is_empty(), "initialized already: no first pass, and nothing pending");
    }

    /// `rrdhost_cleanup_data_collection_and_health()`: the host's alerts go, with their REMOVED entries; once its
    /// charts are gone the alert index starts over (a data answer's hard hash counts from 0 again) and the log is
    /// emptied. The ids and the count of transitions go on from where they were, and the host stays initialized.
    #[test]
    fn a_cleanup_empties_the_index_and_the_log_and_keeps_the_counters() {
        let health = health_with(&rules());
        let host = host(&[("region", "eu")]);
        chart(&host, "t.a1", None, "ctx.a", &[]);
        health.host_link(&host, &|| NOW, &running);
        let alerts = health.host(&host).expect("the host's alerts");
        assert_eq!((alerts.log_entries().len(), alerts.transitions(), alerts.version()), (1, 1, 1));
        let (next_log_id, next_alarm_id, _) = alerts.log_counters();

        health.host_cleanup(&host, &Idle, &|| NOW + 1);
        assert!(alerts.alerts().is_empty());
        assert_eq!((alerts.log_entries().len(), alerts.transitions(), alerts.version()), (2, 2, 2));

        health.host_charts_flushed(&host);
        assert_eq!((alerts.log_entries().len(), alerts.transitions(), alerts.version()), (0, 2, 0));
        assert_eq!(alerts.log_counters(), (next_log_id + 1, next_alarm_id, 0));
        assert!(alerts.is_initialized());
    }
}
