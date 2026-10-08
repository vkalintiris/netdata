//! The contexts of the contexts v2 engine (`src/database/contexts/api_v2_contexts.c`): the per-request dictionary
//! with its merge across hosts (`contexts_conflict_callback()`) and its lists (`contexts_react_callback()`), the
//! writer (`contexts_v2_contexts_to_json()`) and the categorized output of a long MCP answer
//! (`rrdcontext_categorize_and_output()`). Decisions D231 and D232 in the status repository.

use indexmap::{IndexMap, IndexSet};

use netdata_agent_query::tables::contexts_options::{
    DIMENSIONS, FAMILY, INSTANCES, LABELS, LIVENESS, MCP, PRIORITIES, RETENTION, RFC3339, TITLES, UNITS,
};
use netdata_agent_query::target::matches_retention;
use netdata_agent_rrd::contexts::{Context, ContextState, flags, string_2way_merge};
use netdata_agent_text::json::JsonWriter;

use super::labels::{AggregatedLabels, limited_items_to_json};
use super::search::{Matches, per_context_limit};
use super::{Request, Window};

/// `MCP_INFO_CONTEXT_NEXT_STEPS` (`src/web/mcp/mcp.h`).
const MCP_INFO_CONTEXT_NEXT_STEPS: &str = concat!(
    "Next Steps: Query time-series data with the 'query_metrics' tool, using different aggregations to inspect ",
    "different views:\n",
    "   - 'group_by: dimension' will aggregate all time-series by the listed dimensions\n",
    "   - 'group_by: instance' will aggregate all time-series by the listed instances\n",
    "   - 'group_by: label, group_by_label: {label_key}' will aggregate by the listed label values\n",
    "\n",
    "Dimensions, instances and labels can also be used for filtering in 'query_metrics':\n",
    "   - 'dimensions: dimension1|dimension2|*dimension*' will select only the time-series with the given ",
    "dimension\n",
    "   - 'instances: instance1|instance2|*instance*' will select only the time-series with the given instance\n",
    "   - 'labels' can be specified in two formats:\n",
    "      \u{2022} String format: 'labels: key1:value1|key1:value2|key2:value3' (values with same key are ORed, ",
    "different keys are ANDed)\n",
    "      \u{2022} Structured format: 'labels: {\"key1\": [\"value1\", \"value2\"], \"key2\": \"value3\"}' (array ",
    "values are ORed, different keys are ANDed)",
);

/// `MCP_INFO_CONTEXT_ARRAY_RESPONSE`.
const MCP_INFO_CONTEXT_ARRAY_RESPONSE: &str =
    "Next Steps: run the 'get_metrics_details' tool to get more information for the contexts of interest.";

/// `MCP_INFO_TOO_MANY_CONTEXTS_GROUPED_IN_CATEGORIES`.
const MCP_INFO_TOO_MANY_CONTEXTS_GROUPED_IN_CATEGORIES: &str = "The response has been grouped into categories to \
minimize size.\nNext Steps: repeat the 'list_metrics' call with a pattern to match what is interesting, or run \
'get_metrics_details' to get more information for the contexts of interest.";

/// The `info` of a search whose contexts were cut, for an MCP caller.
const MCP_INFO_SEARCH_CARDINALITY_LIMIT: &str =
    "Cardinality limit reached. Use cardinality_limit parameter to see more results.";

/// The `help` of the categorized output's `__info__`.
const CATEGORIZED_HELP: &str = "Results grouped by category with samples. Use 'metrics' parameter with specific \
patterns like 'system.*' to get full details for a category.";

/// The options that make `contexts` an object of contexts; without any of them it is an array of ids, which only a
/// caller that sets no default option reaches (no HTTP route does).
const OBJECT_OPTIONS: u64 =
    TITLES | FAMILY | UNITS | PRIORITIES | RETENTION | LIVENESS | DIMENSIONS | LABELS | INSTANCES;

/// `struct context_v2_entry`, the parts that are printed.
#[derive(Debug)]
struct Entry {
    title: Vec<u8>,
    family: Vec<u8>,
    units: String,
    priority: u32,
    first_time_s: i64,
    last_time_s: i64,
    flags: u32,
    instances: Option<IndexSet<String>>,
    dimensions: Option<IndexSet<String>>,
    labels: Option<AggregatedLabels>,
    /// What of the context a search matched, over the hosts walked; nothing in the other modes.
    matches: Matches,
}

impl Entry {
    /// A host's context as `rrdcontext_to_json_v2_add_context()` hands it to the dictionary.
    fn new(state: ContextState, flags: u32) -> Self {
        Entry {
            title: state.title,
            family: state.family,
            units: state.units,
            priority: state.priority,
            first_time_s: state.first_time_s,
            last_time_s: state.last_time_s,
            flags,
            instances: None,
            dimensions: None,
            labels: None,
            matches: Matches::default(),
        }
    }

    /// `contexts_conflict_callback()`: a later host brings the same context, and each field an option asks for is
    /// settled between the two.
    ///
    /// C ORs the newcomer's flags into the kept ones before it compares them, so its branches "the kept one is not
    /// collected and the newcomer is: take the newcomer's" never run. What is left: a title, family or priority
    /// that differs stays the kept one's when the two together are collected and the newcomer is not; otherwise
    /// the texts are merged and the lower priority wins. The units always stay.
    fn merge(&mut self, new: &Entry, options: u64) {
        self.flags |= new.flags;
        let keep = self.flags & flags::COLLECTED != 0 && new.flags & flags::COLLECTED == 0;
        if options & TITLES != 0 && self.title != new.title && !keep {
            self.title = string_2way_merge(&self.title, &new.title);
        }
        if options & FAMILY != 0 && self.family != new.family && !keep {
            self.family = string_2way_merge(&self.family, &new.family);
        }
        if options & PRIORITIES != 0 && !keep {
            self.priority = self.priority.min(new.priority);
        }
        if options & RETENTION != 0 {
            // the earliest first and the latest last of the two; a zero side takes the other's
            if self.first_time_s != 0 && new.first_time_s != 0 {
                self.first_time_s = self.first_time_s.min(new.first_time_s);
            } else if self.first_time_s == 0 {
                self.first_time_s = new.first_time_s;
            }
            if self.last_time_s != 0 && new.last_time_s != 0 {
                self.last_time_s = self.last_time_s.max(new.last_time_s);
            } else if self.last_time_s == 0 {
                self.last_time_s = new.last_time_s;
            }
        }
    }

