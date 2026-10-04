//! URL routing, ported from `web_client_process_request_from_web_server()`, `web_client_process_url()`,
//! `web_client_switch_host()` and `web_client_api_request()` in `src/web/server/web_client.c`, and
//! `web_client_api_request_vX()` in `src/web/api/web_api.c`.
//!
//! Not ported yet: `/mcp` and `/sse`, and the API commands other than `info`, `chart`, `charts`, `context`,
//! `contexts`, `data`, `dbengine_stats`, `function`, `functions`, `me`, `progress`, `stream_info` and `stream_path`.
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
    // alerts: the alert log's endpoints come with its tables
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
        name: "info",
        acl: acl::bits::NOCHECK,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::info(route, query),
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
        name: "data",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| data::v23(route, query, 3),
    },
    Command {
        name: "context",
        acl: acl::bits::METRICS,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |_, host, query| v1_contexts::context(host, query),
    },
    Command {
        name: "info",
        acl: acl::bits::NOCHECK,
        access: access::NONE,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::info(route, query),
    },
    Command {
        name: "stream_path",
        acl: acl::bits::NODES,
        access: access::ANONYMOUS_DATA,
        allow_subpaths: false,
        callback: |route, _, query| contexts_v2::stream_path(route, query),
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
    pub query: &'a [u8],
    /// `w->payload`: a POST or PUT body.
    pub payload: Option<&'a Payload>,
    /// `X-Forwarded-For` as received (cut at 45 bytes), for a call's source.
    pub forwarded_for: &'a [u8],
    /// `w->response.data` as the request left it: what was received, which a callback that returns before
    /// flushing it sends back.
    pub input: &'a [u8],
    /// `WEB_CLIENT_FLAG_PATH_IS_V0` .. `_V3`.
    pub version: Option<u8>,
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
        query: &req.query,
        payload: req.payload.as_ref(),
        forwarded_for: &req.headers.forwarded_for,
        input,
        version: None,
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
            grouping_windows: Default::default(),
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

        // the chart's free reaches health through the database's hook (this fixture's localhost has a storage of its
        // own; the daemon's hosts share one)
        host.storage().set_health_hook({
            let health = Arc::clone(&s.health);
            let env = crate::health::LiveEnv::new(Arc::clone(&s.hosts), Default::default(), false);
            move |event| crate::health::database_event(&health, &env, event)
        });
        assert!(host.charts().free_if(&chart, |_| true));
        assert!(s.health.host(host).unwrap().alerts().is_empty());
        assert!(body("/api/v1/charts", "").contains("\"alarms_count\":0,"));
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
}
