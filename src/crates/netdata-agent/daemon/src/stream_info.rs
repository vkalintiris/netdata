//! `/api/v3/stream_info`, ported from `api_v3_stream_info()` (`src/web/api/v3/api_v3_stream_info.c`) and
//! `stream_info_to_json_v1()` (`src/streaming/stream-parents.c`): what a child asks a parent before connecting to it.
//! Decisions D48 in the status repository.

use netdata_agent_rrd::host::Hosts;
use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::server::Reply;

/// The answer about the host whose machine GUID is the last non-empty `machine_guid` of `query`: 404 (as JSON) when
/// there is none.
pub fn reply(hosts: &Hosts, query: &[u8], now: i64) -> Reply {
    let mut guid = None;
    let mut rest = Some(query);
    while rest.is_some() {
        let mut value = Some(strsep_skip(&mut rest, b"&"));
        let name = strsep_skip(&mut value, b"=");
        let value = value.unwrap_or(b"");
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if name == b"machine_guid" {
            guid = Some(value);
        }
    }
    // rrdhost_find_by_guid(): the stored text, exactly
    let status = guid
        .and_then(|guid| hosts.find_by_guid(&String::from_utf8_lossy(guid)))
        .map(|host| host.status_basic(now));
    let code = if status.is_some() {
        status::OK
    } else {
        status::NOT_FOUND
    };
    let all = hosts.all();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_uint64("version", 1);
    w.member_add_uint64("status", u64::from(code));
    let localhost_id =
        uuid_parse_flexi(hosts.localhost().machine_guid().as_bytes()).unwrap_or_default();
    w.member_add_uuid("host_id", &localhost_id);
    w.member_add_uint64("nodes", all.len() as u64);
    // stream_receivers_currently_connected()
    let receivers = hosts.receivers_connected();
    w.member_add_uint64("receivers", receivers as u64);
    // os_random32(): v4 UUIDs fix only bytes 6 and 8
    let random = uuid::Uuid::new_v4();
    let nonce = u32::from_ne_bytes([
        random.as_bytes()[0],
        random.as_bytes()[1],
        random.as_bytes()[2],
        random.as_bytes()[3],
    ]);
    w.member_add_uint64("nonce", u64::from(nonce));
    // backfill never runs here (no tiers), so an offline host is never reported initializing
    if let Some(s) = status {
        w.member_add_string("db_status", s.db_status.name());
        w.member_add_string("db_liveness", s.db_liveness.name());
        w.member_add_string("ingest_type", s.ingest_type.name());
        w.member_add_string("ingest_status", s.ingest_status.name());
        w.member_add_uint64("first_time_s", s.first_time_s as u64);
        w.member_add_uint64("last_time_s", s.last_time_s as u64);
    }
    w.finalize();
    Reply {
        code,
        content_type: ContentType::ApplicationJson,
        body: w.into_bytes(),
        ..Reply::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
    use netdata_agent_rrd::host::{Host, HostInfo, ReceiverLink, ReceiverSlot};
    use netdata_agent_rrd::mode::DbMode;

    use super::*;

    const LOCALHOST: &str = "5a1e0000-0000-4000-8000-0000000000aa";
    const CHILD: &str = "5a1e0000-0000-4000-8000-00000000c004";
    const NOW: i64 = 1_790_400_218;

    fn info(hostname: &str) -> HostInfo {
        HostInfo {
            hostname: hostname.into(),
            registry_hostname: hostname.into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 3600,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        }
    }

    fn attach(host: &Host) -> Arc<ReceiverSlot> {
        let slot = Arc::new(ReceiverSlot::new(
            0,
            Default::default(),
            ReceiverLink::default(),
            Box::new(|| {}),
        ));
        assert!(host.set_receiver(Arc::clone(&slot)));
        slot
    }

    /// The body with the nonce replaced, as the parity check masks it.
    fn body(hosts: &Hosts, query: &str) -> (u16, String) {
        let reply = reply(hosts, query.as_bytes(), NOW);
        let text = String::from_utf8(reply.body).unwrap();
        let (head, tail) = text.split_once("\"nonce\":").unwrap();
        let tail = tail.trim_start_matches(|c: char| c.is_ascii_digit());
        (reply.code, format!("{head}\"nonce\":X{tail}"))
    }

    /// C's bodies byte for byte (the spec's captures, `evidence/2026-09-26-sp1-stream-info-spec.md` §1.4 and §5).
    #[test]
    fn answers_as_c() {
        let hosts = Hosts::new(Host::new(LOCALHOST, true, info("parity-parent")));
        let not_found = |nodes: usize, receivers: usize| {
            format!(
                "{{\n    \"version\":1,\n    \"status\":404,\n    \"host_id\":\"{LOCALHOST}\",\n    \"nodes\":{nodes},\n    \"receivers\":{receivers},\n    \"nonce\":X\n}}\n"
            )
        };
        for query in [
            "",
            "machine_guid=",
            "machine_guid=nope",
            "machine_guid=5A1E0000-0000-4000-8000-0000000000AA",
            "machine_guid=parity-parent",
            "machine_guid=localhost",
            &format!("machine_guid={LOCALHOST}&machine_guid=bogus"),
            &format!("machine_guid=={LOCALHOST}"),
        ] {
            assert_eq!(body(&hosts, query), (404, not_found(1, 0)), "{query:?}");
        }
        let found = |fields: &str| {
            format!(
                "{{\n    \"version\":1,\n    \"status\":200,\n    \"host_id\":\"{LOCALHOST}\",\n    \"nodes\":2,\n    \"receivers\":1,\n    \"nonce\":X,\n{fields}\n}}\n"
            )
        };
        let child = hosts.find_or_create(CHILD, DbMode::Ram, || info("parity-cchild-none"), |_| {});
        let slot = attach(&child);
        // the child just attached: nothing post-processed yet
        let initializing = found(&format!(
            "    \"db_status\":\"initializing\",\n    \"db_liveness\":\"stale\",\n    \"ingest_type\":\"child\",\n    \"ingest_status\":\"initializing\",\n    \"first_time_s\":0,\n    \"last_time_s\":{NOW}"
        ));
        for query in [
            format!("machine_guid={CHILD}"),
            format!("machine_guid=bogus&machine_guid={CHILD}"),
            format!("&&machine_guid={CHILD}&machine_guid=&"),
        ] {
            assert_eq!(
                body(&hosts, &query),
                (200, initializing.clone()),
                "{query:?}"
            );
        }
        let (chart, _) = child.charts().create(&ChartSpec {
            type_: "netdata",
            id: "c",
            name: None,
            family: Some("f"),
            context: Some("netdata.c"),
            title: "t",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        });
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        for t in NOW - 24..=NOW - 20 {
            dim.store_metric(t as u64 * 1_000_000, 1.0, 0);
        }
        netdata_agent_rrd::contexts::collected_rrdset(&chart);
        child.contexts().worker_cycle();
        let first = child.contexts().retention().0;
        let online = |liveness: &str, status: &str| {
            found(&format!(
                "    \"db_status\":\"online\",\n    \"db_liveness\":\"{liveness}\",\n    \"ingest_type\":\"child\",\n    \"ingest_status\":\"{status}\",\n    \"first_time_s\":{first},\n    \"last_time_s\":{NOW}"
            ))
        };
        let query = format!("machine_guid={CHILD}");
        assert_eq!(body(&hosts, &query), (200, online("live", "online")));
        chart.update_meta(|m| {
            m.flags |= netdata_agent_rrd::chart::flags::RECEIVER_REPLICATION_IN_PROGRESS;
        });
        assert_eq!(body(&hosts, &query), (200, online("stale", "replicating")));
        // after the disconnect: offline, with the retention's own last time
        child.clear_receiver(&slot);
        let last = child.contexts().retention().1;
        let offline = format!(
            "{{\n    \"version\":1,\n    \"status\":200,\n    \"host_id\":\"{LOCALHOST}\",\n    \"nodes\":2,\n    \"receivers\":0,\n    \"nonce\":X,\n    \"db_status\":\"online\",\n    \"db_liveness\":\"stale\",\n    \"ingest_type\":\"archived\",\n    \"ingest_status\":\"offline\",\n    \"first_time_s\":{first},\n    \"last_time_s\":{last}\n}}\n"
        );
        assert_eq!(body(&hosts, &query), (200, offline));
        // back before its first store: the database is online, nothing is collected yet
        child.contexts().worker_cycle();
        attach(&child);
        let (code, text) = body(&hosts, &query);
        assert_eq!(code, 200);
        assert!(
            text.contains("\"db_status\":\"online\",\n    \"db_liveness\":\"stale\",\n    \"ingest_type\":\"child\",\n    \"ingest_status\":\"replicating\""),
            "{text}"
        );
        // localhost without charts keeps C's startup shape (D48 point 6)
        let (code, text) = body(&hosts, &format!("machine_guid={LOCALHOST}"));
        assert_eq!(code, 200);
        assert!(
            text.contains("\"ingest_type\":\"localhost\",\n    \"ingest_status\":\"initializing\""),
            "{text}"
        );
    }
}
