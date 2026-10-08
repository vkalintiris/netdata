//! URL routing, ported from `web_client_process_request_from_web_server()`, `web_client_process_url()`,
//! `web_client_switch_host()` and `web_client_api_request()` in `src/web/server/web_client.c`, and
//! `web_client_api_request_vX()` in `src/web/api/web_api.c`.
//!
//! Not ported yet: `/mcp` and `/sse`, and the API commands other than `info`, `chart`, `charts`, `context`,
//! `contexts`, `registry`, `data`, `dbengine_stats`, `function`, `functions`, `manage`, `me`, `nodes`, `progress`,
//! `stream_info`, `stream_path`, `versions`, `alerts` and health's (`alarms`, `alarm_log` and the others of its
//! block of the table, and `badge.svg`).
//! `/netdata.conf` shows only the keys of the subsystems ported so far.

use std::sync::Arc;
use std::time::Instant;

use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::print_uuid_lower;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::request::Request;
use netdata_agent_web::url::Payload;
use netdata_agent_web::status;

use crate::access_log::RequestContext;
use crate::api;
use netdata_agent_nrpc::access;

use crate::acl;
use crate::auth;
use crate::config;
use crate::contexts_v2;
use crate::data;
use crate::dbengine_stats;
use crate::functions;
use crate::health_api;
use crate::manage;
use crate::registry;
use crate::server::{self, Reply, Shared};
use crate::static_file;
use crate::stream_info;
use crate::v1_charts;
use crate::v1_contexts;

/// `FILENAME_MAX`: the path and filename copies are truncated to it.
pub const FILENAME_MAX: usize = 4096;

/// A host the request is routed to (`RRDHOST *`).
pub type Host = Arc<netdata_agent_rrd::host::Host>;

/// An API command (`struct web_api_command`).
#[derive(Clone, Copy)]
struct Command {
    name: &'static str,
    /// `HTTP_ACL` bits the client must hold.
    acl: u32,
    /// `HTTP_ACCESS` bits the user must hold.
    access: u32,
    allow_subpaths: bool,
    callback: fn(&Route<'_>, &Host, &[u8]) -> Reply,
}

const API_V1: &[Command] = &[
    Command {
        name: "info",
        acl: acl::bits::NODES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, host, _| {
            not_ready(route).unwrap_or_else(|| Reply {
                code: status::OK,
                content_type: ContentType::ApplicationJson,
                body: api::info_json(host, route.shared),
                ..Reply::default()
            })
        },
    },
    Command {
        name: "chart",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, host, query| v1_charts::chart(host, &route.shared.health, query),
    },
    Command {
        name: "charts",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, host, _| {
            v1_charts::charts(
                host,
                &route.shared.hosts,
                &route.shared.health,
                route.shared.release_channel,
                route.shared.custom_dashboard_info(),
            )
        },
    },
    Command {
        name: "context",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |_, host, query| v1_contexts::context(host, query),
    },
    Command {
        name: "contexts",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |_, host, query| v1_contexts::contexts(host, query),
    },
    Command {
        name: "registry",
        acl: acl::bits::NONE,
        access: access::NONE,
        allow_subpaths: false,
        callback: |route, host, query| registry::api_v1_registry(route, host, query),
    },
    Command {
        name: "data",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: data::v1,
    },
    Command {
        name: "dbengine_stats",
        acl: acl::bits::NODES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, _| {
            not_ready(route).unwrap_or_else(|| dbengine_stats::reply(route.shared.hosts.storage()))
        },
    },
    Command {
        name: "function",
        acl: acl::bits::FUNCTIONS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: functions::call,
    },
    Command {
        name: "functions",
        acl: acl::bits::FUNCTIONS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: functions::list,
    },
    // alerts
    Command {
        name: "alarm_log",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::alarm_log,
    },
    Command {
        name: "alarms",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::alarms,
    },
    Command {
        name: "alarms_values",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::alarms_values,
    },
    Command {
        name: "alarm_count",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::alarm_count,
    },
    // the only v1 command with subpaths: it reads the path itself, and the key is its authorization
    Command {
        name: "manage",
        acl: acl::bits::MANAGEMENT,
        access: access::NONE,
        allow_subpaths: true,
        callback: manage::api_v1_manage,
    },
    Command {
        name: "alarm_variables",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::alarm_variables,
    },
    Command {
        name: "variable",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::variable,
    },
    Command {
        name: "badge.svg",
        acl: acl::bits::BADGES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::badge,
    },
    // dyncfg APIs
    Command {
        name: "config",
        acl: acl::bits::DYNCFG,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: config::call,
    },
];

/// `api_v1_info()` and `api_v1_dbengine_stats()` until startup completes: 503, the response buffer unflushed (the
/// request).
pub(crate) fn not_ready(route: &Route<'_>) -> Option<Reply> {
    (!(route.shared.ready)()).then(|| Reply {
        code: status::SERVICE_UNAVAILABLE,
        content_type: ContentType::TextPlain,
        body: route.input.to_vec(),
        ..Reply::default()
    })
}
const API_V2: &[Command] = &[
    Command {
        name: "data",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| data::v23(route, query, 2),
    },
    Command {
        name: "contexts",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::contexts(route, query),
    },
    Command {
        name: "alerts",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::alerts(route, query),
    },
    Command {
        name: "alert_config",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::alert_config,
    },
    Command {
        name: "info",
        acl: acl::bits::NOCHECK,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::info(route, query),
    },
    Command {
        name: "nodes",
        acl: acl::bits::NODES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::nodes(route, query),
    },
    Command {
        name: "versions",
        acl: acl::bits::NODES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::versions(route, query),
    },
    Command {
        name: "progress",
        acl: acl::bits::NOCHECK,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: functions::progress,
    },
    Command {
        name: "functions",
        acl: acl::bits::FUNCTIONS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::functions(route, query),
    },
    CLOUD_ONLY[0],
    CLOUD_ONLY[1],
    CLOUD_ONLY[2],
];
/// The commands of v2 and v3 only the Cloud reaches (`HTTP_ACL_ACLK`, which no local connection holds): a local
/// request gets the ACL's 451. Their handlers arrive with the ACLK (M11).
const CLOUD_ONLY: [Command; 3] = [
    Command {
        name: "rtc_offer",
        acl: acl::bits::ACLK,
        access: access::SIGNED_ID | access::SAME_SPACE,
        allow_subpaths: false,
        callback: |_, _, _| server::permission_denied_acl(),
    },
    Command {
        name: "bearer_protection",
        acl: acl::bits::ACLK,
        access: access::SIGNED_ID
            | access::SAME_SPACE
            | access::VIEW_AGENT_CONFIG
            | access::EDIT_AGENT_CONFIG,
        allow_subpaths: false,
        callback: |_, _, _| server::permission_denied_acl(),
    },
    Command {
        name: "bearer_get_token",
        acl: acl::bits::ACLK,
        access: access::SIGNED_ID | access::SAME_SPACE,
        allow_subpaths: false,
        callback: |_, _, _| server::permission_denied_acl(),
    },
];
const API_V3: &[Command] = &[
    Command {
        name: "variable",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::variable,
    },
    Command {
        name: "badge.svg",
        acl: acl::bits::BADGES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::badge,
    },
    Command {
        name: "data",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| data::v23(route, query, 3),
    },
    Command {
        name: "alerts",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::alerts(route, query),
    },
    Command {
        name: "alert_config",
        acl: acl::bits::ALERTS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: health_api::alert_config,
    },
    Command {
        name: "context",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |_, host, query| v1_contexts::context(host, query),
    },
    Command {
        name: "contexts",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::contexts(route, query),
    },
    Command {
        name: "info",
        acl: acl::bits::NOCHECK,
        access: access::NONE,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::info(route, query),
    },
    Command {
        name: "nodes",
        acl: acl::bits::NODES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::nodes(route, query),
    },
    Command {
        name: "stream_path",
        acl: acl::bits::NODES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::stream_path(route, query),
    },
    // v3's row asks for no feature of the client, v2's for the nodes' (web_api_v3.c:141-148, web_api_v2.c:98-105)
    Command {
        name: "versions",
        acl: acl::bits::NOCHECK,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::versions(route, query),
    },
    Command {
        name: "progress",
        acl: acl::bits::NOCHECK,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: functions::progress,
    },
    Command {
        name: "stream_info",
        acl: acl::bits::NOCHECK,
        access: access::NONE,
        allow_subpaths: false,
        callback: |route, _, query| stream_info::reply(&route.shared.hosts, query, netdata_agent_rrd::clock::now_realtime_s()),
    },
    Command {
        name: "me",
        acl: acl::bits::NOCHECK,
        access: access::NONE,
        allow_subpaths: false,
        callback: |route, _, _| auth::me(&route.ctx.auth),
    },
    Command {
        name: "function",
        acl: acl::bits::FUNCTIONS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: functions::call,
    },
    Command {
        name: "functions",
        acl: acl::bits::FUNCTIONS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::functions(route, query),
    },
    // dyncfg APIs
    Command {
        name: "config",
        acl: acl::bits::DYNCFG,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: config::call,
    },
    CLOUD_ONLY[0],
    CLOUD_ONLY[1],
    CLOUD_ONLY[2],
];

/// The per-request routing state (`WEB_CLIENT_FLAG_PATH_*`).
pub struct Route<'a> {
    pub shared: &'a Shared,
    /// `w->acl`.
    pub acl: u32,
    /// When the complete request was received (`w->timings.tv_in`).
    pub received: Instant,
    /// Whether the client went away (`web_client_interrupt_callback()`).
    pub interrupted: &'a dyn Fn(&mut i32) -> bool,
    /// The request as its log frames and records see it.
    pub ctx: &'a RequestContext,
    pub url_as_received: &'a [u8],
    /// The request's path, decoded, without its query. C's `w->url_path_decoded` is this without a `/host/<name>`
    /// prefix, which C cuts off as it switches hosts; the one reader, the management route, finds the same either
    /// way.
    pub path_decoded: &'a [u8],
    pub query: &'a [u8],
    /// `w->auth_bearer_token` as `X-Auth-Token` sets it: the management API's key.
    pub auth_token: Option<&'a [u8]>,
    /// `w->payload`: a POST or PUT body.
    pub payload: Option<&'a Payload>,
    /// `X-Forwarded-For` as received (cut at 45 bytes), for a call's source.
    pub forwarded_for: &'a [u8],
    /// `w->response.data` as the request left it: what was received, which a callback that returns before
    /// flushing it sends back.
    pub input: &'a [u8],
    /// `WEB_CLIENT_FLAG_PATH_IS_V0` .. `_V3`.
    pub version: Option<u8>,
    /// `web_client_has_donottrack()`: `DNT: 1` under `[web] respect do not track policy`.
    pub do_not_track: bool,
    pub trailing_slash: bool,
    pub has_extension: bool,
}

/// The GET/POST/PUT/DELETE branch of `web_client_process_request_from_web_server()`.
pub fn process_request(
    req: &Request,
    input: &[u8],
    acl: u32,
    shared: &Shared,
    received: Instant,
    ctx: &RequestContext,
    interrupted: &dyn Fn(&mut i32) -> bool,
) -> Reply {
    // web_client_process_url(): once the exit started every request is refused
    if (shared.exiting)() {
        return Reply {
            code: status::SERVICE_UNAVAILABLE,
            content_type: ContentType::TextPlain,
            body: b"This service is currently unavailable.".to_vec(),
            ..Reply::default()
        };
    }
    let path = &req.path[..req.path.len().min(FILENAME_MAX)];
    let end = path.iter().position(|&c| c == b'?').unwrap_or(path.len());
    // The first byte is never inspected for a dot, as in C.
    let last_marker = (1..end)
        .rev()
        .map(|i| path[i])
        .find(|&c| c == b'/' || c == b'.');
    let mut route = Route {
        shared,
        acl,
        received,
        interrupted,
        ctx,
        url_as_received: &req.url_as_received,
        path_decoded: &path[..end],
        query: &req.query,
        auth_token: req.headers.auth_token.as_deref(),
        payload: req.payload.as_ref(),
        forwarded_for: &req.headers.forwarded_for,
        input,
        version: None,
        do_not_track: req.headers.do_not_track,
        trailing_slash: end == 0 || path[end - 1] == b'/',
        has_extension: last_marker == Some(b'.'),
    };
    route.process_url(Arc::clone(shared.hosts.localhost()), Some(path))
}

