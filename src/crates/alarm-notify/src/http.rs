//! Outbound HTTP, replacing the script's `docurl()` wrapper around curl.
//!
//! Wire compatibility matters more than elegance here: curl's defaults are part of
//! the contract every webhook integration was tested against. In particular `--data`
//! without an explicit header sends `application/x-www-form-urlencoded`, and several
//! senders rely on that, so `Body::Raw` with no content type reproduces it rather
//! than "fixing" it to `application/json`.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};

use crate::config::Config;

/// curl's default when the script did not override it.
const DEFAULT_CONNECT_TIMEOUT: u64 = 5;
/// Nothing in the script bounded the whole request, but the daemon kills the
/// notifier after 120s by default; a per-request ceiling keeps one dead endpoint
/// from starving the methods dispatched after it.
const DEFAULT_REQUEST_TIMEOUT: u64 = 30;

pub enum Body {
    None,
    /// A pre-rendered body. `content_type` of `None` reproduces curl's `--data`
    /// default of `application/x-www-form-urlencoded`.
    Raw {
        content_type: Option<String>,
        data: String,
    },
    /// `--data-urlencode` pairs.
    Form(Vec<(String, String)>),
    /// `--form-string` pairs (multipart/form-data).
    Multipart(Vec<(String, String)>),
}

pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub basic_auth: Option<(String, String)>,
    pub body: Body,
}

impl Request {
    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::POST, url)
    }

    pub fn put(url: impl Into<String>) -> Self {
        Self::new(Method::PUT, url)
    }

    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            basic_auth: None,
            body: Body::None,
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn basic_auth(mut self, user: impl Into<String>, password: impl Into<String>) -> Self {
        self.basic_auth = Some((user.into(), password.into()));
        self
    }

    pub fn json(mut self, body: impl Into<String>) -> Self {
        self.body = Body::Raw {
            content_type: Some("application/json".to_string()),
            data: body.into(),
        };
        self
    }

    /// `--data <body>` with no explicit content type.
    pub fn raw(mut self, body: impl Into<String>) -> Self {
        self.body = Body::Raw {
            content_type: None,
            data: body.into(),
        };
        self
    }

    pub fn text(mut self, body: impl Into<String>) -> Self {
        self.body = Body::Raw {
            content_type: Some("text/plain".to_string()),
            data: body.into(),
        };
        self
    }

    pub fn form(mut self, pairs: Vec<(String, String)>) -> Self {
        self.body = Body::Form(pairs);
        self
    }

    pub fn multipart(mut self, pairs: Vec<(String, String)>) -> Self {
        self.body = Body::Multipart(pairs);
        self
    }
}

/// Outcome of one request. `status` is `None` when the request never got a response.
pub struct Response {
    pub status: Option<u16>,
    pub body: String,
}

impl Response {
    pub fn is(&self, code: u16) -> bool {
        self.status == Some(code)
    }

    pub fn is_any(&self, codes: &[u16]) -> bool {
        self.status.is_some_and(|s| codes.contains(&s))
    }

    /// The value senders put in their log lines.
    pub fn code_str(&self) -> String {
        match self.status {
            Some(s) => s.to_string(),
            None => "none".to_string(),
        }
    }
}

pub struct HttpClient {
    client: Client,
    debug: bool,
}

impl HttpClient {
    pub fn new(cfg: &Config, debug: bool) -> Result<Self> {
        let opts = CurlOptions::parse(cfg.str("curl_options"));

        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(
                opts.connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            ))
            .timeout(Duration::from_secs(
                opts.max_time.unwrap_or(DEFAULT_REQUEST_TIMEOUT),
            ))
            .user_agent(concat!("netdata-alarm-notify/", env!("CARGO_PKG_VERSION")));

        if opts.insecure {
            tracing::warn!("curl_options requested insecure TLS; certificate validation is off");
            builder = builder
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }
        if let Some(proxy) = &opts.proxy {
            builder =
                builder.proxy(reqwest::Proxy::all(proxy).context("invalid proxy in curl_options")?);
        }