    /// `contexts_react_callback()`: with any list asked, this host's context adds its instances' names, their
    /// metrics' names and their labels, each a set in the order first seen. With a window, an instance or a metric
    /// is left out when its retention misses the window by more than twice the instance's update every. The
    /// dictionary refuses an empty name.
    fn react(&mut self, rc: &Context, options: u64, window: Window) {
        if options & (INSTANCES | DIMENSIONS | LABELS) == 0 {
            return;
        }
        if options & INSTANCES != 0 {
            self.instances.get_or_insert_default();
        }
        if options & DIMENSIONS != 0 {
            self.dimensions.get_or_insert_default();
        }
        if options & LABELS != 0 {
            self.labels.get_or_insert_default();
        }
        for ri in rc.instances() {
            let instance = ri.state();
            let update_every = i64::from(instance.update_every_s);
            let in_window = |collected: bool, first: i64, last: i64| match window.range {
                None => true,
                Some((after, before)) => {
                    matches_retention(after, before, first, if collected { window.now } else { last }, update_every)
                }
            };
            if !in_window(ri.flags.is_collected(), instance.first_time_s, instance.last_time_s) {
                continue;
            }
            if let Some(dimensions) = &mut self.dimensions {
                for rm in ri.metrics() {
                    let metric = rm.state();
                    if in_window(rm.flags.is_collected(), metric.first_time_s, metric.last_time_s)
                        && !metric.name.is_empty()
                    {
                        dimensions.insert(metric.name);
                    }
                }
            }
            if let Some(labels) = &mut self.labels {
                labels.add_from(&ri.labels());
            }
            if let Some(instances) = &mut self.instances
                && !instance.name.is_empty()
            {
                instances.insert(instance.name);
            }
        }
    }

    /// One context of the `contexts` object: the members its options ask for, in C's order.
    fn to_json(&self, w: &mut JsonWriter, id: &str, req: &Request, now: i64) {
        let (options, limit) = (req.options, limit(req));
        let collected = self.flags & flags::COLLECTED != 0;
        let rfc3339 = options & RFC3339 != 0;
        w.member_add_object(id);
        if options & TITLES != 0 {
            w.member_add_string("title", &self.title);
        }
        if options & FAMILY != 0 {
            w.member_add_string("family", &self.family);
        }
        if options & UNITS != 0 {
            w.member_add_string("units", &self.units);
        }
        if options & PRIORITIES != 0 {
            w.member_add_uint64("priority", u64::from(self.priority));
        }
        if options & RETENTION != 0 {
            w.member_add_time_t_formatted("first_entry", self.first_time_s, rfc3339);
            let last = if collected { now } else { self.last_time_s };
            w.member_add_time_t_formatted("last_entry", last, rfc3339);
        }
        if options & LIVENESS != 0 {
            w.member_add_boolean("live", collected);
        }
        // the lists exist only when their options asked for them
        if let Some(dimensions) = &self.dimensions {
            w.member_add_array(Some(b"dimensions"));
            limited_items_to_json(w, dimensions.iter().map(String::as_bytes), limit, "dimensions");
            w.array_close();
        }
        if let Some(labels) = &self.labels {
            labels.to_json(w, b"labels", limit);
        }
        if let Some(instances) = &self.instances {
            w.member_add_array(Some(b"instances"));
            limited_items_to_json(w, instances.iter().map(String::as_bytes), limit, "instances");
            w.array_close();
        }
        w.object_close();
    }
}

/// The request's cardinality limit as a count (C's `size_t`); 0 is none.
fn limit(req: &Request) -> usize {
    req.cardinality_limit as usize
}

/// The `__truncated__` member that ends a `contexts` object cut at the cardinality limit.
fn truncated_to_json(w: &mut JsonWriter, total: usize, returned: usize) {
    w.member_add_object("__truncated__");
    w.member_add_uint64("total_contexts", total as u64);
    w.member_add_uint64("returned", returned as u64);
    w.member_add_uint64("remaining", (total - returned) as u64);
    w.object_close();
}

/// The category of a context id (`rrdcontext_categorize_and_output()`): the id up to its second dot, or up to its
/// only dot, or the whole id; 255 bytes of it at most.
///
/// An id with a leading dot and no other has the empty category. C's dictionary refuses an empty name and C then
/// dereferences NULL; here the context is listed under the empty name (D231 F8).
fn category(id: &[u8]) -> &[u8] {
    let dot = |from: usize| id[from..].iter().position(|&b| b == b'.').map(|at| from + at);
    let end = match dot(0) {
        Some(first) => dot(first + 1).unwrap_or(first),
        None => id.len(),
    };
    &id[..end.min(255)]
}

/// `rrdcontext_categorize_and_output()`: the ids grouped by category, categories and ids in the order first seen,
/// each category showing as many ids as the limit leaves it (the limit shared among the categories, three at the
/// least); a category with more shows one fewer and then how many it has more.
fn categorized_to_json<'a>(w: &mut JsonWriter, ids: impl Iterator<Item = &'a str>, limit: usize) {
    let mut categories: IndexMap<&[u8], Vec<&str>> = IndexMap::new();
    let mut total = 0;
    for id in ids {
        categories.entry(category(id.as_bytes())).or_default().push(id);
        total += 1;
    }
    let samples = match (categories.len(), limit) {
        (0, _) | (_, 0) => 3,
        (count, limit) => (limit / count).max(3),
    };
    w.member_add_object("__info__");
    w.member_add_string("status", "categorized");
    w.member_add_uint64("total_contexts", total);
    w.member_add_uint64("categories", categories.len() as u64);
    w.member_add_uint64("samples_per_category", samples as u64);
    w.member_add_string("help", CATEGORIZED_HELP);
    w.object_close();
    for (name, ids) in &categories {
        w.member_add_array(Some(*name));
        let shown = if ids.len() > samples { samples - 1 } else { ids.len() };
        for id in &ids[..shown] {
            w.add_array_item_string(id);
        }
        if ids.len() > samples {
            w.add_array_item_string(format!("... and {} more", ids.len() - shown));
        }
        w.array_close();
    }
}

