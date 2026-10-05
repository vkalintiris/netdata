//! The inline API (`dyncfg-inline.c`, `dyncfg_node_find_and_call()` of `libnetdata/inicfg/dyncfg.c`): nodes of the
//! agent itself, whose commands a callback of the agent answers on the caller's thread.
//!
//! A private map by id holds each node's callback; the core gets one built-in handler for all of them, so a node
//! registered again keeps the same handler, as C's one function pointer does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use netdata_agent_nrpc::Handler;
use netdata_agent_nrpc::reply::{ContentType, Payload, Reply};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::c::c_str;
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

use crate::model::{Cmds, SourceType, Status, Type, default_response};
use crate::{AddSpec, Dyncfg};

/// `MAX_FUNCTION_PARAMETERS`.
const MAX_FUNCTION_PARAMETERS: usize = 1024;

/// `dyncfg_cb_t`, the parts its one caller (health) reads: the reply, the node's id, the command, the name an `add`
/// carries, the payload, the caller's source. C's callback also gets the transaction, the caller's deadline, its
/// cancellation and its access, which health ignores.
pub type InlineCallback = Arc<dyn Fn(&mut Reply, &[u8], Cmds, Option<&[u8]>, Option<&Payload>, &[u8]) -> u16 + Send + Sync>;

/// `struct dyncfg_add_inline_spec`.
pub struct InlineSpec<'a> {
    pub host: &'a Host,
    pub id: &'a [u8],
    pub path: &'a [u8],
    pub status: Status,
    pub kind: Type,
    pub source_type: SourceType,
    pub source: &'a [u8],
    pub cmds: Cmds,
    pub view_access: u32,
    pub edit_access: u32,
    pub cb: InlineCallback,
}

/// `dyncfg_nodes` of `dyncfg-inline.c`: the callbacks by id. A leaf lock: a callback is cloned out and called with
/// the lock released, as it may register a node itself.
type Callbacks = Mutex<HashMap<Vec<u8>, InlineCallback>>;

pub(crate) struct Inline {
    callbacks: Arc<Callbacks>,
    /// `dyncfg_inline_callback`, the handler of every inline node.
    handler: Handler,
}

impl Inline {
    pub(crate) fn new() -> Inline {
        let callbacks: Arc<Callbacks> = Arc::default();
        let found = Arc::clone(&callbacks);
        let handler = Handler::Builtin(Arc::new(
            move |reply: &mut Reply, function: &[u8], payload: Option<&Payload>, source: &[u8]| {
                find_and_call(&found, reply, function, payload, source)
            },
        ));
        Inline { callbacks, handler }
    }

    fn callbacks(&self) -> std::sync::MutexGuard<'_, HashMap<Vec<u8>, InlineCallback>> {
        self.callbacks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the id has a callback.
    #[cfg(test)]
    pub(crate) fn has(&self, id: &[u8]) -> bool {
        self.callbacks().contains_key(id)
    }
}

/// `dyncfg_node_find_and_call()`: the function's second word is the node's id, its third the command, its fourth
/// the name; the node's callback answers into an emptied JSON reply, and an answer without an expiry expires now.
fn find_and_call(
    callbacks: &Callbacks,
    reply: &mut Reply,
    function: &[u8],
    payload: Option<&Payload>,
    source: &[u8],
) -> u16 {
    if c_str(function).is_empty() {
        return default_response(reply, 400, "command received is empty");
    }
    let words = quoted_strings_splitter(function, MAX_FUNCTION_PARAMETERS, Separators::Whitespace);
    let word = |index: usize| words.get(index).map(Vec::as_slice).filter(|word| !word.is_empty());
    let Some(id) = word(1) else {
        return default_response(reply, 400, "dyncfg node: id is missing from the request");
    };
    let Some(action) = word(2) else {
        return default_response(reply, 400, "dyncfg node: action is missing from the request");
    };
    let cmd = Cmds::parse(action);
    if cmd == Cmds::NONE {
        return default_response(reply, 400, "dyncfg node: action given in request is unknown");
    }
    let callback = callbacks.lock().unwrap_or_else(PoisonError::into_inner).get(id).cloned();
    let Some(callback) = callback else {
        return default_response(reply, 404, "dyncfg node: id is not found");
    };

    reply.body.clear();
    reply.content_type = ContentType::ApplicationJson;
    let code = callback(reply, id, cmd, word(3), payload, source);
    if reply.expires == 0 {
        reply.expires = now_realtime_s();
    }
    code
}

impl Dyncfg {
    /// `dyncfg_add()`: the node's callback kept by its id, then the node registered with the core as a synchronous
    /// one with the inline handler; a registration the core refuses takes the callback out again. The core's first
    /// echo of a job, and a template's saved jobs, run the callback before this returns.
    pub fn add_inline(&self, spec: InlineSpec<'_>) -> bool {
        self.inline.callbacks().insert(spec.id.to_vec(), spec.cb);
        let added = self.add_low_level(AddSpec {
            host: spec.host,
            id: spec.id,
            path: spec.path,
            status: spec.status,
            kind: spec.kind,
            source_type: spec.source_type,
            source: spec.source,
            cmds: spec.cmds,
            sync: true,
            view_access: spec.view_access,
            edit_access: spec.edit_access,
            handler: self.inline.handler.clone(),
        });
        if !added {
            self.inline.callbacks().remove(spec.id);
        }
        added
    }

