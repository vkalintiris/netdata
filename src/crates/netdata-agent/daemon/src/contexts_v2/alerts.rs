//! The alerts of the contexts v2 engine (`/api/v2/alerts`, `/api/v3/alerts`), ported from
//! `src/database/contexts/api_v2_contexts_alerts.c`: the alerts the walk keeps, summarised by name, counted by type,
//! component, classification, recipient and collecting module, and listed one by one.
//!
//! What a request asks for decides what is collected and printed: `summary` the summary and the five groupings,
//! `instances` or `values` the list of instances. C's dictionaries keep their items in the order they came, and so
//! do the maps here; the five sets of a summary entry are label sets in C, walked in an order of addresses, and
//! here in the order they came (D220 fork 9).

use indexmap::{IndexMap, IndexSet};
use netdata_agent_health::alert::Status;
use netdata_agent_health::alerts::HostAlerts;
use netdata_agent_health::pass::PassCounts;
use netdata_agent_health::prototype::Prototypes;
use netdata_agent_query::keys::Keys;
use netdata_agent_query::tables::alert_statuses::{CLEAR, CRITICAL, RAISED, UNDEFINED, UNINITIALIZED, WARNING};
use netdata_agent_query::tables::contexts_options::{INSTANCES, JSON_LONG_KEYS, RFC3339, SUMMARY, VALUES};
use netdata_agent_rrd::contexts::Context;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::labels::{Labels, SRC_AUTO};
use netdata_agent_text::json::JsonWriter;
use netdata_agent_text::simple_pattern::SimplePattern;

/// `CONTEXTS_ALERT_STATUSES`: every status word's bit.
pub(super) const STATUSES: u64 = UNINITIALIZED | UNDEFINED | CLEAR | RAISED | WARNING | CRITICAL;

/// `rrdhost_alert_status_snapshot_matches_filter()` over `rrdhost_alert_status_snapshot_read()`: whether a host
/// whose last complete health pass counted `counts` can have an alert the status words keep. A host without a
/// complete pass can (C reads its counts as not valid), and so can any host when no word is given.
pub(super) fn host_may_match(status: u64, counts: Option<PassCounts>) -> bool {
    let Some(c) = counts else {
        return true;
    };
    status & STATUSES == 0
        || (status & UNINITIALIZED != 0 && c.uninitialized != 0)
        || (status & UNDEFINED != 0 && c.undefined != 0)
        || (status & CLEAR != 0 && c.clear != 0)
        || (status & WARNING != 0 && c.warning != 0)
        || (status & CRITICAL != 0 && c.critical != 0)
        || (status & RAISED != 0 && (c.warning != 0 || c.critical != 0))
}

/// The status test of `rrdcontext_matches_alert()`: no word keeps every alert, a removed one too; `raised` keeps
/// RAISED and everything above it.
fn status_kept(filter: u64, status: Status) -> bool {
    filter & STATUSES == 0
        || (filter & UNINITIALIZED != 0 && status == Status::Uninitialized)
        || (filter & UNDEFINED != 0 && status == Status::Undefined)
        || (filter & CLEAR != 0 && status == Status::Clear)
        || (filter & RAISED != 0 && status as i32 >= Status::Raised as i32)
        || (filter & WARNING != 0 && status == Status::Warning)
        || (filter & CRITICAL != 0 && status == Status::Critical)
}

/// What a request keeps of the alerts the walk meets (`rrdcontext_matches_alert()`'s three skips).
pub(super) struct Filters<'a> {
    /// `alert=`: a pattern on the rule's name.
    pub(super) name: Option<&'a SimplePattern>,
    /// The alarm id a `transition=` found; 0 for none (an alarm with id 0 cannot be asked for).
    pub(super) alarm_id: i64,
    /// `status=`: the status words' bits.
    pub(super) status: u64,
}

/// `struct alert_counts` with `alert_counts_add()`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    critical: u64,
    warning: u64,
    clear: u64,
    error: u64,
}

impl Counts {
    /// A removed or an uninitialized alert counts nowhere; an undefined one (or a RAISED one) is an error when
    /// its value is not a number.
    fn add(&mut self, status: Status, value: f64) {
        match status {
            Status::Critical => self.critical += 1,
            Status::Warning => self.warning += 1,
            Status::Clear => self.clear += 1,
            Status::Removed | Status::Uninitialized => {}
            Status::Undefined | Status::Raised => {
                if !value.is_finite() {
                    self.error += 1;
                }
            }
        }
    }

