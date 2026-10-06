//! `/api/v1/registry` (`api_v1_registry()` in `src/web/api/v1/api_v1_registry.c`, its JSON in
//! `src/registry/registry.c`) with the registry itself disabled, as C's default `[registry] enabled = no` leaves it
//! (D220 fork 3, D223): `hello` in full, C's `disabled` document for the other four actions, and every 400 and 451 on
//! the way. With `enabled = yes` C answers from its database; that answer comes with the registry's milestone.

use std::sync::{Mutex, PoisonError};

use netdata_agent_inicfg::Config;
use netdata_agent_log::netdata_log_error;
use netdata_agent_query::request::pairs;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_web::status;

use crate::acl;
use crate::auth;
use crate::cloud;
use crate::router::Route;
use crate::server::{Reply, permission_denied_acl};
use crate::startup;
use crate::status_file::cloud_status;
use crate::v1_charts::json_reply;

/// What `registry_init()` leaves for the requests: `registry.registry_to_announce`, and `registry.cloud_base_url`,
/// cloud.conf's URL as `registry_update_cloud_base_url()` copies it at the registry's init and at each claim reload
/// (`netdatacli reload-claiming-state`). A parent's NODE_ID changes cloud.conf's URL in between, not this copy. Kept
/// as bytes: C prints a configured value as it is.
#[derive(Default)]
pub struct Settings {
    pub announce: Vec<u8>,
    /// Under its own lock, as C's `registry_cloud_base_url_spinlock`.
    cloud_base_url: Mutex<Vec<u8>>,
}

impl Settings {
    pub fn new(announce: Vec<u8>, cloud_base_url: Vec<u8>) -> Self {
        Settings { announce, cloud_base_url: Mutex::new(cloud_base_url) }
    }

    /// `registry_update_cloud_base_url()` at a claim reload: cloud.conf's URL copied again and exported for the
    /// plugins started from then on.
    pub fn update_cloud_base_url(&self, cloud_conf: &mut Config) {
        let url = cloud::url(cloud_conf);
        let mut current = self.cloud_base_url.lock().unwrap_or_else(PoisonError::into_inner);
        crate::conf::export("NETDATA_REGISTRY_CLOUD_BASE_URL", &String::from_utf8_lossy(&url));
        *current = url;
    }

    pub fn cloud_base_url(&self) -> Vec<u8> {
        self.cloud_base_url.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    Access,
    Hello,
    Delete,
    Search,
    Switch,
}

#[derive(Default, Debug, PartialEq)]
struct Params<'a> {
    action: Option<Action>,
    machine: Option<&'a [u8]>,
    url: Option<&'a [u8]>,
    name: Option<&'a [u8]>,
    delete_url: Option<&'a [u8]>,
    search_for: Option<&'a [u8]>,
    to: Option<&'a [u8]>,
}

/// The query's loop: an unknown action leaves the one set before it, and an action's own parameter counts only when
/// that action is already set where it appears.
fn parse(query: &[u8]) -> Params<'_> {
    let mut p = Params::default();
    for (name, value) in pairs(query) {
        match name {
            b"action" => match value {
                b"access" => p.action = Some(Action::Access),
                b"hello" => p.action = Some(Action::Hello),
                b"delete" => p.action = Some(Action::Delete),
                b"search" => p.action = Some(Action::Search),
                b"switch" => p.action = Some(Action::Switch),
                _ => {}
            },
            b"machine" => p.machine = Some(value),
            b"url" => p.url = Some(value),
            _ => match (p.action, name) {
                (Some(Action::Access), b"name") => p.name = Some(value),
                (Some(Action::Delete), b"delete_url") => p.delete_url = Some(value),
                (Some(Action::Search), b"for") => p.search_for = Some(value),
                (Some(Action::Switch), b"to") => p.to = Some(value),
                _ => {}
            },
        }
    }
    p
}

fn unset(value: Option<&[u8]>) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(value.unwrap_or(b"UNSET"))
}

