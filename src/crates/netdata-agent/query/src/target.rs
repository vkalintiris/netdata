//! The query target (`query_target_create()`, `src/database/contexts/query_target.c`): the nodes, contexts,
//! instances and dimensions a data request selects, and the metrics admitted for querying. Spec §3.1-3.8.
//!
//! The window is converted here only for admission; `query_target_calculate_window()` (spec §4.1) is `window.rs`'s.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Instant;

use netdata_agent_rrd::chart::{Chart, dim_flags};
use netdata_agent_rrd::contexts::{self, Context, Instance, Metric, flags};
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::labels::PatternArray;
use netdata_agent_rrd::storage::{AlertClass, ChartAlert, TierHandle};
use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;
use netdata_agent_storage::storage_point::StoragePoint;
use netdata_agent_text::print::print_uuid_lower;
use netdata_agent_text::simple_pattern::{SimplePattern, SimplePatternResult};
use netdata_agent_text::time_window::relative_window_to_absolute_query;

use crate::id::{self, IdKind};
use crate::request::{DataRequest, is_valid_sp};
use crate::tables::{Aggregation, group_by, options};

/// `QUERY_STATUS_*`.
pub mod status {
    pub const QUERIED: u32 = 1;
    pub const DIMENSION_HIDDEN: u32 = 2;
    pub const EXCLUDED: u32 = 4;
    pub const FAILED: u32 = 8;
}

/// `RRDR_DIMENSION_*`: a query metric's status, which becomes its result column's flags (`r->od`).
pub mod metric_status {
    pub const HIDDEN: u32 = 1 << 0;
    pub const NONZERO: u32 = 1 << 1;
    pub const SELECTED: u32 = 1 << 2;
    pub const QUERIED: u32 = 1 << 3;
    pub const FAILED: u32 = 1 << 4;
    pub const GROUPED: u32 = 1 << 5;
}

/// The counters a node, context or instance keeps (`QUERY_METRICS_COUNTS`, `QUERY_INSTANCES_COUNTS`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub selected: u32,
    pub excluded: u32,
    pub queried: u32,
    pub failed: u32,
}

/// `QUERY_ALERTS_COUNTS`: the alerts of a version-2 query's queryable instances, by published status.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AlertCounts {
    pub clear: u32,
    pub warning: u32,
    pub critical: u32,
    pub other: u32,
}

impl AlertCounts {
    /// One alert more of that class.
    pub fn count(&mut self, class: AlertClass) {
        match class {
            AlertClass::Clear => self.clear += 1,
            AlertClass::Warning => self.warning += 1,
            AlertClass::Critical => self.critical += 1,
            AlertClass::Other => self.other += 1,
        }
    }

    pub fn add(&mut self, other: &AlertCounts) {
        self.clear += other.clear;
        self.warning += other.warning;
        self.critical += other.critical;
        self.other += other.other;
    }

    /// No alert at all: C then prints nothing.
    pub fn is_empty(&self) -> bool {
        *self == AlertCounts::default()
    }
}

#[derive(Debug)]
pub struct QueryNode {
    pub host: Arc<Host>,
    pub node_id: Option<String>,
    pub metrics: Counts,
    pub instances: Counts,
    pub alerts: AlertCounts,
    /// The positive points of its queried metrics, merged (v2).
    pub query_points: StoragePoint,
    /// How long its metrics took to execute (`qn->duration_ut`); never set for the last node queried.
    pub duration_ut: u64,
}

#[derive(Debug)]
pub struct QueryContext {
    pub node: usize,
    pub rc: Arc<Context>,
    pub metrics: Counts,
    pub instances: Counts,
    pub alerts: AlertCounts,
    pub query_points: StoragePoint,
}

#[derive(Debug)]
pub struct QueryInstance {
    pub context: usize,
    pub ri: Arc<Instance>,
    /// `<id>@<machine_guid>` (v1: the id), `<name>@<hostname>` (v1: the name).
    pub id_fqdn: String,
    pub name_fqdn: String,
    pub metrics: Counts,
    pub alerts: AlertCounts,
    pub query_points: StoragePoint,
}

#[derive(Debug)]
pub struct QueryDimension {
    pub instance: usize,
    pub rm: Arc<Metric>,
    pub status: u32,
    /// The position among its instance's metrics, deleted and dropped ones included.
    pub priority: usize,
}

/// A tier's storage and retention at admission (`qm->tiers[t]`): no handle when the tier does not hold the metric.
#[derive(Debug, Clone, Default)]
pub struct TierSnapshot {
    pub handle: Option<TierHandle>,
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub update_every_s: i64,
    /// `weight`: the points density the best-tier choice gave the tier, `-i64::MAX` when the tier does not overlap
    /// the window; 0 when no choice ran.
    pub weight: i64,
}

/// An admitted metric (`QUERY_METRIC`).
#[derive(Debug)]
pub struct QueryMetric {
    pub dimension: usize,
    pub status: u32,
    pub values_stored_as_rates: bool,
    /// Every tier in use; the others stay empty.
    pub tiers: [TierSnapshot; RRD_STORAGE_TIERS],
    /// What the execution read, merged (`qm->query_points`).
    pub query_points: StoragePoint,
    /// `qm->plan`: the plans in start order, kept for a failed plan; empty for the LATEST fast path and when planning
    /// fails before the first plan.
    pub plan: Vec<crate::plan::PlanEntry>,
    /// The v2 group it joined (`qm->grouped_as`).
    pub grouped_as: GroupedAs,
}