impl<'a> Route<'a> {
    fn process_url(&mut self, host: Host, decoded: Option<&[u8]>) -> Reply {
        let filename = decoded.unwrap_or(b"");
        let mut rest = decoded;
        let version = match strsep_skip(&mut rest, b"/?") {
            b"api" => return self.api_request(&host, rest),
            b"host" => return self.switch_host(host, rest, false),
            b"node" => return self.switch_host(host, rest, true),
            b"v3" => 3,
            b"v2" => 2,
            b"v1" => 1,
            b"v0" => 0,
            b"netdata.conf" => return self.netdata_conf(),
            _ => return static_file::serve(self, filename),
        };
        if self.version.is_some() {
            return Reply::text(
                status::BAD_REQUEST,
                "Multiple dashboard versions given at the URL.",
            );
        }
        self.version = Some(version);
        self.process_url(host, rest)
    }

    /// `netdata.conf`: the configuration as the daemon reads it (`inicfg_generate()`).
    fn netdata_conf(&self) -> Reply {
        if !acl::can(self.acl, acl::bits::NETDATACONF) {
            return server::permission_denied_acl();
        }
        Reply {
            code: status::OK,
            content_type: ContentType::TextPlain,
            body: self.shared.conf().generate(false, true),
            ..Reply::default()
        }
    }

    /// `web_client_switch_host()` for the web server's routes.
    fn switch_host(&mut self, host: Host, mut url: Option<&[u8]>, nodeid: bool) -> Reply {
        if !Arc::ptr_eq(&host, self.shared.hosts.localhost()) {
            return Reply::text(status::BAD_REQUEST, "Nesting of hosts is not allowed.");
        }
        let tok = strsep_skip(&mut url, b"/");
        if !tok.is_empty() {
            if let Some(found) = self.find_host(tok, nodeid) {
                let Some(url) = url else {
                    return static_file::append_slash_and_redirect(self.url_as_received);
                };
                let path = [b"/", url].concat();
                return self.process_url(found, Some(&path));
            }
        }
        Reply::html(
            status::NOT_FOUND,
            "This netdata does not maintain a database for host: ",
            tok,
        )
    }

    /// The lookups of `web_client_switch_host()`: by machine GUID, node ID and hostname (node ID first for
    /// `/node/`), then by the canonical lowercase form of whatever `uuid_parse_flexi()` accepts.
    fn find_host(&self, tok: &[u8], nodeid: bool) -> Option<Host> {
        let hosts = &self.shared.hosts;
        let by_guid = |t: &[u8]| hosts.find_by_guid(&String::from_utf8_lossy(t));
        let by_node_id = |t: &[u8]| uuid_parse_flexi(t).and_then(|u| hosts.find_by_node_id(&u));
        let found = if nodeid {
            by_node_id(tok).or_else(|| by_guid(tok))
        } else {
            by_guid(tok).or_else(|| by_node_id(tok))
        };
        found
            .or_else(|| hosts.find_by_hostname(&String::from_utf8_lossy(tok)))
            .or_else(|| {
                let uuid = uuid_parse_flexi(tok)?;
                let mut canonical = Vec::new();
                print_uuid_lower(&mut canonical, &uuid);
                by_guid(&canonical)
            })
    }

    /// `web_client_api_request()`: `/api/<version>/<command>`, under its own frame.
    fn api_request(&self, host: &Host, mut rest: Option<&[u8]>) -> Reply {
        let _frame = self.ctx.api_frame();
        let table = match strsep_skip(&mut rest, b"/") {
            b"" => return Reply::text(status::BAD_REQUEST, "Which API version?"),
            b"v3" => API_V3,
            b"v2" => API_V2,
            b"v1" => API_V1,
            other => return Reply::html(status::NOT_FOUND, "Unsupported API version: ", other),
        };
        self.api_command(host, rest.unwrap_or(b""), table)
    }