/// `api_v1_registry()`. The person's id C looks for in the received request (`api_v1_registry.c:47-52`) is read
/// only by the enabled registry, so it is not looked for here (D223 F3).
pub fn api_v1_registry(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let p = parse(query);
    // C's respect_web_browser_do_not_track_policy && the DNT flag: the flag is set only under the policy
    let do_not_track = route.do_not_track;

    if p.action == Some(Action::Hello) {
        // analytics_log_dashboard() counts the hit here, before the ACL check; its counter comes with the ANALYTICS
        // thread (D223 F2)
        if !acl::can(route.acl, acl::bits::DASHBOARD) {
            return permission_denied_acl();
        }
    } else {
        if !acl::can(route.acl, acl::bits::REGISTRY) {
            return permission_denied_acl();
        }
        if do_not_track {
            return Reply::text(
                status::BAD_REQUEST,
                "Your web browser is sending 'DNT: 1' (Do Not Track). The registry requires persistent cookies on \
                 your browser to work.",
            );
        }
    }

    match p.action {
        Some(Action::Access) => {
            if p.machine.is_none() || p.url.is_none() || p.name.is_none() {
                netdata_log_error!(
                    "Invalid registry request - access requires these parameters: machine ('{}'), url ('{}'), name \
                     ('{}')",
                    unset(p.machine),
                    unset(p.url),
                    unset(p.name)
                );
                return Reply::text(status::BAD_REQUEST, "Invalid registry Access request.");
            }
            disabled(route, host, "access")
        }
        Some(Action::Delete) => {
            if p.machine.is_none() || p.url.is_none() || p.delete_url.is_none() {
                netdata_log_error!(
                    "Invalid registry request - delete requires these parameters: machine ('{}'), url ('{}'), \
                     delete_url ('{}')",
                    unset(p.machine),
                    unset(p.url),
                    unset(p.delete_url)
                );
                return Reply::text(status::BAD_REQUEST, "Invalid registry Delete request.");
            }
            disabled(route, host, "delete")
        }
        Some(Action::Search) => {
            if p.search_for.is_none() {
                netdata_log_error!(
                    "Invalid registry request - search requires these parameters: for ('{}')",
                    unset(p.search_for)
                );
                return Reply::text(status::BAD_REQUEST, "Invalid registry Search request.");
            }
            disabled(route, host, "search")
        }
        Some(Action::Switch) => {
            if p.machine.is_none() || p.url.is_none() || p.to.is_none() {
                netdata_log_error!(
                    "Invalid registry request - switching identity requires these parameters: machine ('{}'), url \
                     ('{}'), to ('{}')",
                    unset(p.machine),
                    unset(p.url),
                    unset(p.to)
                );
                return Reply::text(status::BAD_REQUEST, "Invalid registry Switch request.");
            }
            disabled(route, host, "switch")
        }
        Some(Action::Hello) => hello(route, host, do_not_track),
        // C's text names no `switch`
        None => Reply::text(
            status::BAD_REQUEST,
            "Invalid registry request - you need to set an action: hello, access, delete, search",
        ),
    }
}

/// `registry_json_header()`: the routed host's registry hostname and GUID.
fn header(w: &mut JsonWriter, host: &Host, action: &str, status: &str) {
    w.member_add_string("action", action);
    w.member_add_string("status", status);
    w.member_add_string("hostname", host.registry_hostname());
    w.member_add_string("machine_guid", host.machine_guid());
}

fn json(mut w: JsonWriter, tracking_required: bool) -> Reply {
    w.finalize();
    Reply { tracking_required, ..json_reply(w.into_bytes()) }
}

/// `registry_json_disabled()`: every action but hello checks `registry.enabled` first, before its URL checks. The
/// request already asked for tracking (`web_client_enable_tracking_required()`): under `[web] respect do not track
/// policy` the answer says `Tk: T;cookies`.
fn disabled(route: &Route<'_>, host: &Host, action: &str) -> Reply {
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    header(&mut w, host, action, "disabled");
    w.member_add_string("registry", &route.shared.registry.announce);
    json(w, true)
}

/// `registry_request_hello_json()`: the routed host in the header, localhost as `agent` (but the routed host's claim
/// id, as C's), and every host of the index.
fn hello(route: &Route<'_>, host: &Host, do_not_track: bool) -> Reply {
    let shared = route.shared;
    let localhost = shared.hosts.localhost();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    header(&mut w, host, "hello", "ok");
    add_node_id(&mut w, host);
    w.member_add_object("agent");
    w.member_add_string("machine_guid", localhost.machine_guid());
    add_node_id(&mut w, localhost);
    // rrdhost_claim_id_get(): the parent's fallback comes with the Cloud milestone (D223 F1)
    if let Some(claim_id) = host.claim_id() {
        w.member_add_uuid("claim_id", &claim_id);
    }
    w.member_add_boolean("bearer_protection", auth::bearer_protection());
    w.object_close();
    w.member_add_string("cloud_status", cloud_status::name(cloud::status()));
    w.member_add_string("cloud_base_url", shared.registry.cloud_base_url());
    w.member_add_string("registry", &shared.registry.announce);
    w.member_add_boolean("anonymous_statistics", !do_not_track && startup::anonymous_statistics());
    w.member_add_boolean("X-Netdata-Auth", true);
    w.member_add_array(Some(b"nodes"));
    for h in shared.hosts.all() {
        w.add_array_item_object();
        w.member_add_string("machine_guid", h.machine_guid());
        add_node_id(&mut w, &h);
        w.member_add_string("hostname", h.registry_hostname());
        w.object_close();
    }
    w.array_close();
    json(w, false)
}

fn add_node_id(w: &mut JsonWriter, host: &Host) {
    let node_id = host.node_id();
    if node_id != [0; 16] {
        w.member_add_uuid("node_id", &node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_keeps_c_s_order_rules() {
        let cases: [(&[u8], Params<'_>); 7] = [
            (b"action=hello&action=bogus", Params { action: Some(Action::Hello), ..Params::default() }),
            (b"action=bogus&action=hello", Params { action: Some(Action::Hello), ..Params::default() }),
            (b"action=HELLO", Params::default()),
            (b"action=", Params::default()),
            (
                b"name=n&action=access&machine=m&url=u",
                Params { action: Some(Action::Access), machine: Some(b"m"), url: Some(b"u"), ..Params::default() },
            ),
            (
                b"machine=m&action=delete&delete_url=d&for=f&to=t",
                Params {
                    action: Some(Action::Delete),
                    machine: Some(b"m"),
                    delete_url: Some(b"d"),
                    ..Params::default()
                },
            ),
            (
                b"action=search&for=a&for=b&name=n",
                Params { action: Some(Action::Search), search_for: Some(b"b"), ..Params::default() },
            ),
        ];
        for (query, want) in cases {
            assert_eq!(parse(query), want, "{}", String::from_utf8_lossy(query));
        }
    }
}
