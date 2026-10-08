//! `JSKEY()`: the short JSON member names of the v2 output, or the long ones with `long-json-keys`
//! (`src/libnetdata/json/json-keys.c`).

use crate::tables::options;

/// The key table the options select.
#[derive(Debug, Clone, Copy)]
pub struct Keys {
    long: bool,
}

macro_rules! keys {
    ($($name:ident: $short:literal, $long:literal;)*) => {
        impl Keys {
            $(
                pub fn $name(self) -> &'static str {
                    if self.long { $long } else { $short }
                }
            )*
        }
    };
}

impl Keys {
    pub fn new(options: u64) -> Self {
        Self::with_long(options & options::LONG_JSON_KEYS != 0)
    }

    /// The long names when `long`, as `CONTEXTS_OPTION_JSON_LONG_KEYS` selects them.
    pub fn with_long(long: bool) -> Self {
        Keys { long }
    }
}

keys! {
    selected: "sl", "selected";
    excluded: "ex", "excluded";
    queried: "qr", "queried";
    failed: "fl", "failed";
    dimensions: "ds", "dimensions";
    instances: "is", "instances";
    alerts: "al", "alerts";
    clear: "cl", "clear";
    warning: "wr", "warning";
    critical: "cr", "critical";
    other: "ot", "other";
    statistics: "sts", "statistics";
    name: "nm", "name";
    hostname: "nm", "hostname";
    node_id: "nd", "node_id";
    value: "vl", "value";
    label_values: "vl", "label_values";
    machine_guid: "mg", "machine_guid";
    agent_index: "ai", "agents_array_index";
    count: "cnt", "count";
    volume: "vol", "volume";
    anomaly_rate: "arp", "anomaly_rate_percent";
    anomaly_count: "arc", "anomalous_points_count";
    contribution: "con", "contribution_percent";
    point_annotations: "pa", "point_annotations_bitmap";
    point_schema: "point", "point_schema";
    priority: "pri", "priority";
    update_every: "ue", "update_every";
    tier: "tr", "tier";
    after: "af", "after";
    before: "bf", "before";
    status: "st", "status";
    units: "un", "units";
    first_entry: "fe", "first_entry";
    last_entry: "le", "last_entry";
    node_index: "ni", "nodes_array_index";
    weight: "wg", "weight";
    // the alerts endpoints' (`/api/v2/alerts`, `/api/v2/alert_transitions`)
    contexts: "ctx", "contexts";
    error: "er", "error";
    summary: "sum", "summary";
    instances_count: "in", "instances_count";
    nodes_count: "nd", "nodes_count";
    configurations_count: "cfg", "configurations_count";
    instance_id: "ch", "instance_id";
    instance_name: "ch_n", "instance";
    family: "fami", "family";
    context: "ctx", "context";
    alerts_index_id: "ati", "alerts_array_index_id";
    alert_global_id: "gi", "global_id";
    alert_name: "nm", "alert";
    last_transition_id: "tr_i", "last_transition_id";
    last_transition_value: "tr_v", "last_transition_value";
    last_transition_timestamp: "tr_t", "last_transition_timestamp";
    last_updated_value: "v", "last_updated_value";
    last_updated_timestamp: "t", "last_updated_timestamp";
    classification: "cl", "classification";
    classifications: "cls", "classifications";
    component: "cm", "component";
    components: "cp", "components";
    type_: "tp", "type";
    types: "ty", "types";
    recipients: "to", "recipients";
    source: "src", "source";
    config_hash_id: "cfg", "config_hash_id";
}

#[cfg(test)]
mod tests {
    use super::*;

    type Key = fn(Keys) -> &'static str;

