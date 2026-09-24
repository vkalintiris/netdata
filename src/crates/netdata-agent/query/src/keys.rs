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
        Keys {
            long: options & options::LONG_JSON_KEYS != 0,
        }
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
    first_entry: "fe", "first_entry";
    last_entry: "le", "last_entry";
    node_index: "ni", "nodes_array_index";
    weight: "wg", "weight";
}