impl QueryMetric {
    /// The tiers as the planner sees them.
    pub fn tier_views(&self) -> [crate::plan::TierView; RRD_STORAGE_TIERS] {
        std::array::from_fn(|t| {
            let tier = &self.tiers[t];
            crate::plan::TierView {
                has_handle: tier.handle.is_some(),
                first_time_s: tier.first_time_s,
                last_time_s: tier.last_time_s,
                update_every_s: tier.update_every_s,
            }
        })
    }
}

/// `qm->grouped_as`: the slot in the first group-by pass and in the latest one, and that group's identity.
#[derive(Debug, Default, Clone)]
pub struct GroupedAs {
    pub first_slot: usize,
    pub slot: usize,
    pub id: String,
    pub name: String,
    pub units: String,
}

/// `qt->db`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Db {
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub minimum_latest_update_every_s: i64,
    /// `qt->db.tiers[]`.
    pub tiers: [TierStats; RRD_STORAGE_TIERS],
}

/// `qt->db.tiers[t]`: over every candidate metric, the smallest update every and the widest retention; the plans
/// initialised and the points read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TierStats {
    pub update_every: i64,
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub queries: usize,
    pub points: usize,
}

/// The selection window (`qt->window` before `query_target_calculate_window()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionWindow {
    pub after: i64,
    pub before: i64,
    pub relative: bool,
    /// The request options without PERCENTAGE when a pass groups by percentage (`window.options`).
    pub options: u64,
}

#[derive(Debug)]
pub struct QueryTarget {
    pub request: DataRequest,
    pub window: SelectionWindow,
    pub start_s: i64,
    pub nodes: Vec<QueryNode>,
    pub contexts: Vec<QueryContext>,
    pub instances: Vec<QueryInstance>,
    pub dimensions: Vec<QueryDimension>,
    pub query: Vec<QueryMetric>,
    pub db: Db,
    /// `qt->id` (spec §3.9).
    pub id: String,
    /// v1 `chart=` named the chart (`qt->request.st`).
    pub chart_scoped: bool,
    /// `qt->instances.chart_label_key_pattern`.
    pub chart_label_key: Option<SimplePattern>,
    /// When the target was built and when its metrics were executed (`qt->timings`).
    pub preprocessed: Instant,
    pub executed: Option<Instant>,
    /// The positive points of every queried metric, merged (`qt->query_points`, v2).
    pub query_points: StoragePoint,
    /// Each group-by pass's label keys after the passes were merged (`qt->group_by[g].label_keys`).
    pub group_by_label_keys: [Vec<Vec<u8>>; 2],
    /// `qt->versions` (v2).
    pub versions: Versions,
}

/// `struct query_versions` plus the host index's version: the dictionary versions summed over the hosts in scope.
/// The hub queue is not ported, so its term is 0 (as in C for an unclaimed agent).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Versions {
    pub nodes_hard_hash: u64,
    pub contexts_hard_hash: u64,
    pub contexts_soft_hash: u64,
    /// The hosts' alert dictionaries' versions, and their counts of alert transitions.
    pub alerts_hard_hash: u64,
    pub alerts_soft_hash: u64,
}

/// What selects the metrics: v1's routed host (and chart), or every host for v2/v3.
pub enum Source<'a> {
    /// `chart` is the chart `chart=` found (`qtr->st`).
    V1 {
        host: &'a Arc<Host>,
        chart: Option<Arc<Chart>>,
    },
    V2 { hosts: Vec<Arc<Host>> },
}

/// `pattern_array_add_simple_pattern()` for labels (`database/pattern-array.c`): per label key, the exact
/// `key:value` patterns; the words stop at the first lone `*` or at the first word without `:`.
fn label_pattern_array(sp: &SimplePattern) -> PatternArray {
    let mut array = PatternArray::default();
    for word in sp.words() {
        let Some(word) = word else { break };
        let Some(colon) = word.iter().position(|&c| c == b':') else {
            break;
        };
        let pattern = SimplePattern::new(
            word,
            netdata_agent_text::simple_pattern::Separators::None,
            netdata_agent_text::simple_pattern::SimplePatternMode::Exact,
            true,
        );
        array.add(&word[..colon.min(200)], pattern);
    }
    array
}

/// `query_matches_retention()`.
pub fn matches_retention(after: i64, before: i64, first: i64, last: i64, ue: i64) -> bool {
    first - 2 * ue <= before && last + 2 * ue >= after
}

struct Walk<'a> {
    req: &'a DataRequest,
    window: SelectionWindow,
    start_s: i64,
    match_ids: bool,
    match_names: bool,
    scope_instances: Option<SimplePattern>,
    instances: Option<SimplePattern>,
    scope_dimensions: Option<SimplePattern>,
    dimensions: Option<SimplePattern>,
    scope_labels: Option<PatternArray>,
    labels: Option<PatternArray>,
    alerts: Option<SimplePattern>,
    needs_all_dimensions: bool,
    qt: QueryTarget,
}

fn pattern(v: &Option<Vec<u8>>) -> Option<SimplePattern> {
    v.as_deref().and_then(SimplePattern::from_web)
}

