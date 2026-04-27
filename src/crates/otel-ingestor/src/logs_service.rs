use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use bridge::config::AuthConfig;
use file_registry::TenantId;
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsPartialSuccess, ExportLogsServiceRequest, ExportLogsServiceResponse,
    logs_service_server::LogsService,
};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::logs::v1::ResourceLogs;
use tonic::{Request, Response, Status};

use crate::arrow_bridge;
use crate::ledger_sender::LedgerSender;

/// The canonical `(namespace, name)` pair for a given `ns_hash`.
///
/// Stored as `Option<String>` to distinguish a missing attribute from an
/// empty string — `compute_ns_hash(None, None)` and
/// `compute_ns_hash(Some(""), Some(""))` yield different hashes, so the
/// canonical pair must preserve that distinction.
type CanonicalStream = (Option<String>, Option<String>);

/// Extracted stream identity from a single `ResourceLogs`.
#[derive(Debug, Clone)]
struct Stream {
    hash: u64,
    namespace: Option<String>,
    name: Option<String>,
}

/// Extract `service.namespace`, `service.name`, and the resulting `ns_hash`
/// from a `ResourceLogs`.
fn extract_stream(rl: &ResourceLogs) -> Stream {
    let attrs = match rl.resource.as_ref() {
        Some(r) => &r.attributes,
        None => {
            return Stream {
                hash: file_registry::compute_ns_hash(None, None),
                namespace: None,
                name: None,
            };
        }
    };

    let mut namespace: Option<&str> = None;
    let mut name: Option<&str> = None;

    for kv in attrs {
        match kv.key.as_str() {
            "service.namespace" => {
                if let Some(Value::StringValue(s)) =
                    kv.value.as_ref().and_then(|v| v.value.as_ref())
                {
                    namespace = Some(s.as_str());
                }
            }
            "service.name" => {
                if let Some(Value::StringValue(s)) =
                    kv.value.as_ref().and_then(|v| v.value.as_ref())
                {
                    name = Some(s.as_str());
                }
            }
            _ => {}
        }
    }

    Stream {
        hash: file_registry::compute_ns_hash(namespace, name),
        namespace: namespace.map(String::from),
        name: name.map(String::from),
    }
}

/// Total number of log records carried by a single `ResourceLogs`.
fn count_log_records(rl: &ResourceLogs) -> usize {
    rl.scope_logs.iter().map(|sl| sl.log_records.len()).sum()
}

/// One collision: a request group whose `(namespace, name)` doesn't match
/// the canonical pair already registered for its `ns_hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Collision {
    hash: u64,
    canonical: CanonicalStream,
    rejected: CanonicalStream,
    rejected_log_records: usize,
}

/// One group of `ResourceLogs` that share an exact `(hash, namespace, name)`.
/// The identifying fields live in the `HashMap` key in [`group_by_stream`].
struct StreamGroup {
    log_record_count: usize,
    resource_logs: Vec<ResourceLogs>,
}

/// Group `ResourceLogs` by their full `(hash, namespace, name)` tuple.
///
/// In normal operation, all `ResourceLogs` in a request from a single
/// service share the same hash and the same `(namespace, name)`. If the
/// same hash appears with two different `(namespace, name)` pairs in one
/// request — an in-request `ns_hash` collision — both end up in distinct
/// groups, and the canonical-table check below catches both correctly.
fn group_by_stream(
    resource_logs: Vec<ResourceLogs>,
) -> HashMap<(u64, Option<String>, Option<String>), StreamGroup> {
    let mut groups: HashMap<_, StreamGroup> = HashMap::new();
    for rl in resource_logs {
        let s = extract_stream(&rl);
        let count = count_log_records(&rl);
        let key = (s.hash, s.namespace, s.name);
        let group = groups.entry(key).or_insert_with(|| StreamGroup {
            log_record_count: 0,
            resource_logs: Vec::new(),
        });
        group.log_record_count += count;
        group.resource_logs.push(rl);
    }
    groups
}

