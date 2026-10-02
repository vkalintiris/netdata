//! `nrpc-catalog.c`'s renderers for users: a host's functions as `/api/v1/functions` and `/api/v1/info` show them,
//! and the version-keyed entries contexts v2 merges across hosts.

use std::sync::Arc;

use netdata_agent_text::json::JsonWriter;

use crate::{Filter, Method, Registry, access};

/// `NRPC_VERSION_SEPARATOR`.
const VERSION_SEPARATOR: u8 = b'|';

/// `nrpc_catalog_host2json()`: a `"functions"` member holding one object per method users see, in registration order;
/// omitted, not empty, when the host has no registry.
pub fn to_json(registry: &Registry, w: &mut JsonWriter) {
    if !registry.exists() {
        return;
    }
    w.member_add_object("functions");
    for (name, m) in registry.visible(Filter::User).0 {
        w.member_add_object(&name);
        w.member_add_string("help", &m.help);
        w.member_add_int64("timeout", i64::from(m.timeout_s));
        w.member_add_uint64("version", u64::from(m.version));
        w.member_add_array(Some(b"options"));
        // every method is host-wide: C keeps the retired per-chart scope's word for its consumers
        w.add_array_item_string("GLOBAL");
        w.array_close();
        w.member_add_string("tags", &m.tags);
        w.member_add_array(Some(b"access"));
        for name in access::names(m.access) {
            w.add_array_item_string(name);
        }
        w.array_close();
        // C's int printed as uint64
        w.member_add_uint64("priority", m.priority as u64);
        w.object_close();
    }
    w.object_close();
}

/// `nrpc_catalog_host_to_dict()`'s entries: each method users see under its `"<version>|<name>"` key, in registration
/// order, for the caller to merge (its first entry for a key wins); none when the host has no registry.
pub fn to_dict(registry: &Registry) -> Vec<(Vec<u8>, Arc<Method>)> {
    if !registry.exists() {
        return Vec::new();
    }
    registry
        .visible(Filter::User)
        .0
        .into_iter()
        .map(|(name, m)| {
            let mut key = m.version.to_string().into_bytes();
            key.push(VERSION_SEPARATOR);
            key.extend_from_slice(&name);
            (key, m)
        })
        .collect()
}