        Ok(Self {
            client: builder.build().context("could not build the HTTP client")?,
            debug,
        })
    }

    pub async fn send(&self, req: Request) -> Response {
        let mut builder = self.client.request(req.method.clone(), &req.url);

        let mut headers = HeaderMap::new();
        for (name, value) in &req.headers {
            match (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                (Ok(n), Ok(v)) => {
                    headers.insert(n, v);
                }
                _ => tracing::error!("dropping malformed HTTP header '{name}'"),
            }
        }

        match req.body {
            Body::None => {}
            Body::Raw { content_type, data } => {
                let ct =
                    content_type.unwrap_or_else(|| "application/x-www-form-urlencoded".to_string());
                if !headers.contains_key(CONTENT_TYPE) {
                    if let Ok(v) = HeaderValue::from_str(&ct) {
                        headers.insert(CONTENT_TYPE, v);
                    }
                }
                if self.debug {
                    tracing::debug!("request body: {data}");
                }
                builder = builder.body(data);
            }
            Body::Form(pairs) => {
                if self.debug {
                    tracing::debug!("request form fields: {:?}", field_names(&pairs));
                }
                // `.form()` matches what curl's `--data-urlencode` put on the wire:
                // a space as `+` and upper-case percent escapes. Verified by capturing
                // both implementations against the same endpoint.
                builder = builder.form(&pairs);
            }
            Body::Multipart(pairs) => {
                let mut form = reqwest::multipart::Form::new();
                for (k, v) in &pairs {
                    form = form.text(k.clone(), v.clone());
                }
                if self.debug {
                    tracing::debug!("request multipart fields: {:?}", field_names(&pairs));
                }
                builder = builder.multipart(form);
            }
        }

        if let Some((user, password)) = req.basic_auth {
            builder = builder.basic_auth(user, Some(password));
        }
        builder = builder.headers(headers);

        if self.debug {
            tracing::debug!("{} {}", req.method, redact_url(&req.url));
        }

        match builder.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if self.debug {
                    tracing::debug!("received HTTP {status}, body: {body}");
                }
                Response {
                    status: Some(status),
                    body,
                }
            }
            Err(e) => {
                tracing::error!("request to {} failed: {e}", redact_url(&req.url));
                Response {
                    status: None,
                    body: String::new(),
                }
            }
        }
    }
}

/// Field names only - values may be tokens.
fn field_names(pairs: &[(String, String)]) -> Vec<&str> {
    pairs.iter().map(|(k, _)| k.as_str()).collect()
}

/// A loggable form of a URL: scheme, host and port only.
///
/// For most of these services the webhook URL *is* the credential, and the secret is
/// in the path rather than the query string - Slack, Discord, MS Teams, RocketChat,
/// Flock, SIGNL4, ilert, Fleep, Kavenegar. The shell script never logged the URL at
/// all, so nothing beyond the endpoint's identity is kept.
pub fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(parsed) => {
            let mut out = String::new();
            out.push_str(parsed.scheme());
            out.push_str("://");
            if let Some(host) = parsed.host_str() {
                out.push_str(host);
            }
            if let Some(port) = parsed.port() {
                out.push(':');
                out.push_str(&port.to_string());
            }
            if parsed.path().len() > 1 || parsed.query().is_some() {
                out.push_str("/[REDACTED]");
            }
            out
        }
        // Unparseable, so no part of it can be assumed safe to keep.
        Err(_) => "[REDACTED_URL]".to_string(),
    }
}

/// The subset of `curl_options` that maps onto a native client.
#[derive(Debug, Default, PartialEq, Eq)]
struct CurlOptions {
    connect_timeout: Option<u64>,
    max_time: Option<u64>,
    insecure: bool,
    proxy: Option<String>,
}

impl CurlOptions {
    fn parse(raw: &str) -> Self {
        let mut opts = Self::default();
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        let mut i = 0;
        while i < tokens.len() {
            let t = tokens[i];
            i += 1;
            match t {
                "-k" | "--insecure" => opts.insecure = true,
                "--connect-timeout" => {
                    opts.connect_timeout = tokens.get(i).and_then(|v| v.parse().ok());
                    i += 1;
                }
                "--max-time" | "-m" => {
                    opts.max_time = tokens.get(i).and_then(|v| v.parse().ok());
                    i += 1;
                }
                "--proxy" | "-x" => {
                    opts.proxy = tokens.get(i).map(|v| v.to_string());
                    i += 1;
                }
                other => {
                    tracing::warn!(
                        "curl_options entry '{other}' is not supported by the native notifier and was ignored"
                    );
                    // A value may follow an unknown flag; skipping it would risk
                    // mis-parsing, so the value is reported too on the next pass.
                }
            }
        }
        opts
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
