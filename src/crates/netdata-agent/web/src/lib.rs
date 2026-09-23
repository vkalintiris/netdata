//! The HTTP/1.1 protocol core of the agent's web server, ported byte for byte from the C web server
//! (`src/web/server/web_client.c`, `src/web/api/http_header.c`, `src/libnetdata/url/url.c`,
//! `src/libnetdata/http/`). Sans-io: connections feed received bytes in and write the produced bytes out; the
//! event loops live in `netdata-agent-evloop` (decisions D5, D8).

#![forbid(unsafe_code)]

pub mod content_type;
pub mod request;
pub mod response;
pub mod status;
pub mod url;
