//! The Functions endpoints (`src/web/api/v1/api_v1_function.c`, `api_v1_functions.c`): a call of one of a host's
//! methods that waits for its answer, and the list of the methods users see.

use netdata_agent_nrpc::call::{CallSpec, Calls};
use netdata_agent_nrpc::catalog;
use netdata_agent_nrpc::reply::{ContentType, Reply as NrpcReply};
use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::parse::strtoul0;
use netdata_agent_text::print::print_uuid_lower_compact;
use netdata_agent_web::status;

use crate::access_log::RequestContext;
use crate::router::{Host, Route, not_ready};
use crate::server::Reply;

/// `api_v1_function()`: the method named by `function=` called with the caller's access, its source and the request's
/// transaction as the call id, waiting for the answer until its timeout, unless the client goes away.
pub fn call(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    if let Some(reply) = not_ready(route) {
        return reply;
    }
    let (function, timeout_s) = parameters(query);
    let Some(function) = function.filter(|f| !f.is_empty()) else {
        // C answers 400 here rather than a misleading 404 for a NULL command
        let mut reply = NrpcReply::new(ContentType::ApplicationJson);
        let code = reply.error("No function given to execute.", status::BAD_REQUEST);
        return reply_of(reply, code);
    };
    let mut transaction = Vec::with_capacity(32);
    print_uuid_lower_compact(&mut transaction, &route.ctx.transaction);
    let source = source(route.ctx, route.forwarded_for);
    let hostname = host.hostname();
    // web_client_interrupt_callback(): the peek's errno stays off the request's record, as C's (none observed)
    let gone = || (route.interrupted)(&mut 0);
    let called = Calls::process().call(CallSpec {
        owner: Some((host.functions(), &hostname)),
        cmd: function,
        source: &source,
        user_access: route.ctx.auth.access(),
        timeout_s,
        wait: true,
        allow_restricted: false,
        call_id: Some(&transaction),
        payload: route.payload.cloned(),
        reply: NrpcReply::new(ContentType::ApplicationJson),
        done: None,
        progress: None,
        is_cancelled: Some(&gone),
    });
    let reply = called.reply.unwrap_or_else(|| NrpcReply::new(ContentType::ApplicationJson));
    reply_of(reply, called.code)
}

/// The query's `function` and `timeout`, as `api_v1_function()` reads them: `&`-separated pairs split at their first
/// `=`, empty ones skipped, the last occurrence winning (a `function` without `=` clears it), the timeout as
/// `(int) strtoul(v, NULL, 0)`. A bare `timeout` crashes C (`strtoul(NULL)`, D135.9); here it is the method's.
fn parameters(query: &[u8]) -> (Option<&[u8]>, i32) {
    let (mut function, mut timeout_s) = (None, 0);
    let mut rest = Some(query);
    while rest.is_some() {
        let pair = strsep_skip(&mut rest, b"&");
        if pair.is_empty() {
            continue;
        }
        let mut value = Some(pair);
        let name = strsep_skip(&mut value, b"=");
        if name.is_empty() {
            continue;
        }
        match name {
            b"function" => function = value,
            b"timeout" => timeout_s = value.map_or(0, |v| strtoul0(v).0 as i32),
            _ => {}
        }
    }
    (function, timeout_s)
}

/// `user_auth_to_source_buffer()`: who calls, for the method's handler (a plugin reads it on its stdin): the method
/// (`none`, or `api-bearer` for a token), role, permissions, then the token's user and account, the client's ip (none
/// after a keep-alive request's first) and `X-Forwarded-For` (cut at 45 bytes), each when set.
pub fn source(ctx: &RequestContext, forwarded_for: &[u8]) -> Vec<u8> {
    let auth = &ctx.auth;
    let method = if auth.is_bearer() { "api-bearer" } else { "none" };
    let mut out = format!("method={method},role={},permissions=0x{:x}", auth.role(), auth.access()).into_bytes();
    let (user, account) = auth.identity();
    if !user.is_empty() {
        out.extend_from_slice(b",user=");
        out.extend_from_slice(user.as_bytes());
    }
    if account != [0; 16] {
        out.extend_from_slice(b",account=");
        print_uuid_lower_compact(&mut out, &account);
    }
    if !ctx.ip.is_empty() {
        out.extend_from_slice(b",ip=");
        out.extend_from_slice(ctx.ip.as_bytes());
    }
    if !forwarded_for.is_empty() {
        out.extend_from_slice(b",forwarded_for=");
        out.extend_from_slice(forwarded_for);
    }
    out
}

/// The call's answer as the web server sends it: a cacheable 200 keeps its expiry, anything else is no-cache with
/// none (`buffer_no_cacheable()` zeroes it, so `Expires` is the date).
fn reply_of(reply: NrpcReply, code: u16) -> Reply {
    let cacheable = code == status::OK && reply.cacheable;
    Reply {
        code,
        content_type: reply.content_type,
        body: reply.body,
        no_cacheable: !cacheable,
        expires: if cacheable { reply.expires } else { 0 },
        ..Reply::default()
    }
}

/// `api_v1_functions()`: the host's methods users see, as C's pretty JSON (an object without `functions` for a host
/// without a registry).
pub fn list(route: &Route<'_>, host: &Host, _query: &[u8]) -> Reply {
    if let Some(reply) = not_ready(route) {
        return reply;
    }
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    catalog::to_json(host.functions(), &mut w);
    w.finalize();
    Reply {
        code: status::OK,
        content_type: ContentType::ApplicationJson,
        body: w.into_bytes(),
        ..Reply::default()
    }
}

#[cfg(test)]
mod tests;
