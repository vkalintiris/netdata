//! Stream capabilities (`src/streaming/stream-capabilities.{h,c}`): the bits, their names and what this agent
//! offers. Shared by the handshake (`streaming`) and the keyword handlers (`ingest`) that print or parse them.

use netdata_agent_text::json::JsonWriter;

pub const V1: u32 = 1 << 3;
pub const V2: u32 = 1 << 4;
pub const VN: u32 = 1 << 5;
pub const VCAPS: u32 = 1 << 6;
pub const HLABELS: u32 = 1 << 7;
pub const CLAIM: u32 = 1 << 8;
pub const CLABELS: u32 = 1 << 9;
pub const LZ4: u32 = 1 << 10;
pub const FUNCTIONS: u32 = 1 << 11;
pub const REPLICATION: u32 = 1 << 12;
pub const BINARY: u32 = 1 << 13;
pub const INTERPOLATED: u32 = 1 << 14;
pub const IEEE754: u32 = 1 << 15;
pub const DATA_WITH_ML: u32 = 1 << 16;
pub const SLOTS: u32 = 1 << 18;
pub const ZSTD: u32 = 1 << 19;
pub const GZIP: u32 = 1 << 20;
pub const BROTLI: u32 = 1 << 21;
pub const PROGRESS: u32 = 1 << 22;
pub const DYNCFG: u32 = 1 << 23;
pub const NODE_ID: u32 = 1 << 24;
pub const PATHS: u32 = 1 << 25;
pub const ML_MODELS: u32 = 1 << 26;
pub const FLOAT_BASELINE: u32 = 1 << 27;
pub const FUNCTION_DEL: u32 = 1 << 28;
/// `STREAM_CAP_INVALID`: no version seen yet.
pub const INVALID: u32 = 1 << 30;

/// `STREAM_CAP_ALWAYS_DISABLED`.
pub const ALWAYS_DISABLED: u32 = DATA_WITH_ML;

/// The compressions the C production build offers (`STREAM_CAP_COMPRESSIONS_AVAILABLE` with lz4, zstd, brotli).
pub const COMPRESSIONS: u32 = LZ4 | ZSTD | BROTLI | GZIP;

/// Compressions this agent can decompress (`decompress.rs`, decisions D14 and D23): all the C build offers.
pub const COMPRESSIONS_AVAILABLE: u32 = COMPRESSIONS;

/// `stream_our_capabilities(NULL, false)`: what a receiver offers. `disabled` is `globally_disabled_capabilities`.
pub fn ours(disabled: u32) -> u32 {
    (V1 | V2
        | VN
        | VCAPS
        | HLABELS
        | CLAIM
        | CLABELS
        | FUNCTIONS
        | FUNCTION_DEL
        | REPLICATION
        | BINARY
        | INTERPOLATED
        | SLOTS
        | PROGRESS
        | COMPRESSIONS_AVAILABLE
        | DYNCFG
        | NODE_ID
        | PATHS
        | IEEE754
        | ML_MODELS
        | FLOAT_BASELINE)
        & !disabled
}

/// `globally_disabled_capabilities` after `check_local_streaming_capabilities()` on an IEEE-754 host.
pub const GLOBALLY_DISABLED: u32 = ALWAYS_DISABLED;

/// `capability_names[]`, in table order (not bit order).
const NAMES: [(u32, &str); 25] = [
    (V1, "V1"),
    (V2, "V2"),
    (VN, "VN"),
    (VCAPS, "VCAPS"),
    (HLABELS, "HLABELS"),
    (CLAIM, "CLAIM"),
    (CLABELS, "CLABELS"),
    (LZ4, "LZ4"),
    (FUNCTIONS, "FUNCTIONS"),
    (FUNCTION_DEL, "FUNCDEL"),
    (REPLICATION, "REPLICATION"),
    (BINARY, "BINARY"),
    (INTERPOLATED, "INTERPOLATED"),
    (IEEE754, "IEEE754"),
    (DATA_WITH_ML, "ML"),
    (ML_MODELS, "MLMODELS"),
    (DYNCFG, "DYNCFG"),
    (SLOTS, "SLOTS"),
    (ZSTD, "ZSTD"),
    (GZIP, "GZIP"),
    (BROTLI, "BROTLI"),
    (PROGRESS, "PROGRESS"),
    (NODE_ID, "NODEID"),
    (PATHS, "PATHS"),
    (FLOAT_BASELINE, "FLOATBASELINE"),
];

/// `stream_capabilities_to_string()`: every name set in `caps`, each followed by a space.
pub fn to_string(caps: u32) -> String {
    let mut out = String::new();
    for (cap, name) in NAMES {
        if caps & cap != 0 {
            out.push_str(name);
            out.push(' ');
        }
    }
    out
}

/// `stream_capabilities_parse_one()`: the bit of a name (exact, case-sensitive), 0 for an unknown or empty one.
pub fn parse_one(name: &[u8]) -> u32 {
    NAMES
        .iter()
        .find(|(_, n)| n.as_bytes() == name)
        .map_or(0, |&(cap, _)| cap)
}

/// `stream_capabilities_to_json_array()`: the names set in `caps`, in table order, as the array `key` (or an array
/// item when `key` is `None`).
pub fn to_json_array(w: &mut JsonWriter, caps: u32, key: Option<&[u8]>) {
    match key {
        Some(key) => w.member_add_array(Some(key)),
        None => w.add_array_item_array(),
    }
    for (cap, name) in NAMES {
        if caps & cap != 0 {
            w.add_array_item_string(name);
        }
    }
    w.array_close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_text::json::JsonOptions;

    #[test]
    fn names_round_trip_in_table_order() {
        assert_eq!(to_string(V1 | PATHS | FUNCTION_DEL), "V1 FUNCDEL PATHS ");
        assert_eq!(parse_one(b"FUNCDEL"), FUNCTION_DEL);
        assert_eq!(parse_one(b"funcdel"), 0);
        assert_eq!(parse_one(b""), 0);
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        to_json_array(&mut w, ML_MODELS | DATA_WITH_ML | V2, Some(b"capabilities"));
        w.finalize();
        assert_eq!(w.as_bytes(), br#"{"capabilities":["V2","ML","MLMODELS"]}"#);
    }
}
