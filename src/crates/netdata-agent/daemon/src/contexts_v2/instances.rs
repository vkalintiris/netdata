//! A node's instances in the contexts v2 engine's NODE_INSTANCES mode: the `instances` array of
//! `rrdcontext_to_json_v2_rrdhost()`, with `rrdhost_receiver_to_json()`, `rrdhost_sender_to_json()` and
//! `host_dyncfg_to_json_v2()` (`src/database/contexts/api_v2_contexts.c`), over the host's status (`rrd::status`).
//! Not written yet, with the part of the status it prints: the counts of a host whose ML runs.

use netdata_agent_nrpc::catalog;
use netdata_agent_pluginsd_proto::caps;
use netdata_agent_query::jsonwrap_v2::agent_status_id;
use netdata_agent_query::keys::Keys;
use netdata_agent_query::tables::contexts_options::RFC3339;
use netdata_agent_rrd::clock::now_realtime_ut;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::status::{IngestStatus, IngestType, ParentStatus, SocketPeers, Status, StreamStatus};
use netdata_agent_rrd::upstream::Traffic;
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
    let localhost = shared.hosts.localhost();
    status_to_json(w, &s, options & RFC3339 != 0, now_realtime_ut(), |w| {
        netdata_agent_ingest::stream_path::to_json(w, host, localhost, b"streaming_path", false, None);
    });
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

/// The members the status decides, in C's order: `db`, `ingest` (`rrdhost_receiver_to_json()`), `stream`
/// (`rrdhost_sender_to_json()`, none for a host without a sender) and `ml`. With `rfc3339` the four times are UTC
/// texts, `null` for 0; an age is always a number of seconds. A child (a host with an attached receiver) adds to
/// its `ingest`: `replication` while it replicates, `source` (the two ends of its connection, each `[address]:port`
/// with `:SSL` on TLS, and its capabilities by name) while it replicates or is online, and `reason` when it is
/// offline: a cleanup that gives up waiting for its receiver to stop orphans the host with the receiver still
/// attached, in C (`database/rrdhost.c:976`, `:1026`) as here, and here a detach that ends between two reads of the
/// status is one too (D241 F5). `now_ut` is the parents' clock; `path` writes the host's `streaming_path`.
fn status_to_json(w: &mut JsonWriter, s: &Status, rfc3339: bool, now_ut: u64, path: impl FnOnce(&mut JsonWriter)) {
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
            w.member_add_object("source");
            ends_to_json(w, &s.ingest.peers, s.ingest.tls);
            caps::to_json_array(w, s.ingest.capabilities, Some(b"capabilities"));
            w.object_close();
        }
    }
    w.object_close();

    stream_to_json(w, s, rfc3339, now_ut, path);

    w.member_add_object("ml");
    w.member_add_string("status", s.ml.status.name());
    w.member_add_string("type", s.ml.kind.name());
    w.object_close();
}

/// A connection's two ends, each `[address]:port`, with `:SSL` on TLS.
fn ends_to_json(w: &mut JsonWriter, ends: &SocketPeers, tls: bool) {
    let ssl = if tls { ":SSL" } else { "" };
    w.member_add_string("local", format!("[{}]:{}{ssl}", ends.local_ip, ends.local_port));
    w.member_add_string("remote", format!("[{}]:{}{ssl}", ends.peer_ip, ends.peer_port));
}

/// `rrdhost_sender_to_json()`: where the host streams to. `reason` only while offline; the destination's capabilities
/// only while connected (none otherwise), its ends the dispatched connection's (`not connected` without one), its
/// bytes the last connection's.
fn stream_to_json(w: &mut JsonWriter, s: &Status, rfc3339: bool, now_ut: u64, path: impl FnOnce(&mut JsonWriter)) {
    let st = &s.stream;
    if st.status == StreamStatus::Disabled {
        return;
    }
    w.member_add_object("stream");
    w.member_add_uint64("id", u64::from(st.id));
    // C writes the int16_t through its unsigned writer
    w.member_add_uint64("hops", st.hops as u64);
    w.member_add_string("status", st.status.name());
    w.member_add_time_t_formatted("since", st.since_s, rfc3339);
    w.member_add_time_t("age", s.now - st.since_s);
    if st.status == StreamStatus::Offline {
        w.member_add_string("reason", Reason(st.reason).text());
    }
    w.member_add_object("replication");
    w.member_add_boolean("in_progress", st.replication.in_progress);
    w.member_add_double("completion", st.replication.completion);
    w.member_add_uint64("instances", st.replication.instances);
    w.object_close();
    w.member_add_object("destination");
    ends_to_json(w, &st.peers, st.tls);
    caps::to_json_array(w, st.capabilities, Some(b"capabilities"));
    w.member_add_object("traffic");
    w.member_add_boolean("compression", st.compression);
    let by_type = [
        ("data", Traffic::Data),
        ("metadata", Traffic::Metadata),
        ("functions", Traffic::Functions),
        ("replication", Traffic::Replication),
    ];
    for (key, traffic) in by_type {
        w.member_add_uint64(key, st.sent_bytes[traffic as usize] as u64);
    }
    w.object_close();
    w.member_add_array(Some(b"parents"));
    for d in &st.parents {
        parent_to_json(w, d, now_ut);
    }
    w.array_close();
    path(w);
    w.object_close();
    w.object_close();
}

