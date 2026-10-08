//! The full-text search of `/api/v2/q` and `/api/v3/q` (`src/database/contexts/api_v2_contexts.c`):
//! `rrdcontext_to_json_v2_full_text_search()` with `rrdlabels_full_text_search()`, the merge of one context's
//! matches across hosts (the search part of `contexts_conflict_callback()`), and one context of
//! `contexts_v2_search_results_to_json()`.

use indexmap::IndexSet;
use netdata_agent_query::tables::contexts_options::{DIMENSIONS, FAMILY, INSTANCES, LABELS, TITLES, UNITS};
use netdata_agent_query::target::matches_retention;
use netdata_agent_rrd::contexts::{Context, ContextState};
use netdata_agent_rrd::labels::Labels;
use netdata_agent_text::json::JsonWriter;
use netdata_agent_text::simple_pattern::SimplePattern;

use super::Window;
use super::labels::{AggregatedLabels, limited_items_to_json};

/// `SEARCH_MATCH_TYPE`: where a context matched.
mod matched {
    pub(super) const ID: u8 = 1 << 0;
    pub(super) const TITLE: u8 = 1 << 1;
    pub(super) const UNITS: u8 = 1 << 2;
    pub(super) const FAMILY: u8 = 1 << 3;
    pub(super) const INSTANCE: u8 = 1 << 4;
    pub(super) const DIMENSION: u8 = 1 << 5;
    pub(super) const LABEL: u8 = 1 << 6;
}

/// The names `matched` prints, in C's order.
const MATCHED_NAMES: [(u8, &str); 7] = [
    (matched::ID, "id"),
    (matched::TITLE, "title"),
    (matched::UNITS, "units"),
    (matched::FAMILY, "families"),
    (matched::INSTANCE, "instances"),
    (matched::DIMENSION, "dimensions"),
    (matched::LABEL, "labels"),
];

/// `FTS_INDEX`: how many texts the search tested, which the answer prints as `searches`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Fts {
    strings: u64,
    chars: u64,
    total: u64,
}

impl Fts {
    /// `full_text_search_string()`: counted, then tested.
    fn string(&mut self, q: &SimplePattern, text: &[u8]) -> bool {
        self.total += 1;
        self.strings += 1;
        q.matches(text)
    }

    /// An id and its name as C tests them: the id first, and the name only when the id missed and the name is not
    /// the id (C compares the two interned strings).
    fn named(&mut self, q: &SimplePattern, id: &str, name: &str) -> bool {
        self.string(q, id.as_bytes()) || (name != id && self.string(q, name.as_bytes()))
    }

    /// The `searches` member.
    pub(super) fn to_json(self, w: &mut JsonWriter) {
        w.member_add_object("searches");
        w.member_add_uint64("strings", self.strings);
        w.member_add_uint64("char", self.chars);
        w.member_add_uint64("total", self.total);
        w.object_close();
    }
}

/// `struct fts_search_results`: what of a context matched. A set exists once something matched for it, also when
/// the name that matched could not be stored (C's dictionary refuses an empty name).
#[derive(Debug, Default)]
pub(super) struct Matches {
    types: u8,
    instances: Option<IndexSet<String>>,
    dimensions: Option<IndexSet<String>>,
    labels: Option<AggregatedLabels>,
}

/// A later host's names join the kept ones, after them.
fn merge_names(kept: &mut Option<IndexSet<String>>, new: Option<IndexSet<String>>) {
    match kept {
        Some(kept) => kept.extend(new.into_iter().flatten()),
        None => *kept = new,
    }
}

impl Matches {
    /// Whether anything matched: a context without a match is not stored and does not count for its host.
    pub(super) fn any(&self) -> bool {
        self.types != 0
    }

    /// The search part of `contexts_conflict_callback()`: a later host's matches of the same context.
    pub(super) fn merge(&mut self, new: Matches) {
        self.types |= new.types;
        merge_names(&mut self.instances, new.instances);
        merge_names(&mut self.dimensions, new.dimensions);
        match (&mut self.labels, new.labels) {
            (Some(kept), Some(new)) => kept.merge(new),
            (kept, new) => {
                if kept.is_none() {
                    *kept = new;
                }
            }
        }
    }