    fn to_json(self, w: &mut JsonWriter, k: Keys) {
        w.member_add_uint64(k.critical(), self.critical);
        w.member_add_uint64(k.warning(), self.warning);
        w.member_add_uint64(k.clear(), self.clear);
        w.member_add_uint64(k.error(), self.error);
    }
}

/// A kept alert as the summary and the groupings read it: the rule's texts (empty for one the rule lacks), the
/// chart's context and collecting module, the host, and the published status and value.
#[derive(Debug, Clone, Copy)]
struct Kept<'a> {
    name: &'a [u8],
    /// The rule's summary as written, not the alert's with its variables replaced.
    summary: &'a [u8],
    recipient: &'a [u8],
    classification: &'a [u8],
    component: &'a [u8],
    r#type: &'a [u8],
    hash: &'a [u8; 16],
    context: &'a [u8],
    /// The chart's `_collect_module` label value, at most 127 bytes; `[unset]` for a chart without one.
    module: &'a [u8],
    machine_guid: &'a str,
    status: Status,
    value: f64,
}

/// `struct alert_v2_entry`: the alerts of one name. Its index in the summary is the entry's `ati`.
#[derive(Debug, Default)]
struct SummaryEntry {
    summary: Vec<u8>,
    contexts: Labels,
    recipients: Labels,
    classifications: Labels,
    components: Labels,
    types: Labels,
    counts: Counts,
    instances: u64,
    /// The machine GUIDs of the hosts with an alert of the name.
    nodes: IndexSet<String>,
    /// The hashes of the rules its alerts come from.
    configs: IndexSet<[u8; 16]>,
}

/// `struct alert_by_x_entry`: the alerts, and the rules by name, that share one type, component, classification,
/// recipient or module.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ByX {
    counts: Counts,
    running: u64,
    silent: u64,
    available: u64,
}

/// One grouping, by its key. An empty key is refused, as C's dictionary refuses a name without a byte.
#[derive(Debug, Default)]
struct Grouping(IndexMap<Vec<u8>, ByX>);

impl Grouping {
    /// `alerts_by_x_insert_callback()`: an alert counts as running (and silent, when its recipient is exactly
    /// `silent`); a rule's name, which comes without an alert, as available.
    fn bump(&mut self, key: &[u8], alert: Option<&Kept<'_>>) {
        if key.is_empty() {
            return;
        }
        let entry = match self.0.get_index_of(key) {
            Some(at) => &mut self.0[at],
            None => self.0.entry(key.to_vec()).or_default(),
        };
        match alert {
            None => entry.available += 1,
            Some(alert) => {
                entry.counts.add(alert.status, alert.value);
                entry.running += 1;
                if alert.recipient == b"silent" {
                    entry.silent += 1;
                }
            }
        }
    }

    /// `contexts_v2_alerts_by_x_to_json()`.
    fn to_json(&self, w: &mut JsonWriter, key: &str, k: Keys) {
        w.member_add_array(Some(key.as_bytes()));
        for (name, entry) in &self.0 {
            w.add_array_item_object();
            w.member_add_string("name", name);
            entry.counts.to_json(w, k);
            w.member_add_uint64("running", entry.running);
            w.member_add_uint64("running_silent", entry.silent);
            if entry.available != 0 {
                w.member_add_uint64("available", entry.available);
            }
            w.object_close();
        }
        w.array_close();
    }
}

/// What `summary` collects: the alerts by name and the five groupings.
#[derive(Debug, Default)]
struct Summary {
    by_name: IndexMap<Vec<u8>, SummaryEntry>,
    by_type: Grouping,
    by_component: Grouping,
    by_classification: Grouping,
    by_recipient: Grouping,
    by_module: Grouping,
}

