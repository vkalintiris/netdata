//! The query target (`query_target_create()`, `src/database/contexts/query_target.c`): the nodes, contexts,
//! instances and dimensions a data request selects, and the metrics admitted for querying. Spec §3.1-3.8.
//!
//! The window is converted here only for admission; `query_target_calculate_window()` (spec §4.1) comes with the
//! planner.

use std::sync::Arc;
use std::time::Instant;

use netdata_agent_rrd::chart::{Chart, Dim, dim_flags, flags as chart_flags};
use netdata_agent_rrd::contexts::{self, Context, Instance, Metric, flags};
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::labels::Labels;
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

#[derive(Debug)]
pub struct QueryNode {
    pub host: Arc<Host>,
    pub node_id: Option<String>,
    pub metrics: Counts,
    pub instances: Counts,
}

#[derive(Debug)]
pub struct QueryContext {
    pub node: usize,
    pub rc: Arc<Context>,
    pub metrics: Counts,
    pub instances: Counts,
}

#[derive(Debug)]
pub struct QueryInstance {
    pub context: usize,
    pub ri: Arc<Instance>,
    /// `<id>@<machine_guid>` (v1: the id), `<name>@<hostname>` (v1: the name).
    pub id_fqdn: String,
    pub name_fqdn: String,
    pub metrics: Counts,
}

#[derive(Debug)]
pub struct QueryDimension {
    pub instance: usize,
    pub rm: Arc<Metric>,
    pub status: u32,
    /// The position among its instance's metrics, deleted and dropped ones included.
    pub priority: usize,
}

/// A tier's retention snapshot at admission (`qm->tiers[t]`).
#[derive(Debug, Clone)]
pub struct TierSnapshot {
    pub dim: Arc<Dim>,
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub update_every_s: i64,
}

/// An admitted metric (`QUERY_METRIC`).
#[derive(Debug)]
pub struct QueryMetric {
    pub dimension: usize,
    pub status: u32,
    pub values_stored_as_rates: bool,
    pub tier0: TierSnapshot,
    /// What the execution read, merged (`qm->query_points`).
    pub query_points: StoragePoint,
    /// The tier-0 plan's `(after, before)` (`qm->plan.array[0]`); none for the LATEST fast path or a failed plan.
    pub plan: Option<(i64, i64)>,
}

/// `qt->db`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Db {
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub minimum_latest_update_every_s: i64,
    /// Tier 0 (the only tier until dbengine): the smallest update every and the widest retention of every candidate.
    pub tier0_update_every: i64,
    pub tier0_first: i64,
    pub tier0_last: i64,
    /// Plans initialised and points read on tier 0 (`qt->db.tiers[0].queries`, `.points`).
    pub tier0_queries: usize,
    pub tier0_points: usize,
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
}

/// What selects the metrics: v1's routed host (and chart), or every host for v2/v3.
pub enum Source<'a> {
    /// `chart` is the chart `chart=` found (`qtr->st`).
    V1 {
        host: &'a Arc<Host>,
        chart: Option<Arc<Chart>>,
    },
    V2 {
        hosts: Vec<Arc<Host>>,
    },
}

/// `pattern_array` for labels (`src/libnetdata/simple_pattern/pattern_array.c`): per label key, the exact `key:value`
/// patterns; the words stop at the first lone `*` or at the first word without `:`.
struct PatternArray(Vec<(Vec<u8>, Vec<SimplePattern>)>);

impl PatternArray {
    fn new(sp: &SimplePattern) -> Self {
        let mut keys: Vec<(Vec<u8>, Vec<SimplePattern>)> = Vec::new();
        for word in sp.words() {
            let Some(word) = word else { break };
            let Some(colon) = word.iter().position(|&c| c == b':') else {
                break;
            };
            let key = word[..colon.min(200)].to_vec();
            let pattern = SimplePattern::new(
                word,
                netdata_agent_text::simple_pattern::Separators::None,
                netdata_agent_text::simple_pattern::SimplePatternMode::Exact,
                true,
            );
            match keys.iter_mut().find(|(k, _)| *k == key) {
                Some((_, patterns)) => patterns.push(pattern),
                None => keys.push((key, vec![pattern])),
            }
        }
        PatternArray(keys)
    }