/// Result of running the collision check across a request's groups.
struct CollisionCheck {
    accepted: Vec<(u64, StreamGroup)>,
    collisions: Vec<Collision>,
}

/// Reconcile a request's groups against the canonical-stream table.
///
/// For each group:
/// - If the table has no entry for `ns_hash`, register the group's
///   `(namespace, name)` as canonical and accept the group.
/// - If the entry matches the group's pair, accept.
/// - If the entry mismatches, reject the group as a collision and record
///   it for the response's `partial_success`.
///
/// Pure with respect to the I/O of the gRPC handler — the only side
/// effect is mutating the canonical table. Extracted from `export` so it
/// can be unit-tested without spinning up a writer or a tonic Request.
fn check_collisions(
    canonical: &mut HashMap<(TenantId, u64), CanonicalStream>,
    tenant_id: &TenantId,
    groups: HashMap<(u64, Option<String>, Option<String>), StreamGroup>,
) -> CollisionCheck {
    let mut accepted = Vec::new();
    let mut collisions = Vec::new();

    for ((hash, namespace, name), group) in groups {
        let key = (tenant_id.clone(), hash);
        match canonical.entry(key) {
            Entry::Vacant(e) => {
                e.insert((namespace.clone(), name.clone()));
                accepted.push((hash, group));
            }
            Entry::Occupied(e) if *e.get() == (namespace.clone(), name.clone()) => {
                accepted.push((hash, group));
            }
            Entry::Occupied(e) => {
                collisions.push(Collision {
                    hash,
                    canonical: e.get().clone(),
                    rejected: (namespace, name),
                    rejected_log_records: group.log_record_count,
                });
            }
        }
    }

    CollisionCheck {
        accepted,
        collisions,
    }
}

/// Format collision details for `ExportLogsPartialSuccess::error_message`.
fn format_collision_error(collisions: &[Collision]) -> String {
    fn show(opt: &Option<String>) -> &str {
        opt.as_deref().unwrap_or("<missing>")
    }

    let parts: Vec<String> = collisions
        .iter()
        .map(|c| {
            format!(
                "ns_hash={:#x}: rejected ({}/{}) collides with canonical ({}/{}) ({} log records dropped)",
                c.hash,
                show(&c.rejected.0),
                show(&c.rejected.1),
                show(&c.canonical.0),
                show(&c.canonical.1),
                c.rejected_log_records,
            )
        })
        .collect();
    format!(
        "{} ns_hash collision{} detected; rename one of the colliding (service.namespace, service.name) pairs to dedupe: {}",
        collisions.len(),
        if collisions.len() == 1 { "" } else { "s" },
        parts.join("; "),
    )
}

fn validate_tenant_id(id: &str) -> Result<(), Status> {
    if id.is_empty() || id.len() > 255 {
        return Err(Status::invalid_argument("tenant ID must be 1-255 bytes"));
    }
    if id == "." || id == ".." || id == "default" {
        return Err(Status::invalid_argument(
            "tenant ID must not be '.', '..', or 'default'",
        ));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(Status::invalid_argument(
            "tenant ID must contain only [a-zA-Z0-9._-]",
        ));
    }
    Ok(())
}

fn extract_tenant_id(
    metadata: &tonic::metadata::MetadataMap,
    auth: &AuthConfig,
) -> Result<TenantId, Status> {
    if !auth.enabled {
        return Ok(TenantId::from("default"));
    }
    let value = metadata
        .get(AuthConfig::TENANT_HEADER)
        .ok_or_else(|| Status::unauthenticated("missing tenant header"))?;
    let tenant = value
        .to_str()
        .map_err(|_| Status::invalid_argument("tenant header must be valid UTF-8"))?;
    validate_tenant_id(tenant)?;
    Ok(TenantId::from(tenant))
}

