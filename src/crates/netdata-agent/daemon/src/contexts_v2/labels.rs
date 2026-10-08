//! Labels aggregated over instances: each key with the set of its values (`src/database/rrdlabels-aggregated.c`).
//!
//! C keeps the keys and each key's values in Judy arrays indexed by the address of their interned strings, so it
//! walks them in address order. Here both keep the order first seen (D220 fork 9); the parity checks compare them as
//! sets.

use indexmap::{IndexMap, IndexSet};
use netdata_agent_rrd::labels::Labels;
use netdata_agent_text::json::JsonWriter;

/// A list under a cardinality limit, as C's three writers cut theirs (`api_v2_contexts.c:1228-1234`, `:1257-1263`,
/// `rrdlabels-aggregated.c:157-163`): only a list with more items than the limit is cut, and then after one item
/// fewer than the limit, with `... N <what> more` for the rest. No limit is 0.
pub(super) fn limited_items_to_json<'a>(
    w: &mut JsonWriter,
    items: impl ExactSizeIterator<Item = &'a [u8]>,
    limit: usize,
    what: &str,
) {
    let total = items.len();
    for (count, item) in items.enumerate() {
        if limit != 0 && count >= limit - 1 && total > limit {
            w.add_array_item_string(format!("... {} {what} more", total - count));
            break;
        }
        w.add_array_item_string(item);
    }
}

/// `RRDLABELS_AGGREGATED`.
#[derive(Debug, Default)]
pub(super) struct AggregatedLabels {
    keys: IndexMap<Vec<u8>, IndexSet<Vec<u8>>>,
}

impl AggregatedLabels {
    /// `rrdlabels_aggregated_add_from_rrdlabels()`: each label's value joins its key's set.
    pub(super) fn add_from(&mut self, labels: &Labels) {
        for label in labels.iter() {
            match self.keys.get_mut(&label.name) {
                Some(values) => {
                    values.insert(label.value.clone());
                }
                None => {
                    self.keys.insert(label.name.clone(), IndexSet::from([label.value.clone()]));
                }
            }
        }
    }

    /// `rrdlabels_aggregated_to_buffer_json()`: an object of the keys, each an array of its values under the limit.
    pub(super) fn to_json(&self, w: &mut JsonWriter, key: &[u8], limit: usize) {
        w.member_add_object(key);
        for (name, values) in &self.keys {
            w.member_add_array(Some(name.as_slice()));
            limited_items_to_json(w, values.iter().map(Vec::as_slice), limit, "values");
            w.array_close();
        }
        w.object_close();
    }
}

#[cfg(test)]
mod tests {
    use netdata_agent_rrd::labels::SRC_CONFIG;
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> Labels {
        let mut labels = Labels::default();
        for (name, value) in pairs {
            labels.add(name.as_bytes(), value.as_bytes(), SRC_CONFIG);
        }
        labels
    }

    /// A key's values are a set in the order first seen, across the label sets added; a limit cuts only the keys
    /// with more values than it, after one fewer (`rrdlabels-aggregated.c:157-163`): with a limit of 1 a key of one
    /// value still prints it.
    #[test]
    fn values_are_sets_and_the_limit_cuts_the_long_ones() {
        let mut aggregated = AggregatedLabels::default();
        aggregated.add_from(&labels(&[("plugin", "p"), ("k", "v1")]));
        aggregated.add_from(&labels(&[("plugin", "p"), ("k", "v2"), ("only", "x")]));
        aggregated.add_from(&labels(&[("k", "v1"), ("k2", "a")]));
        aggregated.add_from(&labels(&[("k", "v3")]));
        let printed = |limit| {
            let mut w = JsonWriter::new(JsonOptions::MINIFY);
            aggregated.to_json(&mut w, b"labels", limit);
            w.finalize();
            String::from_utf8(w.into_bytes()).unwrap()
        };
        let with_k = |k: &str| format!(r#"{{"labels":{{"plugin":["p"],"k":[{k}],"only":["x"],"k2":["a"]}}}}"#);
        assert_eq!(printed(0), with_k(r#""v1","v2","v3""#));
        assert_eq!(printed(3), with_k(r#""v1","v2","v3""#));
        assert_eq!(printed(2), with_k(r#""v1","... 2 values more""#));
        assert_eq!(printed(1), with_k(r#""... 3 values more""#));
    }
}