    /// AND across keys, OR within a key.
    fn matches(&self, labels: &Labels) -> bool {
        self.0.iter().all(|(_, patterns)| {
            patterns
                .iter()
                .any(|p| labels.match_simple_pattern_parsed(p, b':').is_positive())
        })
    }
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
    alerts: bool,
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

    /// `query_metric_add()`: tier 0 from the dimension's ring; `false` when the window misses its retention.
    fn metric_add(
        &mut self,
        dimension: usize,
        rm: &Metric,
        ri: &Instance,
        metric_status: u32,
    ) -> bool {
        let ue = i64::from(ri.state().update_every_s);
        let Some(dim) = rm.storage_dim() else {
            return false;
        };
        let Some(ring) = dim.ring() else {
            return false;
        };
        let (first, last) = (ring.oldest_time_s(), ring.latest_time_s());
        let db = &mut self.qt.db;
        if ue != 0 && (db.tier0_update_every == 0 || ue < db.tier0_update_every) {
            db.tier0_update_every = ue;
        }
        if first != 0 && (db.tier0_first == 0 || first < db.tier0_first) {
            db.tier0_first = first;
        }
        if last > db.tier0_last {
            db.tier0_last = last;
        }
        if !matches_retention(self.window.after, self.window.before, first, last, ue) {
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
            tier0: TierSnapshot {
                dim,
                first_time_s: first,
                last_time_s: last,
                update_every_s: ue,
            },
            // C zeroes the metric; only execution sets its points.
            query_points: StoragePoint::default(),
            plan: None,
        });
        true
    }

    /// The dimensions of one instance (`QT:418-547`); returns (kept, admitted).
    fn dimensions(
        &mut self,
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
                if self.metric_add(d, &rm, ri, qm_status) {
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
        key_ok && array.as_ref().is_none_or(|a| a.matches(&labels))
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
        let queryable = context_queryable
            && instances_ok
            && self.labels.as_ref().is_none_or(|a| a.matches(&ri.labels()))
            && !(self.req.version >= 2 && self.alerts);
        let instance = self.qt.instances.len();
        self.qt.instances.push(QueryInstance {
            context,
            ri: Arc::clone(ri),
            id_fqdn,
            name_fqdn,
            metrics: Counts::default(),
        });
        let (kept, admitted) = self.dimensions(instance, ri, queryable);
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

/// `scope_nodes` and `nodes` match a host by hostname, machine GUID, then its lowercase node id.
fn host_matches(sp: &SimplePattern, host: &Host) -> bool {
    if sp.matches(host.hostname().as_bytes()) || sp.matches(host.machine_guid().as_bytes()) {
        return true;
    }
    let id = host.node_id();
    if id == [0; 16] {
        return false;
    }
    let mut s = Vec::new();
    print_uuid_lower(&mut s, &id);
    sp.matches(&s)
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
    let label_array = |v: &Option<Vec<u8>>| pattern(v).map(|sp| PatternArray::new(&sp));
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
        alerts: pattern(&req.alerts).is_some(),
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
            for host in &hosts {
                if let Some(sp) = &scope_nodes
                    && !host_matches(sp, host)
                {
                    continue;
                }
                let queryable = nodes.as_ref().is_none_or(|sp| host_matches(sp, host));
                walk.node(host, queryable, None);
            }
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

/// Whether a v1 `chart=` names a chart the data API accepts: obsolete charts only while replicating.
pub fn chart_is_queryable(flags: u32) -> bool {
    flags & chart_flags::OBSOLETE == 0 || flags & chart_flags::RECEIVER_REPLICATION_IN_PROGRESS != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::parse_v2;
    use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
    use netdata_agent_rrd::host::HostInfo;
    use netdata_agent_rrd::mode::DbMode;

    const T: i64 = 1_700_000_000;

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
            parse_v2(query.as_bytes(), 2, 1),
            Source::V2 {
                hosts: vec![Arc::clone(h)],
            },
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
        let ring = qt.query[0].tier0.dim.ring().unwrap();
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
        // `instances` is not promoted: a non-matching instance stays, with its dimensions excluded.
        let qt = v2(&h, "instances=nope");
        assert!(qt.query.is_empty());
        assert_eq!(qt.instances.len(), 1);
        assert_eq!(qt.nodes[0].instances.excluded, 1);
    }

    #[test]
    fn label_pattern_arrays_follow_c() {
        let h = host();
        let chart = h.charts().find("t.a").unwrap();
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
}