impl Summary {
    /// `alerts_v2_insert_callback()` or `alerts_v2_conflict_callback()`, then the five groupings; the entry's `ati`.
    fn add(&mut self, alert: &Kept<'_>) -> usize {
        let ati = match self.by_name.get_index_of(alert.name) {
            Some(at) => at,
            None => {
                let entry = SummaryEntry { summary: alert.summary.to_vec(), ..SummaryEntry::default() };
                self.by_name.insert_full(alert.name.to_vec(), entry).0
            }
        };
        let entry = &mut self.by_name[ati];
        // the keys of a label set: sanitised as label names, each once
        let sets = [
            (&mut entry.contexts, alert.context),
            (&mut entry.recipients, alert.recipient),
            (&mut entry.classifications, alert.classification),
            (&mut entry.components, alert.component),
            (&mut entry.types, alert.r#type),
        ];
        for (set, text) in sets {
            if !text.is_empty() {
                set.add(text, b"yes", SRC_AUTO);
            }
        }
        entry.instances += 1;
        entry.counts.add(alert.status, alert.value);
        entry.nodes.insert(alert.machine_guid.to_owned());
        entry.configs.insert(*alert.hash);

        self.by_type.bump(alert.r#type, Some(alert));
        self.by_component.bump(alert.component, Some(alert));
        self.by_classification.bump(alert.classification, Some(alert));
        self.by_recipient.bump(alert.recipient, Some(alert));
        self.by_module.bump(alert.module, Some(alert));
        ati
    }
}

/// `struct sql_alert_instance_v2_entry`: one kept alert, as `alert_instances` lists it.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    ati: usize,
    /// The index its host gets in `nodes` (the host is listed after its contexts are walked).
    ni: usize,
    global_id: u64,
    name: Vec<u8>,
    context: String,
    chart_id: String,
    chart_name: String,
    status: Status,
    family: String,
    /// The rule's info as written.
    info: Vec<u8>,
    /// The alert's summary, its variables replaced.
    summary: Vec<u8>,
    units: Vec<u8>,
    last_transition_id: [u8; 16],
    last_status_change_value: f64,
    last_status_change: i64,
    config_hash_id: [u8; 16],
    source: Vec<u8>,
    recipient: Vec<u8>,
    r#type: Vec<u8>,
    component: Vec<u8>,
    classification: Vec<u8>,
    value: f64,
    last_updated: i64,
}

impl Row {
    /// `contexts_v2_alert_instance_to_json_callback()`, without `mcp`.
    fn to_json(&self, w: &mut JsonWriter, options: u64, k: Keys) {
        let instances = options & INSTANCES != 0;
        let rfc3339 = options & RFC3339 != 0;
        w.add_array_item_object();
        if options & SUMMARY != 0 {
            w.member_add_uint64(k.alerts_index_id(), self.ati as u64);
        }
        w.member_add_uint64(k.node_index(), self.ni as u64);
        if instances {
            w.member_add_uint64(k.alert_global_id(), self.global_id);
        }
        w.member_add_string(k.alert_name(), &self.name);
        if instances {
            w.member_add_string(k.context(), &self.context);
        }
        w.member_add_string(k.instance_id(), &self.chart_id);
        w.member_add_string(k.instance_name(), &self.chart_name);
        if instances {
            w.member_add_string(k.status(), self.status.name());
            w.member_add_string(k.family(), &self.family);
            w.member_add_string("info", &self.info);
            w.member_add_string(k.summary(), &self.summary);
            w.member_add_string("units", &self.units);
            w.member_add_uuid(k.last_transition_id(), &self.last_transition_id);
            w.member_add_double(k.last_transition_value(), self.last_status_change_value);
            w.member_add_time_t_formatted(k.last_transition_timestamp(), self.last_status_change, rfc3339);
            w.member_add_uuid(k.config_hash_id(), &self.config_hash_id);
            w.member_add_string(k.source(), &self.source);
            w.member_add_string(k.recipients(), &self.recipient);
            w.member_add_string(k.type_(), &self.r#type);
            w.member_add_string(k.component(), &self.component);
            w.member_add_string(k.classification(), &self.classification);
        }
        if options & VALUES != 0 {
            w.member_add_double(k.last_updated_value(), self.value);
            w.member_add_time_t_formatted(k.last_updated_timestamp(), self.last_updated, rfc3339);
        }
        w.object_close();
    }
}

/// The text of a rule's optional field: empty for one the rule lacks, as C's `string2str()` of a NULL.
fn text(field: &Option<Vec<u8>>) -> &[u8] {
    field.as_deref().unwrap_or_default()
}

/// The alerts an alerts request keeps (`ctl->alerts`).
#[derive(Debug, Default)]
pub(super) struct Collector {
    /// With `summary`.
    summary: Option<Summary>,
    /// With `instances` or `values`.
    rows: Option<Vec<Row>>,
}

impl Collector {
    /// `rrdcontexts_v2_init_alert_dictionaries()`, for the request's `options`.
    pub(super) fn new(options: u64) -> Self {
        Collector {
            summary: (options & SUMMARY != 0).then(Summary::default),
            rows: (options & (INSTANCES | VALUES) != 0).then(Vec::new),
        }
    }

