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

/// `progress_function_result()`'s cells (`progress.c:439-508`): a finished row's literal `100.00 %%` and its code,
/// size and sent; a running row's share or steps and its nulls; the severities; the client's fallbacks by ACL; the
/// STREAM key masked (D176.7); the maxima of finished rows only and `expires` a second past the clock.
#[test]
fn api_calls_cells_are_cs() {
    use netdata_agent_web::progress::{Finish, Start, Table};
    use netdata_agent_web::request::Mode;

    fn clock() -> u64 {
        10_000_000
    }
    let table = Table::with_clock(clock);
    let tx = |n: u8| [n; 16];
    let start = |n: u8, mode: Mode, acl: u32, query: &'static [u8], client: &'static [u8]| {
        table.start(&tx(n), 4_000_000, Start { mode: Some(mode), acl, query, client });
    };
    start(1, Mode::Get, 0, b"/api/v1/info", b"localhost");
    start(2, Mode::Stream, 0, b"/stream?key=secret&hostname=c", b"10.0.0.1");
    start(3, Mode::Get, crate::acl::bits::ACLK, b"/api/v1/data", b"");
    start(4, Mode::Post, crate::acl::bits::WEBRTC, b"/api/v1/function", b"");
    start(5, Mode::Get, 0, b"/x", b"");
    table.set_finish_line(&tx(3), 3);
    table.done_step(&tx(3), 1);
    table.done_step(&tx(4), 7);
    for (n, code, size) in [(1, 200, 100), (2, 304, 900), (5, 503, 50)] {
        table.finished(&tx(n), 9_000_000, Finish { code, duration_ut: 2500, response_size: size, sent_size: size / 2 });
    }
    let mut reply = Reply::new(ContentType::TextPlain);
    assert_eq!(api_calls::render(&table, &mut reply, "box"), 200);
    let body = String::from_utf8(reply.body).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&body).unwrap();
    let mut rows: Vec<Vec<serde_json::Value>> =
        doc["data"].as_array().unwrap().iter().map(|r| r.as_array().unwrap().clone()).collect();
    rows.sort_by_key(|r| r[0].as_str().unwrap().to_string());
    let cells = |r: &[serde_json::Value]| {
        let text = |v: &serde_json::Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
        r[2..].iter().map(text).collect::<Vec<_>>().join(" | ")
    };
    let got: Vec<String> = rows.iter().map(|r| cells(r)).collect();
    assert_eq!(
        got,
        [
            "GET | /api/v1/info | localhost | finished | 100.00 %% | 2.5 | 200 | 100 | 50 | {\"severity\":\"normal\"}",
            "STREAM | /stream?key=[REDACTED]&hostname=c | 10.0.0.1 | finished | 100.00 %% | 2.5 | 304 | 900 | 450 | \
             {\"severity\":\"debug\"}",
            "GET | /api/v1/data | ACLK | in-progress | 33.33 % | 6000 | null | null | null | {\"severity\":\"notice\"}",
            "POST | /api/v1/function | WEBRTC | in-progress | 7 | 6000 | null | null | null | {\"severity\":\"notice\"}",
            "GET | /x | unknown | finished | 100.00 %% | 2.5 | 503 | 50 | 25 | {\"severity\":\"error\"}",
        ]
    );
    assert_eq!(rows[0][0], "01010101010101010101010101010101");
    assert_eq!(rows[0][1], 4_000_000);
    let columns = &doc["columns"];
    assert_eq!((columns["Duration"]["max"].clone(), columns["Size"]["max"].clone()), (6000.into(), 900.into()));
    assert_eq!((columns["Sent"]["max"].clone(), columns["rowOptions"]["sortable"].clone()), (450.into(), false.into()));
    assert_eq!((doc["expires"].clone(), doc["hostname"].clone()), (11.into(), "box".into()));
    assert_eq!((reply.content_type, reply.cacheable, reply.expires), (ContentType::ApplicationJson, false, 0));
}

/// `function_metrics_cardinality()`'s counts (`function-metrics-cardinality.c:97-245`): a collected instance counts
/// its metrics by their own flag, one never collected counts all of its metrics archived; rows by context or by
/// hostname in first-seen order; `info` answers the header alone; the maxima over the rows.
#[test]
fn cardinality_counts_are_cs() {
    use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
    use netdata_agent_rrd::contexts::collected_rrdset;
    use netdata_agent_rrd::mode::DbMode;

    let hosts = hosts();
    let localhost = hosts.localhost();
    let spec = |id: &'static str, context: &'static str| ChartSpec {
        type_: "t",
        id,
        name: None,
        family: Some("f"),
        context: Some(context),
        title: "T",
        units: "u",
        plugin: "p",
        module: None,
        priority: 1000,
        update_every: 1,
        chart_type: ChartType::Line,
        mode: DbMode::Ram,
        history_entries: 5,
        page_size: 4096,
    };
    let chart = |id, context, dims: &[(&str, bool)], collect: bool| {
        let (chart, _) = localhost.charts().create(&spec(id, context));
        for (name, store) in dims {
            let (dim, _) = chart.dim_add(name, None, 1, 1, Algorithm::Absolute);
            if *store {
                dim.store_metric(1_000_000_000, 1.0, 0);
            }
        }
        if collect {
            collected_rrdset(&chart);
        }
    };
    chart("a", "ctx.a", &[("x", true), ("y", true)], true);
    chart("b", "ctx.a", &[("z", false)], false);
    chart("c", "ctx.b", &[("u", true), ("v", false)], true);
    localhost.contexts().process_queued();

    let render = |function: &str| {
        let mut reply = Reply::new(ContentType::TextPlain);
        assert_eq!(cardinality::render(&hosts, &mut reply, function.as_bytes()), 200);
        serde_json::from_slice::<serde_json::Value>(&reply.body).unwrap()
    };
    let rows = |doc: &serde_json::Value| -> Vec<String> {
        doc["data"].as_array().unwrap().iter().map(|r| r.to_string()).collect()
    };
    let by_context = render("netdata-metrics-cardinality");
    assert_eq!(
        rows(&by_context),
        [
            r#"["ctx.a",1,1,0,2,1,1,3,2,1,50,33.3333333,66.6666667,50,100,60,66.6666667,50]"#,
            r#"["ctx.b",1,1,0,1,1,0,2,1,1,0,50,33.3333333,50,0,40,33.3333333,50]"#,
        ]
    );
    assert_eq!(by_context["columns"]["All Instances"]["max"], 2);
    assert_eq!(by_context["columns"]["Old Dimensions"]["max"], 1);
    assert_eq!(by_context["columns"]["Context"]["index"], 0);
    assert_eq!(by_context["columns"]["Old Dimensions %"]["index"], 17);
    let by_node = render("netdata-metrics-cardinality x 'group:by-node'");
    assert_eq!(rows(&by_node), [r#"["box",3,2,1,5,3,2,33.3333333,40,100,100,100,100,100,100]"#]);
    assert_eq!(by_node["columns"]["Old Dimensions %"]["index"], 14);
    assert!(by_node["columns"]["All Nodes"].is_null());
    let info = render("netdata-metrics-cardinality group:by-node info");
    assert!(info["data"].is_null() && info["expires"].is_null() && info["required_params"].is_array());
}