    /// `dyncfg_del()`: the callback, then the core's node. (`dyncfg_status()` is `status_low_level()`.)
    pub fn del_inline(&self, host: &Host, id: &[u8]) {
        self.inline.callbacks().remove(id);
        self.del_low_level(host, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callbacks_with(id: &[u8], callback: InlineCallback) -> Callbacks {
        let callbacks = Callbacks::default();
        callbacks.lock().unwrap().insert(id.to_vec(), callback);
        callbacks
    }

    /// `dyncfg_node_find_and_call()`'s own refusals, each C's default answer, and none of them reaches a callback.
    #[test]
    fn a_request_without_a_node_is_refused_in_c_s_words() {
        let callbacks = callbacks_with(b"a:node", Arc::new(|_, _, _, _, _, _| panic!("no callback is called")));
        for (function, code, message) in [
            ("", 400, "command received is empty"),
            ("config", 400, "dyncfg node: id is missing from the request"),
            ("config a:node", 400, "dyncfg node: action is missing from the request"),
            ("config a:node dance", 400, "dyncfg node: action given in request is unknown"),
            ("config another get", 404, "dyncfg node: id is not found"),
        ] {
            let mut reply = Reply::new(ContentType::TextPlain);
            reply.body = b"left over".to_vec();
            assert_eq!(find_and_call(&callbacks, &mut reply, function.as_bytes(), None, b""), code, "{function}");
            let body = format!("{{\"status\":{code},\"message\":\"{message}\"}}");
            assert_eq!(String::from_utf8_lossy(&reply.body), body);
            assert_eq!(reply.content_type, ContentType::ApplicationJson);
        }
    }

    /// The callback gets the id, the command, the name and the payload, into an emptied JSON reply; an answer it
    /// gives no expiry expires now, and one it gave keeps it.
    #[test]
    fn a_node_s_callback_answers_its_commands() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = Arc::clone(&seen);
        let callback: InlineCallback = Arc::new(move |reply, id, cmd, name, payload, source| {
            log.lock().unwrap().push(format!(
                "{} {} {:?} {:?} {} body={:?} type={:?}",
                String::from_utf8_lossy(id),
                cmd.name_one().unwrap_or("?"),
                name.map(String::from_utf8_lossy),
                payload.map(|p| String::from_utf8_lossy(&p.body).into_owned()),
                String::from_utf8_lossy(source),
                String::from_utf8_lossy(&reply.body),
                reply.content_type,
            ));
            if cmd == Cmds::GET {
                reply.expires = 7;
            }
            reply.body = b"answered".to_vec();
            202
        });
        let callbacks = callbacks_with(b"a:node", callback);

        let before = now_realtime_s();
        let mut reply = Reply::new(ContentType::TextPlain);
        reply.body = b"left over".to_vec();
        let payload = Payload { body: b"{}".to_vec(), content_type: ContentType::ApplicationJson };
        let code = find_and_call(&callbacks, &mut reply, b"config a:node add a_name more", Some(&payload), b"src");
        assert_eq!((code, reply.body.as_slice()), (202, b"answered".as_slice()));
        assert!(reply.expires >= before && reply.expires <= before + 2, "{}", reply.expires);

        let mut reply = Reply::new(ContentType::TextPlain);
        assert_eq!(find_and_call(&callbacks, &mut reply, b"config a:node get", None, b"src"), 202);
        assert_eq!(reply.expires, 7);

        assert_eq!(
            *seen.lock().unwrap(),
            [
                "a:node add Some(\"a_name\") Some(\"{}\") src body=\"\" type=ApplicationJson",
                "a:node get None None src body=\"\" type=ApplicationJson",
            ]
        );
    }

    /// The map's lock is not held while a callback runs: one that adds a callback returns.
    #[test]
    fn a_callback_may_reach_the_map_it_is_called_from() {
        let callbacks: Arc<Callbacks> = Arc::default();
        let map = Arc::clone(&callbacks);
        let callback: InlineCallback = Arc::new(move |_, _, _, _, _, _| {
            map.lock().unwrap().insert(b"made:inside".to_vec(), Arc::new(|_, _, _, _, _, _| 200));
            200
        });
        callbacks.lock().unwrap().insert(b"a:node".to_vec(), callback);
        let mut reply = Reply::new(ContentType::TextPlain);
        assert_eq!(find_and_call(&callbacks, &mut reply, b"config a:node enable", None, b""), 200);
        assert!(callbacks.lock().unwrap().contains_key(b"made:inside".as_slice()));
    }
}
