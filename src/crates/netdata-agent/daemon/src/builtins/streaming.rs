//! The two streaming built-ins (`function-netdata-streaming.c`, `function-topology-streaming.c`). Both read every
//! host's status, which comes with M10; until then `topology:streaming info` is C's, and every other call answers
//! D176.3's placeholder.

use netdata_agent_nrpc::reply::{Payload, Reply};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

use super::{STREAMING_TOPOLOGY_HELP, json_reply, not_implemented};

/// `STREAMING_FUNCTION_UPDATE_EVERY`.
const UPDATE_EVERY: i64 = 10;

/// `function_netdata_streaming()`: M10 (the host status).
pub(super) fn netdata_streaming(reply: &mut Reply, _: &[u8], _: Option<&Payload>, _: &[u8]) -> u16 {
    not_implemented(reply)
}

/// `function_streaming_topology()`: `info` as any word after the name answers the metadata at once; the topology
/// itself is M10.
pub(super) fn topology(reply: &mut Reply, function: &[u8], _: Option<&Payload>, _: &[u8]) -> u16 {
    let info = quoted_strings_splitter(function, 1024, Separators::Whitespace).iter().skip(1).any(|w| w == b"info");
    if !info {
        return not_implemented(reply);
    }
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_uint64("status", 200);
    w.member_add_string("type", "topology");
    w.member_add_time_t("update_every", UPDATE_EVERY);
    w.member_add_boolean("has_history", false);
    w.member_add_string("help", STREAMING_TOPOLOGY_HELP);
    w.member_add_array(Some(b"accepted_params"));
    w.add_array_item_string("info");
    w.array_close();
    w.member_add_array(Some(b"required_params"));
    w.array_close();
    w.member_add_time_t("expires", now_realtime_s() + UPDATE_EVERY);
    json_reply(w, reply)
}