    /// Every key of C's two tables (`json_short_keys` and `json_long_keys`, `json-keys.c:6-142`), in C's order:
    /// the member's name without `long-json-keys`, then with it.
    #[test]
    fn the_keys_are_c_s_two_tables() {
        let c: [(Key, &str, &str); 64] = [
            (Keys::selected, "sl", "selected"),
            (Keys::excluded, "ex", "excluded"),
            (Keys::queried, "qr", "queried"),
            (Keys::failed, "fl", "failed"),
            (Keys::dimensions, "ds", "dimensions"),
            (Keys::instances, "is", "instances"),
            (Keys::contexts, "ctx", "contexts"),
            (Keys::alerts, "al", "alerts"),
            (Keys::statistics, "sts", "statistics"),
            (Keys::name, "nm", "name"),
            (Keys::hostname, "nm", "hostname"),
            (Keys::node_id, "nd", "node_id"),
            (Keys::value, "vl", "value"),
            (Keys::label_values, "vl", "label_values"),
            (Keys::machine_guid, "mg", "machine_guid"),
            (Keys::agent_index, "ai", "agents_array_index"),
            (Keys::clear, "cl", "clear"),
            (Keys::warning, "wr", "warning"),
            (Keys::critical, "cr", "critical"),
            (Keys::error, "er", "error"),
            (Keys::other, "ot", "other"),
            (Keys::count, "cnt", "count"),
            (Keys::volume, "vol", "volume"),
            (Keys::anomaly_rate, "arp", "anomaly_rate_percent"),
            (Keys::anomaly_count, "arc", "anomalous_points_count"),
            (Keys::contribution, "con", "contribution_percent"),
            (Keys::point_annotations, "pa", "point_annotations_bitmap"),
            (Keys::point_schema, "point", "point_schema"),
            (Keys::priority, "pri", "priority"),
            (Keys::update_every, "ue", "update_every"),
            (Keys::tier, "tr", "tier"),
            (Keys::after, "af", "after"),
            (Keys::before, "bf", "before"),
            (Keys::status, "st", "status"),
            (Keys::first_entry, "fe", "first_entry"),
            (Keys::last_entry, "le", "last_entry"),
            (Keys::node_index, "ni", "nodes_array_index"),
            (Keys::units, "un", "units"),
            (Keys::weight, "wg", "weight"),
            (Keys::summary, "sum", "summary"),
            (Keys::instances_count, "in", "instances_count"),
            (Keys::nodes_count, "nd", "nodes_count"),
            (Keys::configurations_count, "cfg", "configurations_count"),
            (Keys::instance_id, "ch", "instance_id"),
            (Keys::instance_name, "ch_n", "instance"),
            (Keys::family, "fami", "family"),
            (Keys::context, "ctx", "context"),
            (Keys::alerts_index_id, "ati", "alerts_array_index_id"),
            (Keys::alert_global_id, "gi", "global_id"),
            (Keys::alert_name, "nm", "alert"),
            (Keys::last_transition_id, "tr_i", "last_transition_id"),
            (Keys::last_transition_value, "tr_v", "last_transition_value"),
            (Keys::last_transition_timestamp, "tr_t", "last_transition_timestamp"),
            (Keys::last_updated_value, "v", "last_updated_value"),
            (Keys::last_updated_timestamp, "t", "last_updated_timestamp"),
            (Keys::classification, "cl", "classification"),
            (Keys::classifications, "cls", "classifications"),
            (Keys::component, "cm", "component"),
            (Keys::components, "cp", "components"),
            (Keys::type_, "tp", "type"),
            (Keys::types, "ty", "types"),
            (Keys::recipients, "to", "recipients"),
            (Keys::source, "src", "source"),
            (Keys::config_hash_id, "cfg", "config_hash_id"),
        ];
        for (i, (key, short, long)) in c.into_iter().enumerate() {
            assert_eq!((key(Keys::with_long(false)), key(Keys::with_long(true))), (short, long), "key {i}");
        }
        // the data query's option selects the table too
        assert_eq!((Keys::new(0).alert_name(), Keys::new(options::LONG_JSON_KEYS).alert_name()), ("nm", "alert"));
    }
}
