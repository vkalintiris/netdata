//! A node's instances in the contexts v2 engine's NODE_INSTANCES mode: the `instances` array of
//! `rrdcontext_to_json_v2_rrdhost()`, with `rrdhost_receiver_to_json()` and `host_dyncfg_to_json_v2()`
//! (`src/database/contexts/api_v2_contexts.c`), over the host's status (`rrd::status`). Not written yet, with the
//! parts of the status they print: the host's `stream`, and the counts of a host whose ML runs.

use netdata_agent_nrpc::catalog;
use netdata_agent_pluginsd_proto::caps;
use netdata_agent_query::jsonwrap_v2::agent_status_id;
use netdata_agent_query::keys::Keys;
use netdata_agent_query::tables::contexts_options::RFC3339;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::status::{IngestStatus, IngestType, Status};
use netdata_agent_streaming::reason::Reason;
use netdata_agent_text::json::JsonWriter;

use super::nodes::health_to_json;
use crate::capas;
use crate::server::Shared;

/// The node's `instances`: an array of exactly one object, this agent's view of the host at the walk's clock `now`.
pub(super) fn to_json(w: &mut JsonWriter, host: &Host, shared: &Shared, k: Keys, options: u64, now: i64) {
    let s = host.status(now);
    w.member_add_array(Some(b"instances"));
    w.add_array_item_object();
    agent_status_id(w, k, 0, 0);
    status_to_json(w, &s, options & RFC3339 != 0);
    health_to_json(w, b"health", host, shared.health.host(host).as_deref());
    catalog::to_json(host.functions(), w);
    capas::to_json(w, b"capabilities", host);
    // host_dyncfg_to_json_v2()
    w.member_add_object("dyncfg");
    w.member_add_string("status", s.dyncfg.name());
    w.object_close();
    w.object_close();
    w.array_close();
}

/// The members the status alone decides, in C's order: `db`, `ingest` (`rrdhost_receiver_to_json()`) and `ml`. With
/// `rfc3339` the three times are UTC texts, `null` for 0; an age is always a number of seconds. A child (a host
/// with an attached receiver) adds to its `ingest`: `replication` while it replicates, `source` (the two ends of
/// its connection, each `[address]:port` with `:SSL` on TLS, and its capabilities by name) while it replicates or
/// is online, and `reason` when it is offline. No state of C's receiver paths is a child that is offline; here a
/// detach that ends between two reads of the status is one (D241 F5).
fn status_to_json(w: &mut JsonWriter, s: &Status, rfc3339: bool) {
    w.member_add_object("db");
    w.member_add_string("status", s.db.status.name());
    w.member_add_string("liveness", s.db.liveness.name());
    w.member_add_string("mode", s.db.mode.name());
    w.member_add_time_t_formatted("first_time", s.db.first_time_s, rfc3339);
    w.member_add_time_t_formatted("last_time", s.db.last_time_s, rfc3339);
    w.member_add_uint64("metrics", s.db.metrics);
    w.member_add_uint64("instances", s.db.instances);
    w.member_add_uint64("contexts", s.db.contexts);
    w.object_close();

    w.member_add_object("ingest");
    w.member_add_uint64("id", u64::from(s.ingest.id));
    w.member_add_int64("hops", i64::from(s.ingest.hops));
    w.member_add_string("type", s.ingest.kind.name());
    w.member_add_string("status", s.ingest.status.name());
    w.member_add_time_t_formatted("since", s.ingest.since_s, rfc3339);
    w.member_add_time_t("age", s.now - s.ingest.since_s);
    w.member_add_uint64("metrics", s.ingest.metrics);
    w.member_add_uint64("instances", s.ingest.instances);
    w.member_add_uint64("contexts", s.ingest.contexts);
    if s.ingest.kind == IngestType::Child {
        if s.ingest.status == IngestStatus::Offline {
            w.member_add_string("reason", Reason(s.ingest.reason).text());
        }
        if s.ingest.status == IngestStatus::Replicating {
            w.member_add_object("replication");
            w.member_add_boolean("in_progress", s.ingest.replication.in_progress);
            w.member_add_double("completion", s.ingest.replication.completion);
            w.member_add_uint64("instances", s.ingest.replication.instances);
            w.object_close();
        }
        if matches!(s.ingest.status, IngestStatus::Replicating | IngestStatus::Online) {
            let (ends, ssl) = (&s.ingest.peers, if s.ingest.tls { ":SSL" } else { "" });
            w.member_add_object("source");
            w.member_add_string("local", format!("[{}]:{}{ssl}", ends.local_ip, ends.local_port));
            w.member_add_string("remote", format!("[{}]:{}{ssl}", ends.peer_ip, ends.peer_port));
            caps::to_json_array(w, s.ingest.capabilities, Some(b"capabilities"));
            w.object_close();
        }
    }
    w.object_close();

    w.member_add_object("ml");
    w.member_add_string("status", s.ml.status.name());
    w.member_add_string("type", s.ml.kind.name());
    w.object_close();
}

