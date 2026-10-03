use std::sync::Arc;

use netdata_agent_nrpc::call::{CallSpec, Calls, SystemClock};
use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_nrpc::{FLAG_RESTRICTED, Filter, Source as NrpcSource};
use netdata_agent_rrd::host::{Host, HostInfo, Hosts};

use super::*;

fn hosts() -> Arc<Hosts> {
    Arc::new(Hosts::new(Host::new(
        "0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e",
        true,
        HostInfo {
            hostname: "box".into(),
            registry_hostname: "box".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: netdata_agent_rrd::mode::DbMode::Ram,
            history_entries: 4096,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        },
    )))
}

/// `global_functions_add()` (`functions.c:5-66`): C's five on localhost, in C's order, sync and the daemon's, with
/// C's tags, timeouts, priorities and accesses; `bearer_get_token` RESTRICTED by its `hidden` tag, so only the re-list
/// carries it.
#[test]
fn builtins_are_cs_and_in_cs_order() {
    let hosts = hosts();
    global_functions_add(&hosts);
    let registry = hosts.localhost().functions();
    let (methods, _) = registry.visible(Filter::StreamGlobal);
    let listed: Vec<_> = methods
        .iter()
        .map(|(name, m)| {
            (
                String::from_utf8_lossy(name).into_owned(),
                String::from_utf8_lossy(&m.tags).into_owned(),
                m.timeout_s,
                m.priority,
                m.version,
                m.access,
                m.sync,
                m.source,
                m.flags,
            )
        })
        .collect();
    let row = |name: &str, tags: &str, priority, access, flags| {
        (name.to_string(), tags.to_string(), 10, priority, 0, access, true, NrpcSource::Daemon, flags)
    };
    assert_eq!(
        listed,
        [
            row("netdata-streaming", "top", 101, 0x13, 0),
            row("topology:streaming", "top", 101, 0x13, 0),
            row("netdata-api-calls", "top", 101, 0x13, 0),
            row("bearer_get_token", "hidden", 103, 0x13, FLAG_RESTRICTED),
            row("netdata-metrics-cardinality", "top", 101, 0x8, 0),
        ]
    );
    assert_eq!(registry.get(b"netdata-metrics-cardinality").unwrap().help, METRICS_CARDINALITY_HELP.as_bytes());
    let (user, _) = registry.visible(Filter::User);
    assert!(user.iter().all(|(name, _)| name.as_slice() != b"bearer_get_token"));
    assert_eq!(user.len(), 4);
}

/// The handlers hold the hosts weakly: localhost's registry holds them, so dropping the hosts frees localhost.
#[test]
fn builtin_handlers_hold_no_strong_hosts() {
    let hosts = hosts();
    global_functions_add(&hosts);
    let localhost = Arc::downgrade(hosts.localhost());
    let index = Arc::downgrade(&hosts);
    drop(hosts);
    assert!(index.upgrade().is_none() && localhost.upgrade().is_none());
}

/// A call as the web server makes it, waiting for its answer.
fn call(hosts: &Arc<Hosts>, cmd: &str) -> (u16, Reply) {
    let calls = Calls::new(Box::new(SystemClock));
    let localhost = hosts.localhost();
    let called = calls.call(CallSpec {
        owner: Some((localhost.functions(), &localhost.hostname())),
        cmd: cmd.as_bytes(),
        source: b"test",
        user_access: access::ALL,
        timeout_s: 10,
        wait: true,
        allow_restricted: true,
        call_id: None,
        payload: None,
        reply: Reply::new(ContentType::ApplicationJson),
        done: None,
        progress: None,
        is_cancelled: None,
        tag: None,
    });
    (called.code, called.reply.unwrap())
}

/// D176.3: until M10 the streaming two answer nRPC's 501 but for `topology:streaming info`, which is C's
/// (`function-topology-streaming.c:2949-2958`: `info` as any word after the name, pretty, not cacheable); the
/// handlers of later steps answer the same placeholder.
#[test]
fn the_streaming_placeholder_answers_as_decided() {
    let hosts = hosts();
    global_functions_add(&hosts);
    let placeholder = format!("{{\"status\":501,\"errorMessage\":\"{NOT_IMPLEMENTED}\"}}");
    for cmd in ["netdata-streaming", "netdata-streaming info", "topology:streaming", "topology:streaming x"] {
        let (code, reply) = call(&hosts, cmd);
        assert_eq!((code, String::from_utf8_lossy(&reply.body).into_owned()), (501, placeholder.clone()), "{cmd}");
    }
    for cmd in ["topology:streaming info", "topology:streaming x 'info'"] {
        let (code, reply) = call(&hosts, cmd);
        let body = String::from_utf8_lossy(&reply.body).into_owned();
        let expires = body.rsplit("\"expires\":").next().unwrap().split('\n').next().unwrap().parse::<i64>().unwrap();
        assert!((netdata_agent_rrd::clock::now_realtime_s() + 9..=netdata_agent_rrd::clock::now_realtime_s() + 10)
            .contains(&expires));
        assert_eq!(
            (code, body.replace(&expires.to_string(), "E"), reply.cacheable),
            (
                200,
                format!(
                    "{{\n    \"status\":200,\n    \"type\":\"topology\",\n    \"update_every\":10,\n    \"has_history\":false,\
                     \n    \"help\":\"{STREAMING_TOPOLOGY_HELP}\",\n    \"accepted_params\":[\"info\"],\n    \
                     \"required_params\":[],\n    \"expires\":E\n}}\n"
                ),
                false
            ),
            "{cmd}"
        );
    }
}