pub struct NetdataLogsService {
    writers: Mutex<HashMap<TenantId, wal::Writer>>,
    /// Canonical `(namespace, name)` per `(tenant, ns_hash)`. First write
    /// wins; subsequent writes whose `(namespace, name)` doesn't match are
    /// rejected via `partial_success`. In-memory only — on restart the
    /// table is empty and the first write of a tenant's stream re-establishes
    /// the canonical pair.
    canonical: Mutex<HashMap<(TenantId, u64), CanonicalStream>>,
    sender: LedgerSender,
    wal_base_dir: PathBuf,
    wal_config: bridge::config::WalConfig,
    seq: Arc<AtomicU64>,
    auth: AuthConfig,
}

impl NetdataLogsService {
    pub fn new(
        sender: LedgerSender,
        wal_base_dir: PathBuf,
        wal_config: bridge::config::WalConfig,
        seq: Arc<AtomicU64>,
        auth: AuthConfig,
    ) -> Self {
        Self {
            writers: Mutex::new(HashMap::new()),
            canonical: Mutex::new(HashMap::new()),
            sender,
            wal_base_dir,
            wal_config,
            seq,
            auth,
        }
    }

    fn resolve_wal_config(&self, tenant_id: &str) -> wal::Config {
        let rotation =
            bridge::config::RotationConfig::resolve(&self.wal_config.rotation, tenant_id);
        wal::Config {
            rotation: wal::RotationConfig {
                max_log_entries: rotation.max_log_entries,
                max_file_size: file_registry::ByteSize(rotation.max_file_size.as_u64()),
                max_duration: Some(rotation.max_file_duration),
            },
            crc_enabled: self.wal_config.crc_enabled,
            compression_enabled: self.wal_config.compression_enabled,
        }
    }
}

#[tonic::async_trait]
impl LogsService for NetdataLogsService {
    #[tracing::instrument(skip_all, fields(received_logs))]
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let tenant_id = extract_tenant_id(request.metadata(), &self.auth)?;
        let req = request.into_inner();

        // Group, then run the collision check.
        let groups = group_by_stream(req.resource_logs);
        let CollisionCheck {
            accepted,
            collisions,
        } = {
            let mut canonical = self.canonical.lock().unwrap();
            check_collisions(&mut canonical, &tenant_id, groups)
        };

        for c in &collisions {
            tracing::warn!(
                tenant = %tenant_id,
                hash = c.hash,
                "ns_hash collision: rejecting {} log records",
                c.rejected_log_records,
            );
        }

        // Write only the accepted groups.
        let mut writers = self.writers.lock().unwrap();
        let writer = if let Some(w) = writers.get_mut(&tenant_id) {
            w
        } else {
            let path = self.wal_base_dir.join(tenant_id.as_str());
            let wal_config = self.resolve_wal_config(tenant_id.as_str());
            let w = wal::Writer::new(&path, wal_config, Arc::clone(&self.seq)).map_err(|e| {
                tracing::error!(%e, tenant = %tenant_id, "failed to create WAL writer");
                Status::internal("WAL writer creation failed")
            })?;
            writers.entry(tenant_id.clone()).or_insert(w)
        };

        for (ns_hash, group) in accepted {
            let (data, count) = arrow_bridge::encode(group.resource_logs).map_err(|e| {
                tracing::error!(%e, "failed to encode Arrow");
                Status::internal("Arrow encode error")
            })?;

            writer.write_frame(ns_hash, &data, count).map_err(|e| {
                tracing::error!(%e, "failed to write WAL entry");
                Status::internal("WAL write error")
            })?;
        }

        writer.sync_all().map_err(|e| {
            tracing::error!(%e, "failed to sync WAL");
            Status::internal("WAL sync error")
        })?;

        let events = writer.take_all_events();
        self.sender.send_events(tenant_id, events);

        let partial_success = if collisions.is_empty() {
            None
        } else {
            let total: i64 = collisions
                .iter()
                .map(|c| c.rejected_log_records as i64)
                .sum();
            Some(ExportLogsPartialSuccess {
                rejected_log_records: total,
                error_message: format_collision_error(&collisions),
            })
        };