#[cfg(test)]
mod tests {
    use netdata_agent_rrd::mode::DbMode;
    use netdata_agent_rrd::status::{
        Db, DbLiveness, DbStatus, DyncfgStatus, Ingest, Ml, MlStatus, MlType, Replication, SocketPeers, Stream,
        StreamStatus,
    };
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    /// Localhost of an agent that collects nothing, as the oracle answered `/api/v3/node_instances` in the round
    /// the harness records (`tests/parity/dash_norm_contexts_test.go`, `ni`): asked at 1791312192 by an agent that
    /// started at 1791312184.
    fn localhost() -> Status {
        Status {
            now: 1_791_312_192,
            db: Db {
                status: DbStatus::Initializing,
                liveness: DbLiveness::Stale,
                mode: DbMode::Dbengine,
                first_time_s: 0,
                last_time_s: 1_791_312_192,
                metrics: 0,
                instances: 0,
                contexts: 0,
            },
            ingest: Ingest {
                id: 0,
                hops: 0,
                kind: IngestType::Localhost,
                status: IngestStatus::Initializing,
                since_s: 1_791_312_184,
                reason: 0,
                metrics: 0,
                instances: 0,
                contexts: 0,
                replication: Replication::default(),
                capabilities: 0,
                peers: SocketPeers::default(),
                tls: false,
            },
            // an agent that streams nowhere
            stream: Stream {
                id: 0,
                hops: 1,
                status: StreamStatus::Disabled,
                since_s: 1_791_312_184,
                reason: 0,
                replication: Replication::default(),
                capabilities: 0,
                peers: SocketPeers::default(),
                tls: false,
                compression: false,
                sent_bytes: [0; 4],
            },
            ml: Ml {
                status: MlStatus::Disabled,
                kind: MlType::Disabled,
            },
            dyncfg: DyncfgStatus::Available,
        }
    }

    fn rendered(s: &Status, rfc3339: bool) -> String {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        status_to_json(&mut w, s, rfc3339);
        w.finalize();
        String::from_utf8(w.into_bytes()).unwrap()
    }

    /// The bytes of `db`, `ingest` and `ml` are the oracle's for that localhost.
    #[test]
    fn the_status_members_of_localhost_are_cs() {
        assert_eq!(
            rendered(&localhost(), false),
            concat!(
                r#"{"db":{"status":"initializing","liveness":"stale","mode":"dbengine","first_time":0,"#,
                r#""last_time":1791312192,"metrics":0,"instances":0,"contexts":0},"ingest":{"id":0,"hops":0,"#,
                r#""type":"localhost","status":"initializing","since":1791312184,"age":8,"metrics":0,"instances":0,"#,
                r#""contexts":0},"ml":{"status":"disabled","type":"disabled"}}"#
            )
        );
    }