    /// `web_client_api_request_vX()`.
    fn api_command(&self, host: &Host, endpoint: &[u8], table: &[Command]) -> Reply {
        // web_client_ensure_proper_authorization()
        self.ctx.auth.authorize_anonymous(auth::bearer_protection());
        if endpoint.is_empty() {
            return Reply::text(status::BAD_REQUEST, "Which API command?");
        }
        let slash = endpoint.iter().position(|&c| c == b'/');
        let name = &endpoint[..slash.unwrap_or(endpoint.len())];
        let Some(command) = table.iter().find(|c| c.name.as_bytes() == name) else {
            let mut reply = Reply::text(status::NOT_FOUND, "Unsupported API command: ");
            netdata_agent_text::print::html_escape(&mut reply.body, endpoint);
            return reply;
        };
        if !command.allow_subpaths && slash.is_some() {
            return Reply::text(
                status::BAD_REQUEST,
                &format!("API command '{}' does not support subpaths.", command.name),
            );
        }
        if !acl::can(self.acl, command.acl) && command.acl & acl::bits::NOCHECK == 0 {
            return server::permission_denied_acl();
        }
        let held = self.ctx.auth.access();
        if held & command.access != command.access {
            // web_client_permission_denied()
            return if held & access::SIGNED_ID != 0 {
                Reply::text(
                    status::FORBIDDEN,
                    "You don't have enough permissions to access this resource",
                )
            } else {
                Reply::text(
                    status::PRECOND_FAIL,
                    "You need to be authorized to access this resource",
                )
            };
        }
        let query = self.query.strip_prefix(b"?").unwrap_or(self.query);
        (command.callback)(self, host, query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_nrpc::testing::inert;

    fn shared() -> Shared {
        Shared {
            settings: netdata_agent_web::request::Settings {
                gzip: false,
                respect_do_not_track: false,
            },
            version: "v0",
            gzip_level: 3,
            web_dir: "/nonexistent-web-dir".into(),
            x_frame_options: None,
            tls: None,
            acl: test_acl(),
            first_request_timeout_s: 60,
            idle_timeout_s: 60,
            health: netdata_agent_health::Health::init(Default::default(), Box::new(|_| {})),
            management_key: b"5a1e0000-0000-4000-8000-00000000c0de".to_vec(),
            meta: None,
            user_config_dir: "/etc/netdata".into(),
            grouping_windows: Default::default(),
            gap_when_lost_iterations_above: 3,
            release_channel: "nightly",
            netdata_conf: Default::default(),
            custom_dashboard_info: Default::default(),
            ready: || true,
            exiting: || false,
            multidb_disk_quota_mb: 1024,
            page_cache_mb: 32,
            history_entries: 3600,
            build_info: crate::buildinfo::BuildInfo::new(&crate::buildinfo::Inputs {
                dirs: &Default::default(),
                home: "/nonexistent-home",
                system: &Default::default(),
                profile: "standalone",
                parent: false,
                child: false,
                memory: Default::default(),
            }),
            cloud_conf: Default::default(),
            cloud_conf_file: "/nonexistent-cloud.conf".into(),
            registry: crate::registry::Settings::new(
                b"https://registry.my-netdata.io".to_vec(),
                b"https://app.netdata.cloud".to_vec(),
            ),
            hosts: Arc::new(netdata_agent_rrd::host::Hosts::new(
                netdata_agent_rrd::host::Host::new(
                    "0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e",
                    true,
                    netdata_agent_rrd::host::HostInfo {
                        hostname: "box".into(),
                        registry_hostname: "box".into(),
                        os: "linux".into(),
                        timezone: "UTC".into(),
                        abbrev_timezone: "UTC".into(),
                        utc_offset: 0,
                        program_name: "netdata".into(),
                        program_version: "v0".into(),
                        update_every: 1,
                        db_mode: netdata_agent_rrd::mode::DbMode::Ram,
                        history_entries: 4096,
                        health_enabled: false,
                        system_info: Default::default(),
                        replication_enabled: false,
                        replication_period: 0,
                        replication_step: 0,
                        stream_send: None,
                        cache_dir: None,
                    },
                ),
            )),
        }
    }

    /// Lists that let every client use everything.
    fn test_acl() -> acl::WebAcl {
        use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};
        let any = || acl::AclPattern {
            pattern: SimplePattern::new(
                b"*",
                Separators::Whitespace,
                SimplePatternMode::Exact,
                true,
            ),
            dns: false,
        };
        acl::WebAcl {
            connections: any(),
            dashboard: any(),
            mcp: any(),
            badges: any(),
            registry: any(),
            streaming: any(),
            netdataconf: any(),
            management: any(),
        }
    }

    fn route(shared: &Shared, path: &[u8]) -> Reply {
        let mut req = Request::default();
        req.path = path.to_vec();
        req.url_as_received = path.to_vec();
        process_request(
            &req,
            path,
            acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES,
            shared,
            Instant::now(),
            &crate::access_log::RequestContext::default(),
            &|_| false,
        )
    }

    /// Until startup completes `/api/v1/info` and `/api/v1/dbengine_stats` answer 503 with what was received, as
    /// `api_v1_info()` and `api_v1_dbengine_stats()` return before flushing the buffer the request was read into; the
    /// other commands answer.
    #[test]
    fn info_and_dbengine_stats_wait_for_startup() {
        let s = Shared {
            ready: || false,
            exiting: || false,
            ..shared()
        };
        let input = b"GET /api/v1/info HTTP/1.1\r\nHost: localhost\r\n\r\n";
        for path in [
            &b"/api/v1/info"[..],
            b"/host/box/api/v1/info",
            b"/api/v1/dbengine_stats",
            b"/api/v1/function",
            b"/api/v1/functions",
        ] {
            let mut req = Request::default();
            req.path = path.to_vec();
            req.url_as_received = path.to_vec();
            let r = process_request(
                &req,
                input,
                acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES,
                &s,
                Instant::now(),
                &crate::access_log::RequestContext::default(),
                &|_| false,
            );
            assert_eq!(
                (r.code, r.content_type, r.body.as_slice()),
                (
                    status::SERVICE_UNAVAILABLE,
                    ContentType::TextPlain,
                    &input[..]
                )
            );
        }
        assert_eq!(route(&s, b"/api/v1/charts").code, status::OK);
        // the v2 and v3 functions and progress answer at once
        for path in [&b"/api/v2/functions"[..], b"/api/v3/functions"] {
            assert_eq!(route(&s, path).code, status::OK);
        }
        for path in [&b"/api/v2/progress"[..], b"/api/v3/progress"] {
            assert_eq!(route(&s, path).code, status::NOT_FOUND);
        }
    }

    /// A request as a client with the given features sends it, with its query.
    fn asked(shared: &Shared, path: &[u8], query: &[u8], features: u32) -> Reply {
        let mut req = Request::default();
        req.path = path.to_vec();
        req.query = query.to_vec();
        req.url_as_received = [path, b"?", query].concat();
        process_request(
            &req,
            path,
            acl::bits::TRANSPORTS | features,
            shared,
            Instant::now(),
            &crate::access_log::RequestContext::default(),
            &|_| false,
        )
    }

    /// `badge.svg` is a command of v1 and v3 (C's two tables hold the same row), not of v2, and takes no subpath.
    /// A client needs the `badges` feature for it, and that feature alone is enough. A chart the host has not is
    /// C's 200 with a badge that says so; no chart at all is the 400 with its text.
    #[test]
    fn badges_are_routed_in_v1_and_v3() {
        let s = shared();
        for path in [&b"/api/v1/badge.svg"[..], b"/api/v3/badge.svg", b"/host/box/api/v1/badge.svg"] {
            let r = asked(&s, path, b"chart=no.such", acl::bits::ALL_LISTENER_FEATURES);
            let shown = String::from_utf8_lossy(path).into_owned();
            assert_eq!((r.code, r.content_type), (status::OK, ContentType::ImageSvgXml), "{shown}");
            assert!(r.body.starts_with(b"<svg ") && r.body.ends_with(b"</svg>"), "{shown}");
            let needle = b">chart not found</text>";
            assert!(r.body.windows(needle.len()).any(|w| w == needle), "{shown}");
            assert!(r.no_cacheable && r.headers.is_empty() && (r.date, r.expires) == (0, 0), "{shown}");

            let r = asked(&s, path, b"", acl::bits::ALL_LISTENER_FEATURES);
            let want = (status::BAD_REQUEST, ContentType::TextPlain, &b"No chart id is given at the request."[..]);
            assert_eq!((r.code, r.content_type, r.body.as_slice()), want, "{shown}");
        }
        let v2 = asked(&s, b"/api/v2/badge.svg", b"chart=x", acl::bits::ALL_LISTENER_FEATURES);
        assert_eq!(v2.code, status::NOT_FOUND);
        let subpath = asked(&s, b"/api/v1/badge.svg/x", b"chart=x", acl::bits::ALL_LISTENER_FEATURES);
        assert_eq!(subpath.code, status::BAD_REQUEST);
        assert_eq!(subpath.body, b"API command 'badge.svg' does not support subpaths.");

        // the `badges` feature alone reaches the badge, and nothing but it does
        assert_eq!(asked(&s, b"/api/v1/badge.svg", b"chart=no.such", acl::bits::BADGES).code, status::OK);
        let others = acl::bits::ALL_LISTENER_FEATURES & !acl::bits::BADGES;
        let refused = asked(&s, b"/api/v1/badge.svg", b"chart=no.such", others);
        assert_eq!(refused.code, 451);
        assert_eq!(asked(&s, b"/api/v1/charts", b"", acl::bits::BADGES).code, 451);
    }

    /// Once the exit started every request answers C's 503, static files and the netdata.conf page included.
    #[test]
    fn requests_after_the_exit_started_are_refused() {
        let s = Shared { exiting: || true, ..shared() };
        for path in [&b"/api/v1/info"[..], b"/api/v1/charts", b"/host/box/api/v1/info", b"/index.html", b"/netdata.conf"] {
            let r = route(&s, path);
            assert_eq!(
                (r.code, r.content_type, r.body.as_slice()),
                (status::SERVICE_UNAVAILABLE, ContentType::TextPlain, &b"This service is currently unavailable."[..]),
                "{}",
                String::from_utf8_lossy(path)
            );
        }
    }

    /// Without the dbengine `/api/v1/dbengine_stats` answers 404 with C's text, the reply otherwise as it started.
    #[test]
    fn dbengine_stats_without_the_dbengine() {
        let r = route(&shared(), b"/api/v1/dbengine_stats");
        assert_eq!(
            (r.code, r.content_type, r.body.as_slice(), r.no_cacheable),
            (
                status::NOT_FOUND,
                ContentType::TextPlain,
                &b"dbengine is not enabled"[..],
                true
            )
        );
    }

    /// `/api/v1/info` ends with the host's functions after its labels (`api_v1_info.c:132-134`), then the routed host's
    /// memory mode and the dbengine's quota and page cache size, as `api_v1_info()` writes them after the flags (the
    /// members between are not ported yet, D84.2).
    #[test]
    fn info_ends_with_the_functions_and_the_dbengine_members() {
        let s = Shared {
            multidb_disk_quota_mb: 25,
            page_cache_mb: 8,
            ..shared()
        };
        let body = String::from_utf8(route(&s, b"/api/v1/info").body).unwrap();
        let tail = &body[body.find("\"host_labels\"").unwrap()..];
        let tail = &tail[tail.find('}').unwrap() + 1..];
        assert_eq!(
            tail.trim_end(),
            ",\n    \"functions\":{\n    },\n    \"memory-mode\":\"ram\",\n    \"multidb-disk-quota\":25,\n    \
             \"page-cache-size\":8\n}"
        );
    }

    /// `/api/v2/info` and `/api/v3/info` need no listener ACL (NOCHECK) and answer the agent in C's member order.
    /// `/api/v2/functions` (`rrdcontext_to_json_v2()` in FUNCTIONS mode): C's member order (`debug` adds the request
    /// after `api`, `mcp` drops `api` and the top-level timings), the node's `ni` its match order and a function's the
    /// `ni` of the hosts that have it; a client without the FUNCTIONS ACL gets the ACL's 451.
    #[test]
    fn functions_v2_answer_as_c() {
        use netdata_agent_nrpc::{MethodDesc, Source};
        let s = shared();
        let desc = MethodDesc {
            name: b"top",
            help: b"processes",
            tags: b"",
            timeout_s: 10,
            priority: 0,
            version: 1,
            access: access::ANONYMOUS_DATA,
            sync: false,
            source: Source::Stream,
            handler: inert(),
        };
        s.hosts.localhost().functions().register("box", &desc).unwrap();
        let request = |path: &[u8], query: &str, client_acl: u32| {
            let mut req = Request::default();
            req.path = path.to_vec();
            req.url_as_received = path.to_vec();
            req.query = query.as_bytes().to_vec();
            let ctx = crate::access_log::RequestContext::default();
            process_request(&req, b"", client_acl, &s, Instant::now(), &ctx, &|_| false)
        };
        let all = acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES;
        let body = |query: &str| {
            let r = request(b"/api/v2/functions", query, all);
            assert_eq!(r.code, status::OK);
            String::from_utf8(r.body).unwrap()
        };
        let at = |b: &str, key: &str| b.find(&format!("\"{key}\":")).unwrap_or(usize::MAX);
        let b = body("options=minify");
        assert!(b.starts_with(r#"{"api":2,"nodes":[{"#), "{b}");
        assert!(b.contains(r#""ni":0,"#), "{b}");
        assert!(b.contains(r#""functions":[{"name":"top","help":"processes","ni":[0],"priority":100,"version":1,"#));
        assert!(at(&b, "nodes") < at(&b, "functions") && at(&b, "functions") < at(&b, "versions"));
        assert!(at(&b, "versions") < at(&b, "agents") && at(&b, "agents") < b.rfind(r#""timings":"#).unwrap());
        assert!(b.ends_with("}}"), "the top-level timings close the answer: {b}");
        let b = body("options=debug");
        assert!(at(&b, "api") < at(&b, "request") && at(&b, "request") < at(&b, "nodes"));
        let b = body("options=minify,mcp");
        assert!(!b.contains(r#""api":"#) && b.ends_with("}]}"), "no api, the agents last: {b}");
        for path in [&b"/api/v2/functions"[..], b"/api/v3/functions"] {
            let r = request(path, "", all & !acl::bits::FUNCTIONS);
            assert_eq!(r.code, status::UNAVAILABLE_FOR_LEGAL_REASONS);
        }
    }

    #[test]
    fn info_v2_and_v3_answer_the_agent() {
        let s = shared();
        for path in [&b"/api/v2/info"[..], b"/api/v3/info"] {
            let mut req = Request::default();
            req.path = path.to_vec();
            req.url_as_received = path.to_vec();
            let r = process_request(
                &req,
                path,
                acl::bits::TRANSPORTS,
                &s,
                Instant::now(),
                &crate::access_log::RequestContext::default(),
                &|_| false,
            );
            assert_eq!(r.code, status::OK, "{}", String::from_utf8_lossy(path));
            assert!(r.no_cacheable);
            let body = String::from_utf8(r.body).unwrap();
            let keys = [
                "\"api\"", "\"agents\"", "\"mg\"", "\"nd\"", "\"nm\"", "\"now\"", "\"ai\"",
                "\"application\"", "\"cloud\"", "\"nodes\"", "\"metrics\"", "\"instances\"", "\"contexts\"",
                "\"capabilities\"", "\"api\"", "\"db_size\"", "\"prep_ms\"", "\"routing_ms\"",
            ];
            let mut at = 0;
            for key in keys {
                at += body[at..].find(key).unwrap_or_else(|| panic!("{key} after {at} in {body}")) + key.len();
            }
            assert!(body.contains("\"reason\":\"Agent is not claimed yet\""), "{body}");
            let nodes = &body[body.find("\"nodes\":{").unwrap()..];
            let nodes: String = nodes[..nodes.find('}').unwrap()].split_whitespace().collect();
            assert_eq!(nodes, "\"nodes\":{\"total\":1,\"receiving\":0,\"sending\":0,\"archived\":0");
        }
    }

    /// The top-level members of a pretty answer, in order.
    fn members(body: &str) -> Vec<&str> {
        body.lines().filter(|l| l.starts_with("    \"")).map(|l| &l[5..l.find("\":").unwrap()]).collect()
    }

    /// `/api/v2/versions` and `/api/v3/versions` (`api_v2_versions()`, the VERSIONS mode alone): the hashes between
    /// `api` and `timings`, and with `options=mcp` the hashes alone, where the bytes are C's for an agent alone with
    /// no context (the oracle's answer to `/api/v3/versions?options=mcp`). v2's row asks the client for the nodes
    /// feature; v3's asks for none (`HTTP_ACL_NOCHECK`).
    #[test]
    fn versions_are_routed_in_v2_and_v3() {
        let s = shared();
        let all = acl::bits::ALL_LISTENER_FEATURES;
        for path in [&b"/api/v2/versions"[..], b"/api/v3/versions"] {
            let shown = String::from_utf8_lossy(path).into_owned();
            let r = asked(&s, path, b"", all);
            assert_eq!((r.code, r.content_type), (status::OK, ContentType::ApplicationJson), "{shown}");
            assert!(r.no_cacheable, "{shown}");
            let body = String::from_utf8(r.body).unwrap();
            assert_eq!(members(&body), ["api", "versions", "timings"], "{shown}: {body}");
            let mcp = asked(&s, path, b"options=mcp", all);
            assert_eq!(
                String::from_utf8(mcp.body).unwrap(),
                concat!(
                    "{\n    \"versions\":{\n        \"routing_hard_hash\":1,\n        \"nodes_hard_hash\":1,\n",
                    "        \"contexts_hard_hash\":0,\n        \"contexts_soft_hash\":0,\n",
                    "        \"alerts_hard_hash\":0,\n        \"alerts_soft_hash\":0\n    }\n}\n"
                ),
                "{shown}"
            );
        }
        let denied = server::permission_denied_acl();
        let r = asked(&s, b"/api/v2/versions", b"", all & !acl::bits::NODES);
        assert_eq!((r.code, &r.body), (denied.code, &denied.body));
        assert_eq!(asked(&s, b"/api/v2/versions", b"", acl::bits::NODES).code, status::OK);
        assert_eq!(asked(&s, b"/api/v3/versions", b"", all & !acl::bits::NODES).code, status::OK);
        assert_eq!(asked(&s, b"/api/v3/versions", b"", 0).code, status::OK);
    }

    /// `/api/v2/nodes` and `/api/v3/nodes` (`api_v2_nodes()`, the NODES and NODES_INFO modes): the hosts with their
    /// version, labels, system info and state, then `health` and `capabilities`, between `api` and `timings`; no
    /// `versions` and no `agents`. With `options=mcp` the host is in its MCP form and `api` and `timings` are left
    /// out. Both rows ask the client for the nodes feature, and for that alone. With health on, the host's alerts
    /// are counted under their own status as each request finds them.
    #[test]
    fn nodes_are_routed_in_v2_and_v3() {
        use netdata_agent_health::alert::Status;
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        let s = shared();
        let all = acl::bits::ALL_LISTENER_FEATURES;
        let text = |path: &[u8], query: &[u8]| {
            let r = asked(&s, path, query, all);
            let shown = String::from_utf8_lossy(path).into_owned();
            assert_eq!((r.code, r.content_type), (status::OK, ContentType::ApplicationJson), "{shown}");
            assert!(r.no_cacheable, "{shown}");
            String::from_utf8(r.body).unwrap()
        };
        let last = r#"{"name":"dyncfg","version":2,"enabled":true}]}]"#;
        for path in [&b"/api/v2/nodes"[..], b"/api/v3/nodes"] {
            let shown = String::from_utf8_lossy(path).into_owned();
            assert_eq!(members(&text(path, b"")), ["api", "nodes", "timings"], "{shown}");
            let body = text(path, b"options=minify");
            let head = concat!(
                r#"{"api":2,"nodes":[{"mg":"0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e","nm":"box","ni":0,"v":"v0","#,
                r#""labels":{"#
            );
            assert!(body.starts_with(head), "{shown}: {body}");
            let info = r#""state":"reachable","health":{"status":"disabled"},"capabilities":[{"name":"proto","#;
            assert!(body.contains(info), "{shown}: {body}");
            assert!(body.contains(&format!(r#"{last},"timings":{{"#)), "{shown}: {body}");

            let mcp = text(path, b"options=mcp|minify");
            let head = concat!(
                r#"{"nodes":[{"machine_guid":"0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e","hostname":"box","#,
                r#""relationship":"localhost","connected":true,"v":"v0","labels":{"#
            );
            assert!(mcp.starts_with(head), "{shown}: {mcp}");
            assert!(mcp.trim_end().ends_with(&format!("{last}}}")), "{shown}: {mcp}");

            let denied = server::permission_denied_acl();
            let r = asked(&s, path, b"", all & !acl::bits::NODES);
            assert_eq!((r.code, &r.body), (denied.code, &denied.body), "{shown}");
            assert_eq!(asked(&s, path, b"", acl::bits::NODES).code, status::OK, "{shown}");
        }

        // health on: `online`, with the host's one alert under the status it has at the request
        let host = s.hosts.localhost();
        host.set_health_enabled(true);
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "t",
            id: "c",
            name: None,
            family: Some("f"),
            context: Some("t.ctx"),
            title: "T",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: netdata_agent_rrd::mode::DbMode::Ram,
            history_entries: 5,
            page_size: 4096,
        });
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("a.conf");
        std::fs::write(&rules, "template: a\n on: t.ctx\n every: 10s\n calc: 1\n").unwrap();
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(netdata_agent_health::readfile::health_readfile(&s.health, rules.as_os_str().as_bytes(), false));
        }
        s.health.host_link(host, &|| 1_700_000_000, &|| true);
        let health = || {
            let body = text(b"/api/v3/nodes", b"options=minify");
            let (at, end) = (body.find(r#""health":{"#).unwrap(), body.find(r#","capabilities""#).unwrap());
            body[at..end].to_owned()
        };
        let counted = |status: &str, counts: [u32; 5]| {
            let [critical, warning, clear, undefined, uninitialized] = counts;
            format!(
                concat!(
                    r#""health":{{"status":"{}","alerts":{{"critical":{},"warning":{},"clear":{},"undefined":{},"#,
                    r#""uninitialized":{}}}}}"#
                ),
                status, critical, warning, clear, undefined, uninitialized
            )
        };
        // its chart was never collected: the alert counts nowhere
        assert_eq!(health(), counted("online", [0, 0, 0, 0, 0]));
        chart.update_collection(|collection| collection.last_collected = (5, 0));
        assert_eq!(health(), counted("online", [0, 0, 0, 0, 1]));
        let alert = s.health.host(host).unwrap().chart_alerts(&chart).pop().expect("the alert");
        for (status, counts) in [
            (Status::Critical, [1, 0, 0, 0, 0]),
            (Status::Warning, [0, 1, 0, 0, 0]),
            (Status::Clear, [0, 0, 1, 0, 0]),
            (Status::Undefined, [0, 0, 0, 1, 0]),
            (Status::Removed, [0, 0, 0, 0, 0]),
        ] {
            let mut run = alert.run();
            run.status = status;
            alert.publish(&run, None);
            assert_eq!(health(), counted("online", counts), "{status:?}");
        }
        // a chart that waits for its alerts: `initializing`, still with the counts as they are
        let mut run = alert.run();
        run.status = Status::Warning;
        alert.publish(&run, None);
        host.raise_pending_flags(netdata_agent_rrd::host::pending_flags::HEALTH_INITIALIZATION);
        assert_eq!(health(), counted("initializing", [0, 1, 0, 0, 0]));
        let body = text(b"/api/v3/nodes", b"options=minify");
        assert!(body.contains(r#"{"name":"health","version":2,"enabled":true}"#), "{body}");
    }

    /// Health on for localhost, with the chart `t.c` of `t.ctx` and three rules: `a_first` (of type `System`) and
    /// `a_second` are linked on the chart, `a_third` (of type `System` too) is on a context no chart has. Returns the
    /// directory of the rules' file, the chart, and its two alerts in link order.
    fn two_alerts(
        s: &Shared,
    ) -> (tempfile::TempDir, Arc<netdata_agent_rrd::chart::Chart>, Vec<Arc<netdata_agent_health::alert::Alert>>) {
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        let host = s.hosts.localhost();
        host.set_health_enabled(true);
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "t",
            id: "c",
            name: None,
            family: Some("f"),
            context: Some("t.ctx"),
            title: "T",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: netdata_agent_rrd::mode::DbMode::Ram,
            history_entries: 5,
            page_size: 4096,
        });
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        chart.update_collection(|collection| collection.last_collected = (5, 0));
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("a.conf");
        let text_of_rules = concat!(
            "template: a_first\n on: t.ctx\n type: System\n every: 10s\n calc: 1\n\n",
            "template: a_second\n on: t.ctx\n every: 10s\n calc: 1\n\n",
            "template: a_third\n on: other.ctx\n type: System\n every: 10s\n calc: 1\n",
        );
        std::fs::write(&rules, text_of_rules).unwrap();
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(netdata_agent_health::readfile::health_readfile(&s.health, rules.as_os_str().as_bytes(), false));
        }
        s.health.host_link(host, &|| 1_700_000_000, &|| true);
        let linked = s.health.host(host).unwrap().chart_alerts(&chart);
        (dir, chart, linked)
    }

    /// `/api/v2/alerts` and `/api/v3/alerts` (`api_v2_alerts()`): the nodes, then what the options ask of the alerts,
    /// then the timings; no versions and no agents. Both rows ask the client for the alerts feature, and for that
    /// alone. With health off there is no alert and no rule: the arrays a request asks for are empty and the host is
    /// still listed. With health on, a host's alerts are kept by their name and by their published status; a summary
    /// has them by name in link order and counts them by type and by collecting module, with every rule's name as
    /// available whatever is kept; the instances are listed with the index their host has in `nodes`. A host
    /// without a kept alert is listed while no context pattern is given, and not with one.
    #[test]
    fn alerts_are_routed_in_v2_and_v3() {
        use netdata_agent_health::alert::Status;
        let s = shared();
        let all = acl::bits::ALL_LISTENER_FEATURES;
        let text = |path: &[u8], query: &[u8]| {
            let r = asked(&s, path, query, all);
            let shown = String::from_utf8_lossy(path).into_owned();
            assert_eq!((r.code, r.content_type), (status::OK, ContentType::ApplicationJson), "{shown}");
            assert!(r.no_cacheable, "{shown}");
            String::from_utf8(r.body).unwrap()
        };
        let node = r#""nodes":[{"mg":"0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e","nm":"box","ni":0}]"#;
        let no_summary = concat!(
            r#""alerts":[],"alerts_by_type":[],"alerts_by_component":[],"alerts_by_classification":[],"#,
            r#""alerts_by_recipient":[],"alerts_by_module":[]"#
        );
        for path in [&b"/api/v2/alerts"[..], b"/api/v3/alerts"] {
            let shown = String::from_utf8_lossy(path).into_owned();
            assert_eq!(members(&text(path, b"")), ["api", "nodes", "timings"], "{shown}");
            // the dashboard's three calls on an agent without health
            let body = text(path, b"options=summary,values,instances,minify&status=raised");
            let head = format!(r#"{{"api":2,{node},{no_summary},"alert_instances":[],"timings":{{"#);
            assert!(body.starts_with(&head), "{shown}: {body}");
            let head = format!(r#"{{"api":2,{node},{no_summary},"timings":{{"#);
            for query in [&b"options=minify,summary"[..], b"options=minify,summary&alert=nope"] {
                let body = text(path, query);
                assert!(body.starts_with(&head), "{shown}: {body}");
            }
            let body = text(path, b"options=minify,values");
            let head = format!(r#"{{"api":2,{node},"alert_instances":[],"timings":{{"#);
            assert!(body.starts_with(&head), "{shown}: {body}");

            let denied = server::permission_denied_acl();
            let r = asked(&s, path, b"", all & !acl::bits::ALERTS);
            assert_eq!((r.code, &r.body), (denied.code, &denied.body), "{shown}");
            assert_eq!(asked(&s, path, b"", acl::bits::ALERTS).code, status::OK, "{shown}");
        }
        // the echo: `config` is stripped; the last `status` replaces the first, a word that is no status is
        // ignored, and each bit prints its first word; the name pattern as it was given
        let query = b"options=debug,config,summary&status=active,clear&status=warning|bogus&alert=a*";
        let echo: String = text(b"/api/v2/alerts", query).split_whitespace().collect();
        assert!(echo.contains(r#""mode":["nodes","alerts"],"options":["debug","summary"],"#), "{echo}");
        let selectors = concat!(
            r#""selectors":{"nodes":null,"contexts":null,"#,
            r#""alerts":{"status":["warning"],"alert":"a*","transition":null}},"filters":{"#
        );
        assert!(echo.contains(selectors), "{echo}");
        let body = text(b"/api/v2/alerts", b"options=debug&status=raised,critical");
        let echo: String = body.split_whitespace().collect();
        let alerts = r#""alerts":{"status":["raised","critical"],"alert":null,"transition":null}}"#;
        assert!(echo.contains(alerts), "{echo}");

        // health on: two rules on one chart, and one more that no chart takes
        let (_rules, _chart, linked) = two_alerts(&s);
        let names: Vec<String> = linked.iter().map(|a| String::from_utf8_lossy(a.name()).into_owned()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"a_first".to_owned()) && names.contains(&"a_second".to_owned()), "{names:?}");
        for alert in &linked {
            let mut run = alert.run();
            run.status = if alert.name() == b"a_first" { Status::Warning } else { Status::Clear };
            alert.publish(&run, None);
        }
        let between = |body: &str, from: &str, to: &str| {
            let at = body.find(from).unwrap_or_else(|| panic!("no {from} in {body}"));
            let end = at + body[at..].find(to).unwrap_or_else(|| panic!("no {to} after {from} in {body}"));
            body[at..end].to_owned()
        };
        let kept = |query: &[u8]| {
            let body = text(b"/api/v3/alerts", query);
            let alerts = between(&body, r#""alerts":["#, r#","alerts_by_type""#);
            names.iter().filter(|name| alerts.contains(&format!(r#""nm":"{name}""#))).cloned().collect::<Vec<_>>()
        };
        // every alert, in link order; then by status and by name
        assert_eq!(kept(b"options=minify,summary"), names);
        assert_eq!(kept(b"options=minify,summary&status=warning"), ["a_first"]);
        assert_eq!(kept(b"options=minify,summary&status=raised"), ["a_first"]);
        assert_eq!(kept(b"options=minify,summary&status=clear"), ["a_second"]);
        assert_eq!(kept(b"options=minify,summary&status=clear,active").len(), 2);
        assert!(kept(b"options=minify,summary&status=critical").is_empty());
        assert_eq!(kept(b"options=minify,summary&alert=a_second"), ["a_second"]);
        assert_eq!(kept(b"options=minify,summary&alert=a_s*|nope"), ["a_second"]);
        assert!(kept(b"options=minify,summary&alert=A_SECOND").is_empty());

        // the summary's counts, and the groupings: the type of the kept alerts and of every rule's name (two of the
        // three rules have it), the charts' collecting module (never a rule's)
        let body = text(b"/api/v3/alerts", b"options=minify,summary");
        let first = between(&body, r#"{"ati":"#, r#","ctx":"#);
        // (the first entry is the first alert in link order: the warning one or the clear one)
        let (wr, cl) = if names[0] == "a_first" { (1, 0) } else { (0, 1) };
        let counts = format!(r#""cr":0,"wr":{wr},"cl":{cl},"er":0,"in":1,"nd":1,"cfg":1"#);
        let head = format!(r#"{{"ati":0,"ni":[0],"nm":"{}","#, names[0]);
        assert!(first.starts_with(&head) && first.ends_with(&counts), "{first}");
        let by_type = between(&body, r#""alerts_by_type":"#, r#","alerts_by_component""#);
        let system = concat!(
            r#""alerts_by_type":[{"name":"System","cr":0,"wr":1,"cl":0,"er":0,"running":1,"running_silent":0,"#,
            r#""available":2}]"#
        );
        assert_eq!(by_type, system);
        let by_module = between(&body, r#""alerts_by_module":"#, r#","timings""#);
        let module = concat!(
            r#""alerts_by_module":[{"name":"[none]","cr":0,"wr":1,"cl":1,"er":0,"running":2,"#,
            r#""running_silent":0}]"#
        );
        assert_eq!(by_module, module);
        // nothing kept: the rules' names are still available, and the host is still listed
        let body = text(b"/api/v3/alerts", b"options=minify,summary&status=critical");
        let head = format!(r#"{{"api":2,{node},"alerts":[],"alerts_by_type":[{{"name":"System","cr":0,"#);
        assert!(body.starts_with(&head), "{body}");
        assert!(body.contains(r#""running":0,"running_silent":0,"available":2}],"alerts_by_component":"#), "{body}");
        assert!(body.contains(r#","alerts_by_module":[],"timings":{"#), "{body}");
        // with a context pattern a host is listed only for a context that has a kept alert
        let listed = |query: &[u8]| text(b"/api/v3/alerts", query).contains(node);
        assert!(listed(b"options=minify,summary&scope_contexts=t.ctx"));
        assert!(!listed(b"options=minify,summary&scope_contexts=t.ctx&status=critical"));
        assert!(!listed(b"options=minify,summary&scope_contexts=other.ctx"));
        assert!(!listed(b"options=minify,summary&contexts=t.ctx&alert=nope"));

        // the instances: the index of the host, the alert, its chart; with `instances` its status and the rule's part
        let body = text(b"/api/v3/alerts", b"options=minify,values&status=warning");
        let instance = r#""alert_instances":[{"ni":0,"nm":"a_first","ch":"t.c","ch_n":"t.c","v":"#;
        assert!(body.starts_with(&format!(r#"{{"api":2,{node},{instance}"#)), "{body}");
        // (the one kept alert is the summary's first entry, whatever its place among the chart's alerts)
        let body = text(b"/api/v3/alerts", b"options=minify,summary,instances&status=warning");
        let instance = between(&body, r#""alert_instances":[{"#, r#""info":"#);
        assert!(instance.starts_with(r#""alert_instances":[{"ati":0,"ni":0,"gi":"#), "{instance}");
        let tail = r#""nm":"a_first","ctx":"t.ctx","ch":"t.c","ch_n":"t.c","st":"WARNING","fami":"f","#;
        assert!(instance.ends_with(tail), "{instance}");

        // `options=mcp`: neither `api` nor `timings`; headers and rows, an instance with its host's name, and a
        // limit that cuts the rows says so
        let mcp = ["nodes", "all_alerts_header", "all_alerts", "alert_instances_header", "alert_instances"];
        assert_eq!(members(&text(b"/api/v3/alerts", b"options=mcp,summary,values")), mcp);
        let body = text(b"/api/v3/alerts", b"options=mcp,minify,summary,values&status=warning");
        assert!(body.contains(r#""all_alerts":[["a_first","#), "{body}");
        assert!(body.contains(r#""alert_instances":[["a_first","box","t.c","#), "{body}");
        let body = text(b"/api/v3/alerts", b"options=mcp,minify,summary&cardinality=1");
        let cut = concat!(
            r#""__all_alerts_info__":{"status":"truncated","total_alerts":2,"shown_alerts":1,"#,
            r#""cardinality_limit":1}"#
        );
        assert!(body.contains(cut), "{body}");

        // `transition=`: a text that is no UUID, and an agent without a database, answer 404 with nothing
        for query in [&b"transition=x&options=summary"[..], b"transition=7a7a7a7a-7a7a-7a7a-7a7a-7a7a7a7a7a7a"] {
            let r = asked(&s, b"/api/v3/alerts", query, all);
            let shown = String::from_utf8_lossy(query).into_owned();
            let nothing = (status::NOT_FOUND, ContentType::TextPlain, 0);
            assert_eq!((r.code, r.content_type, r.body.len()), nothing, "{shown}");
        }
    }

    /// `transition=` on the alerts routes (`rrdcontexts_v2_init_alert_dictionaries()`): the id of one transition of
    /// the alert log narrows the request to the alert it belongs to: its host becomes the node scope, its chart's
    /// context the context scope, its alarm id a filter. An id no entry has answers 404. An entry of an alarm that
    /// is gone, or on a context the host no longer has, keeps nothing, and then lists no host: the context scope is
    /// a pattern, so a host counts only for a kept alert. The id may come without its dashes.
    #[test]
    fn a_transition_narrows_an_alerts_request() {
        use netdata_agent_health::alert::Status;
        use netdata_agent_metadata::open::MetaDb;
        let dir = tempfile::tempdir().unwrap();
        let meta = Arc::new(MetaDb::open(dir.path(), &Default::default()).unwrap());
        let s = Shared { meta: Some(Arc::downgrade(&meta)), ..shared() };
        let all = acl::bits::ALL_LISTENER_FEATURES;
        let (_rules, _chart, linked) = two_alerts(&s);
        for alert in &linked {
            let mut run = alert.run();
            run.status = Status::Warning;
            alert.publish(&run, None);
        }
        let id_of = |name: &[u8]| linked.iter().find(|alert| alert.name() == name).expect("the alert").id;
        let host_id = crate::meta_store::host_id(s.hosts.localhost()).unwrap();
        let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        {
            // the log's rows, as health writes them: the transition 7a.. is of `a_second` on its chart's context,
            // 7b.. of an alarm that no longer exists, 7c.. of `a_first` on a context the host does not have
            let c = meta.lock();
            let entries = [
                (1, id_of(b"a_second"), "t.ctx", 0x7a_u8),
                (2, 4000, "t.ctx", 0x7b),
                (3, id_of(b"a_first"), "gone.ctx", 0x7c),
            ];
            for (log_id, alarm_id, context, byte) in entries {
                let log = format!(
                    "INSERT INTO health_log (health_log_id, host_id, alarm_id, name, chart, chart_context) VALUES \
                     ({log_id}, X'{}', {alarm_id}, 'a', 't.c', '{context}')",
                    hex(&host_id)
                );
                c.execute(&log, ()).unwrap();
                let detail = format!(
                    "INSERT INTO health_log_detail (health_log_id, unique_id, alarm_id, transition_id) VALUES \
                     ({log_id}, {log_id}, {alarm_id}, X'{}')",
                    hex(&[byte; 16])
                );
                c.execute(&detail, ()).unwrap();
            }
        }
        let asked_for = |transition: &str| {
            let query = format!("options=minify,summary&transition={transition}");
            let r = asked(&s, b"/api/v3/alerts", query.as_bytes(), all);
            (r.code, String::from_utf8(r.body).unwrap())
        };
        let node = r#""nodes":[{"mg":"0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e","nm":"box","ni":0}]"#;
        // the alert of the transition alone, though both alerts of the chart would be kept without it
        for transition in ["7a7a7a7a-7a7a-7a7a-7a7a-7a7a7a7a7a7a", "7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a"] {
            let (code, body) = asked_for(transition);
            assert_eq!(code, status::OK, "{transition}: {body}");
            let head = format!(r#"{{"api":2,{node},"alerts":[{{"ati":0,"ni":[0],"nm":"a_second","#);
            assert!(body.starts_with(&head), "{transition}: {body}");
            assert!(!body.contains(r#""nm":"a_first""#), "{transition}: {body}");
        }
        // an id no entry has
        let (code, body) = asked_for("7d7d7d7d-7d7d-7d7d-7d7d-7d7d7d7d7d7d");
        assert_eq!((code, body.as_str()), (status::NOT_FOUND, ""));
        // an alarm that is gone, and a context the host does not have: found, and nothing kept, so no host either
        for transition in ["7b7b7b7b-7b7b-7b7b-7b7b-7b7b7b7b7b7b", "7c7c7c7c-7c7c-7c7c-7c7c-7c7c7c7c7c7c"] {
            let (code, body) = asked_for(transition);
            assert_eq!(code, status::OK, "{transition}: {body}");
            assert!(body.starts_with(r#"{"api":2,"nodes":[],"alerts":[],"alerts_by_type":["#), "{transition}: {body}");
        }
    }

    /// `/api/v2/contexts` and `/api/v3/contexts` (`api_v2_contexts()`): the nodes, the contexts, the versions and the
    /// agent between `api` and `timings`; with `options=mcp` neither of those two, and an `info` text after the
    /// contexts. Both rows ask the client for the metrics feature, and for that alone. A host without a context is
    /// listed while nothing selects contexts. A scope selects contexts, `contexts=` filters none, a limit cuts
    /// them by both of its names, and a window lists a context only when the context's own retention meets it: a
    /// context that is no longer collected is left out of a later window though its host is online.
    #[test]
    fn contexts_are_routed_in_v2_and_v3() {
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        let s = shared();
        let all = acl::bits::ALL_LISTENER_FEATURES;
        let text = |path: &[u8], query: &[u8]| {
            let r = asked(&s, path, query, all);
            let shown = String::from_utf8_lossy(path).into_owned();
            assert_eq!((r.code, r.content_type), (status::OK, ContentType::ApplicationJson), "{shown}");
            assert!(r.no_cacheable, "{shown}");
            String::from_utf8(r.body).unwrap()
        };
        let node = concat!(
            r#""nodes":[{"mg":"0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e","nm":"box","ni":0,"#,
            r#""st":{"ai":0,"code":200,"msg":""}}]"#
        );
        for path in [&b"/api/v2/contexts"[..], b"/api/v3/contexts"] {
            let shown = String::from_utf8_lossy(path).into_owned();
            let top = ["api", "nodes", "contexts", "versions", "agents", "timings"];
            assert_eq!(members(&text(path, b"")), top, "{shown}");
            let body = text(path, b"options=minify");
            assert!(body.starts_with(&format!(r#"{{"api":2,{node},"contexts":{{}},"versions":{{"#)), "{shown}: {body}");
            let mcp = ["nodes", "contexts", "info", "versions", "agents"];
            assert_eq!(members(&text(path, b"options=mcp")), mcp, "{shown}");
            // a selector with no word in it is no pattern: the host without a context is still listed
            let body = text(path, b"options=minify&contexts=|");
            assert!(body.starts_with(&format!(r#"{{"api":2,{node},"contexts":{{}},"versions":{{"#)), "{shown}: {body}");

            let denied = server::permission_denied_acl();
            let r = asked(&s, path, b"", all & !acl::bits::METRICS);
            assert_eq!((r.code, &r.body), (denied.code, &denied.body), "{shown}");
            assert_eq!(asked(&s, path, b"", acl::bits::METRICS).code, status::OK, "{shown}");
        }

        // two contexts whose data ended 50 seconds ago and that nothing collects any more (as after a child's
        // disconnect: the worker's next cycle finds them not collected)
        let host = s.hosts.localhost();
        let now = netdata_agent_rrd::clock::now_realtime_s();
        for (id, context) in [("c", "t.ctx"), ("d", "u.ctx")] {
            let (chart, _) = host.charts().create(&ChartSpec {
                type_: "t",
                id,
                name: None,
                family: Some("f"),
                context: Some(context),
                title: "T",
                units: "u",
                plugin: "p",
                module: None,
                priority: 1000,
                update_every: 1,
                chart_type: ChartType::Line,
                mode: netdata_agent_rrd::mode::DbMode::Ram,
                history_entries: 3600,
                page_size: 4096,
            });
            let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
            for t in now - 100..=now - 50 {
                dim.store_metric(t as u64 * 1_000_000, 1.0, 0);
            }
        }
        host.contexts().process_queued();
        host.contexts().child_disconnected();
        host.contexts().worker_cycle();
        let contexts = |query: &str| {
            let body = text(b"/api/v2/contexts", format!("options=minify&{query}").as_bytes());
            let (at, end) = (body.find(r#""nodes":"#).unwrap(), body.find(r#","versions""#).unwrap());
            body[at..end].to_owned()
        };
        let one = |id: &str| {
            let state = host.contexts().get(id).expect("the context").state();
            assert!(state.first_time_s <= now - 99 && state.last_time_s == now - 50, "{state:?}");
            format!(
                r#""{id}":{{"family":"f","units":"u","priority":1000,"first_entry":{},"last_entry":{},"live":false}}"#,
                state.first_time_s, state.last_time_s
            )
        };
        let both = format!(r#"{node},"contexts":{{{},{}}}"#, one("t.ctx"), one("u.ctx"));
        assert_eq!(contexts(""), both);
        for limit in ["cardinality=1", "cardinality_limit=1"] {
            let cut = r#""__truncated__":{"total_contexts":2,"returned":1,"remaining":1}"#;
            assert_eq!(contexts(limit), format!(r#"{node},"contexts":{{{},{cut}}}"#, one("t.ctx")), "{limit}");
        }
        assert_eq!(contexts("contexts=nomatch"), both);
        // a selector with no word in it (only separators, a lone `!`) is no pattern, as C's NULL (`contexts=`
        // filters nothing here; the host without a context, above, holds it)
        for no_word in ["scope_contexts=|", "scope_nodes=,", "nodes=!"] {
            assert_eq!(contexts(no_word), both, "{no_word}");
        }
        assert_eq!(contexts("scope_contexts=u.ctx"), format!(r#"{node},"contexts":{{{}}}"#, one("u.ctx")));
        assert_eq!(contexts("scope_contexts=u.*"), format!(r#"{node},"contexts":{{{}}}"#, one("u.ctx")));
        let nothing = r#""nodes":[],"contexts":{}"#;
        assert_eq!(contexts("scope_contexts=nomatch"), nothing);
        assert_eq!(contexts(&format!("after={}&before={}", now - 80, now - 60)), both);
        assert_eq!(contexts(&format!("after={}&before={}", now - 10, now)), nothing);

        // the v1 list of the same host: a filter with no word in it lists what no filter lists
        let v1 = |query: &[u8]| {
            let body = String::from_utf8(asked(&s, b"/api/v1/contexts", query, all).body).unwrap();
            body[body.find("\"contexts\"").expect("the contexts member")..].to_owned()
        };
        assert!(v1(b"").contains("\"t.ctx\"") && v1(b"").contains("\"u.ctx\""), "{}", v1(b""));
        for no_word in [&b"chart_label_key=|"[..], b"chart_labels_filter=,", b"dimensions=,", b"dims=!"] {
            assert_eq!(v1(no_word), v1(b""), "{}", String::from_utf8_lossy(no_word));
        }
    }

    /// `/api/v1/alarm_variables`, `/api/v1/variable` and `/api/v3/variable`, and the alert members of the chart JSON,
    /// `/api/v1/charts` and `/api/v1/info`: C's error answers (as the C agent sent them), a host without alerts,
    /// then with one linked; a freed chart's alert goes where the database frees it.
    #[test]
    fn the_variable_endpoints_and_the_alert_members() {
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        let s = shared();
        let request = |path: &str, query: &str| {
            let mut req = Request::default();
            req.path = path.as_bytes().to_vec();
            req.url_as_received = path.as_bytes().to_vec();
            req.query = query.as_bytes().to_vec();
            let ctx = crate::access_log::RequestContext::default();
            let all = acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES;
            process_request(&req, b"", all, &s, Instant::now(), &ctx, &|_| false)
        };
        let body = |path: &str, query: &str| {
            let r = request(path, query);
            assert_eq!((r.code, r.content_type), (status::OK, ContentType::ApplicationJson), "{path}{query}");
            String::from_utf8(r.body).unwrap()
        };

        let recorded = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../health/tests/vectors/variables/off");
        for (file, path, query, code) in [
            ("055.body", "/api/v1/alarm_variables", "", status::BAD_REQUEST),
            ("056.body", "/api/v1/alarm_variables", "?chart=no<chart", status::NOT_FOUND),
            ("057.body", "/api/v1/variable", "?chart=hv.a", status::BAD_REQUEST),
            ("058.body", "/api/v1/variable", "?variable=a", status::BAD_REQUEST),
            ("059.body", "/api/v1/variable", "?chart=no<chart&variable=a", status::NOT_FOUND),
            ("059.body", "/api/v3/variable", "?chart=no<chart&variable=a", status::NOT_FOUND),
        ] {
            let r = request(path, query);
            assert_eq!((r.code, r.content_type), (code, ContentType::TextPlain), "{path}{query}");
            assert_eq!(r.body, std::fs::read(recorded.join(file)).unwrap(), "{path}{query}");
        }

        // the commands are for clients with the alerts ACL, whatever else they hold
        let with_acl = |path: &str, client_acl: u32| {
            let mut req = Request::default();
            req.path = path.as_bytes().to_vec();
            req.url_as_received = path.as_bytes().to_vec();
            let ctx = crate::access_log::RequestContext::default();
            process_request(&req, b"", client_acl, &s, Instant::now(), &ctx, &|_| false)
        };
        let all = acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES;
        let denied = server::permission_denied_acl();
        for path in ["/api/v1/alarm_variables", "/api/v1/variable", "/api/v3/variable"] {
            let r = with_acl(path, all & !acl::bits::ALERTS);
            assert_eq!((r.code, &r.body), (denied.code, &denied.body), "{path} without the alerts ACL");
            let r = with_acl(path, all & !acl::bits::METRICS);
            assert_eq!(r.code, status::BAD_REQUEST, "{path} without the metrics ACL");
        }

        // a chart with a variable, on a host without alerts
        let host = s.hosts.localhost();
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "t",
            id: "c",
            name: None,
            family: Some("f"),
            context: Some("t.ctx"),
            title: "T",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: netdata_agent_rrd::mode::DbMode::Ram,
            history_entries: 5,
            page_size: 4096,
        });
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        chart.set_variable("cv", 7.0);
        let variables = body("/api/v1/alarm_variables", "?chart=t.c");
        assert!(variables.contains("\"last_collected_t\":0,\n        \"cv\":7\n    }"), "{variables}");
        assert!(variables.contains("\"alerts\":{\n    }"), "{variables}");
        for path in ["/api/v1/variable", "/api/v3/variable"] {
            let trace = body(path, "?chart=t.c&variable=cv");
            assert!(trace.contains("\"found\":true,\n    \"value\":7,"), "{trace}");
            assert!(trace.contains("\"description\":\"chart variable\""), "{trace}");
        }
        let json = body("/api/v1/chart", "?chart=t.c");
        assert!(json.contains("\"chart_variables\":{\n        \"cv\":7\n    }"), "{json}");
        assert!(json.contains("\"alarms\":{\n    }"), "{json}");
        assert!(body("/api/v1/charts", "").contains("\"alarms_count\":0,"));
        assert!(body("/api/v1/info", "").contains("\"normal\":0,\n        \"warning\":0,\n        \"critical\":0"));

        // a rule, linked by the host's first pass
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("a.conf");
        std::fs::write(&rules, "template: a\n on: t.ctx\n every: 10s\n calc: $cv\n").unwrap();
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(netdata_agent_health::readfile::health_readfile(&s.health, rules.as_os_str().as_bytes(), false));
        }
        s.health.host_link(host, &|| 1_700_000_000, &|| true);
        let json = body("/api/v1/chart", "?chart=t.c");
        let alarm = [
            "\"alarms\":{",
            "        \"a\":{",
            "            \"id\":\"a\",",
            "            \"status\":\"UNINITIALIZED\",",
            "            \"units\":\"u\",",
            "            \"duration\":10",
            "        }",
            "    }",
        ];
        assert!(json.contains(&alarm.join("\n")), "{json}");
        assert!(body("/api/v1/charts", "").contains("\"alarms_count\":1,"));
        let variables = body("/api/v1/alarm_variables", "?chart=t.c");
        assert!(variables.contains("\"alerts\":{\n        \"a\":{\n            \"value\":null,"), "{variables}");
        // counted once its chart was collected
        assert!(body("/api/v1/info", "").contains("\"normal\":0,"));
        // the three alarm endpoints answer before the chart has data too, with no alarm
        assert!(body("/api/v1/alarms", "?all").ends_with("\"alarms\": {\n\n\t}\n}\n"));
        assert_eq!(body("/api/v1/alarm_count", "?status=uninitialized"), "[0]\n");
        chart.update_collection(|collection| collection.last_collected = (5, 0));
        assert!(body("/api/v1/info", "").contains("\"normal\":1,\n        \"warning\":0,\n        \"critical\":0"));
        // and once it has: the alarm is listed among all, not among the raised, and counted by its status
        let all = body("/api/v1/alarms", "?all");
        assert!(all.contains("\n\t\t\"t.c.a\": {\n\t\t\t\"id\": 1700000000,\n"), "{all}");
        assert!(all.contains("\t\t\t\"status\": \"UNINITIALIZED\",\n"), "{all}");
        assert!(body("/api/v1/alarms", "").ends_with("\"alarms\": {\n\n\t}\n}\n"));
        let values = body("/api/v1/alarms_values", "?all=true");
        assert!(values.contains("\t\t\"t.c.a\": {\n\t\t\t\"id\": 1700000000,\n\t\t\t\"value\":null,"), "{values}");
        assert_eq!(body("/api/v1/alarm_count", "?status=uninitialized"), "[1]\n");
        assert_eq!(body("/api/v1/alarm_count", "?status=uninitialized&context=t.ctx|t.ctx"), "[2]\n");
        assert_eq!(body("/api/v1/alarm_count", ""), "[0]\n");
        // the query engine sees the host's alerts through the view the daemon installs
        host.storage().set_alert_view(Arc::new(crate::health::View(Arc::clone(&s.health))));
        let view = host.storage().alert_view().expect("the view");
        let seen = view.chart_alerts(host, &chart);
        // not evaluated yet: below CLEAR, without a value, with the units its chart gave it
        assert!(seen.len() == 1 && seen[0].value.is_nan() && !seen[0].at_least_clear);
        let seen: Vec<_> = seen.into_iter().map(|a| (a.name, a.class, a.status_name, a.units)).collect();
        let other = netdata_agent_rrd::storage::AlertClass::Other;
        assert_eq!(seen, [(b"a".to_vec(), other, "UNINITIALIZED", b"u".to_vec())]);
        // evaluated to CLEAR: shown from that status up, with the value the alert published
        {
            let alert = s.health.host(host).unwrap().chart_alerts(&chart).pop().expect("the alert");
            let mut run = alert.run();
            run.status = netdata_agent_health::alert::Status::Clear;
            (run.value, run.last_status_change_value) = (7.0, 3.0);
            alert.publish(&run, None);
        }
        let seen = view.chart_alerts(host, &chart).pop().expect("the alert");
        let clear = netdata_agent_rrd::storage::AlertClass::Clear;
        assert_eq!((seen.at_least_clear, seen.value, seen.class, seen.status_name), (true, 7.0, clear, "CLEAR"));
        // one alert inserted, one entry logged (its link)
        let data = body("/api/v2/data", "?contexts=t.ctx&options=minify");
        assert!(data.contains("\"alerts_hard_hash\":1,\"alerts_soft_hash\":1"), "{data}");

        // the alert's badge through the route: its published status' color and its value with the chart's units.
        // With a refresh the reply may be cached and carries the handler's date, an expiry exactly that many
        // seconds later (C sets both from one reading of the clock), and the header line
        let has = |body: &[u8], needle: &[u8]| body.windows(needle.len()).any(|w| w == needle);
        let badge = request("/api/v1/badge.svg", "?chart=t.c&alarm=a&refresh=5");
        assert_eq!((badge.code, badge.content_type), (status::OK, ContentType::ImageSvgXml));
        assert!(!badge.no_cacheable);
        assert!(badge.date > 1_700_000_000 && badge.expires == badge.date + 5, "{} {}", badge.date, badge.expires);
        assert_eq!(badge.headers, b"Refresh: 5\r\n");
        assert!(has(&badge.body, b">7 u</text>") && has(&badge.body, b"fill=\"#4c1\""));
        assert!(has(&badge.body, b">a</text>"));
        let plain = request("/api/v3/badge.svg", "?chart=t.c&alarm=a");
        assert!(plain.no_cacheable && plain.headers.is_empty() && (plain.date, plain.expires) == (0, 0));
        // the chart's own value: it has no stored point, so its value is too old to ask for: an empty badge, not
        // to be cached, and no header whatever was asked
        let stale = request("/api/v1/badge.svg", "?chart=t.c&refresh=5");
        assert_eq!((stale.code, stale.content_type), (status::OK, ContentType::ImageSvgXml));
        assert!(stale.no_cacheable && stale.headers.is_empty() && (stale.date, stale.expires) == (0, 0));
        assert!(has(&stale.body, b">-</text>") && has(&stale.body, b">t.c</text>"));
        // a chart collected up to now is asked for its value: one query, counted as a badge's in the pulse counters
        // and not as data's or health's. Over the default window, which is relative, the body is not to be cached
        // and still carries an expiry that many seconds from now beside its header line; the date is not set
        {
            use netdata_agent_rrd::collection;
            use netdata_agent_rrd::pulse::QuerySource;
            use netdata_agent_rrd::upstream::BufferSource;
            let (fresh, _) = host.charts().create(&ChartSpec {
                type_: "t",
                id: "b",
                name: None,
                family: Some("f"),
                context: Some("t.other"),
                title: "T",
                units: "u",
                plugin: "p",
                module: None,
                priority: 1000,
                update_every: 1,
                chart_type: ChartType::Line,
                mode: netdata_agent_rrd::mode::DbMode::Ram,
                history_entries: 5,
                page_size: 4096,
            });
            let (dim, _) = fresh.dim_add("d", None, 1, 1, Algorithm::Absolute);
            let now = netdata_agent_rrd::clock::now_realtime_s();
            for (i, value) in [10, 20, 30, 40].into_iter().enumerate() {
                let at = (now - 3 + i as i64, 0);
                collection::next_usec_unfiltered(&fresh, at, 1_000_000);
                collection::set_value(&dim, at, value);
                collection::timed_done(host, &fresh, at, false, 3, BufferSource::Thread);
            }
            let counted = || {
                let queries = &s.hosts.storage().pulse().queries;
                let sources = [QuerySource::ApiBadge, QuerySource::ApiData, QuerySource::Health];
                sources.map(|source| queries.source(source).queries)
            };
            let before = counted();
            let value = request("/api/v1/badge.svg", "?chart=t.b&refresh=5&precision=0");
            let after = counted();
            assert_eq!((after[0] - before[0], after[1] - before[1], after[2] - before[2]), (1, 0, 0));
            assert_eq!((value.code, value.content_type), (status::OK, ContentType::ImageSvgXml));
            assert!(!has(&value.body, b">-</text>"), "a value is shown");
            assert_eq!(value.headers, b"Refresh: 5\r\n");
            let asked_at = netdata_agent_rrd::clock::now_realtime_s();
            assert!(value.no_cacheable && value.date == 0, "{}", value.date);
            assert!((now + 5..=asked_at + 5).contains(&value.expires), "{}", value.expires);
            // an alert's badge asks for nothing
            let before = counted();
            let _ = request("/api/v1/badge.svg", "?chart=t.c&alarm=a");
            assert_eq!(counted(), before);
        }

        // the chart's free reaches health through the database's hook (this fixture's localhost has a storage of its
        // own; the daemon's hosts share one)
        host.storage().set_health_hook({
            let health = Arc::clone(&s.health);
            let queue = crate::metasync::MetaQueue::unread().0;
            let env = crate::health::LiveEnv::new(Arc::clone(&s.hosts), Default::default(), None, queue);
            move |event| crate::health::database_event(&health, &env, event)
        });
        assert!(host.charts().free_if(&chart, |_| true));
        assert!(s.health.host(host).unwrap().alerts().is_empty());
        assert!(body("/api/v1/charts", "").contains("\"alarms_count\":0,"));
    }

    /// `/api/v1/alarm_log` and `/api/v2|v3/alert_config` over the metadata database. The alert log: always 200; an
    /// empty body without a database; the empty array for a host whose health never ran (its limit is 0); then the
    /// host's entries above `after`, which is read as C's `strtoul(.., 0)` reads it and of which the last one given
    /// counts, and of the chart named. A rule's configuration: 400 without `config`, 500 without a database, 404
    /// for a hash no rule has and for a text that is no hash, else the rule, by its hash in any case and without
    /// dashes too; the last `config` counts, and the name is case-sensitive.
    #[test]
    fn the_alert_log_and_a_rule_s_configuration_come_from_the_table() {
        use netdata_agent_metadata::health_log::EntryRow;
        use netdata_agent_metadata::open::MetaDb;
        let ask = |s: &Shared, path: &str, query: &str| {
            let mut req = Request::default();
            req.path = path.as_bytes().to_vec();
            req.url_as_received = path.as_bytes().to_vec();
            req.query = query.as_bytes().to_vec();
            let ctx = crate::access_log::RequestContext::default();
            let all = acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES;
            let reply = process_request(&req, b"", all, s, Instant::now(), &ctx, &|_| false);
            (reply.code, reply.content_type, String::from_utf8(reply.body).unwrap())
        };
        let (json, text) = (ContentType::ApplicationJson, ContentType::TextPlain);
        let no_config = "A config hash ID is required. Add ?config=UUID query param";

        // no database
        let s = shared();
        assert_eq!(ask(&s, "/api/v1/alarm_log", ""), (status::OK, json, String::new()));
        assert_eq!(ask(&s, "/api/v2/alert_config", ""), (status::BAD_REQUEST, text, no_config.to_owned()));
        let failed = (status::INTERNAL_SERVER_ERROR, text, "Failed to execute SQL query.".to_owned());
        assert_eq!(ask(&s, "/api/v2/alert_config", "?config=aa"), failed);

        // a database with one rule and one entry of localhost's
        let dir = tempfile::tempdir().unwrap();
        let meta = Arc::new(MetaDb::open(dir.path(), &Default::default()).unwrap());
        let s = Shared { meta: Some(Arc::downgrade(&meta)), ..shared() };
        let host = s.hosts.localhost();
        let host_id = crate::meta_store::host_id(host).unwrap();
        let mut rule = netdata_agent_health::prototype::Rule::default();
        rule.config.hash_id = [0xab; 16];
        rule.config.name = Some(b"an_alarm".to_vec());
        assert!(meta.store_alert_config(&netdata_agent_health::store::alert_hash_row(&rule)));
        let entry = EntryRow {
            unique_id: 5,
            alarm_id: 7,
            alarm_event_id: 1,
            config_hash_id: &[0xab; 16],
            transition_id: &[0x11; 16],
            updated_by_id: 0,
            updates_id: 0,
            when: 1_700_000_000,
            duration: 0,
            non_clear_duration: 0,
            flags: 1,
            exec_run_timestamp: 0,
            delay_up_to_timestamp: 1_700_000_000,
            name: Some(b"an_alarm"),
            chart: Some(b"t.c"),
            chart_context: Some(b"t.ctx"),
            chart_name: Some(b"t.c"),
            exec: None,
            recipient: None,
            units: Some(b"things"),
            info: None,
            summary: None,
            exec_code: 0,
            new_status: 1,
            old_status: 0,
            delay: 0,
            new_value: 12.0,
            old_value: f64::NAN,
            last_repeat: 0,
            global_id: 5,
        };
        assert!(meta.health_alarm_log_insert(&host.hostname(), &host_id, &entry, false, true));

        // health never ran for the host: its limit is 0
        assert_eq!(ask(&s, "/api/v1/alarm_log", ""), (status::OK, json, "\n    []\n".to_owned()));
        // its first pass sets the limit
        s.health.host_link(host, &|| 1_700_000_100, &|| true);
        let log = |query: &str| {
            let (code, content_type, body) = ask(&s, "/api/v1/alarm_log", query);
            assert_eq!((code, content_type), (status::OK, json), "{query}");
            body
        };
        let whole = log("");
        for member in ["\"unique_id\":5,", "\"name\":\"an_alarm\",", "\"status\":\"CLEAR\",", "\"value\":12,"] {
            assert!(whole.contains(member), "{member} in {whole}");
        }
        assert!(whole.contains("\"old_value_string\":\"-\",") && whole.contains("\"old_value\":null"), "{whole}");
        let empty = "\n    []\n";
        for (query, has_it) in [
            ("?after=4", true),
            ("?after=5", false),
            ("?after=0x4", true),
            ("?after=0x5", false),
            ("?after=04", true),
            ("?after=010", false),
            ("?after=9&after=4", true),
            ("?after=4&after=", true),
            ("?chart=t.c", true),
            ("?chart=t.other", false),
            ("?chart=t.c&after=5", false),
            ("?AFTER=5", true),
        ] {
            assert_eq!(log(query) != empty, has_it, "{query}");
        }

        let config = |version: &str, query: &str| ask(&s, &format!("/api/{version}/alert_config"), query);
        let missing = (status::NOT_FOUND, text, "Config is not found.".to_owned());
        assert_eq!(config("v2", "?config=5a1e0000-0000-4000-8000-00000000dead"), missing);
        assert_eq!(config("v3", "?config=nonsense"), missing);
        let hash = "abababab-abab-abab-abab-abababababab";
        let (upper, undashed) = (hash.to_uppercase(), hash.replace('-', ""));
        for (version, query) in [
            ("v2", format!("?config={hash}")),
            ("v3", format!("?config={upper}")),
            ("v2", format!("?config={undashed}")),
            ("v2", format!("?config=nonsense&config={hash}")),
            ("v3", format!("?config={hash}&config=")),
        ] {
            let (code, content_type, body) = config(version, &query);
            assert_eq!((code, content_type), (status::OK, json), "{query}: {body}");
            assert!(body.contains(&format!("\"config_hash_id\":\"{hash}\"")), "{query}: {body}");
            assert!(body.contains("\"name\":\"an_alarm\",") && body.contains("\"to\":\"root\","), "{body}");
        }
        for query in ["", "?config=", &format!("?CONFIG={hash}")] {
            assert_eq!(config("v2", query), (status::BAD_REQUEST, text, no_config.to_owned()), "{query}");
        }
    }

    #[test]
    fn api_routing_matches_c() {
        let s = shared();
        let cases: [(&[u8], u16, &[u8]); 10] = [
            (b"/api", status::BAD_REQUEST, b"Which API version?"),
            (
                b"/api/v9",
                status::NOT_FOUND,
                b"Unsupported API version: v9",
            ),
            (b"/api/v1", status::BAD_REQUEST, b"Which API command?"),
            (b"/api/v1/", status::BAD_REQUEST, b"Which API command?"),
            (
                b"/api/v1/nope/x",
                status::NOT_FOUND,
                b"Unsupported API command: nope&#x2F;x",
            ),
            (
                b"/api/v1/info/x",
                status::BAD_REQUEST,
                b"API command 'info' does not support subpaths.",
            ),
            (
                b"/api/v3/stream_info/",
                status::BAD_REQUEST,
                b"API command 'stream_info' does not support subpaths.",
            ),
            (
                b"/api/v3/stream_path/x",
                status::BAD_REQUEST,
                b"API command 'stream_path' does not support subpaths.",
            ),
            (
                b"/api/v3/functions/x",
                status::BAD_REQUEST,
                b"API command 'functions' does not support subpaths.",
            ),
            (
                b"/v1/v2/",
                status::BAD_REQUEST,
                b"Multiple dashboard versions given at the URL.",
            ),
        ];
        for (path, code, body) in cases {
            let r = route(&s, path);
            assert_eq!(
                (r.code, r.body.as_slice()),
                (code, body),
                "{}",
                String::from_utf8_lossy(path)
            );
        }
        assert_eq!(route(&s, b"//api/v1/info").code, status::OK);
        // strchr() finds the second slash at offset 0: the command name is empty.
        let r = route(&s, b"/api/v1//info");
        assert_eq!(
            (r.code, r.body.as_slice()),
            (
                status::NOT_FOUND,
                &b"Unsupported API command: &#x2F;info"[..]
            )
        );
    }

    #[test]
    fn host_switching_matches_c() {
        let s = shared();
        let cases: [(&[u8], u16, &[u8]); 8] = [
            (
                b"/host/other/api/v1/info",
                status::NOT_FOUND,
                b"This netdata does not maintain a database for host: other",
            ),
            (
                b"/host/",
                status::NOT_FOUND,
                b"This netdata does not maintain a database for host: ",
            ),
            (b"/host/box/api/v1/info", status::OK, b""),
            (
                b"/node/0F4B6E5C-1D2A-4B3C-9D8E-7F6A5B4C3D2E/api/v1/info",
                status::OK,
                b"",
            ),
            // C's other matches: the localhost alias, the nil node ID of an unclaimed host, and whatever
            // uuid_parse_flexi() accepts (32 hex digits, trailing text).
            (b"/host/localhost/api/v1/info", status::OK, b""),
            (
                b"/node/00000000-0000-0000-0000-000000000000/api/v1/info",
                status::OK,
                b"",
            ),
            (
                b"/host/0F4B6E5C1D2A4B3C9D8E7F6A5B4C3D2E/api/v1/info",
                status::OK,
                b"",
            ),
            (
                b"/host/0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2exyz/api/v1/info",
                status::OK,
                b"",
            ),
        ];
        for (path, code, body) in cases {
            let r = route(&s, path);
            assert_eq!(r.code, code, "{}", String::from_utf8_lossy(path));
            if !body.is_empty() {
                assert_eq!(r.body, body);
            }
        }
        let r = route(&s, b"/host/box");
        assert_eq!(
            (r.code, r.headers.as_slice()),
            (status::MOVED_PERM, &b"Location: box/\r\n"[..])
        );
    }

    /// `/api/v1/manage/health`: the path's two 404 texts, the ACL bit before the key, the key, and a request the
    /// silencers answer.
    #[test]
    fn the_management_route_asks_for_its_path_its_acl_bit_and_its_key() {
        let s = shared();
        let key = s.management_key.clone();
        let all = acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES;
        let ask = |path: &[u8], token: Option<&[u8]>, acl: u32| {
            let mut req = Request::default();
            req.path = path.to_vec();
            req.url_as_received = path.to_vec();
            req.query = b"?cmd=LIST".to_vec();
            req.headers.auth_token = token.map(<[u8]>::to_vec);
            let context = crate::access_log::RequestContext::default();
            let reply = process_request(&req, path, acl, &s, Instant::now(), &context, &|_| false);
            (reply.code, reply.content_type, reply.no_cacheable, String::from_utf8(reply.body).expect("a text"))
        };
        let plain = |code, text: &str| (code, ContentType::TextPlain, true, text.to_owned());
        let health = b"/api/v1/manage/health";

        let listed = ask(health, Some(&key), all);
        assert_eq!((listed.0, listed.1, listed.2), (status::OK, ContentType::ApplicationJson, true));
        assert_eq!(listed.3, "{\n\t\"all\": false,\n\t\"type\": \"None\",\n\t\"silencers\": []\n}\n");
        // the first `manage/health` of the path must end it, wherever it stands
        assert_eq!(ask(b"/api/v1/manage/x/manage/health", Some(&key), all), listed);
        let curently = "Invalid management request. Curently only 'health' is supported.";
        assert_eq!(ask(b"/api/v1/manage", Some(&key), all), plain(status::NOT_FOUND, curently));
        assert_eq!(ask(b"/api/v1/manage/other", Some(&key), all), plain(status::NOT_FOUND, curently));
        assert_eq!(
            ask(b"/api/v1/manage/health/more", Some(&key), all),
            plain(status::NOT_FOUND, "Invalid management request. Currently only 'health' is supported.")
        );
        // the key: `X-Auth-Token`, whole
        assert_eq!(ask(health, None, all), plain(status::FORBIDDEN, "Auth Error\n"));
        assert_eq!(ask(health, Some(b"another"), all), plain(status::FORBIDDEN, "Auth Error\n"));
        assert_eq!(ask(health, Some(&key[..35]), all), plain(status::FORBIDDEN, "Auth Error\n"));
        // the ACL bit comes before the path and the key
        let denied = plain(status::UNAVAILABLE_FOR_LEGAL_REASONS, "You need to be authorized to access this resource");
        assert_eq!(ask(health, Some(&key), all & !acl::bits::MANAGEMENT), denied);
        assert_eq!(ask(b"/api/v1/manage", None, all & !acl::bits::MANAGEMENT), denied);
        // not a command of the later versions
        assert_eq!(ask(b"/api/v2/manage/health", Some(&key), all).0, status::NOT_FOUND);
        assert_eq!(ask(b"/api/v3/manage/health", Some(&key), all).0, status::NOT_FOUND);
        // under a host's prefix the route is the same, and the host is not looked at
        assert_eq!(ask(b"/host/box/api/v1/manage/health", Some(&key), all), listed);
    }

    /// A request that changes the state, through the route: the reply is text, and the silencers' file is written.
    #[test]
    fn a_management_request_through_the_route_saves_the_silencers() {
        let dir = tempfile::tempdir().expect("a directory");
        let file = dir.path().join("health.silencers.json");
        let config = netdata_agent_health::config::HealthConfig {
            silencers_filename: file.clone().into_os_string().into_encoded_bytes(),
            ..Default::default()
        };
        let s = Shared { health: netdata_agent_health::Health::init(config, Box::new(|_| {})), ..shared() };
        let mut req = Request::default();
        req.path = b"/api/v1/manage/health".to_vec();
        req.url_as_received = req.path.clone();
        req.query = b"?cmd=SILENCE ALL&alarm=a".to_vec();
        req.headers.auth_token = Some(s.management_key.clone());
        let all = acl::bits::TRANSPORTS | acl::bits::ALL_LISTENER_FEATURES;
        let context = crate::access_log::RequestContext::default();
        let (reply, records) = netdata_agent_log::capture(|| {
            process_request(&req, &req.path, all, &s, Instant::now(), &context, &|_| false)
        });
        assert_eq!((reply.code, reply.content_type), (status::OK, ContentType::TextPlain));
        assert_eq!(reply.body, b"All alarm notifications are silenced\nAlarm selector added\n");
        let written = "{\n\t\"all\": true,\n\t\"type\": \"SILENCE\",\n\t\"silencers\": [\
                       \n\t\t{\n\t\t\t\"alarm\": \"a\"\n\t\t}\n\t]\n}\n";
        assert_eq!(std::fs::read_to_string(&file).expect("the file"), written);
        let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
        assert_eq!(messages, [format!("Silencer changes written to {}", file.display())]);
    }

    /// A registry request with its features, and `DNT: 1` as the server takes it under the policy.
    fn registry_request(shared: &Shared, path: &[u8], query: &[u8], features: u32, dnt: bool) -> Reply {
        let mut req = Request::default();
        req.path = path.to_vec();
        req.query = query.to_vec();
        req.url_as_received = [path, b"?", query].concat();
        req.headers.do_not_track = dnt;
        process_request(
            &req,
            path,
            acl::bits::TRANSPORTS | features,
            shared,
            Instant::now(),
            &crate::access_log::RequestContext::default(),
            &|_| false,
        )
    }

    /// `/api/v1/registry` with the registry disabled (D223): hello in full, the disabled document for the other four
    /// actions (asking for tracking), the 400 texts, the ACL and DNT gates in C's order; v1 only, no subpath.
    #[test]
    fn the_registry_answers_as_c_with_the_registry_disabled() {
        let s = shared();
        let all = acl::bits::ALL_LISTENER_FEATURES;
        let body = |r: &Reply| String::from_utf8_lossy(&r.body).into_owned();
        let hello = registry_request(&s, b"/api/v1/registry", b"action=hello", all, false);
        assert_eq!(
            (hello.code, hello.content_type, hello.tracking_required),
            (status::OK, ContentType::ApplicationJson, false)
        );
        let guid = "0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e";
        assert_eq!(
            body(&hello),
            format!(
                "{{\n    \"action\":\"hello\",\n    \"status\":\"ok\",\n    \"hostname\":\"box\",\n    \
                 \"machine_guid\":\"{guid}\",\n    \"agent\":{{\n        \"machine_guid\":\"{guid}\",\n        \
                 \"bearer_protection\":false\n    }},\n    \"cloud_status\":\"available\",\n    \
                 \"cloud_base_url\":\"https://app.netdata.cloud\",\n    \
                 \"registry\":\"https://registry.my-netdata.io\",\n    \"anonymous_statistics\":true,\n    \
                 \"X-Netdata-Auth\":true,\n    \"nodes\":[{{\n            \
                 \"machine_guid\":\"{guid}\",\n            \"hostname\":\"box\"\n        }}]\n}}\n"
            )
        );
        // the Cloud URL is the registry's copy: a later cloud.conf URL (a parent's NODE_ID) is not it until a claim
        // reload copies it again
        s.cloud_conf().set(netdata_agent_inicfg::SECTION_GLOBAL, "url", "https://other.invalid");
        assert_eq!(body(&registry_request(&s, b"/api/v1/registry", b"action=hello", all, false)), body(&hello));
        s.registry.update_cloud_base_url(&mut s.cloud_conf());
        let reloaded = body(&registry_request(&s, b"/api/v1/registry", b"action=hello", all, false));
        assert_eq!(reloaded, body(&hello).replace("https://app.netdata.cloud", "https://other.invalid"));
        s.cloud_conf().set(netdata_agent_inicfg::SECTION_GLOBAL, "url", "https://app.netdata.cloud");
        s.registry.update_cloud_base_url(&mut s.cloud_conf());
        // DNT: hello answers with the statistics off; every other action is refused before its parameters
        let dnt = registry_request(&s, b"/api/v1/registry", b"action=hello", all, true);
        assert!(body(&dnt).contains("\"anonymous_statistics\":false,"), "{}", body(&dnt));
        let dnt_text = "Your web browser is sending 'DNT: 1' (Do Not Track). The registry requires persistent cookies \
                        on your browser to work.";
        for query in [&b"action=search"[..], b"action=access&machine=m&url=u&name=n", b""] {
            let r = registry_request(&s, b"/api/v1/registry", query, all, true);
            assert_eq!((r.code, body(&r)), (status::BAD_REQUEST, dnt_text.to_owned()), "{query:?}");
        }

        // the four disabled documents, after each action's parameter check and before the URL's
        for (query, action) in [
            (&b"action=access&machine=m&url=bad&name=n"[..], "access"),
            (b"action=delete&machine=m&url=u&delete_url=d", "delete"),
            (b"action=search&for=m", "search"),
            (b"action=switch&machine=m&url=u&to=p", "switch"),
        ] {
            let r = registry_request(&s, b"/api/v1/registry", query, all, false);
            assert_eq!((r.code, r.content_type, r.tracking_required), (status::OK, ContentType::ApplicationJson, true));
            assert_eq!(
                body(&r),
                format!(
                    "{{\n    \"action\":\"{action}\",\n    \"status\":\"disabled\",\n    \"hostname\":\"box\",\n    \
                     \"machine_guid\":\"{guid}\",\n    \"registry\":\"https://registry.my-netdata.io\"\n}}\n"
                )
            );
        }
        // the 400s: C's texts (the default one names no `switch`), an action's own parameter only after the action
        for (query, text) in [
            (&b""[..], "Invalid registry request - you need to set an action: hello, access, delete, search"),
            (b"action=HELLO", "Invalid registry request - you need to set an action: hello, access, delete, search"),
            (b"name=n&action=access&machine=m&url=u", "Invalid registry Access request."),
            (b"action=delete&machine=m&url=u", "Invalid registry Delete request."),
            (b"action=search", "Invalid registry Search request."),
            (b"action=switch&machine=m&url=u", "Invalid registry Switch request."),
        ] {
            let r = registry_request(&s, b"/api/v1/registry", query, all, false);
            assert_eq!(
                (r.code, r.content_type, r.tracking_required),
                (status::BAD_REQUEST, ContentType::TextPlain, false)
            );
            assert_eq!(body(&r), text, "{query:?}");
        }

        // hello needs the dashboard's features, the rest the registry's, and that comes before DNT
        let denied = "You need to be authorized to access this resource";
        for (query, features, dnt) in [
            (&b"action=hello"[..], acl::bits::REGISTRY, false),
            (b"action=search&for=m", acl::bits::DASHBOARD, false),
            (b"", acl::bits::DASHBOARD, false),
            (b"action=search&for=m", acl::bits::DASHBOARD, true),
        ] {
            let r = registry_request(&s, b"/api/v1/registry", query, features, dnt);
            assert_eq!((r.code, body(&r)), (status::UNAVAILABLE_FOR_LEGAL_REASONS, denied.to_owned()), "{query:?}");
        }
        let hello = registry_request(&s, b"/api/v1/registry", b"action=hello", acl::bits::DASHBOARD, false);
        assert_eq!(hello.code, status::OK);
        let search = registry_request(&s, b"/api/v1/registry", b"action=search&for=m", acl::bits::REGISTRY, false);
        assert_eq!(search.code, status::OK);

        // v1 only, no subpath
        assert_eq!(registry_request(&s, b"/api/v3/registry", b"action=hello", all, false).code, status::NOT_FOUND);
        assert_eq!(registry_request(&s, b"/api/v1/registry/hello", b"", all, false).code, status::BAD_REQUEST);
    }

    /// Hello through a child: the header is the routed host's (its registry hostname, GUID and node id), `agent` is
    /// localhost's GUID and node id with the routed host's claim id, and `nodes[]` lists every host in creation order.
    #[test]
    fn the_registry_s_hello_names_the_routed_host_and_localhost_apart() {
        let s = shared();
        let localhost = Arc::clone(s.hosts.localhost());
        localhost.set_node_id([0x11; 16]);
        let mut info = localhost.info();
        info.hostname = "child".into();
        info.registry_hostname = "child-registry".into();
        let guid = "22222222-2222-4222-8222-222222222222";
        let child = s
            .hosts
            .find_or_create(guid, netdata_agent_rrd::mode::DbMode::Ram, || info, |_| {})
            .expect("created");
        child.set_node_id([0x33; 16]);
        child.set_claim_id_of_origin([0x44; 16]);
        let all = acl::bits::ALL_LISTENER_FEATURES;
        let r = registry_request(&s, b"/host/child/api/v1/registry", b"action=hello", all, false);
        let local = "0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e";
        let (n1, n3, c4) = (
            "11111111-1111-1111-1111-111111111111",
            "33333333-3333-3333-3333-333333333333",
            "44444444-4444-4444-4444-444444444444",
        );
        assert_eq!(
            String::from_utf8_lossy(&r.body),
            format!(
                "{{\n    \"action\":\"hello\",\n    \"status\":\"ok\",\n    \"hostname\":\"child-registry\",\n    \
                 \"machine_guid\":\"{guid}\",\n    \"node_id\":\"{n3}\",\n    \"agent\":{{\n        \
                 \"machine_guid\":\"{local}\",\n        \"node_id\":\"{n1}\",\n        \"claim_id\":\"{c4}\",\n        \
                 \"bearer_protection\":false\n    }},\n    \"cloud_status\":\"available\",\n    \
                 \"cloud_base_url\":\"https://app.netdata.cloud\",\n    \
                 \"registry\":\"https://registry.my-netdata.io\",\n    \"anonymous_statistics\":true,\n    \
                 \"X-Netdata-Auth\":true,\n    \"nodes\":[{{\n            \"machine_guid\":\"{local}\",\n            \
                 \"node_id\":\"{n1}\",\n            \"hostname\":\"box\"\n        }},{{\n            \
                 \"machine_guid\":\"{guid}\",\n            \"node_id\":\"{n3}\",\n            \
                 \"hostname\":\"child-registry\"\n        }}]\n}}\n"
            )
        );
    }
}
