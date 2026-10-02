//! `/api/v1/config` and `/api/v3/config` (`src/web/api/v1/api_v1_config.c`): DynCfg's `config` function called with
//! the request's action on the host, waiting for its answer. Unlike `/api/v1/function`, it does not wait for the
//! agent to be ready.

use netdata_agent_dyncfg::model::{Cmds, is_valid_id};
use netdata_agent_nrpc::reply::{ContentType, Reply as NrpcReply};
use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::parse::strtoll10;
use netdata_agent_web::status;

use crate::functions::{reply_of, wait_call};
use crate::router::{Host, Route};
use crate::server::Reply;

/// `api_v1_config()`.
pub fn call(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let p = parameters(query);
    match command(&p) {
        Ok(cmd) => wait_call(route, host, &cmd, p.timeout_s),
        Err(text) => {
            let mut reply = NrpcReply::new(ContentType::ApplicationJson);
            let code = reply.error(text, status::BAD_REQUEST);
            reply_of(reply, code)
        }
    }
}

/// The request's parameters, with C's defaults.
#[derive(Debug, PartialEq)]
struct Parameters<'a> {
    action: &'a [u8],
    path: &'a [u8],
    id: Option<&'a [u8]>,
    name: Option<&'a [u8]>,
    timeout_s: i32,
}

/// C's loop: `&`-separated pairs split at their first `=`, a pair without a name or a value skipped, the last
/// occurrence winning; the timeout as `(int) strtol(v, NULL, 10)`, at least 10.
fn parameters(query: &[u8]) -> Parameters<'_> {
    let mut p = Parameters { action: b"tree", path: b"/", id: None, name: None, timeout_s: 120 };
    let mut rest = Some(query);
    while rest.is_some() {
        let pair = strsep_skip(&mut rest, b"&");
        if pair.is_empty() {
            continue;
        }
        let mut value = Some(pair);
        let name = strsep_skip(&mut value, b"=");
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        match name {
            b"action" => p.action = value,
            b"path" => p.path = value,
            b"id" => p.id = Some(value),
            b"name" => p.name = Some(value),
            b"timeout" => p.timeout_s = (strtoll10(value).0 as i32).max(10),
            _ => {}
        }
    }
    p
}

/// The `config` command of the request, or C's 400 text: `tree` with its path and id quoted; another action needs a
/// valid id and a known command; `add`, `userconfig` and `test` (alone) need a valid name, which `test` takes from
/// the id's last `:` part, else `test`. A set of more than one command prints as C's `(null)`.
fn command(p: &Parameters) -> Result<Vec<u8>, &'static str> {
    let mut cmd = b"config ".to_vec();
    if p.action == b"tree" {
        cmd.extend_from_slice(b"tree '");
        cmd.extend_from_slice(p.path);
        cmd.extend_from_slice(b"' '");
        cmd.extend_from_slice(p.id.unwrap_or_default());
        cmd.push(b'\'');
        return Ok(cmd);
    }
    let c = Cmds::parse(p.action);
    let Some(mut id) = p.id.filter(|id| is_valid_id(id)) else {
        return Err("Invalid id");
    };
    if c == Cmds::NONE {
        return Err("Invalid action");
    }
    let mut name = p.name;
    if c == Cmds::ADD || c == Cmds::USERCONFIG || c == Cmds::TEST {
        if c == Cmds::TEST && name.is_none() {
            // backwards compatibility for a test without a name
            match id.iter().rposition(|&b| b == b':') {
                Some(colon) => (id, name) = (&id[..colon], Some(&id[colon + 1..])),
                None => name = Some(b"test"),
            }
        }
        let Some(name) = name.filter(|n| !n.is_empty() && is_valid_id(n)) else {
            return Err("Invalid name");
        };
        cmd.extend_from_slice(id);
        cmd.push(b' ');
        cmd.extend_from_slice(c.name_one().unwrap_or("(null)").as_bytes());
        cmd.push(b' ');
        cmd.extend_from_slice(name);
    } else {
        cmd.extend_from_slice(id);
        cmd.push(b' ');
        cmd.extend_from_slice(c.name_one().unwrap_or("(null)").as_bytes());
    }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C's loop: defaults, empty names and values skipped, the last occurrence winning, the timeout's floor and its
    /// `(int) strtol()`.
    #[test]
    fn parameters_read_as_c() {
        let defaults = Parameters { action: b"tree", path: b"/", id: None, name: None, timeout_s: 120 };
        assert_eq!(parameters(b""), defaults);
        assert_eq!(
            parameters(b"action=get&&id=&=x&id=a:b&name=n&path=/p&timeout=3&action"),
            Parameters { action: b"get", path: b"/p", id: Some(b"a:b"), name: Some(b"n"), timeout_s: 10 }
        );
        assert_eq!(parameters(b"timeout=60x").timeout_s, 60);
        assert_eq!(parameters(b"timeout=4294967346").timeout_s, 50, "(int) of 2^32 + 50");
    }

    /// The commands C builds and its 400s: a tree, a single command, `(null)` for a set of them, the names `add`,
    /// `userconfig` and `test` need, `test`'s name from the id.
    #[test]
    fn commands_are_cs() {
        let p = |action: &'static [u8], id: Option<&'static [u8]>, name: Option<&'static [u8]>| Parameters {
            action,
            path: b"/collectors",
            id,
            name,
            timeout_s: 120,
        };
        let cases: [(Parameters, Result<&[u8], &str>); 11] = [
            (p(b"tree", None, None), Ok(b"config tree '/collectors' ''")),
            (p(b"tree", Some(b"a'b"), None), Ok(b"config tree '/collectors' 'a'b'")),
            (p(b"get", Some(b"go.d:x"), None), Ok(b"config go.d:x get")),
            (p(b"get update", Some(b"go.d:x"), None), Ok(b"config go.d:x (null)")),
            (p(b"get", None, None), Err("Invalid id")),
            (p(b"get", Some(b"a b"), None), Err("Invalid id")),
            (p(b"nope", Some(b"x"), None), Err("Invalid action")),
            (p(b"add", Some(b"t"), None), Err("Invalid name")),
            (p(b"add", Some(b"t"), Some(b"j")), Ok(b"config t add j")),
            (p(b"test", Some(b"t:j9"), None), Ok(b"config t test j9")),
            (p(b"test", Some(b"single"), None), Ok(b"config single test test")),
        ];
        for (params, want) in cases {
            assert_eq!(command(&params).as_deref().map_err(|e| *e), want, "{params:?}");
        }
        assert_eq!(command(&p(b"test", Some(b"t:"), None)), Err("Invalid name"));
    }
}
