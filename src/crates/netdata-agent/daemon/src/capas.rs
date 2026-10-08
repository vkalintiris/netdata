//! `aclk_get_node_instance_capas()` (`src/aclk/aclk_capas.c`) and `agent_capabilities_to_json()`: what a host of the
//! agent offers its clients. A capability of a subsystem not ported yet is off (D92.1), so no client asks for it.

use netdata_agent_pluginsd_proto::caps;
use netdata_agent_rrd::host::Host;
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

/// `aclk_get_node_instance_capas()`'s entries in its order, with what is the same for every host; `funcs`, `health`
/// and `dyncfg` are each host's own, set by [`to_json`].
pub const NODE_INSTANCE: [Capability; 9] = [
    // ACLK's protocol (M11)
    capa("proto", 1, false),
    // ml_capable() and ml_enabled(): no ML (M14)
    capa("ml", 0, false),
    // metric_correlations_version: the weights API
    capa("mc", 1, true),
    capa("ctx", 1, true),
    // localhost's own, or what a child's receiver negotiated
    capa("funcs", 1, true),
    capa("http_api_v2", HTTP_API_V2_VERSION, true),
    // host->health.enabled
    capa("health", 2, false),
    // ACLK's request cancellation (M11)
    capa("req_cancel", 1, false),
    // localhost's own, or a host's whose `config` method is available
    capa("dyncfg", 2, true),
];

/// `agent_capabilities_to_json()` over `aclk_get_node_instance_capas()`: the host's capabilities. Functions are
/// localhost's, or a child's whose receiver negotiated them (`receiver_has_capability()`), in version and flag
/// alike; DynCfg is localhost's, or a host's whose `config` method is available now
/// (`dyncfg_available_for_rrdhost()`); health is the host's own setting.
pub fn to_json(w: &mut JsonWriter, key: &[u8], host: &Host) {
    let local = host.is_localhost();
    let functions = local || host.receiver().is_some_and(|slot| slot.link.capabilities & caps::FUNCTIONS != 0);
    let dyncfg = host.dyncfg_available();
    w.member_add_array(Some(key));
    for c in NODE_INSTANCE {
        let (version, enabled) = match c.name {
            "funcs" => (u64::from(functions), functions),
            "health" => (c.version, host.health_enabled()),
            "dyncfg" => (c.version, dyncfg),
            _ => (c.version, c.enabled),
        };
        w.add_array_item_object();
        w.member_add_string("name", c.name);
        w.member_add_uint64("version", version);
        w.member_add_boolean("enabled", enabled);
        w.object_close();
    }
    w.array_close();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use netdata_agent_nrpc::testing::inert;
    use netdata_agent_nrpc::{MethodDesc, Source, access};
    use netdata_agent_rrd::host::{Attach, ReceiverLink, ReceiverSlot};
    use netdata_agent_text::json::JsonOptions;

    use super::*;
    use crate::testing::host_info;

    fn host(local: bool) -> Host {
        Host::new("0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e", local, host_info("box"))
    }

    fn rendered(host: &Host) -> String {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        to_json(&mut w, b"capabilities", host);
        w.finalize();
        String::from_utf8(w.into_bytes()).unwrap()
    }

    /// The three entries that are the host's own, as (`funcs`, `health`, `dyncfg`), each `version/enabled`; the other
    /// six must be the table's.
    fn own(host: &Host) -> [String; 3] {
        let text = rendered(host);
        let entry = |name: &str| {
            let at = text.find(&format!(r#"{{"name":"{name}","#)).unwrap_or_else(|| panic!("{name} in {text}"));
            let rest = &text[at..];
            let (version, enabled) = rest[..rest.find('}').unwrap()].split_once(r#","enabled":"#).unwrap();
            format!("{}/{enabled}", &version[version.rfind(':').unwrap() + 1..])
        };
        for c in NODE_INSTANCE.iter().filter(|c| !["funcs", "health", "dyncfg"].contains(&c.name)) {
            assert_eq!(entry(c.name), format!("{}/{}", c.version, c.enabled), "{}", c.name);
        }
        ["funcs", "health", "dyncfg"].map(entry)
    }

    /// Localhost's nine entries, in C's order (`aclk_capas.c:44-55`), with C's versions but `ml`'s; the flags of the
    /// subsystems not ported yet are off. `/api/v2/info` prints these.
    #[test]
    fn localhost_s_capabilities_are_the_table_s() {
        assert_eq!(
            rendered(&host(true)),
            concat!(
                r#"{"capabilities":[{"name":"proto","version":1,"enabled":false},"#,
                r#"{"name":"ml","version":0,"enabled":false},{"name":"mc","version":1,"enabled":true},"#,
                r#"{"name":"ctx","version":1,"enabled":true},{"name":"funcs","version":1,"enabled":true},"#,
                r#"{"name":"http_api_v2","version":7,"enabled":true},{"name":"health","version":2,"enabled":false},"#,
                r#"{"name":"req_cancel","version":1,"enabled":false},{"name":"dyncfg","version":2,"enabled":true}]}"#
            )
        );
    }

    /// Functions, health and DynCfg are each host's own: localhost has functions and DynCfg whatever its receiver;
    /// a child has functions only while its receiver negotiated them (version and flag both 0 otherwise), and
    /// DynCfg only while its `config` method is available; health follows the host's setting.
    #[test]
    fn functions_health_and_dyncfg_follow_the_host() {
        let attach = |host: &Host, capabilities: u32| {
            let link = ReceiverLink { capabilities, ..ReceiverLink::default() };
            let slot = Arc::new(ReceiverSlot::new(0, Default::default(), link, Box::new(|| {})));
            assert_eq!(host.set_receiver(slot), Attach::Attached);
        };
        // C's name of the method, `PLUGINSD_FUNCTION_CONFIG`
        let config = MethodDesc {
            name: b"config",
            help: b"",
            tags: b"",
            timeout_s: 10,
            priority: 0,
            version: 1,
            access: access::ANONYMOUS_DATA,
            sync: false,
            source: Source::Stream,
            handler: inert(),
        };

        let local = host(true);
        assert_eq!(own(&local), ["1/true", "2/false", "2/true"]);
        local.set_health_enabled(true);
        assert_eq!(own(&local), ["1/true", "2/true", "2/true"]);

        // a virtual node of one of this agent's plugins is not localhost: C's shortcut is `host == localhost` alone
        let vnode = host(false);
        vnode.set_virtual();
        assert_eq!(own(&vnode), ["0/false", "2/false", "2/false"]);

        // a child without a receiver, then with one that negotiated everything but functions
        let without = host(false);
        assert_eq!(own(&without), ["0/false", "2/false", "2/false"]);
        attach(&without, !caps::FUNCTIONS);
        assert_eq!(own(&without), ["0/false", "2/false", "2/false"]);
        // a child whose receiver negotiated functions
        let child = host(false);
        attach(&child, caps::FUNCTIONS);
        assert_eq!(own(&child), ["1/true", "2/false", "2/false"]);
        child.set_health_enabled(true);
        assert_eq!(own(&child), ["1/true", "2/true", "2/false"]);
        // its `config` method: there, then retired by the host's next epoch; another method does not count
        child.functions().register("h", &MethodDesc { name: b"configuration", ..config.clone() }).unwrap();
        assert_eq!(own(&child), ["1/true", "2/true", "2/false"]);
        child.functions().register("h", &config).unwrap();
        assert_eq!(own(&child), ["1/true", "2/true", "2/true"]);
        child.functions().activate();
        assert_eq!(own(&child), ["1/true", "2/true", "2/false"]);
    }
}