/// One parent of `rrdhost_stream_parents_to_json()` (`streaming/stream-parents.c:174-225`) at the clock `now_ut`:
/// the times in local time with two fraction digits, the ages as duration texts; a banned parent says why and no
/// more, any other its last handshake, its next check while postponed, and its place in the last pass.
fn parent_to_json(w: &mut JsonWriter, d: &ParentStatus, now_ut: u64) {
    w.add_array_item_object();
    w.member_add_uint64("attempts", u64::from(d.attempts.wrapping_add(1)));
    if d.ssl {
        w.member_add_string("destination", format!("{}:SSL", d.destination));
    } else {
        w.member_add_string("destination", &d.destination);
    }
    w.member_add_string("since", netdata_agent_log::rfc3339_local(d.since_ut, 2));
    w.member_add_duration_ut("age", if d.since_ut < now_ut { (now_ut - d.since_ut) as i64 } else { 0 });
    if !d.banned_for_this_session && !d.banned_permanently && !d.banned_temporarily_erroneous {
        w.member_add_string("last_handshake", Reason(d.reason).text());
        if d.postpone_until_ut > now_ut {
            w.member_add_string("next_check", netdata_agent_log::rfc3339_local(d.postpone_until_ut, 2));
            w.member_add_duration_ut("next_in", (d.postpone_until_ut - now_ut) as i64);
        }
        if d.batch != 0 {
            w.member_add_uint64("batch", d.batch as u64);
            w.member_add_uint64("order", d.order as u64);
            w.member_add_boolean("random", d.random);
        }
        w.member_add_boolean("info", d.info);
        w.member_add_boolean("skipped", d.skipped);
    } else if d.banned_permanently {
        w.member_add_string("ban", "it is the localhost");
    } else if d.banned_for_this_session {
        w.member_add_string("ban", "it is our parent");
    } else {
        w.member_add_string("ban", "it is erroneous");
    }
    w.object_close();
}