    /// One context of `contexts_v2_search_results_to_json()`: its title, family and units, each only when it
    /// matched; `matched` (not with `mcp`); the instances and the dimensions that matched, when there are any, and
    /// the labels that matched, each list under the per-context limit `per`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn to_json(
        &self,
        w: &mut JsonWriter,
        id: &str,
        title: &[u8],
        family: &[u8],
        units: &str,
        per: usize,
        mcp: bool,
    ) {
        w.member_add_object(id);
        if self.types & matched::TITLE != 0 {
            w.member_add_string("title", title);
        }
        if self.types & matched::FAMILY != 0 {
            w.member_add_string("family", family);
        }
        if self.types & matched::UNITS != 0 {
            w.member_add_string("units", units);
        }
        if !mcp {
            w.member_add_array(Some(b"matched"));
            for (bit, name) in MATCHED_NAMES {
                if self.types & bit != 0 {
                    w.add_array_item_string(name);
                }
            }
            w.array_close();
        }
        for (key, names, what) in
            [(&b"instances"[..], &self.instances, "instances"), (b"dimensions", &self.dimensions, "dimensions")]
        {
            if let Some(names) = names.as_ref().filter(|names| !names.is_empty()) {
                w.member_add_array(Some(key));
                limited_items_to_json(w, names.iter().map(String::as_bytes), per, what);
                w.array_close();
            }
        }
        if let Some(labels) = &self.labels {
            labels.to_json(w, b"labels", per);
        }
        w.object_close();
    }
}

/// `contexts_v2_search_results_to_json()`'s limit of each list of a context: 3, or the share of `cardinality` each
/// shown context gets when that is more.
pub(super) fn per_context_limit(cardinality: usize, total: usize) -> usize {
    let shown = if cardinality != 0 && total > cardinality { cardinality } else { total };
    match (cardinality, shown) {
        (0, _) | (_, 0) => 3,
        (cardinality, shown) => (cardinality / shown).max(3),
    }
}

/// `rrdlabels_full_text_search()`: every label whose key or value matches joins `found` as that one pair. Returns
/// how many keys and values matched, which is what C adds to its counters here: the matches, not the tests.
fn label_search(labels: &Labels, q: &SimplePattern, found: &mut Option<AggregatedLabels>) -> u64 {
    let mut matches = 0;
    for label in labels.iter() {
        let hits = u64::from(q.matches(&label.name)) + u64::from(q.matches(&label.value));
        if hits != 0 {
            found.get_or_insert_default().add(&label.name, &label.value);
            matches += hits;
        }
    }
    matches
}