/// `ctl->contexts.dict`: the contexts of the hosts walked, by id in the order first seen.
#[derive(Debug, Default)]
pub(super) struct ContextsDict {
    entries: IndexMap<String, Entry>,
}

impl ContextsDict {
    /// `dictionary_set()` of a host's context: a new id is inserted, a known one merged; either way the entry then
    /// takes this host's lists, which are the contexts answer's alone.
    pub(super) fn add(&mut self, rc: &Context, state: ContextState, options: u64, window: Window) {
        let new = Entry::new(state, rc.flags.get());
        let at = match self.entries.get_index_of(rc.id()) {
            Some(at) => {
                self.entries[at].merge(&new, options);
                at
            }
            None => self.entries.insert_full(rc.id().to_owned(), new).0,
        };
        self.entries[at].react(rc, options, window);
    }

    /// The search's `dictionary_set()` of a host's context: inserted or merged as [`Self::add`] does, without the
    /// lists, which are the contexts answer's alone, and with what the search matched of it on this host.
    pub(super) fn add_searched(&mut self, rc: &Context, state: ContextState, options: u64, found: Matches) {
        let new = Entry::new(state, rc.flags.get());
        match self.entries.get_index_of(rc.id()) {
            Some(at) => {
                self.entries[at].merge(&new, options);
                self.entries[at].matches.merge(found);
            }
            None => {
                self.entries.insert(rc.id().to_owned(), Entry { matches: found, ..new });
            }
        }
    }

    /// `contexts_v2_search_results_to_json()`: the `contexts` member of a search, each context with what matched
    /// of it, at most `cardinality` of them with what was cut after them, and the `info` text an MCP caller gets
    /// when contexts were cut.
    pub(super) fn search_to_json(&self, w: &mut JsonWriter, req: &Request) {
        let (limit, total) = (limit(req), self.entries.len());
        let mcp = req.options & MCP != 0;
        let per = per_context_limit(limit, total);
        w.member_add_object("contexts");
        for (count, (id, entry)) in self.entries.iter().enumerate() {
            if limit != 0 && count >= limit {
                truncated_to_json(w, total, count);
                break;
            }
            entry.matches.to_json(w, id, &entry.title, &entry.family, &entry.units, per, mcp);
        }
        w.object_close();
        if mcp && limit != 0 && total > limit {
            w.member_add_string("info", MCP_INFO_SEARCH_CARDINALITY_LIMIT);
        }
    }

    /// `contexts_v2_contexts_to_json()`: the `contexts` member, and the `info` text an MCP caller gets after it.
    /// `now` is the walk's clock, a collected context's last entry.
    pub(super) fn to_json(&self, w: &mut JsonWriter, req: &Request, now: i64) {
        let (limit, total) = (limit(req), self.entries.len());
        let mcp = req.options & MCP != 0;
        if mcp && limit != 0 && total > limit {
            w.member_add_object("contexts");
            categorized_to_json(w, self.entries.keys().map(String::as_str), limit);
            w.object_close();
            w.member_add_string("info", MCP_INFO_TOO_MANY_CONTEXTS_GROUPED_IN_CATEGORIES);
            return;
        }
        let object = req.options & OBJECT_OPTIONS != 0;
        if object {
            w.member_add_object("contexts");
        } else {
            w.member_add_array(Some(b"contexts"));
        }
        for (count, (id, entry)) in self.entries.iter().enumerate() {
            if limit != 0 && count >= limit {
                if object {
                    truncated_to_json(w, total, count);
                } else {
                    w.add_array_item_string(format!("... {} contexts more", total - count));
                }
                break;
            }
            if object {
                entry.to_json(w, id, req, now);
            } else {
                w.add_array_item_string(id);
            }
        }
        if object {
            w.object_close();
        } else {
            w.array_close();
        }
        if mcp {
            w.member_add_string(
                "info",
                if object { MCP_INFO_CONTEXT_NEXT_STEPS } else { MCP_INFO_CONTEXT_ARRAY_RESPONSE },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use netdata_agent_rrd::chart::{Algorithm, Chart, ChartSpec, ChartType};
    use netdata_agent_rrd::host::Host;
    use netdata_agent_rrd::labels::SRC_CONFIG;
    use netdata_agent_rrd::mode::DbMode;
    use netdata_agent_text::json::JsonOptions;

    use super::*;
    use crate::testing::host_info;

    /// The last second of the fixture's data, and the walk's clock some seconds later: a collected context's last
    /// entry is the clock, not its data's end.
    const T: i64 = 1_700_000_060;
    const NOW: i64 = T + 7;
    const NO_WINDOW: Window = Window { range: None, now: NOW };

    /// A chart of a test host: its id and name under type `q`, the words of its CHART line that reach its context,
    /// its update every, its dimensions as (id, name) and its labels.
    struct TestChart<'a> {
        id: &'a str,
        name: Option<&'a str>,
        title: &'a str,
        units: &'a str,
        family: &'a str,
        context: &'a str,
        priority: i64,
        update_every: i32,
        dims: &'a [(&'a str, Option<&'a str>)],
        labels: &'a [(&'a str, &'a str)],
    }

    /// The parity fixture's two charts of `q.ctx` (`streamChartsFixture` in `tests/parity/data_test.go`), whose
    /// contexts answer C recorded.
    const Q_A: TestChart = TestChart {
        id: "a",
        name: Some("q_a_name"),
        title: "title a",
        units: "units",
        family: "fam",
        context: "q.ctx",
        priority: 1000,
        update_every: 1,
        dims: &[("a", Some("alpha")), ("b", None), ("z", None), ("inc", None), ("h", None)],
        labels: &[("_collect_plugin", "fixture-pusher"), ("_collect_module", "corpus"), ("k", "v1")],
    };
    const Q_TWO: TestChart = TestChart {
        id: "two",
        name: None,
        title: "title two",
        priority: 1001,
        update_every: 2,
        dims: &[("a", None), ("b", None)],
        labels: &[("_collect_plugin", "fixture-pusher"), ("_collect_module", "corpus"), ("k", "v2")],
        ..Q_A
    };

    /// A host with these charts, each with data from `first` to `T`, and its contexts processed: collected.
    fn host(guid: &str, charts: &[(&TestChart, i64)]) -> Arc<Host> {
        let host = Arc::new(Host::new(guid, false, host_info("child")));
        for (c, first) in charts {
            let chart = add_chart(&host, c);
            store(&chart, *first, T);
        }
        host.contexts().process_queued();
        host
    }

    fn add_chart(host: &Host, c: &TestChart) -> Arc<Chart> {
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "q",
            id: c.id,
            name: c.name,
            family: Some(c.family),
            context: Some(c.context),
            title: c.title,
            units: c.units,
            plugin: "p",
            module: None,
            priority: c.priority,
            update_every: c.update_every,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        });
        for (id, name) in c.dims {
            chart.dim_add(id, *name, 1, 1, Algorithm::Absolute);
        }
        chart.update_meta(|meta| {
            for (name, value) in c.labels {
                meta.labels.add(name.as_bytes(), value.as_bytes(), SRC_CONFIG);
            }
        });
        chart
    }