impl Walk<'_> {
    /// `query_instance_matches()`: the first match that is not NOT decides.
    fn instance_matches(
        &self,
        sp: &SimplePattern,
        ri: &Instance,
        qi_id: &str,
        qi_name: &str,
        node_id: Option<&str>,
    ) -> bool {
        let m = |s: &str| sp.matches_extract(s.as_bytes(), 0).0;
        let name = ri.state().name;
        let mut r = if self.match_ids {
            m(ri.id())
        } else {
            SimplePatternResult::NotMatched
        };
        if r == SimplePatternResult::NotMatched
            && self.match_names
            && (name != ri.id() || !self.match_ids)
        {
            r = m(&name);
        }
        if r == SimplePatternResult::NotMatched && self.match_ids {
            r = m(qi_id);
        }
        if r == SimplePatternResult::NotMatched && self.match_names {
            r = m(qi_name);
        }
        if r == SimplePatternResult::NotMatched
            && self.match_ids
            && let Some(node_id) = node_id
        {
            r = m(&format!("{}@{node_id}", ri.id()));
        }
        r == SimplePatternResult::MatchedPositive
    }

    /// Id then name, as the dimension patterns match.
    fn dimension_matches(&self, sp: &SimplePattern, rm: &Metric) -> SimplePatternResult {
        let name = rm.state().name;
        let mut r = if self.match_ids {
            sp.matches_extract(rm.id().as_bytes(), 0).0
        } else {
            SimplePatternResult::NotMatched
        };
        if r == SimplePatternResult::NotMatched
            && self.match_names
            && (name != rm.id() || !self.match_ids)
        {
            r = sp.matches_extract(name.as_bytes(), 0).0;
        }
        r
    }

    /// Retention from the contexts tree, for dimensions that are counted but not queried.
    fn excluded_retention_matches(&self, rm: &Metric, ri: &Instance) -> bool {
        let state = rm.state();
        let last = if rm.flags.is_collected() {
            self.start_s
        } else {
            state.last_time_s
        };
        matches_retention(
            self.window.after,
            self.window.before,
            state.first_time_s,
            last,
            i64::from(ri.state().update_every_s),
        )
    }

    /// `query_metric_add()`: each tier in use with its storage and retention (the tier's update every is its
    /// grouping of the chart's); every candidate counts in `db.tiers`; admitted when some tier holds the metric and
    /// the window meets their common retention. `false` releases the handles.
    fn metric_add(
        &mut self,
        host: &Host,
        dimension: usize,
        rm: &Metric,
        ri: &Instance,
        metric_status: u32,
    ) -> bool {
        let ri_ue = i64::from(ri.state().update_every_s);
        let storage_tiers = self.qt.request.profile.storage_tiers as usize;
        let mut tiers: [TierSnapshot; RRD_STORAGE_TIERS] = Default::default();
        let (mut first, mut last, mut ue, mut added) = (0i64, 0i64, 0i64, 0usize);
        for (t, tier) in tiers.iter_mut().enumerate().take(storage_tiers) {
            let Some(handle) = host.tier_handle(t, rm) else {
                continue;
            };
            let (f, l) = handle.retention();
            let tier_ue = host.storage().tier_grouping(t) as i64 * ri_ue;
            if first == 0 {
                first = f;
            } else if f != 0 {
                first = first.min(f);
            }
            last = if last == 0 { l } else { last.max(l) };
            if ue == 0 {
                ue = tier_ue;
            } else if tier_ue != 0 {
                ue = ue.min(tier_ue);
            }
            *tier = TierSnapshot {
                handle: Some(handle),
                first_time_s: f,
                last_time_s: l,
                update_every_s: tier_ue,
                weight: 0,
            };
            added += 1;
        }
        let db = &mut self.qt.db;
        for (stats, tier) in db.tiers.iter_mut().zip(&tiers).take(storage_tiers) {
            if stats.update_every == 0
                || (tier.update_every_s != 0 && tier.update_every_s < stats.update_every)
            {
                stats.update_every = tier.update_every_s;
            }
            if stats.first_time_s == 0
                || (tier.first_time_s != 0 && tier.first_time_s < stats.first_time_s)
            {
                stats.first_time_s = tier.first_time_s;
            }
            if stats.last_time_s == 0 || tier.last_time_s > stats.last_time_s {
                stats.last_time_s = tier.last_time_s;
            }
        }
        if added == 0 || !matches_retention(self.window.after, self.window.before, first, last, ue)
        {
            return false;
        }
        if db.first_time_s == 0 || first < db.first_time_s {
            db.first_time_s = first;
        }
        if db.last_time_s == 0 || last > db.last_time_s {
            db.last_time_s = last;
        }
        let values_stored_as_rates =
            rm.state().algorithm == netdata_agent_rrd::chart::Algorithm::Incremental;
        self.qt.query.push(QueryMetric {
            dimension,
            status: metric_status,
            values_stored_as_rates,
            tiers,
            // C zeroes the metric; only execution sets its points.
            query_points: StoragePoint::default(),
            plan: Vec::new(),
            grouped_as: GroupedAs::default(),
        });
        true
    }

    /// The dimensions of one instance (`QT:418-547`); returns (kept, admitted).
    fn dimensions(
        &mut self,
        host: &Host,
        instance: usize,
        ri: &Arc<Instance>,
        queryable: bool,
    ) -> (usize, usize) {
        let (mut kept, mut admitted) = (0, 0);
        for (priority, rm) in ri.metrics().into_iter().enumerate() {
            if rm.flags.is_deleted() {
                continue;
            }
            if let Some(sp) = &self.scope_dimensions
                && self.dimension_matches(sp, &rm) != SimplePatternResult::MatchedPositive
            {
                continue;
            }
            let dim_hidden = rm
                .dim()
                .is_some_and(|d| d.meta().flags & dim_flags::HIDDEN != 0);
            // (needed, metric status, dimension status)
            let (needed, qm_status, qd_status) = if !queryable {
                (false, 0, status::EXCLUDED)
            } else if let Some(sp) = &self.dimensions {
                if self.dimension_matches(sp, &rm) == SimplePatternResult::MatchedPositive {
                    (true, metric_status::SELECTED | metric_status::NONZERO, 0)
                } else if self.needs_all_dimensions {
                    (true, metric_status::HIDDEN, 0)
                } else {
                    (false, 0, status::EXCLUDED)
                }
            } else {
                let hidden = rm.flags.check(flags::HIDDEN) || dim_hidden;
                if hidden {
                    if self.needs_all_dimensions {
                        (true, metric_status::HIDDEN, status::DIMENSION_HIDDEN)
                    } else {
                        (false, 0, status::DIMENSION_HIDDEN | status::EXCLUDED)
                    }
                } else {
                    (true, metric_status::SELECTED, 0)
                }
            };
            let d = self.qt.dimensions.len();
            if needed {
                self.qt.dimensions.push(QueryDimension {
                    instance,
                    rm: Arc::clone(&rm),
                    status: qd_status,
                    priority,
                });
                if self.metric_add(host, d, &rm, ri, qm_status) {
                    kept += 1;
                    admitted += 1;
                    self.count(instance, |c| c.selected += 1);
                } else {
                    self.qt.dimensions.pop();
                }
            } else if self.excluded_retention_matches(&rm, ri) {
                self.qt.dimensions.push(QueryDimension {
                    instance,
                    rm: Arc::clone(&rm),
                    status: qd_status,
                    priority,
                });
                kept += 1;
                self.count(instance, |c| c.excluded += 1);
            }
        }
        (kept, admitted)
    }

    /// Metric counters on the instance, its context and its node.
    fn count(&mut self, instance: usize, f: impl Fn(&mut Counts)) {
        let context = self.qt.instances[instance].context;
        let node = self.qt.contexts[context].node;
        f(&mut self.qt.instances[instance].metrics);
        f(&mut self.qt.contexts[context].metrics);
        f(&mut self.qt.nodes[node].metrics);
    }

    fn labels_match(&self, ri: &Instance, scope: bool) -> bool {
        let labels = ri.labels();
        let key_ok = self
            .qt
            .chart_label_key
            .as_ref()
            .is_none_or(|k| labels.match_simple_pattern_parsed(k, 0).is_positive());
        let array = if scope {
            &self.scope_labels
        } else {
            &self.labels
        };
        key_ok && array.as_ref().is_none_or(|a| a.label_match(&labels, b':'))
    }

    /// One instance: scope checks, queryability, its dimensions, pruning.
    fn instance(
        &mut self,
        context: usize,
        ri: &Arc<Instance>,
        context_queryable: bool,
        chart_path: bool,
    ) {
        if ri.flags.is_deleted() {
            return;
        }
        let node = self.qt.contexts[context].node;
        let host = Arc::clone(&self.qt.nodes[node].host);
        let node_id = self.qt.nodes[node].node_id.clone();
        let state = ri.state();
        let (id_fqdn, name_fqdn) = if self.req.version >= 2 {
            (
                bounded(format!("{}@{}", ri.id(), host.machine_guid())),
                bounded(format!("{}@{}", state.name, host.hostname())),
            )
        } else {
            (ri.id().to_string(), state.name.clone())
        };
        if !chart_path {
            if let Some(sp) = &self.scope_instances
                && !self.instance_matches(sp, ri, &id_fqdn, &name_fqdn, node_id.as_deref())
            {
                return;
            }
            if !self.labels_match(ri, true) {
                return;
            }
        }
        let instances_ok = chart_path
            || self.instances.as_ref().is_none_or(|sp| {
                self.instance_matches(sp, ri, &id_fqdn, &name_fqdn, node_id.as_deref())
            });
        let mut queryable = context_queryable
            && instances_ok
            && self.labels.as_ref().is_none_or(|a| a.label_match(&ri.labels(), b':'));
        if queryable && let Some(sp) = &self.alerts {
            queryable = alerts_match(sp, &host, ri);
        }
        // query_target_eval_instance_rrdcalc(): the alerts of a v2 query's queryable instance count for it, its
        // context and its node, before its dimensions are looked at: an instance dropped for having none has
        // counted above itself
        let mut alerts = AlertCounts::default();
        if queryable && self.req.version >= 2 {
            for alert in chart_alerts(&host, ri) {
                alerts.count(alert.class);
            }
            self.qt.contexts[context].alerts.add(&alerts);
            self.qt.nodes[node].alerts.add(&alerts);
        }
        let instance = self.qt.instances.len();
        self.qt.instances.push(QueryInstance {
            context,
            ri: Arc::clone(ri),
            id_fqdn,
            name_fqdn,
            metrics: Counts::default(),
            alerts,
            query_points: StoragePoint::default(),
        });
        let (kept, admitted) = self.dimensions(&host, instance, ri, queryable);
        if kept == 0 {
            self.qt.instances.pop();
            return;
        }
        let q = &mut self.qt;
        if admitted > 0 {
            let ue = i64::from(state.update_every_s);
            if q.db.minimum_latest_update_every_s == 0 || ue < q.db.minimum_latest_update_every_s {
                q.db.minimum_latest_update_every_s = ue;
            }
            q.contexts[context].instances.selected += 1;
            q.nodes[node].instances.selected += 1;
        } else {
            q.contexts[context].instances.excluded += 1;
            q.nodes[node].instances.excluded += 1;
        }
    }

    /// One context and its instances; popped when nothing was kept.
    fn context(
        &mut self,
        node: usize,
        rc: &Arc<Context>,
        queryable: bool,
        chart_instance: Option<&Arc<Instance>>,
    ) {
        if rc.flags.is_deleted() {
            return;
        }
        let context = self.qt.contexts.len();
        self.qt.contexts.push(QueryContext {
            node,
            rc: Arc::clone(rc),
            metrics: Counts::default(),
            instances: Counts::default(),
            alerts: AlertCounts::default(),
            query_points: StoragePoint::default(),
        });
        let before = self.qt.instances.len();
        match chart_instance {
            Some(ri) => self.instance(context, ri, queryable, true),
            None => {
                for ri in rc.instances() {
                    self.instance(context, &ri, queryable, false);
                }
            }
        }
        if self.qt.instances.len() == before {
            self.qt.contexts.pop();
        }
    }

    /// One host: its contexts (an exact scope first, else a scan); popped when nothing was kept.
    fn node(&mut self, host: &Arc<Host>, queryable: bool, chart_instance: Option<&Arc<Instance>>) {
        let node_id = host.node_id();
        let node_id = (node_id != [0; 16]).then(|| {
            let mut s = Vec::new();
            print_uuid_lower(&mut s, &node_id);
            String::from_utf8_lossy(&s).into_owned()
        });
        let node = self.qt.nodes.len();
        self.qt.nodes.push(QueryNode {
            host: Arc::clone(host),
            node_id,
            metrics: Counts::default(),
            instances: Counts::default(),
            alerts: AlertCounts::default(),
            query_points: StoragePoint::default(),
            duration_ut: 0,
        });
        let before = self.qt.contexts.len();
        if let Some(ri) = chart_instance {
            if let Some(rc) = ri.context() {
                self.context(node, &rc, queryable, Some(ri));
            }
        } else {
            let contexts_sp = pattern(&self.req.contexts);
            let ok = |rc: &Context| {
                contexts_sp
                    .as_ref()
                    .is_none_or(|sp| sp.matches(rc.id().as_bytes()))
            };
            match &self.req.scope_contexts {
                Some(scope) => {
                    if let Some(rc) = host.contexts().get(&String::from_utf8_lossy(scope)) {
                        let q = queryable && ok(&rc);
                        self.context(node, &rc, q, None);
                    } else {
                        let sp = SimplePattern::from_web(scope);
                        for rc in host.contexts().all() {
                            if sp.as_ref().is_none_or(|sp| sp.matches(rc.id().as_bytes())) {
                                let q = queryable && ok(&rc);
                                self.context(node, &rc, q, None);
                            }
                        }
                    }
                }
                None => {
                    for rc in host.contexts().all() {
                        let q = queryable && ok(&rc);
                        self.context(node, &rc, q, None);
                    }
                }
            }
        }
        if self.qt.contexts.len() == before {
            self.qt.nodes.pop();
        }
    }
}