/// `rrdcontext_to_json_v2_full_text_search()`: the search of one context of one host. In order: the context's id,
/// then its family, title and units (each under its option); then per instance, in creation order, its id and name
/// (the name is what is stored), its metrics' ids and names likewise, and its labels' keys and values. With a
/// window, an instance or a metric whose retention misses it is skipped, an instance with its metrics and labels;
/// this test has no slack (the lists of the contexts answer allow the instance's update every twice).
pub(super) fn search(
    rc: &Context,
    state: &ContextState,
    q: &SimplePattern,
    options: u64,
    window: Window,
    fts: &mut Fts,
) -> Matches {
    let mut found = Matches::default();
    if fts.string(q, rc.id().as_bytes()) {
        found.types |= matched::ID;
    }
    if options & FAMILY != 0 && fts.string(q, &state.family) {
        found.types |= matched::FAMILY;
    }
    if options & TITLES != 0 && fts.string(q, &state.title) {
        found.types |= matched::TITLE;
    }
    if options & UNITS != 0 && fts.string(q, state.units.as_bytes()) {
        found.types |= matched::UNITS;
    }
    let in_window = |collected: bool, first: i64, last: i64| match window.range {
        None => true,
        Some((after, before)) => {
            matches_retention(after, before, first, if collected { window.now } else { last }, 0)
        }
    };
    for ri in rc.instances() {
        let instance = ri.state();
        if !in_window(ri.flags.is_collected(), instance.first_time_s, instance.last_time_s) {
            continue;
        }
        if options & INSTANCES != 0 && fts.named(q, ri.id(), &instance.name) {
            let names = found.instances.get_or_insert_default();
            if !instance.name.is_empty() {
                names.insert(instance.name.clone());
            }
            found.types |= matched::INSTANCE;
        }
        if options & DIMENSIONS != 0 {
            for rm in ri.metrics() {
                let metric = rm.state();
                if !in_window(rm.flags.is_collected(), metric.first_time_s, metric.last_time_s) {
                    continue;
                }
                if fts.named(q, rm.id(), &metric.name) {
                    let names = found.dimensions.get_or_insert_default();
                    if !metric.name.is_empty() {
                        names.insert(metric.name);
                    }
                    found.types |= matched::DIMENSION;
                }
            }
        }
        if options & LABELS != 0 {
            let labels = ri.labels();
            if !labels.is_empty() {
                let matches = label_search(&labels, q, &mut found.labels);
                // (a context's earlier instance may have brought the labels)
                if found.labels.is_some() {
                    found.types |= matched::LABEL;
                }
                fts.total += matches;
                fts.chars += matches;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    fn substring(q: &str) -> SimplePattern {
        SimplePattern::from_web_nocase_substring(q.as_bytes()).expect("a pattern")
    }

    fn names(names: &[&str]) -> Option<IndexSet<String>> {
        Some(names.iter().map(|name| (*name).to_owned()).collect())
    }

    fn printed(matches: &Matches, per: usize, mcp: bool) -> String {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        matches.to_json(&mut w, "q.ctx", b"title [x]", b"fam", "units", per, mcp);
        w.finalize();
        String::from_utf8(w.into_bytes()).unwrap()
    }

    /// A test counts before it tests; a name is tested only when its id missed and it is not the id.
    #[test]
    fn a_name_is_tested_after_its_id_missed() {
        let q = substring("alpha|B");
        let mut fts = Fts::default();
        // the id matches: the name is not tested
        assert!(fts.named(&q, "alpha", "other"));
        assert_eq!(fts, Fts { strings: 1, chars: 0, total: 1 });
        // the id misses and the name is the id: one test
        assert!(!fts.named(&q, "z", "z"));
        assert_eq!(fts, Fts { strings: 2, chars: 0, total: 2 });
        // the id misses, the name is tested: a part of it, whatever the case
        assert!(fts.named(&q, "z", "the_Beta"));
        assert_eq!(fts, Fts { strings: 4, chars: 0, total: 4 });
        assert!(!fts.named(&q, "z", ""));
        assert_eq!(fts, Fts { strings: 6, chars: 0, total: 6 });
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        Fts { strings: 15, chars: 2, total: 17 }.to_json(&mut w);
        w.finalize();
        assert_eq!(w.into_bytes(), br#"{"searches":{"strings":15,"char":2,"total":17}}"#);
    }

    /// `rrdlabels_full_text_search()`: a label joins with its one pair when its key or its value matches, and the
    /// count is of the matches: two for a label whose key and value both match, none for the labels tested in vain.
    #[test]
    fn a_label_matches_by_its_key_or_its_value() {
        let mut labels = Labels::default();
        for (name, value) in [("k", "v1"), ("plugin", "kernel"), ("other", "x"), ("key", "k")] {
            labels.add(name.as_bytes(), value.as_bytes(), netdata_agent_rrd::labels::SRC_CONFIG);
        }
        let mut found = None;
        assert_eq!(label_search(&labels, &substring("k"), &mut found), 4);
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        found.as_ref().expect("the labels that matched").to_json(&mut w, b"labels", 0);
        w.finalize();
        assert_eq!(w.into_bytes(), br#"{"labels":{"k":["v1"],"plugin":["kernel"],"key":["k"]}}"#);
        // nothing matches: nothing is made
        let mut found = None;
        assert_eq!(label_search(&labels, &substring("nomatch"), &mut found), 0);
        assert!(found.is_none());
        // a later label set adds to what an earlier one found
        let mut found = None;
        label_search(&labels, &substring("v1"), &mut found);
        let mut second = Labels::default();
        second.add(b"k", b"v10", netdata_agent_rrd::labels::SRC_CONFIG);
        assert_eq!(label_search(&second, &substring("v1"), &mut found), 1);
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        found.as_ref().unwrap().to_json(&mut w, b"labels", 0);
        w.finalize();
        assert_eq!(w.into_bytes(), br#"{"labels":{"k":["v1","v10"]}}"#);
    }

    /// One context of the search's answer: the three texts only when they matched, `matched` in C's order and not
    /// with `mcp`, a list only when it has a name, each list under the per-context limit.
    #[test]
    fn a_context_prints_what_matched() {
        let dimensions = Matches { types: matched::DIMENSION, dimensions: names(&["alpha"]), ..Default::default() };
        assert_eq!(printed(&dimensions, 3, false), r#"{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}}"#);
        assert_eq!(printed(&dimensions, 3, true), r#"{"q.ctx":{"dimensions":["alpha"]}}"#);
        let all = Matches {
            types: 0x7f,
            instances: names(&["i1", "i2", "i3", "i4"]),
            dimensions: names(&["a", "b", "c"]),
            labels: None,
        };
        let text = concat!(
            r#"{"q.ctx":{"title":"title [x]","family":"fam","units":"units","#,
            r#""matched":["id","title","units","families","instances","dimensions","labels"],"#,
            r#""instances":["i1","i2","... 2 instances more"],"dimensions":["a","b","c"]}}"#
        );
        assert_eq!(printed(&all, 3, false), text);
        // a set that exists without a name (the name that matched was empty) is not printed; its bit is
        let empty = Matches { types: matched::INSTANCE, instances: names(&[]), ..Default::default() };
        assert_eq!(printed(&empty, 3, false), r#"{"q.ctx":{"matched":["instances"]}}"#);
        // nothing matched (a request without a pattern stores every context so)
        assert_eq!(printed(&Matches::default(), 3, false), r#"{"q.ctx":{"matched":[]}}"#);
    }

    /// A later host's matches of the same context: the bits add up, the names join after the kept ones, each once.
    #[test]
    fn a_later_host_s_matches_join_the_kept_ones() {
        let mut kept = Matches { types: matched::INSTANCE, instances: names(&["i1", "i2"]), ..Default::default() };
        kept.merge(Matches {
            types: matched::INSTANCE | matched::DIMENSION,
            instances: names(&["i2", "i0"]),
            dimensions: names(&["d"]),
            labels: None,
        });
        assert_eq!(kept.types, matched::INSTANCE | matched::DIMENSION);
        assert_eq!((kept.instances, kept.dimensions), (names(&["i1", "i2", "i0"]), names(&["d"])));
        let mut none = Matches::default();
        none.merge(Matches::default());
        assert!(!none.any() && none.instances.is_none() && none.labels.is_none());
    }

    /// The limit of a context's lists: 3, or the cardinality's share per shown context when that is more.
    #[test]
    fn the_per_context_limit_is_three_or_the_cardinality_s_share() {
        let cases = [((0, 0), 3), ((0, 50), 3), ((1, 50), 3), ((4, 1), 4), ((10, 2), 5), ((10, 4), 3), ((100, 0), 3)];
        for ((cardinality, total), per) in cases {
            assert_eq!(per_context_limit(cardinality, total), per, "{cardinality} {total}");
        }
        // more contexts than the cardinality: the share is of the contexts shown
        assert_eq!(per_context_limit(8, 100), 3);
    }
}