    /// `rrdcontext_matches_alert()`: the alerts of the context's instances that have a chart, in the charts' link
    /// order, less those the filters skip. Whether any was kept: a context without one does not count for its host.
    /// `ni` is the index the host gets if it is listed.
    pub(super) fn context(
        &mut self,
        rc: &Context,
        host: &Host,
        alerts: Option<&HostAlerts>,
        ni: usize,
        filters: &Filters<'_>,
    ) -> bool {
        let Some(alerts) = alerts else {
            return false;
        };
        let mut kept = false;
        for ri in rc.instances() {
            let Some(chart) = ri.chart() else {
                continue;
            };
            let meta = chart.meta();
            let module = meta.labels.get(b"_collect_module").unwrap_or_default();
            let module = if module.is_empty() { &b"[unset]"[..] } else { &module[..module.len().min(127)] };
            for alert in alerts.chart_alerts(&chart) {
                let config = &alert.config;
                let name = text(&config.name);
                if filters.name.is_some_and(|pattern| !pattern.matches(name)) {
                    continue;
                }
                if filters.alarm_id != 0 && filters.alarm_id != i64::from(alert.id) {
                    continue;
                }
                let state = alert.snapshot();
                if !status_kept(filters.status, state.status) {
                    continue;
                }
                kept = true;
                let ati = self.summary.as_mut().map_or(0, |summary| {
                    summary.add(&Kept {
                        name,
                        summary: text(&config.summary),
                        recipient: text(&config.recipient),
                        classification: text(&config.classification),
                        component: text(&config.component),
                        r#type: text(&config.r#type),
                        hash: &config.hash_id,
                        context: meta.context.as_bytes(),
                        module,
                        machine_guid: host.machine_guid(),
                        status: state.status,
                        value: state.value,
                    })
                });
                if let Some(rows) = &mut self.rows {
                    rows.push(Row {
                        ati,
                        ni,
                        global_id: state.global_id,
                        name: name.to_vec(),
                        context: meta.context.clone(),
                        chart_id: chart.id().to_owned(),
                        chart_name: meta.name.clone().unwrap_or_else(|| chart.id().to_owned()),
                        status: state.status,
                        family: meta.family.clone(),
                        info: text(&config.info).to_vec(),
                        summary: state.summary.unwrap_or_default(),
                        units: text(&config.units).to_vec(),
                        last_transition_id: state.last_transition_id,
                        last_status_change_value: state.last_status_change_value,
                        last_status_change: state.last_status_change,
                        config_hash_id: config.hash_id,
                        source: text(&config.source).to_vec(),
                        recipient: text(&config.recipient).to_vec(),
                        r#type: text(&config.r#type).to_vec(),
                        component: text(&config.component).to_vec(),
                        classification: text(&config.classification).to_vec(),
                        value: state.value,
                        last_updated: state.last_updated,
                    });
                }
            }
        }
        kept
    }