#[cfg(test)]
mod tests {
    use netdata_agent_rrd::mode::DbMode;
    use netdata_agent_rrd::status::{
        Db, DbLiveness, DbStatus, DyncfgStatus, Ingest, Ml, MlStatus, MlType, Replication, Stream,
    };
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    /// The parents' clock in these units: a second after the status's.
    const NOW_UT: u64 = 1_791_312_193_000_000;

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
                parents: Vec::new(),
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
        status_to_json(&mut w, s, rfc3339, NOW_UT, |w| {
            w.member_add_array(Some(b"streaming_path"));
            w.array_close();
        });
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
        // a child that returned and has not collected yet replicates with no chart in progress: still printed
        s.ingest.replication = Replication { in_progress: false, completion: 37.5, instances: 0 };
        assert!(tail(&s).starts_with(r#","replication":{"in_progress":false,"completion":37.5,"instances":0},"source":"#));
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

    /// A host that streams (`rrdhost_sender_to_json()`, `api_v2_contexts.c:382-433`): `stream` between `ingest` and
    /// `ml`; the destination's two ends and capabilities, the bytes by traffic type, each parent at the parents' clock
    /// (a usable one with its next check and its place in the last pass, a banned one with the ban alone), then the
    /// path. Online it says no reason, offline it says why; `hops` goes through C's unsigned writer; with rfc3339 the
    /// stream's time is a UTC text. A parent placed in a pass prints its place whether or not it was drawn at random;
    /// a banned one the text of its ban, the localhost's first, then a parent's, then an erroneous one's.
    #[test]
    fn a_streaming_host_s_stream_is_cs() {
        let usable = ParentStatus {
            destination: "parent-a:19999".into(),
            ssl: true,
            reason: Reason::SP_CONNECTED.0,
            attempts: 1,
            since_ut: NOW_UT - 90_000_000,
            postpone_until_ut: NOW_UT + 30_000_000,
            batch: 1,
            order: 2,
            random: true,
            info: true,
            ..ParentStatus::default()
        };
        let banned = ParentStatus {
            destination: "parent-b".into(),
            banned_permanently: true,
            since_ut: NOW_UT + 5,
            ..ParentStatus::default()
        };
        let mut s = localhost();
        s.stream = Stream {
            id: 3,
            hops: 2,
            status: StreamStatus::Replicating,
            since_s: 1_791_312_150,
            reason: Reason::SP_CONNECTED.0,
            replication: Replication { in_progress: true, completion: 37.5, instances: 2 },
            capabilities: caps::VCAPS | caps::HLABELS,
            peers: SocketPeers {
                local_ip: "10.0.0.1".into(),
                local_port: 40000,
                peer_ip: "10.0.0.2".into(),
                peer_port: 19999,
            },
            tls: true,
            compression: true,
            // by STREAM_TRAFFIC_TYPE: replication, functions, metadata, data
            sent_bytes: [40, 30, 20, 10],
            parents: vec![usable, banned],
        };
        // the time and duration texts by their own writers, which their crates' units pin
        let local = |ut| netdata_agent_log::rfc3339_local(ut, 2);
        let duration = |us| netdata_agent_text::duration::duration_to_string(us, "us", true).unwrap();
        let want = format!(
            concat!(
                r#"}},"stream":{{"id":3,"hops":2,"status":"replicating","since":1791312150,"age":42,"#,
                r#""replication":{{"in_progress":true,"completion":37.5,"instances":2}},"#,
                r#""destination":{{"local":"[10.0.0.1]:40000:SSL","remote":"[10.0.0.2]:19999:SSL","#,
                r#""capabilities":["VCAPS","HLABELS"],"#,
                r#""traffic":{{"compression":true,"data":10,"metadata":20,"functions":30,"replication":40}},"#,
                r#""parents":[{{"attempts":2,"destination":"parent-a:19999:SSL","since":"{}","age":"{}","#,
                r#""last_handshake":"{}","next_check":"{}","next_in":"{}","batch":1,"order":2,"random":true,"#,
                r#""info":true,"skipped":false}},"#,
                r#"{{"attempts":1,"destination":"parent-b","since":"{}","age":"{}","ban":"it is the localhost"}}],"#,
                r#""streaming_path":[]}}}},"ml":"#
            ),
            local(NOW_UT - 90_000_000),
            duration(90_000_000),
            Reason::SP_CONNECTED.text(),
            local(NOW_UT + 30_000_000),
            duration(30_000_000),
            local(NOW_UT + 5),
            duration(0),
        );
        let text = rendered(&s, false);
        assert!(text.contains(&want), "{text}\n{want}");
        let text = rendered(&s, true);
        assert!(text.contains(r#""status":"replicating","since":"2026-10-06T18:42:30Z","age":42,"#), "{text}");

        s.stream.status = StreamStatus::Online;
        s.stream.replication = Replication { in_progress: false, completion: 37.5, instances: 0 };
        let text = rendered(&s, false);
        let want = r#""status":"online","since":1791312150,"age":42,"replication":{"in_progress":false,"#;
        assert!(text.contains(want), "{text}");

        s.stream.parents[0].random = false;
        assert!(rendered(&s, false).contains(r#""batch":1,"order":2,"random":false,"info":true,"#));
        s.stream.parents[1].banned_for_this_session = true;
        assert!(rendered(&s, false).contains(r#""ban":"it is the localhost"}"#));
        s.stream.parents[1].banned_permanently = false;
        assert!(rendered(&s, false).contains(r#""ban":"it is our parent"}"#));
        s.stream.parents[1].banned_for_this_session = false;
        s.stream.parents[1].banned_temporarily_erroneous = true;
        assert!(rendered(&s, false).contains(r#""ban":"it is erroneous"}"#));

        s.stream.status = StreamStatus::Offline;
        s.stream.hops = -1;
        s.stream.reason = Reason::SP_CONNECTION_REFUSED.0;
        let text = rendered(&s, false);
        let want = format!(
            r#""hops":18446744073709551615,"status":"offline","since":1791312150,"age":42,"reason":"{}","replication":"#,
            Reason::SP_CONNECTION_REFUSED.text()
        );
        assert!(text.contains(&want), "{text}");
    }
}
