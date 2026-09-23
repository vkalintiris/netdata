//! URL routing, ported from `web_client_process_request_from_web_server()`, `web_client_process_url()`,
//! `web_client_switch_host()` and `web_client_api_request()` in `src/web/server/web_client.c`, and
//! `web_client_api_request_vX()` in `src/web/api/web_api.c`.
//!
//! Not ported yet: ACL and bearer checks (with the `[web]` section), `/mcp` and `/sse`, `/netdata.conf` (it needs
//! the config reads in C's order), and every API command other than `/api/v1/info`.

use std::sync::Arc;

use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::print_uuid_lower;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::request::Request;
use netdata_agent_web::status;

use crate::api;
use crate::server::{Reply, Shared};
use crate::static_file;

/// `FILENAME_MAX`: the path and filename copies are truncated to it.
pub const FILENAME_MAX: usize = 4096;

/// A host the request is routed to (`RRDHOST *`).
pub type Host = Arc<netdata_agent_rrd::host::Host>;

/// An API command: its name, whether it accepts a sub-path, and its handler (`struct web_api_command`).
type Command = (&'static str, bool, fn(&Route<'_>, &Host, &[u8]) -> Reply);

const API_V1: &[Command] = &[("info", false, |route, _, _| Reply {
    code: status::OK,
    content_type: ContentType::ApplicationJson,
    body: api::info_json(&route.shared.info),
    ..Reply::default()
})];
const API_V2: &[Command] = &[];
const API_V3: &[Command] = &[];

/// The per-request routing state (`WEB_CLIENT_FLAG_PATH_*`).
pub struct Route<'a> {
    pub shared: &'a Shared,
    pub url_as_received: &'a [u8],
    pub query: &'a [u8],
    /// `WEB_CLIENT_FLAG_PATH_IS_V0` .. `_V3`.
    pub version: Option<u8>,
    pub trailing_slash: bool,
    pub has_extension: bool,
}

/// The GET/POST/PUT/DELETE branch of `web_client_process_request_from_web_server()`.
pub fn process_request(req: &Request, shared: &Shared) -> Reply {
    let path = &req.path[..req.path.len().min(FILENAME_MAX)];
    let end = path.iter().position(|&c| c == b'?').unwrap_or(path.len());
    // The first byte is never inspected for a dot, as in C.
    let last_marker = (1..end)
        .rev()
        .map(|i| path[i])
        .find(|&c| c == b'/' || c == b'.');
    let mut route = Route {
        shared,
        url_as_received: &req.url_as_received,
        query: &req.query,
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

    /// `web_client_api_request()`: `/api/<version>/<command>`.
    fn api_request(&self, host: &Host, mut rest: Option<&[u8]>) -> Reply {
        let table = match strsep_skip(&mut rest, b"/") {
            b"" => return Reply::text(status::BAD_REQUEST, "Which API version?"),
            b"v3" => API_V3,
            b"v2" => API_V2,
            b"v1" => API_V1,
            other => return Reply::html(status::NOT_FOUND, "Unsupported API version: ", other),
        };
        let mut reply = self.api_command(host, rest.unwrap_or(b""), table);
        reply.no_cacheable = true;
        reply
    }

    /// `web_client_api_request_vX()`.
    fn api_command(&self, host: &Host, endpoint: &[u8], table: &[Command]) -> Reply {
        if endpoint.is_empty() {
            return Reply::text(status::BAD_REQUEST, "Which API command?");
        }
        let slash = endpoint.iter().position(|&c| c == b'/');
        let name = &endpoint[..slash.unwrap_or(endpoint.len())];
        let Some(&(name, allow_subpaths, handler)) =
            table.iter().find(|(n, _, _)| n.as_bytes() == name)
        else {
            let mut reply = Reply::text(status::NOT_FOUND, "Unsupported API command: ");
            netdata_agent_text::print::html_escape(&mut reply.body, endpoint);
            return reply;
        };
        if !allow_subpaths && slash.is_some() {
            return Reply::text(
                status::BAD_REQUEST,
                &format!("API command '{name}' does not support subpaths."),
            );
        }
        let query = self.query.strip_prefix(b"?").unwrap_or(self.query);
        handler(self, host, query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared {
            settings: netdata_agent_web::request::Settings {
                gzip: false,
                respect_do_not_track: false,
            },
            version: "v0",
            gzip_level: 3,
            info: api::Info {
                version: "v0",
                machine_guid: "0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e".into(),
                hostname: "box".into(),
            },
            web_dir: "/nonexistent-web-dir".into(),
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
                    },
                ),
            )),
        }
    }

    fn route(shared: &Shared, path: &[u8]) -> Reply {
        let mut req = Request::default();
        req.path = path.to_vec();
        req.url_as_received = path.to_vec();
        process_request(&req, shared)
    }

    #[test]
    fn api_routing_matches_c() {
        let s = shared();
        let cases: [(&[u8], u16, &[u8]); 7] = [
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
