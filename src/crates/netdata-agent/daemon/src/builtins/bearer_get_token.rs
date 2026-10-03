//! `bearer_get_token` (`src/web/api/functions/function-bearer_get_token.c`, then `src/web/api/v2/api_v2_bearer.c`
//! `bearer_get_token_json_response()`): a token for Cloud's direct access to this agent, created on a call through a
//! parent with the role and access the call states (D176.4).

use netdata_agent_ingest::jsonc::{self, Presence::Required};
use netdata_agent_nrpc::access;
use netdata_agent_nrpc::reply::{Payload, Reply};
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::print::print_uuid_lower;
use serde_json::{Map, Value};

/// `struct bearer_token_request`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Request {
    pub(super) claim_id: [u8; 16],
    pub(super) machine_guid: [u8; 16],
    pub(super) node_id: [u8; 16],
    pub(super) role: u8,
    pub(super) access: u32,
    pub(super) account: [u8; 16],
    /// `None` for C's NULL `STRING`: a null or empty text.
    pub(super) client_name: Option<String>,
}

/// `bearer_parse_json_payload()`: the members in C's order, all required.
pub(super) fn parse(obj: &Map<String, Value>, e: &mut String) -> Option<Request> {
    let claim_id = jsonc::uuid(obj, "", "claim_id", Required, e)?;
    let machine_guid = jsonc::uuid(obj, "", "machine_guid", Required, e)?;
    let node_id = jsonc::uuid(obj, "", "node_id", Required, e)?;
    let role = jsonc::enum_text(obj, "", "user_role", Required, e)?.map_or(access::role::NONE, access::role::to_id);
    let access = jsonc::bitmap(obj, "", "access", access::id_one, Required, e)?;
    let account = jsonc::uuid(obj, "", "cloud_account_id", Required, e)?;
    let client_name = jsonc::txt(obj, "", "client_name", Required, e)?;
    Some(Request { claim_id, machine_guid, node_id, role, access, account, client_name })
}

/// `claim_id_matches_any()`: a non-zero id that is the agent's own (none: the Rust agent is never claimed, D61.3),
/// localhost's parent's (from `NODE_ID`) or its origin's.
fn claim_id_matches_any(localhost: &Host, claim_id: &[u8; 16]) -> bool {
    let matches = |having: [u8; 16]| having != [0; 16] && having == *claim_id;
    matches(localhost.claim_id_of_parent()) || localhost.claim_id().is_some_and(matches)
}

/// `verify_host_uuids()`: the machine GUID's text as the host keeps it, and a node id the host has.
fn verify_host_uuids(localhost: &Host, machine_guid: &[u8; 16], node_id: &[u8; 16]) -> bool {
    let mut guid = Vec::with_capacity(36);
    print_uuid_lower(&mut guid, machine_guid);
    let having = localhost.node_id();
    guid == localhost.machine_guid().as_bytes() && having != [0; 16] && having == *node_id
}

/// `function_bearer_get_token()`: only for Cloud's calls; the payload's request for this agent and node, then its
/// token, minified.
pub(super) fn call(localhost: &Host, reply: &mut Reply, payload: Option<&Payload>, source: &[u8]) -> u16 {
    // user_auth_source_is_cloud()
    if !source.starts_with(b"method=NC,") {
        return reply.error("Bearer tokens can only be provided via NC.", 400);
    }
    let rq = match jsonc::function_payload_or_error(reply, payload.map(|p| p.body.as_slice()), parse) {
        Ok(rq) => rq,
        Err(code) => return code,
    };
    if !claim_id_matches_any(localhost, &rq.claim_id) {
        return reply.error("The request is for a different agent", 400);
    }
    if !verify_host_uuids(localhost, &rq.machine_guid, &rq.node_id) {
        return reply.error("The request is missing or not matching local node UUIDs", 400);
    }
    let client_name = rq.client_name.as_deref().unwrap_or_default();
    let Some((token, expires_s)) = crate::bearer::create(rq.role, rq.access, rq.account, client_name.as_bytes()) else {
        return reply.error("Failed to create a bearer token", 500);
    };
    let mut w = JsonWriter::new(JsonOptions::MINIFY);
    w.member_add_int64("status", 200);
    w.member_add_string("mg", localhost.machine_guid());
    w.member_add_boolean("bearer_protection", crate::auth::bearer_protection());
    w.member_add_uuid("token", &token);
    w.member_add_time_t("expiration", expires_s);
    super::json_reply(w, reply)
}