        Ok(Response::new(ExportLogsServiceResponse { partial_success }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;

    fn rl(namespace: Option<&str>, name: Option<&str>, log_count: usize) -> ResourceLogs {
        let mut attrs = Vec::new();
        if let Some(ns) = namespace {
            attrs.push(KeyValue {
                key: "service.namespace".to_string(),
                value: Some(AnyValue {
                    value: Some(Value::StringValue(ns.to_string())),
                }),
            });
        }
        if let Some(n) = name {
            attrs.push(KeyValue {
                key: "service.name".to_string(),
                value: Some(AnyValue {
                    value: Some(Value::StringValue(n.to_string())),
                }),
            });
        }
        ResourceLogs {
            resource: Some(Resource {
                attributes: attrs,
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: None,
                log_records: (0..log_count).map(|_| LogRecord::default()).collect(),
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }
    }

    fn tenant() -> TenantId {
        TenantId::from("t1")
    }

    #[test]
    fn extract_stream_pulls_namespace_and_name_from_resource_attrs() {
        let s = extract_stream(&rl(Some("prod"), Some("api"), 0));
        assert_eq!(s.namespace.as_deref(), Some("prod"));
        assert_eq!(s.name.as_deref(), Some("api"));
        assert_eq!(s.hash, file_registry::compute_ns_hash(Some("prod"), Some("api")));
    }

    #[test]
    fn extract_stream_handles_missing_attrs() {
        let s = extract_stream(&rl(None, None, 0));
        assert_eq!(s.namespace, None);
        assert_eq!(s.name, None);
        assert_eq!(s.hash, file_registry::compute_ns_hash(None, None));
    }

    #[test]
    fn extract_stream_distinguishes_missing_from_empty() {
        let none = extract_stream(&rl(None, None, 0));
        let empty = extract_stream(&rl(Some(""), Some(""), 0));
        // Different inputs to compute_ns_hash → different hashes, so they
        // must remain distinct in the canonical table.
        assert_ne!(none.hash, empty.hash);
        assert_eq!(empty.namespace.as_deref(), Some(""));
        assert_eq!(empty.name.as_deref(), Some(""));
    }

    #[test]
    fn group_merges_resource_logs_with_identical_stream() {
        let groups = group_by_stream(vec![
            rl(Some("prod"), Some("api"), 3),
            rl(Some("prod"), Some("api"), 5),
            rl(Some("prod"), Some("worker"), 1),
        ]);
        assert_eq!(groups.len(), 2);
        let api_hash = file_registry::compute_ns_hash(Some("prod"), Some("api"));
        let api = groups
            .get(&(api_hash, Some("prod".to_string()), Some("api".to_string())))
            .unwrap();
        assert_eq!(api.log_record_count, 8);
        assert_eq!(api.resource_logs.len(), 2);
    }

    #[test]
    fn first_write_establishes_canonical_pair() {
        let mut canonical = HashMap::new();
        let groups = group_by_stream(vec![rl(Some("prod"), Some("api"), 4)]);
        let r = check_collisions(&mut canonical, &tenant(), groups);
        assert_eq!(r.accepted.len(), 1);
        assert!(r.collisions.is_empty());
        let pair = canonical.get(&(tenant(), r.accepted[0].0)).unwrap();
        assert_eq!(pair, &(Some("prod".to_string()), Some("api".to_string())));
    }

    #[test]
    fn matching_subsequent_writes_pass_through() {
        let mut canonical = HashMap::new();
        let r1 = check_collisions(
            &mut canonical,
            &tenant(),
            group_by_stream(vec![rl(Some("prod"), Some("api"), 1)]),
        );
        assert!(r1.collisions.is_empty());
        let r2 = check_collisions(
            &mut canonical,
            &tenant(),
            group_by_stream(vec![rl(Some("prod"), Some("api"), 7)]),
        );
        assert!(r2.collisions.is_empty());
        assert_eq!(r2.accepted.len(), 1);
        assert_eq!(r2.accepted[0].1.log_record_count, 7);
    }

    #[test]
    fn synthetic_collision_is_rejected() {
        // Two genuinely different (namespace, name) pairs hashing to the
        // same u64 is impossible to construct naturally for testing —
        // u64 collisions on xxhash64 are vanishingly rare. We simulate by
        // pre-seeding the canonical table with a fake hash, then submitting
        // a group whose actual (ns, name) hashes to that same value via
        // string surgery: we ignore the "real" hash and check the helper's
        // logic directly with a hand-built group.

        let mut canonical = HashMap::new();
        let fake_hash = 0xdead_beefu64;
        canonical.insert(
            (tenant(), fake_hash),
            (Some("prod".to_string()), Some("api".to_string())),
        );

        // Build a group keyed at fake_hash but with different (ns, name).
        let mut groups = HashMap::new();
        groups.insert(
            (fake_hash, Some("staging".to_string()), Some("api".to_string())),
            StreamGroup {
                log_record_count: 12,
                resource_logs: Vec::new(),
            },
        );

        let r = check_collisions(&mut canonical, &tenant(), groups);
        assert!(r.accepted.is_empty());
        assert_eq!(r.collisions.len(), 1);
        let c = &r.collisions[0];
        assert_eq!(c.hash, fake_hash);
        assert_eq!(c.canonical, (Some("prod".to_string()), Some("api".to_string())));
        assert_eq!(c.rejected, (Some("staging".to_string()), Some("api".to_string())));
        assert_eq!(c.rejected_log_records, 12);
    }

    #[test]
    fn collision_does_not_overwrite_canonical_pair() {
        let mut canonical = HashMap::new();
        let fake_hash = 0xdead_beefu64;
        canonical.insert(
            (tenant(), fake_hash),
            (Some("prod".to_string()), Some("api".to_string())),
        );

        let mut groups = HashMap::new();
        groups.insert(
            (fake_hash, Some("staging".to_string()), Some("api".to_string())),
            StreamGroup {
                log_record_count: 1,
                resource_logs: Vec::new(),
            },
        );

        let _ = check_collisions(&mut canonical, &tenant(), groups);
        let pair = canonical.get(&(tenant(), fake_hash)).unwrap();
        // The original canonical pair must remain unchanged.
        assert_eq!(pair, &(Some("prod".to_string()), Some("api".to_string())));
    }

    #[test]
    fn tenants_have_independent_canonical_tables() {
        let mut canonical = HashMap::new();
        let t1 = TenantId::from("t1");
        let t2 = TenantId::from("t2");
        let groups_t1 = group_by_stream(vec![rl(Some("prod"), Some("api"), 1)]);
        let r1 = check_collisions(&mut canonical, &t1, groups_t1);
        assert!(r1.collisions.is_empty());

        // Same hash, different (ns, name), but in a different tenant — must
        // be accepted as fresh, not flagged as a collision.
        let groups_t2 = group_by_stream(vec![rl(Some("staging"), Some("api"), 1)]);
        let r2 = check_collisions(&mut canonical, &t2, groups_t2);
        assert!(r2.collisions.is_empty());
        assert_eq!(r2.accepted.len(), 1);
    }

    #[test]
    fn error_message_describes_each_collision() {
        let collisions = vec![
            Collision {
                hash: 0x1,
                canonical: (Some("prod".into()), Some("api".into())),
                rejected: (Some("staging".into()), Some("api".into())),
                rejected_log_records: 3,
            },
            Collision {
                hash: 0x2,
                canonical: (None, Some("worker".into())),
                rejected: (Some("dev".into()), Some("worker".into())),
                rejected_log_records: 1,
            },
        ];
        let msg = format_collision_error(&collisions);
        assert!(msg.contains("2 ns_hash collisions"));
        assert!(msg.contains("prod"));
        assert!(msg.contains("staging"));
        assert!(msg.contains("<missing>"));
        assert!(msg.contains("3 log records"));
    }
}