/// The alerts of an instance's chart, in link order, as health shows them; none without health or without the
/// chart. `rrdinstance_acquired_rrdset_acquire()` finds the chart through the host's index, which touches it.
pub fn chart_alerts(host: &Host, ri: &Instance) -> Vec<ChartAlert> {
    let Some(chart) = ri.chart() else {
        return Vec::new();
    };
    chart.touch_last_accessed();
    host.storage().alert_view().map_or_else(Vec::new, |view| view.chart_alerts(host, &chart))
}

/// `query_target_match_alert_pattern()`: each alert of the instance's chart is asked by its name, then as
/// `NAME:STATUS`; the first positive match keeps the instance, the first negative one drops it, and so does a
/// chart without a match.
fn alerts_match(sp: &SimplePattern, host: &Host, ri: &Instance) -> bool {
    for alert in chart_alerts(host, ri) {
        let mut text = alert.name;
        for with_status in [false, true] {
            if with_status {
                text.push(b':');
                text.extend_from_slice(alert.status_name.as_bytes());
            }
            match sp.matches_extract(&text, 0).0 {
                SimplePatternResult::MatchedPositive => return true,
                SimplePatternResult::MatchedNegative => return false,
                SimplePatternResult::NotMatched => {}
            }
        }
    }
    false
}