    /// The chart's dimensions store a point at each of its steps from `first` to `last`.
    fn store(chart: &Chart, first: i64, last: i64) {
        let step = i64::from(chart.meta().update_every).max(1);
        for dim in chart.dims() {
            let mut t = first;
            while t <= last {
                dim.store_metric(t as u64 * 1_000_000, 1.0, 0);
                t += step;
            }
        }
    }

    /// The dictionary of these hosts' contexts, as the engine's walk fills it.
    fn dict(hosts: &[&Arc<Host>], options: u64, window: Window) -> ContextsDict {
        let mut dict = ContextsDict::default();
        for host in hosts {
            for rc in host.contexts().all() {
                dict.add(&rc, rc.state(), options, window);
            }
        }
        dict
    }

    fn request(options: u64, cardinality_limit: u64) -> Request {
        Request { options, cardinality_limit, ..Request::default() }
    }

    fn printed(dict: &ContextsDict, req: &Request, layout: JsonOptions) -> String {
        let mut w = JsonWriter::new(layout);
        dict.to_json(&mut w, req, NOW);
        w.finalize();
        String::from_utf8(w.into_bytes()).unwrap()
    }

    const DEFAULTS: u64 = PRIORITIES | RETENTION | LIVENESS | FAMILY | UNITS;
    const LISTS: u64 = TITLES | LABELS | INSTANCES | DIMENSIONS;
    /// The options of the search's routes.
    const SEARCH: u64 = FAMILY | UNITS | TITLES | LABELS | INSTANCES | DIMENSIONS;

    /// The search of these hosts' contexts for `q`, as the engine's walk runs it, printed: the `contexts` member
    /// under `options` and `cardinality`, then `searches`.
    fn searched(hosts: &[&Arc<Host>], q: Option<&str>, window: Window, options: u64, cardinality: u64) -> String {
        use super::super::search::{Fts, search};
        use netdata_agent_text::simple_pattern::SimplePattern;
        let q = q.and_then(|q| SimplePattern::from_web_nocase_substring(q.as_bytes()));
        let (mut dict, mut fts) = (ContextsDict::default(), Fts::default());
        for host in hosts {
            for rc in host.contexts().all() {
                let state = rc.state();
                let found = q.as_ref().map(|q| search(&rc, &state, q, SEARCH, window, &mut fts));
                if found.as_ref().is_some_and(|found| !found.any()) {
                    continue;
                }
                dict.add_searched(&rc, state, SEARCH, found.unwrap_or_default());
            }
        }
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        dict.search_to_json(&mut w, &request(options, cardinality));
        fts.to_json(&mut w);
        w.finalize();
        String::from_utf8(w.into_bytes()).unwrap()
    }