    /// With `rfc3339` (`buffer_json_member_add_time_t_formatted()`) a time of 0 is `null` and the others are UTC
    /// texts; the age stays a number. The counts are each part's own: the database's are the tree's items, the
    /// ingestion's the collected ones.
    #[test]
    fn the_times_follow_rfc3339_and_the_counts_their_part() {
        let mut s = localhost();
        let text = rendered(&s, true);
        assert!(text.contains(r#""first_time":null,"last_time":"2026-10-06T18:43:12Z","#), "{text}");
        assert!(text.contains(r#""since":"2026-10-06T18:43:04Z","age":8,"#), "{text}");

        s.db = Db { first_time_s: 1_791_312_060, metrics: 7, instances: 2, contexts: 1, ..s.db };
        s.ingest = Ingest { hops: -1, metrics: 5, instances: 1, contexts: 1, ..s.ingest.clone() };
        let text = rendered(&s, true);
        assert!(text.contains(r#""first_time":"2026-10-06T18:41:00Z","#), "{text}");
        assert!(text.contains(r#""metrics":7,"instances":2,"contexts":1},"ingest":{"id":0,"hops":-1,"#), "{text}");
        assert!(text.contains(r#""age":8,"metrics":5,"instances":1,"contexts":1},"ml":"#), "{text}");
    }

    /// The child of that recorded round, as the oracle answered it: connected at 1791312190 from port 34050 to the
    /// parent's 38929, with four capabilities, its 7 metrics collected.
    fn child() -> Status {
        let mut s = localhost();
        s.db = Db {
            status: DbStatus::Queryable,
            liveness: DbLiveness::Live,
            mode: DbMode::Ram,
            first_time_s: 1_791_312_060,
            metrics: 7,
            instances: 2,
            contexts: 1,
            ..s.db
        };
        s.ingest = Ingest {
            id: 1,
            hops: 1,
            kind: IngestType::Child,
            status: IngestStatus::Online,
            since_s: 1_791_312_190,
            reason: 0x41,
            metrics: 7,
            instances: 2,
            contexts: 1,
            replication: Replication { in_progress: false, completion: 100.0, instances: 0 },
            capabilities: caps::VCAPS | caps::HLABELS | caps::CLABELS | caps::INTERPOLATED,
            peers: SocketPeers {
                local_ip: "127.0.0.1".into(),
                local_port: 38929,
                peer_ip: "127.0.0.1".into(),
                peer_port: 34050,
            },
            tls: false,
        };
        s.dyncfg = DyncfgStatus::Unavailable;
        s
    }

    /// The `ingest` object of a text made by [`rendered`].
    fn ingest_of(text: &str) -> &str {
        let (at, end) = (text.find(r#""ingest":"#).unwrap(), text.find(r#","ml":"#).unwrap());
        &text[at..end]
    }

    /// An online child's `db` and `ingest` are the oracle's bytes for that child: after its counts, `source` with
    /// the local end first, each end in brackets, and the capabilities by name; no `replication`, no `reason`.
    #[test]
    fn a_child_s_ingest_is_cs() {
        assert_eq!(
            rendered(&child(), false),
            concat!(
                r#"{"db":{"status":"online","liveness":"live","mode":"ram","first_time":1791312060,"#,
                r#""last_time":1791312192,"metrics":7,"instances":2,"contexts":1},"ingest":{"id":1,"hops":1,"#,
                r#""type":"child","status":"online","since":1791312190,"age":2,"metrics":7,"instances":2,"#,
                r#""contexts":1,"source":{"local":"[127.0.0.1]:38929","remote":"[127.0.0.1]:34050","#,
                r#""capabilities":["VCAPS","HLABELS","CLABELS","INTERPOLATED"]}},"#,
                r#""ml":{"status":"disabled","type":"disabled"}}"#
            )
        );
    }

    /// What a child's `ingest` adds follows its status (`rrdhost_receiver_to_json()`): replicating, `replication`
    /// then `source`; initializing, neither; offline, `reason` alone, the text of the stored code (a capabilities
    /// number reads `CONNECTED`). TLS marks both ends; an IPv6 address sits in the same brackets. A host that is no
    /// child prints none of them, whatever its status and its fields.
    #[test]
    fn a_child_s_additions_follow_its_status() {
        let tail = |s: &Status| {
            let text = rendered(s, false);
            let ingest = ingest_of(&text);
            ingest[ingest.find(r#""contexts":1"#).unwrap() + r#""contexts":1"#.len()..].to_owned()
        };
        let mut s = child();
        s.ingest.status = IngestStatus::Replicating;
        s.ingest.replication = Replication { in_progress: true, completion: 37.5, instances: 3 };
        s.ingest.tls = true;
        s.ingest.peers.peer_ip = "::1".into();
        assert_eq!(
            tail(&s),
            concat!(
                r#","replication":{"in_progress":true,"completion":37.5,"instances":3},"#,
                r#""source":{"local":"[127.0.0.1]:38929:SSL","remote":"[::1]:34050:SSL","#,
                r#""capabilities":["VCAPS","HLABELS","CLABELS","INTERPOLATED"]}}"#
            )
        );
        s.ingest.status = IngestStatus::Initializing;
        assert_eq!(tail(&s), "}");
        s.ingest.status = IngestStatus::Offline;
        assert_eq!(tail(&s), r#","reason":"CONNECTED"}"#);
        s.ingest.reason = -6;
        assert_eq!(tail(&s), format!(r#","reason":"{}"}}"#, Reason(-6).text()));
        assert_ne!(Reason(-6).text(), "UNKNOWN");
        // not a child: nothing is added in any status
        for kind in [IngestType::Archived, IngestType::Virtual, IngestType::Localhost] {
            for status in [IngestStatus::Offline, IngestStatus::Replicating, IngestStatus::Online] {
                s.ingest.kind = kind;
                s.ingest.status = status;
                assert_eq!(tail(&s), "}", "{kind:?} {status:?}");
            }
        }
    }
}
