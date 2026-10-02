//! `aclk_get_node_instance_capas()` (`src/aclk/aclk_capas.c`) and `agent_capabilities_to_json()`: what the agent
//! offers its clients. A capability of a subsystem not ported yet is off (D92.1), so no client asks for it.

use netdata_agent_text::json::JsonWriter;

/// `HTTP_API_V2_VERSION` (`aclk_get_http_api_version()`).
pub const HTTP_API_V2_VERSION: u64 = 7;

/// `struct capability`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub name: &'static str,
    pub version: u64,
    pub enabled: bool,
}

const fn capa(name: &'static str, version: u64, enabled: bool) -> Capability {
    Capability {
        name,
        version,
        enabled,
    }
}

/// `aclk_get_node_instance_capas()`, the same for every host until the subsystems that vary it are ported.
pub const NODE_INSTANCE: [Capability; 9] = [
    // ACLK's protocol (M11)
    capa("proto", 1, false),
    // ml_capable() and ml_enabled(): no ML (M14)
    capa("ml", 0, false),
    // metric_correlations_version: the weights API (M10)
    capa("mc", 1, false),
    capa("ctx", 1, true),
    // Functions: localhost's (children with FUNCTIONS come with M8 commit 7; aclk_capas.c:41,49)
    capa("funcs", 1, true),
    capa("http_api_v2", HTTP_API_V2_VERSION, true),
    // host->health.enabled: no health engine (M9)
    capa("health", 2, false),
    // ACLK's request cancellation (M11)
    capa("req_cancel", 1, false),
    // DynCfg (M8)
    capa("dyncfg", 2, false),
];

/// `agent_capabilities_to_json()`.
pub fn to_json(w: &mut JsonWriter, key: &[u8]) {
    w.member_add_array(Some(key));
    for c in NODE_INSTANCE {
        w.add_array_item_object();
        w.member_add_string("name", c.name);
        w.member_add_uint64("version", c.version);
        w.member_add_boolean("enabled", c.enabled);
        w.object_close();
    }
    w.array_close();
}