    /// The answers C gave for the parity fixture (`/api/v2/q?scope_nodes=*&q=...`, the oracle's dumps of
    /// 2026-10-07): what matched, the names stored, and the counters. A pattern's words are parts of a text,
    /// whatever the case. The 15 tests of a miss: the context's id, family, title and units; `q.a`'s id and name
    /// and its five metrics' ids with `a`'s name; `q.two`'s id and its two metrics' ids. With `a`, two ids match,
    /// so their two names are not tested, and what is stored is the name. A label counts by what matched of it.
    #[test]
    fn the_search_answers_as_c_for_the_fixture() {
        let child = host("11111111-1111-1111-1111-111111111111", &[(&Q_A, T - 60), (&Q_TWO, T - 60)]);
        let answer = |q: &str| searched(&[&child], Some(q), NO_WINDOW, SEARCH, 0);
        let searches = |strings: u32, chars: u32| {
            format!(r#""searches":{{"strings":{strings},"char":{chars},"total":{}}}"#, strings + chars)
        };
        let alpha = r#"{"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"#;
        assert_eq!(answer("*alpha*"), format!("{alpha}{}}}", searches(15, 0)));
        assert_eq!(answer("ALPHA"), format!("{alpha}{}}}", searches(15, 0)));
        let a = concat!(
            r#"{"contexts":{"q.ctx":{"family":"fam","matched":["families","instances","dimensions"],"#,
            r#""instances":["q.q_a_name"],"dimensions":["alpha","a"]}},"#
        );
        assert_eq!(answer("a"), format!("{a}{}}}", searches(13, 0)));
        let k = r#"{"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v1","v2"]}}},"#;
        assert_eq!(answer("k"), format!("{k}{}}}", searches(15, 2)));
        assert_eq!(answer("v"), format!("{k}{}}}", searches(15, 2)));
        let v2 = r#"{"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v2"]}}},"#;
        assert_eq!(answer("v2"), format!("{v2}{}}}", searches(15, 1)));
        // nothing matches: the context is not stored, and every text was still tested
        assert_eq!(answer("nomatch"), format!(r#"{{"contexts":{{}},{}}}"#, searches(15, 0)));
        // the context's own texts, each of them and all together
        let texts = concat!(
            r#"{"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","#,
            r#""matched":["title","units","families"]}},"#
        );
        assert_eq!(answer("fam|units|title"), format!("{texts}{}}}", searches(15, 0)));
        // no pattern, and a text without a word: every context is stored with no match and nothing is tested
        let unsearched = format!(r#"{{"contexts":{{"q.ctx":{{"matched":[]}}}},{}}}"#, searches(0, 0));
        assert_eq!(searched(&[&child], None, NO_WINDOW, SEARCH, 0), unsearched);
        assert_eq!(answer(","), unsearched);
        assert_eq!(answer("*"), unsearched);
    }

    /// The search across hosts, its window and its limits: a later host's names join the first's; with a window an
    /// instance whose retention misses it is not searched (no slack here), so only the context's four texts are
    /// (alone on its host, the chart's title is the context's, and it has the letter);
    /// `cardinality` cuts the contexts and says so, with one more text for an MCP caller, who gets no `matched`.
    #[test]
    fn a_search_spans_hosts_and_obeys_the_window_and_the_limit() {
        let other = TestChart { id: "other", name: Some("q_a_other"), dims: &[("o", None)], labels: &[], ..Q_A };
        let child = host("11111111-1111-1111-1111-111111111111", &[(&Q_A, T - 60)]);
        let child2 = host("22222222-2222-2222-2222-222222222222", &[(&other, T - 60)]);
        let both = searched(&[&child, &child2], Some("q_a"), NO_WINDOW, SEARCH, 0);
        let union = r#"{"contexts":{"q.ctx":{"matched":["instances"],"instances":["q.q_a_name","q.q_a_other"]}},"#;
        assert!(both.starts_with(union), "{both}");

        // the instance's data ends at the walk's clock (it is collected): a window after it misses it
        let after = Window { range: Some((NOW + 1, NOW + 100)), now: NOW };
        let missed = searched(&[&child], Some("a"), after, SEARCH, 0);
        let texts_only = concat!(
            r#"{"contexts":{"q.ctx":{"title":"title a","family":"fam","matched":["title","families"]}},"#,
            r#""searches":{"strings":4,"char":0,"total":4}}"#
        );
        assert_eq!(missed, texts_only);
        // a window that ends at the clock meets it, to the second
        let meets = Window { range: Some((NOW, NOW + 100)), now: NOW };
        assert!(searched(&[&child], Some("a"), meets, SEARCH, 0).contains(r#""instances":["q.q_a_name"]"#));

        // two contexts, room for one
        let second = TestChart { id: "s", name: None, context: "q.second", ..Q_A };
        let two = host("33333333-3333-3333-3333-333333333333", &[(&Q_A, T - 60), (&second, T - 60)]);
        let cut = searched(&[&two], Some("fam"), NO_WINDOW, SEARCH, 1);
        let truncated = concat!(
            r#"{"contexts":{"q.ctx":{"family":"fam","matched":["families"]},"#,
            r#""__truncated__":{"total_contexts":2,"returned":1,"remaining":1}},"searches":{"#
        );
        assert!(cut.starts_with(truncated), "{cut}");
        let mcp = searched(&[&two], Some("fam"), NO_WINDOW, SEARCH | MCP, 1);
        let told = concat!(
            r#"{"contexts":{"q.ctx":{"family":"fam"},"#,
            r#""__truncated__":{"total_contexts":2,"returned":1,"remaining":1}},"#,
            r#""info":"Cardinality limit reached. Use cardinality_limit parameter to see more results.","#,
            r#""searches":{"#
        );
        assert!(mcp.starts_with(told), "{mcp}");
        // both fit: no cut, and nothing told
        let whole = searched(&[&two], Some("fam"), NO_WINDOW, SEARCH | MCP, 2);
        let fits = r#"{"contexts":{"q.ctx":{"family":"fam"},"q.second":{"family":"fam"}},"searches":{"#;
        assert!(whole.starts_with(fits), "{whole}");
    }

    /// The answer C gave for the parity fixture with every list (`/api/v2/contexts?scope_nodes=*&options=titles,
    /// labels,instances,dimensions`, C against C, 2026-10-07): the `contexts` member's bytes, with the fixture's two
    /// seconds written in. The title is the two charts' merged; the priority the lower; the dimensions and the
    /// instances are names, in the order first seen (q.two's `a` is new beside q.a's `alpha`); a label's values
    /// are a set.
    #[test]
    fn the_contexts_object_is_c_s_for_the_fixture() {
        let h = host("guid-1", &[(&Q_A, T - 59), (&Q_TWO, T - 58)]);
        let d = dict(&[&h], DEFAULTS | LISTS, NO_WINDOW);
        let first = h.contexts().get("q.ctx").unwrap().state().first_time_s;
        let c = [
            "{",
            "    \"contexts\":{",
            "        \"q.ctx\":{",
            "            \"title\":\"title [x]\",",
            "            \"family\":\"fam\",",
            "            \"units\":\"units\",",
            "            \"priority\":1000,",
            &format!("            \"first_entry\":{first},"),
            &format!("            \"last_entry\":{NOW},"),
            "            \"live\":true,",
            "            \"dimensions\":[\"alpha\",\"b\",\"z\",\"inc\",\"h\",\"a\"],",
            "            \"labels\":{",
            "                \"_collect_plugin\":[\"fixture-pusher\"],",
            "                \"_collect_module\":[\"corpus\"],",
            "                \"k\":[\"v1\",\"v2\"]",
            "            },",
            "            \"instances\":[\"q.q_a_name\",\"q.two\"]",
            "        }",
            "    }",
            "}",
            "",
        ]
        .join("\n");
        assert_eq!(printed(&d, &request(DEFAULTS | LISTS, 0), JsonOptions::DEFAULT), c);

        // the route's default options: the scalar members alone, no title and no list
        let d = dict(&[&h], DEFAULTS, NO_WINDOW);
        assert_eq!(
            printed(&d, &request(DEFAULTS, 0), JsonOptions::MINIFY),
            format!(
                concat!(
                    r#"{{"contexts":{{"q.ctx":{{"family":"fam","units":"units","priority":1000,"first_entry":{},"#,
                    r#""last_entry":{},"live":true}}}}}}"#
                ),
                first, NOW
            )
        );

        // one list option alone prints that list alone; `rfc3339` prints the entries as texts
        let only = |options: u64| printed(&dict(&[&h], options, NO_WINDOW), &request(options, 0), JsonOptions::MINIFY);
        assert_eq!(only(INSTANCES), r#"{"contexts":{"q.ctx":{"instances":["q.q_a_name","q.two"]}}}"#);
        assert_eq!(only(DIMENSIONS), r#"{"contexts":{"q.ctx":{"dimensions":["alpha","b","z","inc","h","a"]}}}"#);
        // both texts whole, the `Z` and the first entry's own second: 1700000000 is 2023-11-14T22:13:20Z
        let t0 = 1_700_000_000;
        assert!((t0..t0 + 40).contains(&first), "{first}");
        let dated = format!(
            concat!(
                r#"{{"contexts":{{"q.ctx":{{"first_entry":"2023-11-14T22:13:{:02}Z","#,
                r#""last_entry":"2023-11-14T22:14:27Z"}}}}}}"#
            ),
            first - t0 + 20
        );
        assert_eq!(only(RETENTION | RFC3339), dated);

        // with a limit, as C printed them: a list of more items than the limit is cut after one fewer, and a
        // list within it is whole (`cardinality=1`, then `cardinality_limit=2`)
        let lists = |limit: u64| {
            let d = dict(&[&h], DEFAULTS | LISTS, NO_WINDOW);
            let text = printed(&d, &request(DEFAULTS | LISTS, limit), JsonOptions::MINIFY);
            text[text.find(r#""dimensions""#).unwrap()..].to_owned()
        };
        assert_eq!(
            lists(1),
            concat!(
                r#""dimensions":["... 6 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"#,
                r#""_collect_module":["corpus"],"k":["... 2 values more"]},"instances":["... 2 instances more"]}}}"#
            )
        );
        assert_eq!(
            lists(2),
            concat!(
                r#""dimensions":["alpha","... 5 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"#,
                r#""_collect_module":["corpus"],"k":["v1","v2"]},"instances":["q.q_a_name","q.two"]}}}"#
            )
        );
    }

    /// `contexts_conflict_callback()`: what a later host's context changes in the one kept, for each pair of
    /// collected flags. C's "take the newcomer's" branches are dead (the flags are ORed first), so a newcomer never
    /// replaces a text or a priority: they are merged unless the kept side is collected and the newcomer is not.
    #[test]
    fn a_later_host_s_context_merges_as_c_does() {
        let entry = |title: &str, family: &str, units: &str, priority, first, last, collected: bool| Entry {
            title: title.into(),
            family: family.into(),
            units: units.into(),
            priority,
            first_time_s: first,
            last_time_s: last,
            flags: if collected { flags::COLLECTED } else { 0 },
            instances: None,
            dimensions: None,
            labels: None,
            matches: Matches::default(),
        };
        let merged = |mut old: Entry, new: &Entry, options: u64| {
            old.merge(new, options);
            (
                String::from_utf8(old.title).unwrap(),
                String::from_utf8(old.family).unwrap(),
                old.units,
                old.priority,
                (old.first_time_s, old.last_time_s),
                old.flags & flags::COLLECTED != 0,
            )
        };
        let all = TITLES | FAMILY | UNITS | PRIORITIES | RETENTION;
        let kept = |collected| entry("title [x]", "fam", "units", 1000, 100, 200, collected);
        let other = |collected| entry("other [x]", "fam2", "units2", 900, 50, 300, collected);
        let text = |title: &str, family: &str, priority, retention, collected| {
            (title.to_owned(), family.to_owned(), "units".to_owned(), priority, retention, collected)
        };
        // C's answer for the parity fixture's two children, both collected: `[x] [x]`, `fam[x]`, `units`, 900
        assert_eq!(merged(kept(true), &other(true), all), text("[x] [x]", "fam[x]", 900, (50, 300), true));
        // the kept one collected, the newcomer not: its texts and its priority stay
        assert_eq!(merged(kept(true), &other(false), all), text("title [x]", "fam", 1000, (50, 300), true));
        // the kept one not collected, the newcomer collected: merged, not replaced, and collected from now on (C's
        // answer with the first child disconnected and the second collected: `[x] [x]`, `fam[x]`, `units`, 900)
        assert_eq!(merged(kept(false), &other(true), all), text("[x] [x]", "fam[x]", 900, (50, 300), true));
        assert_eq!(merged(kept(false), &other(false), all), text("[x] [x]", "fam[x]", 900, (50, 300), false));
        // a higher priority never wins, equal texts stay as they are
        let higher = entry("title [x]", "fam", "units2", 1100, 100, 200, true);
        assert_eq!(merged(kept(true), &higher, all), text("title [x]", "fam", 1000, (100, 200), true));
        // nor when the kept side is not collected and the newcomer is: the lower stays, the newcomer's is not taken
        assert_eq!(merged(kept(false), &higher, all), text("title [x]", "fam", 1000, (100, 200), true));
        // a third host, not collected, into what the first two left: its texts, which a merge would mix in, and
        // its lower priority are not taken
        let mut three = kept(true);
        three.merge(&other(true), all);
        let third = entry("[x]y", "fam[x]z", "units3", 850, 10, 400, false);
        assert_eq!(merged(three, &third, all), text("[x] [x]", "fam[x]", 900, (10, 400), true));
        // an option that is off leaves its field the first host's
        assert_eq!(merged(kept(true), &other(true), RETENTION), text("title [x]", "fam", 1000, (50, 300), true));
        assert_eq!(merged(kept(true), &other(true), TITLES), text("[x] [x]", "fam", 1000, (100, 200), true));
        let two = FAMILY | PRIORITIES;
        assert_eq!(merged(kept(true), &other(true), two), text("title [x]", "fam[x]", 900, (100, 200), true));
        // retention: a zero side takes the other's, a zero newcomer changes nothing
        let empty = |collected| entry("title [x]", "fam", "units", 1000, 0, 0, collected);
        assert_eq!(merged(empty(true), &other(true), RETENTION).4, (50, 300));
        assert_eq!(merged(kept(true), &empty(true), RETENTION).4, (100, 200));
        let inside = entry("title [x]", "fam", "units", 1000, 150, 180, true);
        assert_eq!(merged(kept(true), &inside, RETENTION).4, (100, 200));
    }

    /// Two hosts with the same context: the dictionary keeps one entry, merged, in the order first seen, and its
    /// lists are both hosts' names in the order first seen, each once. A context only the second host has comes
    /// after. (The second host's chart keeps the plugin and module labels its CHART line gave it.)
    #[test]
    fn the_lists_of_a_context_span_its_hosts() {
        let first = host("guid-1", &[(&Q_A, T - 59), (&Q_TWO, T - 58)]);
        let other = TestChart {
            name: Some("q_a_other"),
            title: "other a",
            units: "units2",
            family: "fam2",
            priority: 900,
            dims: &[("b", None), ("new", Some("fresh"))],
            labels: &[("k", "v1"), ("k", "v3"), ("extra", "e")],
            ..Q_A
        };
        let r = TestChart {
            context: "r.ctx",
            title: "r title",
            units: "runits",
            family: "rfam",
            priority: 1100,
            ..Q_TWO
        };
        let second = host("guid-2", &[(&other, T - 30), (&r, T - 30)]);
        let options = DEFAULTS | LISTS;
        let text = printed(&dict(&[&first, &second], options, NO_WINDOW), &request(options, 0), JsonOptions::MINIFY);
        let q = &text[text.find(r#""q.ctx""#).unwrap()..text.find(r#","r.ctx""#).unwrap()];
        let f = first.contexts().get("q.ctx").unwrap().state().first_time_s;
        assert_eq!(
            q,
            format!(
                concat!(
                    r#""q.ctx":{{"title":"[x]","family":"fam[x]","units":"units","priority":900,"first_entry":{},"#,
                    r#""last_entry":{},"live":true,"dimensions":["alpha","b","z","inc","h","a","fresh"],"#,
                    r#""labels":{{"_collect_plugin":["fixture-pusher","p"],"_collect_module":["corpus","[none]"],"#,
                    r#""k":["v1","v2","v3"],"extra":["e"]}},"instances":["q.q_a_name","q.two","q.q_a_other"]}}"#
                ),
                f, NOW
            )
        );
        let r_ctx = r#","r.ctx":{"title":"r title","family":"rfam","units":"runits","priority":1100,"#;
        assert!(text.contains(r_ctx), "{text}");
    }

    /// With a window, a context's lists leave out the instances and the metrics whose retention misses it by more
    /// than twice the instance's update every (`contexts_react_callback()`; a collected one reaches the walk's
    /// clock). An instance that is left out brings neither its dimensions nor its labels.
    #[test]
    fn a_window_filters_the_lists_with_the_instance_s_slack() {
        let h = host("guid-1", &[(&Q_A, T - 59), (&Q_TWO, T - 58)]);
        let lists = |after: i64, before: i64| {
            let window = Window { range: Some((after, before)), now: T + 100 };
            let text = printed(&dict(&[&h], LISTS, window), &request(LISTS, 0), JsonOptions::MINIFY);
            text[text.find(r#""dimensions""#).unwrap()..].to_owned()
        };
        let both = concat!(
            r#""dimensions":["alpha","b","z","inc","h","a"],"labels":{"_collect_plugin":["fixture-pusher"],"#,
            r#""_collect_module":["corpus"],"k":["v1","v2"]},"instances":["q.q_a_name","q.two"]}}}"#
        );
        // while the charts are collected, their retention and their metrics' reach the walk's clock: a window after
        // the data's end still lists them whole, dimensions and labels too
        assert_eq!(lists(T + 50, T + 90), both);
        // the child is gone: nothing is collected, and the retention ends at T whatever the walk's clock
        h.contexts().child_disconnected();
        h.contexts().worker_cycle();
        // a window that starts 2 seconds after the data's end still meets q.a (every 1 s) and q.two (every 2 s)
        assert_eq!(lists(T + 2, T + 50), both);
        // 3 seconds after: beyond q.a's slack of 2, within q.two's of 4
        let two = concat!(
            r#""dimensions":["a","b"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"#,
            r#""k":["v2"]},"instances":["q.two"]}}}"#
        );
        assert_eq!(lists(T + 3, T + 50), two);
        assert_eq!(lists(T + 4, T + 50), two);
        // beyond both: the context is listed by whoever let it through, with empty lists
        assert_eq!(lists(T + 5, T + 50), r#""dimensions":[],"labels":{},"instances":[]}}}"#);
    }

    /// More contexts than the limit: the object ends with `__truncated__` and its counts; the array form, which only
    /// a caller without a default option reaches, ends with a text. With `mcp`, the text that follows each form is
    /// C's, byte for byte as C escapes it (a raw UTF-8 bullet, `\n` and `\"`; the first is the oracle's answer to
    /// `/api/v2/contexts?scope_nodes=*&options=mcp`).
    #[test]
    fn truncation_the_array_form_and_the_mcp_texts() {
        let r = TestChart { context: "r.ctx", ..Q_TWO };
        let s = TestChart { id: "s", context: "s.ctx", ..Q_TWO };
        let h = host("guid-1", &[(&Q_A, T - 59), (&r, T - 58), (&s, T - 58)]);
        let text = |options: u64, limit: u64| {
            printed(&dict(&[&h], options, NO_WINDOW), &request(options, limit), JsonOptions::MINIFY)
        };
        assert_eq!(
            text(UNITS, 2),
            concat!(
                r#"{"contexts":{"q.ctx":{"units":"units"},"r.ctx":{"units":"units"},"#,
                r#""__truncated__":{"total_contexts":3,"returned":2,"remaining":1}}}"#
            )
        );
        let three = r#""q.ctx":{"units":"units"},"r.ctx":{"units":"units"},"s.ctx":{"units":"units"}"#;
        assert_eq!(text(UNITS, 3), format!(r#"{{"contexts":{{{three}}}}}"#));
        assert_eq!(text(0, 0), r#"{"contexts":["q.ctx","r.ctx","s.ctx"]}"#);
        assert_eq!(text(0, 1), r#"{"contexts":["q.ctx","... 2 contexts more"]}"#);
        let next_steps = concat!(
            r#""info":"Next Steps: Query time-series data with the 'query_metrics' tool, using different "#,
            r#"aggregations to inspect different views:\n"#,
            r#"   - 'group_by: dimension' will aggregate all time-series by the listed dimensions\n"#,
            r#"   - 'group_by: instance' will aggregate all time-series by the listed instances\n"#,
            r#"   - 'group_by: label, group_by_label: {label_key}' will aggregate by the listed label values\n"#,
            r#"\n"#,
            r#"Dimensions, instances and labels can also be used for filtering in 'query_metrics':\n"#,
            r#"   - 'dimensions: dimension1|dimension2|*dimension*' will select only the time-series with the "#,
            r#"given dimension\n"#,
            r#"   - 'instances: instance1|instance2|*instance*' will select only the time-series with the "#,
            r#"given instance\n"#,
            r#"   - 'labels' can be specified in two formats:\n"#,
            r#"      • String format: 'labels: key1:value1|key1:value2|key2:value3' (values with same key are "#,
            r#"ORed, different keys are ANDed)\n"#,
            r#"      • Structured format: 'labels: {\"key1\": [\"value1\", \"value2\"], \"key2\": \"value3\"}' "#,
            r#"(array values are ORed, different keys are ANDed)"}"#,
        );
        assert_eq!(text(UNITS | MCP, 0), format!(r#"{{"contexts":{{{three}}},{next_steps}"#));
        assert_eq!(
            text(MCP, 0),
            concat!(
                r#"{"contexts":["q.ctx","r.ctx","s.ctx"],"info":"Next Steps: run the 'get_metrics_details' tool "#,
                r#"to get more information for the contexts of interest."}"#
            )
        );
        // mcp with more contexts than the limit: grouped by category, then its own text (the oracle's bytes for
        // `options=mcp&cardinality=1` on two contexts, here with a third)
        assert_eq!(
            text(UNITS | MCP, 1),
            concat!(
                r#"{"contexts":{"__info__":{"status":"categorized","total_contexts":3,"categories":3,"#,
                r#""samples_per_category":3,"help":"Results grouped by category with samples. Use 'metrics' "#,
                r#"parameter with specific patterns like 'system.*' to get full details for a category."},"#,
                r#""q":["q.ctx"],"r":["r.ctx"],"s":["s.ctx"]},"#,
                r#""info":"The response has been grouped into categories to minimize size.\nNext Steps: repeat the "#,
                r#"'list_metrics' call with a pattern to match what is interesting, or run 'get_metrics_details' to "#,
                r#"get more information for the contexts of interest."}"#
            )
        );
        // a limit the contexts fit in is not categorized
        assert!(text(UNITS | MCP, 3).starts_with(r#"{"contexts":{"q.ctx":{"units":"units"},"#));
    }

    /// `rrdcontext_categorize_and_output()`: a context's category is its id up to its second dot, or up to its only
    /// dot, or the whole id; categories and their contexts keep the order first seen. Each category shows as many
    /// ids as the limit divided among the categories allows, three at the least; one with more shows one fewer and
    /// then how many are left. An id with a leading dot and no other is listed under the empty category, where C
    /// dereferences NULL (D231 F8); with another dot its category runs to that dot.
    #[test]
    fn contexts_are_grouped_by_category() {
        for (id, want) in [
            ("system.cpu", "system"),
            ("disk.io.read", "disk.io"),
            ("a.b.c.d", "a.b"),
            ("nodot", "nodot"),
            (".foo", ""),
            (".foo.bar", ".foo"),
            ("..x", "."),
            ("trailing.", "trailing"),
        ] {
            assert_eq!(category(id.as_bytes()), want.as_bytes(), "{id}");
        }
        assert_eq!(category(format!("{}.x", "a".repeat(300)).as_bytes()).len(), 255);

        let grouped = |ids: &[&str], limit: usize| {
            let mut w = JsonWriter::new(JsonOptions::MINIFY);
            categorized_to_json(&mut w, ids.iter().copied(), limit);
            w.finalize();
            let text = String::from_utf8(w.into_bytes()).unwrap();
            let info = format!(r#""help":"{CATEGORIZED_HELP}"}},"#);
            let (head, groups) = text.split_once(&info).unwrap_or_else(|| panic!("{text}"));
            (head.to_owned(), groups.to_owned())
        };
        let info = |total: usize, categories: usize, samples: usize| {
            format!(
                concat!(
                    r#"{{"__info__":{{"status":"categorized","total_contexts":{},"categories":{},"#,
                    r#""samples_per_category":{},"#
                ),
                total, categories, samples
            )
        };
        let ids =
            ["system.cpu", "disk.io.a", "system.ram", "system.load", "disk.io.b", "system.uptime", "nodot", ".foo"];
        // 8 contexts in 4 categories under a limit of 4: one each by division, three at the least
        assert_eq!(
            grouped(&ids, 4),
            (
                info(8, 4, 3),
                concat!(
                    r#""system":["system.cpu","system.ram","... and 2 more"],"disk.io":["disk.io.a","disk.io.b"],"#,
                    r#""nodot":["nodot"],"":[".foo"]}"#
                )
                .to_owned()
            )
        );
        // a limit of 16 gives each category 4: none is cut
        assert_eq!(
            grouped(&ids, 16),
            (
                info(8, 4, 4),
                concat!(
                    r#""system":["system.cpu","system.ram","system.load","system.uptime"],"#,
                    r#""disk.io":["disk.io.a","disk.io.b"],"nodot":["nodot"],"":[".foo"]}"#
                )
                .to_owned()
            )
        );
        // a limit that does not divide evenly is rounded down: 18 over 4 gives 4, not 5
        assert_eq!(grouped(&ids, 18).0, info(8, 4, 4));
        // exactly as many as the sample size are all shown
        assert_eq!(grouped(&["a.x", "a.y", "a.z"], 1), (info(3, 1, 3), r#""a":["a.x","a.y","a.z"]}"#.to_owned()));
        assert_eq!(
            grouped(&["a.w", "a.x", "a.y", "a.z"], 1),
            (info(4, 1, 3), r#""a":["a.w","a.x","... and 2 more"]}"#.to_owned())
        );
    }
}