/// `snprintfz(buf, 1200, ...)`: at most 1199 bytes.
fn bounded(mut s: String) -> String {
    if s.len() > 1199 {
        let mut end = 1199;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// `query_scope_foreach_host()`: `scope_nodes` and `nodes` match a host by hostname, machine GUID, then its
/// lowercase node id; the first match that is not NOT decides.
pub fn host_matches(sp: &SimplePattern, host: &Host) -> bool {
    let m = |s: &[u8]| sp.matches_extract(s, 0).0;
    let mut r = m(host.hostname().as_bytes());
    if r == SimplePatternResult::NotMatched {
        r = m(host.machine_guid().as_bytes());
    }
    let id = host.node_id();
    if r == SimplePatternResult::NotMatched && id != [0; 16] {
        let mut s = Vec::new();
        print_uuid_lower(&mut s, &id);
        r = m(&s);
    }
    r == SimplePatternResult::MatchedPositive
}

/// `query_scope_foreach_host()`: each host `scope_nodes` selects, in the index's order, adds its contexts' version
/// to `versions` before `f` sees it with whether `nodes` selects it too (the sums ignore `nodes`). `f` may stop the
/// walk.
pub fn foreach_host<B>(
    hosts: &[Arc<Host>],
    scope_nodes: Option<&SimplePattern>,
    nodes: Option<&SimplePattern>,
    versions: &mut Versions,
    mut f: impl FnMut(&Arc<Host>, bool) -> ControlFlow<B>,
) -> ControlFlow<B> {
    for host in hosts {
        if scope_nodes.is_some_and(|sp| !host_matches(sp, host)) {
            continue;
        }
        versions.contexts_hard_hash += u64::from(host.contexts().version());
        if let Some(view) = host.storage().alert_view() {
            let (hard, soft) = view.versions(host);
            versions.alerts_hard_hash += hard;
            versions.alerts_soft_hash += soft;
        }
        f(host, nodes.is_none_or(|sp| host_matches(sp, host)))?;
    }
    ControlFlow::Continue(())
}

/// `query_target_create()` up to the window calculation. `now_s` is the wall clock (`now_realtime_sec()`).
pub fn create(mut req: DataRequest, source: Source, now_s: i64) -> QueryTarget {
    if req.nodes.is_some() && req.scope_nodes.is_none() {
        req.scope_nodes = req.nodes.clone();
    }
    if req.contexts.is_some() && req.scope_contexts.is_none() {
        req.scope_contexts = req.contexts.clone();
    }
    let percentage_of_group = req.group_by.iter().any(|g| {
        g.group_by & group_by::PERCENTAGE_OF_INSTANCE != 0
            || g.aggregation == Aggregation::Percentage
    });
    let mut window_options = req.options;
    if percentage_of_group {
        window_options &= !options::PERCENTAGE;
    }
    let (after, before, absolute) = relative_window_to_absolute_query(req.after, req.before, now_s);
    let window = SelectionWindow {
        after,
        before,
        relative: !absolute,
        options: window_options,
    };
    let (mut match_ids, mut match_names) = (
        req.options & options::MATCH_IDS != 0,
        req.options & options::MATCH_NAMES != 0,
    );
    if !match_ids && !match_names {
        match_ids = true;
        match_names = true;
    }
    let needs_all_dimensions = req.options & options::PERCENTAGE != 0 || percentage_of_group;
    let label_array = |v: &Option<Vec<u8>>| pattern(v).map(|sp| label_pattern_array(&sp));
    let mut walk = Walk {
        req: &req,
        window,
        start_s: now_s,
        match_ids,
        match_names,
        scope_instances: pattern(&req.scope_instances),
        instances: pattern(&req.instances),
        scope_dimensions: pattern(&req.scope_dimensions),
        dimensions: pattern(&req.dimensions),
        scope_labels: label_array(&req.scope_labels),
        labels: label_array(&req.labels),
        alerts: pattern(&req.alerts),
        needs_all_dimensions,
        qt: QueryTarget {
            request: req.clone(),
            window,
            start_s: now_s,
            nodes: Vec::new(),
            contexts: Vec::new(),
            instances: Vec::new(),
            dimensions: Vec::new(),
            query: Vec::new(),
            db: Db::default(),
            id: String::new(),
            chart_scoped: false,
            chart_label_key: pattern(&req.chart_label_key),
            preprocessed: Instant::now(),
            executed: None,
            query_points: StoragePoint::UNSET,
            group_by_label_keys: Default::default(),
            versions: Versions::default(),
        },
    };
    let (kind_host, kind_chart) = match source {
        Source::V1 { host, chart } => {
            let chart_name = chart
                .as_ref()
                .map(|st| st.meta().name.unwrap_or_else(|| st.id().to_string()));
            let mut instance = chart.as_ref().and_then(|st| st.contexts().instance());
            if let Some(st) = &chart
                && instance.is_none()
            {
                // Not linked to its context yet: link it now, else fall back to a context query on its name.
                contexts::updated_rrdset(st);
                instance = st.contexts().instance();
                if instance.is_none() && !is_valid_sp(req.instances.as_deref()) {
                    walk.instances = chart_name
                        .as_deref()
                        .and_then(|n| SimplePattern::from_web(n.as_bytes()));
                }
            }
            walk.node(host, true, instance.as_ref());
            (Some(host.hostname()), chart_name)
        }
        Source::V2 { hosts } => {
            let scope_nodes = pattern(&req.scope_nodes);
            let nodes = pattern(&req.nodes);
            let mut versions = Versions::default();
            let _: ControlFlow<()> =
                foreach_host(&hosts, scope_nodes.as_ref(), nodes.as_ref(), &mut versions, |host, queryable| {
                    walk.node(host, queryable, None);
                    ControlFlow::Continue(())
                });
            walk.qt.versions = versions;
            (None, None)
        }
    };
    let kind = match (&kind_host, &kind_chart) {
        (Some(hostname), Some(chart_name)) => IdKind::Chart {
            hostname,
            chart_name,
        },
        (Some(hostname), None) => IdKind::Context {
            hostname: Some(hostname),
        },
        (None, _) => IdKind::DataV2,
    };
    let mut qt = walk.qt;
    qt.id = id::generate(&qt.request, kind);
    qt.chart_scoped = kind_chart.is_some();
    qt.preprocessed = Instant::now();
    qt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::parse_v2;
    use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
    use netdata_agent_rrd::host::HostInfo;
    use netdata_agent_rrd::mode::DbMode;

    const T: i64 = 1_700_000_000;

    /// `query_scope_foreach_host()`: the hosts in scope add their contexts' versions whether `nodes` selects them or
    /// not; a stop ends the walk.
    #[test]
    fn the_walk_sums_versions_before_the_nodes_filter() {
        let parent = Arc::new(Host::new("guid-0", false, crate::testing::info("parent", 1, DbMode::Ram)));
        let hosts = [parent, crate::testing::host()];
        let (v0, v1) = (u64::from(hosts[0].contexts().version()), u64::from(hosts[1].contexts().version()));
        assert!(v1 > v0);
        let walk = |scope: &[u8], nodes: &[u8], stop: bool| {
            let (scope, nodes) = (SimplePattern::from_web(scope), SimplePattern::from_web(nodes));
            let mut versions = Versions::default();
            let mut seen = Vec::new();
            let flow = foreach_host(&hosts, scope.as_ref(), nodes.as_ref(), &mut versions, |host, queryable| {
                seen.push((host.hostname(), queryable));
                if stop { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
            });
            (flow.is_break(), versions.contexts_hard_hash, seen)
        };
        let both = |q0, q1| vec![("parent".to_string(), q0), ("child".to_string(), q1)];
        assert_eq!(walk(b"", b"", false), (false, v0 + v1, both(true, true)));
        assert_eq!(walk(b"", b"parent", false), (false, v0 + v1, both(true, false)));
        assert_eq!(walk(b"", b"nomatch", false), (false, v0 + v1, both(false, false)));
        assert_eq!(walk(b"guid-1", b"", false), (false, v1, vec![("child".to_string(), true)]));
        assert_eq!(walk(b"", b"", true), (true, v0, vec![("parent".to_string(), true)]));
    }

    fn host() -> Arc<Host> {
        let info = HostInfo {
            hostname: "child".into(),
            registry_hostname: "child".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "p".into(),
            program_version: "1".into(),
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
        };
        let h = Arc::new(Host::new("guid-1", false, info));
        let (chart, _) = h.charts().create(&ChartSpec {
            type_: "t",
            id: "a",
            name: None,
            family: Some("f"),
            context: Some("ctx.a"),
            title: "T",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        });
        for id in ["d1", "d2", "h"] {
            chart.dim_add(id, None, 1, 1, Algorithm::Absolute);
        }
        chart
            .dim("h")
            .unwrap()
            .update_meta(|m| m.flags |= dim_flags::HIDDEN);
        for t in T - 9..=T {
            for dim in chart.dims() {
                dim.store_metric(t as u64 * 1_000_000, 1.0, 0);
            }
        }
        h.contexts().process_queued();
        h
    }

    fn v2(h: &Arc<Host>, query: &str) -> QueryTarget {
        create(
            parse_v2(query.as_bytes(), 2, &crate::request::Profile::default()),
            Source::V2 { hosts: vec![Arc::clone(h)] },
            T + 1,
        )
    }

    fn statuses(qt: &QueryTarget) -> Vec<(String, u32)> {
        qt.dimensions
            .iter()
            .map(|d| (d.rm.id().to_string(), d.status))
            .collect()
    }

    #[test]
    fn visible_dimensions_are_admitted_hidden_ones_excluded() {
        let h = host();
        let qt = v2(&h, &format!("after={}&before={T}", T - 5));
        assert_eq!(
            (qt.nodes.len(), qt.contexts.len(), qt.instances.len()),
            (1, 1, 1)
        );
        assert_eq!(qt.query.len(), 2);
        assert_eq!(
            statuses(&qt),
            [
                ("d1".into(), 0),
                ("d2".into(), 0),
                ("h".into(), status::DIMENSION_HIDDEN | status::EXCLUDED)
            ]
        );
        assert_eq!(
            qt.nodes[0].metrics,
            Counts {
                selected: 2,
                excluded: 1,
                ..Counts::default()
            }
        );
        let Some(TierHandle::Ram(dim)) = &qt.query[0].tiers[0].handle else {
            panic!("a ram tier 0")
        };
        let ring = dim.ring().unwrap();
        assert_eq!(
            (qt.db.first_time_s, qt.db.last_time_s),
            (ring.oldest_time_s(), T)
        );
        assert_eq!(qt.db.minimum_latest_update_every_s, 1);
        assert!(!qt.window.relative);
    }

    #[test]
    fn a_dimension_filter_selects_and_excludes() {
        let h = host();
        let qt = v2(&h, &format!("after={}&before={T}&dimensions=d2", T - 5));
        assert_eq!(qt.query.len(), 1);
        assert_eq!(
            qt.query[0].status,
            metric_status::SELECTED | metric_status::NONZERO
        );
        assert_eq!(
            statuses(&qt),
            [
                ("d1".into(), status::EXCLUDED),
                ("d2".into(), 0),
                ("h".into(), status::EXCLUDED)
            ]
        );
        // With percentage every dimension is needed: the others are admitted hidden.
        let qt = v2(
            &h,
            &format!(
                "after={}&before={T}&dimensions=d2&options=percentage",
                T - 5
            ),
        );
        assert_eq!(qt.query.len(), 3);
        assert_eq!(qt.query[0].status, metric_status::HIDDEN);
    }

    #[test]
    fn windows_and_scopes_select_nothing_when_they_miss() {
        let h = host();
        // Far in the past: outside the ring and the contexts retention alike.
        let qt = v2(&h, &format!("after={}&before={}", T - 100_000, T - 90_000));
        assert!(qt.query.is_empty() && qt.nodes.is_empty());
        assert!(v2(&h, "scope_nodes=other").nodes.is_empty());
        assert_eq!(
            v2(&h, "scope_nodes=child&scope_contexts=ctx.*").query.len(),
            2
        );
        assert!(v2(&h, "scope_contexts=nope").nodes.is_empty());
        // `nodes` promotes to the scope: a non-matching host is skipped.
        assert!(v2(&h, "nodes=other").nodes.is_empty());
        // The first identifier the pattern matches decides, even negatively; later identifiers are not tried.
        for query in [
            "nodes=!child,*",
            "scope_nodes=!child,*",
            "nodes=!guid-1,guid-*",
        ] {
            assert!(v2(&h, query).nodes.is_empty(), "{query}");
        }
        assert_eq!(v2(&h, "nodes=!other,*").nodes.len(), 1);
        // `instances` is not promoted: a non-matching instance stays, with its dimensions excluded.
        let qt = v2(&h, "instances=nope");
        assert!(qt.query.is_empty());
        assert_eq!(qt.instances.len(), 1);
        assert_eq!(qt.nodes[0].instances.excluded, 1);
    }

    #[test]
    fn label_pattern_arrays_follow_c() {
        let h = host();
        let chart = h.charts().find("t.a", true).unwrap();
        chart.update_meta(|m| {
            m.labels
                .add(b"k", b"v", netdata_agent_rrd::labels::SRC_CONFIG)
        });
        assert_eq!(v2(&h, "labels=k:v").query.len(), 2);
        assert!(v2(&h, "labels=k:x").query.is_empty());
        // A word without ':' stops the array: nothing is required.
        assert_eq!(v2(&h, "labels=foo").query.len(), 2);
        // '!' is dropped from the parsed text: `!k:v` requires k:v.
        assert_eq!(v2(&h, "scope_labels=!k:v").query.len(), 2);
    }

    /// `query_target_eval_instance_rrdcalc()`: the alerts of a version-2 query's queryable instance count for the
    /// instance, its context and its node; the versions are the hosts' sums. A version-1 query counts nothing.
    #[test]
    fn a_v2_query_counts_its_instances_alerts() {
        let h = crate::testing::host_with_alerts();
        let (qt, _) = crate::testing::v2_target(&h, "contexts=ctx.a");
        let three = AlertCounts { clear: 1, warning: 1, critical: 0, other: 1 };
        assert_eq!((qt.instances[0].alerts, qt.contexts[0].alerts, qt.nodes[0].alerts), (three, three, three));
        assert_eq!((qt.versions.alerts_hard_hash, qt.versions.alerts_soft_hash), (7, 9));

        let (qt, _) = crate::testing::v1_target(&h, "points=3");
        assert!(qt.instances[0].alerts.is_empty() && qt.nodes[0].alerts.is_empty());

        // without health nothing is counted and the versions stay 0
        let (qt, _) = crate::testing::v2_target(&crate::testing::host(), "contexts=ctx.a");
        assert!(qt.instances[0].alerts.is_empty());
        assert_eq!((qt.versions.alerts_hard_hash, qt.versions.alerts_soft_hash), (0, 0));
    }

    /// `query_target_match_alert_pattern()`: per alert in link order (a_warn WARNING, a_clear CLEAR, a_undef
    /// UNDEFINED) the name, then `NAME:STATUS`; the first match that is not NOT decides. An instance the filter
    /// drops is not queried and its alerts are not counted.
    #[test]
    fn the_alerts_filter_keeps_an_instance_by_its_first_match() {
        let h = crate::testing::host_with_alerts();
        let cases = [
            ("a_warn", true),
            ("a_undef", true),
            ("nope", false),
            ("a_clear:CLEAR", true),
            ("a_clear:WARNING", false),
            ("*:UNDEFINED", true),
            ("*:CRITICAL", false),
            // the first alert's name is refused before any other alert is asked
            ("!a_warn|*", false),
            // the first alert's name matches `*` before the second alert could be refused
            ("!a_clear|*", true),
            ("!*:WARNING|*", true),
            // no name matches until the first alert's `NAME:STATUS` is refused
            ("!*:WARNING|a_clear", false),
            ("!*:CLEAR|a_undef", false),
            ("!*:CRITICAL|a_undef", true),
        ];
        for (filter, kept) in cases {
            let (qt, _) = crate::testing::v2_target(&h, &format!("contexts=ctx.a&alerts={filter}"));
            let admitted = !qt.query.is_empty();
            assert_eq!(admitted, kept, "{filter}");
            assert_eq!(!qt.nodes[0].alerts.is_empty(), kept, "{filter}");
        }
        // without health a filter keeps nothing; a lone `*` is no filter
        let (qt, _) = crate::testing::v2_target(&crate::testing::host(), "contexts=ctx.a&alerts=a*");
        assert!(qt.query.is_empty());
        let (qt, _) = crate::testing::v2_target(&crate::testing::host(), "contexts=ctx.a&alerts=*");
        assert!(!qt.query.is_empty());
    }
}
