//! A node's instances in the contexts v2 engine's NODE_INSTANCES mode: the `instances` array of
//! `rrdcontext_to_json_v2_rrdhost()`, with `rrdhost_receiver_to_json()` and `host_dyncfg_to_json_v2()`
//! (`src/database/contexts/api_v2_contexts.c`), over the host's status (`rrd::status`). Not written yet, with the
//! parts of the status they print: a child's `replication` and `source`, the host's `stream`, and the counts of a
//! host whose ML runs.

use netdata_agent_nrpc::catalog;
use netdata_agent_query::jsonwrap_v2::agent_status_id;
use netdata_agent_query::keys::Keys;
use netdata_agent_query::tables::contexts_options::RFC3339;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::status::Status;
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
/// `rfc3339` the three times are UTC texts, `null` for 0; an age is always a number of seconds.
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
        Db, DbLiveness, DbStatus, DyncfgStatus, Ingest, IngestStatus, IngestType, Ml, MlStatus, MlType,
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
                metrics: 0,
                instances: 0,
                contexts: 0,
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
        s.ingest = Ingest { hops: -1, metrics: 5, instances: 1, contexts: 1, ..s.ingest };
        let text = rendered(&s, true);
        assert!(text.contains(r#""first_time":"2026-10-06T18:41:00Z","#), "{text}");
        assert!(text.contains(r#""metrics":7,"instances":2,"contexts":1},"ingest":{"id":0,"hops":-1,"#), "{text}");
        assert!(text.contains(r#""age":8,"metrics":5,"instances":1,"contexts":1},"ml":"#), "{text}");
    }
}