    /// `health_prototype_metadata_foreach()` with `contexts_v2_alerts_by_x_update_prototypes()`: every rule name,
    /// by its first rule, is one more "available" under its type, component, classification and recipient (never
    /// under a module), whatever the request keeps. C does it as it prints a summary, between the alerts by name
    /// and the groupings; it is called once, before [`Collector::to_json`].
    pub(super) fn count_prototypes(&mut self, prototypes: &Prototypes) {
        let Some(summary) = &mut self.summary else {
            return;
        };
        for (_, prototype) in prototypes.iter() {
            let Some(rule) = prototype.rules().first() else {
                continue;
            };
            let config = &rule.config;
            summary.by_type.bump(text(&config.r#type), None);
            summary.by_component.bump(text(&config.component), None);
            summary.by_classification.bump(text(&config.classification), None);
            summary.by_recipient.bump(text(&config.recipient), None);
        }
    }

    /// `contexts_v2_alerts_to_json()`, without `mcp`: with `summary` the alerts by name and the five groupings,
    /// with `instances` or `values` the instances. `node_index` gives the index of a listed host by its GUID.
    pub(super) fn to_json(&self, w: &mut JsonWriter, options: u64, node_index: impl Fn(&str) -> Option<usize>) {
        let k = Keys::with_long(options & JSON_LONG_KEYS != 0);
        if let Some(summary) = &self.summary {
            w.member_add_array(Some(b"alerts"));
            for (ati, (name, entry)) in summary.by_name.iter().enumerate() {
                w.add_array_item_object();
                w.member_add_uint64(k.alerts_index_id(), ati as u64);
                w.member_add_array(Some(k.node_index().as_bytes()));
                for ni in entry.nodes.iter().filter_map(|guid| node_index(guid)) {
                    w.add_array_item_int64(ni as i64);
                }
                w.array_close();
                w.member_add_string(k.alert_name(), name);
                w.member_add_string(k.summary(), &entry.summary);
                entry.counts.to_json(w, k);
                w.member_add_uint64(k.instances_count(), entry.instances);
                w.member_add_uint64(k.nodes_count(), entry.nodes.len() as u64);
                w.member_add_uint64(k.configurations_count(), entry.configs.len() as u64);
                let sets = [
                    (k.contexts(), &entry.contexts),
                    (k.classifications(), &entry.classifications),
                    (k.components(), &entry.components),
                    (k.types(), &entry.types),
                    (k.recipients(), &entry.recipients),
                ];
                for (key, set) in sets {
                    w.member_add_array(Some(key.as_bytes()));
                    for label in set.iter() {
                        w.add_array_item_string(&label.name);
                    }
                    w.array_close();
                }
                w.object_close();
            }
            w.array_close();

            summary.by_type.to_json(w, "alerts_by_type", k);
            summary.by_component.to_json(w, "alerts_by_component", k);
            summary.by_classification.to_json(w, "alerts_by_classification", k);
            summary.by_recipient.to_json(w, "alerts_by_recipient", k);
            summary.by_module.to_json(w, "alerts_by_module", k);
        }
        if let Some(rows) = &self.rows {
            w.member_add_array(Some(b"alert_instances"));
            for row in rows {
                row.to_json(w, options, k);
            }
            w.array_close();
        }
    }
}

#[cfg(test)]
mod tests {
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    const EVERY: [Status; 7] = [
        Status::Removed,
        Status::Undefined,
        Status::Uninitialized,
        Status::Clear,
        Status::Raised,
        Status::Warning,
        Status::Critical,
    ];

    /// `alert_counts_add()`: the counter each status feeds, with a number and without one.
    #[test]
    fn the_counters_follow_the_status_and_the_value() {
        let counted = |status: Status, value: f64| {
            let mut counts = Counts::default();
            counts.add(status, value);
            (counts.critical, counts.warning, counts.clear, counts.error)
        };
        for value in [1.5, f64::NAN, f64::INFINITY] {
            assert_eq!(counted(Status::Critical, value), (1, 0, 0, 0));
            assert_eq!(counted(Status::Warning, value), (0, 1, 0, 0));
            assert_eq!(counted(Status::Clear, value), (0, 0, 1, 0));
            assert_eq!(counted(Status::Removed, value), (0, 0, 0, 0));
            assert_eq!(counted(Status::Uninitialized, value), (0, 0, 0, 0));
        }
        for status in [Status::Undefined, Status::Raised] {
            assert_eq!(counted(status, 1.5), (0, 0, 0, 0));
            assert_eq!(counted(status, f64::NAN), (0, 0, 0, 1));
            assert_eq!(counted(status, f64::NEG_INFINITY), (0, 0, 0, 1));
        }
    }

    /// The status words: none keeps every status, REMOVED too; each word its own status; `raised` RAISED and
    /// everything above; several words any of theirs. REMOVED is kept by no word.
    #[test]
    fn the_status_words_keep_their_statuses() {
        let kept = |filter: u64| EVERY.into_iter().filter(|&status| status_kept(filter, status)).collect::<Vec<_>>();
        assert_eq!(kept(0), EVERY);
        assert_eq!(kept(1 << 20), EVERY);
        assert_eq!(kept(UNINITIALIZED), [Status::Uninitialized]);
        assert_eq!(kept(UNDEFINED), [Status::Undefined]);
        assert_eq!(kept(CLEAR), [Status::Clear]);
        assert_eq!(kept(RAISED), [Status::Raised, Status::Warning, Status::Critical]);
        assert_eq!(kept(WARNING), [Status::Warning]);
        assert_eq!(kept(CRITICAL), [Status::Critical]);
        assert_eq!(kept(CLEAR | CRITICAL), [Status::Clear, Status::Critical]);
        assert_eq!(kept(STATUSES).len(), 6);
    }

    /// The prefilter: a host whose last pass counted no alert of a requested status cannot match; a host without
    /// a complete pass can, and so can any host when no word is given. `raised` asks for a warning or a critical.
    #[test]
    fn a_host_s_counts_tell_whether_it_can_match() {
        let counts = |clear, warning, critical, undefined, uninitialized| {
            Some(PassCounts { clear, warning, critical, undefined, uninitialized })
        };
        let none = counts(0, 0, 0, 0, 0);
        for status in [0, UNINITIALIZED, UNDEFINED, CLEAR, RAISED, WARNING, CRITICAL, STATUSES] {
            assert!(host_may_match(status, None), "{status}");
            assert_eq!(host_may_match(status, none), status == 0, "{status}");
        }
        assert!(host_may_match(CLEAR, counts(1, 0, 0, 0, 0)) && !host_may_match(WARNING, counts(1, 0, 0, 0, 0)));
        assert!(host_may_match(WARNING, counts(0, 1, 0, 0, 0)) && !host_may_match(CRITICAL, counts(0, 1, 0, 0, 0)));
        assert!(host_may_match(CRITICAL, counts(0, 0, 1, 0, 0)) && !host_may_match(WARNING, counts(0, 0, 1, 0, 0)));
        assert!(host_may_match(RAISED, counts(0, 1, 0, 0, 0)) && host_may_match(RAISED, counts(0, 0, 1, 0, 0)));
        assert!(!host_may_match(RAISED, counts(1, 0, 0, 1, 1)));
        assert!(host_may_match(UNDEFINED, counts(0, 0, 0, 1, 0)) && !host_may_match(UNDEFINED, counts(1, 1, 1, 0, 1)));
        assert!(host_may_match(UNINITIALIZED, counts(0, 0, 0, 0, 1)));
        assert!(host_may_match(CLEAR | CRITICAL, counts(0, 0, 1, 0, 0)));
    }

    const HASH_A: [u8; 16] = [0xa1; 16];
    const HASH_B: [u8; 16] = [0xb2; 16];

    /// A kept alert of the rule `name` with these texts, on a chart of `context` collected by `module`, on the host
    /// `guid`.
    #[allow(clippy::too_many_arguments)]
    fn kept<'a>(
        name: &'a str,
        texts: [&'a str; 4],
        hash: &'a [u8; 16],
        context: &'a str,
        module: &'a str,
        guid: &'a str,
        status: Status,
        value: f64,
    ) -> Kept<'a> {
        let [r#type, component, classification, recipient] = texts.map(str::as_bytes);
        Kept {
            name: name.as_bytes(),
            summary: b"the summary of ${name}",
            recipient,
            classification,
            component,
            r#type,
            hash,
            context: context.as_bytes(),
            module: module.as_bytes(),
            machine_guid: guid,
            status,
            value,
        }
    }

    fn printed(collector: &Collector, options: u64, nodes: &[&str]) -> String {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        let index = |guid: &str| nodes.iter().position(|node| *node == guid);
        collector.to_json(&mut w, options, index);
        w.finalize();
        String::from_utf8(w.into_bytes()).unwrap()
    }

    /// The summary: one entry per name in the order first seen, its index the `ati`; the rule's summary as written,
    /// from the first alert of the name; the counters; the hosts as their indexes in `nodes`, each once; the rules'
    /// hashes counted once each; the five sets with each text once, as a label name (so a space becomes an
    /// underscore), in the order first seen, and nothing for an empty text.
    #[test]
    fn the_summary_is_by_name_with_its_sets_and_counts() {
        let mut collector = Collector::new(SUMMARY);
        let summary = collector.summary.as_mut().unwrap();
        let texts = ["System", "CPU", "Utilization", "sysadmin"];
        let (bare, other) = ([""; 4], ["Sys tem", "CPU", "Errors", "silent"]);
        let (nan, undefined) = (f64::NAN, Status::Undefined);
        let atis = [
            summary.add(&kept("cpu_high", texts, &HASH_A, "system.cpu", "stat", "guid-1", Status::Warning, 91.0)),
            summary.add(&kept("ram_low", bare, &HASH_B, "system.ram", "meminfo", "guid-1", Status::Clear, 3.0)),
            summary.add(&kept("cpu_high", texts, &HASH_A, "system.cpu", "stat", "guid-2", Status::Critical, 99.0)),
            summary.add(&kept("cpu_high", other, &HASH_B, "cpu.cpu", "stat", "guid-2", undefined, nan)),
        ];
        assert_eq!(atis, [0, 1, 0, 0]);
        let text = printed(&collector, SUMMARY, &["guid-0", "guid-1", "guid-2"]);
        let alerts = concat!(
            r#"{"alerts":[{"ati":0,"ni":[1,2],"nm":"cpu_high","sum":"the summary of ${name}","cr":1,"wr":1,"#,
            r#""cl":0,"er":1,"in":3,"nd":2,"cfg":2,"ctx":["system.cpu","cpu.cpu"],"cls":["Utilization","Errors"],"#,
            r#""cp":["CPU"],"#,
            r#""ty":["System","Sys_tem"],"to":["sysadmin","silent"]},"#,
            r#"{"ati":1,"ni":[1],"nm":"ram_low","sum":"the summary of ${name}","cr":0,"wr":0,"cl":1,"er":0,"#,
            r#""in":1,"nd":1,"cfg":1,"ctx":["system.ram"],"cls":[],"cp":[],"ty":[],"to":[]}],"#,
        );
        assert!(text.starts_with(alerts), "{text}");
        // the groupings: an empty key is refused, a recipient that is exactly `silent` counts as silent
        let groupings = concat!(
            r#""alerts_by_type":[{"name":"System","cr":1,"wr":1,"cl":0,"er":0,"running":2,"running_silent":0},"#,
            r#"{"name":"Sys tem","cr":0,"wr":0,"cl":0,"er":1,"running":1,"running_silent":1}],"#,
            r#""alerts_by_component":[{"name":"CPU","cr":1,"wr":1,"cl":0,"er":1,"running":3,"running_silent":1}],"#,
            r#""alerts_by_classification":["#,
            r#"{"name":"Utilization","cr":1,"wr":1,"cl":0,"er":0,"running":2,"running_silent":0},"#,
            r#"{"name":"Errors","cr":0,"wr":0,"cl":0,"er":1,"running":1,"running_silent":1}],"#,
            r#""alerts_by_recipient":[{"name":"sysadmin","cr":1,"wr":1,"cl":0,"er":0,"running":2,"running_silent":0},"#,
            r#"{"name":"silent","cr":0,"wr":0,"cl":0,"er":1,"running":1,"running_silent":1}],"#,
            r#""alerts_by_module":[{"name":"stat","cr":1,"wr":1,"cl":0,"er":1,"running":3,"running_silent":1},"#,
            r#"{"name":"meminfo","cr":0,"wr":0,"cl":1,"er":0,"running":1,"running_silent":0}]}"#,
        );
        assert_eq!(&text[alerts.len()..], groupings);
        // no summary asked: nothing is collected and nothing printed
        let none = Collector::new(0);
        assert!(none.summary.is_none() && none.rows.is_none());
        assert_eq!(printed(&none, 0, &[]), "{}");
        // asked and empty: the six arrays, empty
        let empty = concat!(
            r#"{"alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"#,
            r#""alerts_by_recipient":[],"alerts_by_module":[],"alert_instances":[]}"#
        );
        assert_eq!(printed(&Collector::new(SUMMARY | VALUES), SUMMARY | VALUES, &[]), empty);
        assert_eq!(printed(&Collector::new(INSTANCES), INSTANCES, &[]), r#"{"alert_instances":[]}"#);
        // a rule's name counts as available under its texts; without a summary nothing is counted
        let mut collector = Collector::new(SUMMARY);
        collector.count_prototypes(&Prototypes::default());
        assert_eq!(printed(&collector, SUMMARY, &[]), empty.replace(r#","alert_instances":[]"#, ""));
        let mut collector = Collector::new(VALUES);
        collector.count_prototypes(&Prototypes::default());
        assert!(collector.summary.is_none());
    }

    fn row() -> Row {
        Row {
            ati: 2,
            ni: 1,
            global_id: 77,
            name: b"cpu_high".to_vec(),
            context: "system.cpu".into(),
            chart_id: "system.cpu".into(),
            chart_name: "system.cpu_name".into(),
            status: Status::Warning,
            family: "cpu".into(),
            info: b"the info of ${name}".to_vec(),
            summary: b"the summary of cpu_high".to_vec(),
            units: b"%".to_vec(),
            last_transition_id: [0x5a; 16],
            last_status_change_value: 91.5,
            last_status_change: 1_700_000_000,
            config_hash_id: HASH_A,
            source: b"line=3,file=/etc/netdata/health.d/cpu.conf".to_vec(),
            recipient: b"sysadmin".to_vec(),
            r#type: b"System".to_vec(),
            component: b"CPU".to_vec(),
            classification: b"Utilization".to_vec(),
            value: 92.25,
            last_updated: 1_700_000_010,
        }
    }

    /// An instance's members by the request's options, in C's order: `ati` only with `summary`; `gi`, `ctx`, `st`
    /// and the rule's part only with `instances`; the last value and its time only with `values`. A nil UUID and a
    /// value that is not a number print null; `rfc3339` prints the times as texts and a time of 0 as null; the long
    /// keys.
    #[test]
    fn an_instance_prints_what_the_options_ask() {
        let printed = |row: &Row, options: u64| {
            let collector = Collector { summary: None, rows: Some(vec![row.clone()]) };
            super::tests::printed(&collector, options, &[])
        };
        // the text of a UUID whose sixteen bytes are all `byte`
        let uuid = |byte: &str| {
            let (two, four, six) = (byte.repeat(2), byte.repeat(4), byte.repeat(6));
            format!("{four}-{two}-{two}-{two}-{six}")
        };
        let rule = format!(
            concat!(
                r#""fami":"cpu","info":"the info of ${{name}}","sum":"the summary of cpu_high","units":"%","#,
                r#""tr_i":"{}","tr_v":91.5,"tr_t":1700000000,"cfg":"{}","#,
                r#""src":"line=3,file=/etc/netdata/health.d/cpu.conf","to":"sysadmin","tp":"System","cm":"CPU","#,
                r#""cl":"Utilization""#
            ),
            uuid("5a"),
            uuid("a1")
        );
        let values = concat!(
            r#"{"alert_instances":[{"ni":1,"nm":"cpu_high","ch":"system.cpu","ch_n":"system.cpu_name","#,
            r#""v":92.25,"t":1700000010}]}"#
        );
        assert_eq!(printed(&row(), VALUES), values);
        let instances = format!(
            concat!(
                r#"{{"alert_instances":[{{"ni":1,"gi":77,"nm":"cpu_high","ctx":"system.cpu","ch":"system.cpu","#,
                r#""ch_n":"system.cpu_name","st":"WARNING",{}}}]}}"#
            ),
            rule
        );
        assert_eq!(printed(&row(), INSTANCES), instances);
        // (the row's `ati` is printed whenever the request has `summary`: no summary was collected here)
        let all = printed(&row(), SUMMARY | INSTANCES | VALUES);
        assert!(all.starts_with(r#"{"alert_instances":[{"ati":2,"ni":1,"gi":77,"nm":"cpu_high","ctx":"#), "{all}");
        assert!(all.ends_with(r#""cl":"Utilization","v":92.25,"t":1700000010}]}"#), "{all}");

        let bare = Row {
            last_transition_id: [0; 16],
            last_status_change_value: f64::NAN,
            last_status_change: 0,
            value: f64::NAN,
            last_updated: 0,
            summary: Vec::new(),
            units: Vec::new(),
            ..row()
        };
        let text = printed(&bare, INSTANCES | VALUES);
        assert!(text.contains(r#""sum":"","units":"","tr_i":null,"tr_v":null,"tr_t":0,"cfg":"#), "{text}");
        assert!(text.ends_with(r#""v":null,"t":0}]}"#), "{text}");
        let text = printed(&bare, INSTANCES | VALUES | RFC3339);
        assert!(text.contains(r#""tr_v":null,"tr_t":null,"#) && text.ends_with(r#""v":null,"t":null}]}"#), "{text}");
        let text = printed(&row(), VALUES | RFC3339);
        assert!(text.ends_with(r#""v":92.25,"t":"2023-11-14T22:13:30Z"}]}"#), "{text}");

        let long = printed(&row(), INSTANCES | VALUES | JSON_LONG_KEYS);
        let head = concat!(
            r#"{"alert_instances":[{"nodes_array_index":1,"global_id":77,"alert":"cpu_high","context":"system.cpu","#,
            r#""instance_id":"system.cpu","instance":"system.cpu_name","status":"WARNING","family":"cpu","info":"#
        );
        assert!(long.starts_with(head), "{long}");
        let tail = r#""last_transition_value":91.5,"last_transition_timestamp":1700000000,"config_hash_id":""#;
        assert!(long.contains(tail), "{long}");
        let end = concat!(
            r#""recipients":"sysadmin","type":"System","component":"CPU","classification":"Utilization","#,
            r#""last_updated_value":92.25,"last_updated_timestamp":1700000010}]}"#
        );
        assert!(long.ends_with(end), "{long}");
    }
}
