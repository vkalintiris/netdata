//! The proxy Netdata Cloud connections go through, ported from `aclk_get_proxy()` and its helpers
//! (`src/aclk/aclk_proxy.c`) and `cloud_config_proxy_get()` (`src/claim/cloud-conf.c`). C resolves it once per
//! process; the first reader at startup is the localhost labels' `_aclk_proxy` (decisions D49 point 7).

use std::sync::OnceLock;

use netdata_agent_inicfg::{Config, SECTION_CLOUD, SECTION_GLOBAL};
use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};

/// `ACLK_PROXY_TYPE` after resolution (`PROXY_TYPE_UNKNOWN` never survives it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyType {
    Socks5,
    Socks5h,
    Http,
    Disabled,
}

impl ProxyType {
    /// The `_aclk_proxy` label (`add_aclk_host_labels()`).
    pub fn label(self) -> &'static str {
        match self {
            ProxyType::Socks5 => "SOCKS5",
            ProxyType::Socks5h => "SOCKS5H",
            ProxyType::Http => "HTTP",
            ProxyType::Disabled => "none",
        }
    }

    /// `aclk_proxy_type_to_url()`.
    fn url(self) -> &'static str {
        match self {
            ProxyType::Socks5 => "socks5://",
            ProxyType::Socks5h => "socks5h://",
            ProxyType::Http => "http://",
            ProxyType::Disabled => "",
        }
    }

    /// The name the "using ... proxy" records print.
    fn record_name(self) -> &'static str {
        match self {
            ProxyType::Http => "HTTP",
            ProxyType::Socks5h => "SOCKS5H",
            _ => "SOCKS5",
        }
    }
}

const SEPARATOR: &str = "://";

/// `aclk_verify_proxy()`: the scheme of a proxy URL after leading spaces; `None` for `PROXY_TYPE_UNKNOWN`.
fn verify(proxy: &str) -> Option<ProxyType> {
    let proxy = proxy.trim_start_matches(' ');
    [ProxyType::Socks5, ProxyType::Socks5h, ProxyType::Http]
        .into_iter()
        .find(|t| proxy.starts_with(t.url()))
}

/// `safe_log_proxy_censor()`: the credentials between the scheme and the last `@` become `X`s.
fn censor(proxy: &str) -> String {
    let mut bytes = proxy.as_bytes().to_vec();
    let Some(at) = bytes.iter().rposition(|&c| c == b'@').filter(|&at| at > 0) else {
        return proxy.to_string();
    };
    let start = proxy.find(SEPARATOR).map_or(0, |i| i + SEPARATOR.len());
    for c in bytes.iter_mut().take(at).skip(start) {
        *c = b'X';
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `aclk_proxy_get_display()`: scheme and address, without credentials.
fn display(proxy: &str, proxy_type: ProxyType) -> String {
    let host = match (proxy.rfind('@'), proxy.find(SEPARATOR)) {
        (Some(at), _) => &proxy[at + 1..],
        (None, Some(sep)) => &proxy[sep + SEPARATOR.len()..],
        (None, None) => proxy,
    };
    format!("{}{host}", proxy_type.url())
}

fn log_using(proxy: &str, proxy_type: ProxyType, source: &str) {
    nd_log!(
        Source::Daemon,
        Priority::Info,
        "ACLK: using {} proxy {} ({}, from {source})",
        proxy_type.record_name(),
        display(proxy, proxy_type),
        if proxy.contains('@') {
            "with credentials"
        } else {
            "without credentials"
        }
    );
}

/// `safe_log_proxy_error()`.
fn log_error(text: &str, proxy: &str) {
    netdata_log_error!("{text} Provided Value:\"{}\"", censor(proxy));
}

/// `cloud_config_proxy_get()`: cloud.conf's `[global] proxy` (default `env`), unless netdata.conf still has the
/// older `[cloud] proxy`, which wins and is copied to cloud.conf; otherwise netdata.conf gets cloud.conf's value.
/// Returns the proxy and whether netdata.conf set it explicitly.
fn configured(netdata: &mut Config, cloud: &mut Config) -> (String, &'static str, bool) {
    let text = |v: Vec<u8>| String::from_utf8_lossy(&v).into_owned();
    let proxy = text(
        cloud
            .get(SECTION_GLOBAL, "proxy", Some("env"))
            .unwrap_or_default(),
    );
    if netdata.exists(SECTION_CLOUD, "proxy") {
        let proxy = text(
            netdata
                .get(SECTION_CLOUD, "proxy", Some(&proxy))
                .unwrap_or_default(),
        );
        let proxy = text(cloud.set(SECTION_GLOBAL, "proxy", &proxy));
        (proxy, "netdata.conf [cloud]", true)
    } else {
        netdata.set(SECTION_CLOUD, "proxy", &proxy);
        (proxy, "cloud.conf", false)
    }
}

/// `check_environment_proxy()`: `http_proxy`, else `https_proxy`.
fn from_environment() -> Option<ProxyType> {
    let (var, value) = ["http_proxy", "https_proxy"].into_iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| (var, v))
    })?;
    match verify(&value) {
        Some(proxy_type) => {
            log_using(&value, proxy_type, &format!("environment variable '{var}'"));
            Some(proxy_type)
        }
        None => {
            log_error(
                &format!(
                    "Environment var '{var}' defined but of unknown format '{value}'. Supported syntax: \
                     'http://[user:pass@]host:port' or 'socks5[h]://[user:pass@]host:port'."
                ),
                &value,
            );
            None
        }
    }
}

/// `aclk_lws_wss_get_proxy_setting()`.
fn resolve(netdata: &mut Config, cloud: &mut Config) -> ProxyType {
    let (proxy, source, explicit) = configured(netdata, cloud);
    if proxy.is_empty() || proxy == "none" {
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "ACLK: proxy is {}, will connect directly without proxy.",
            if proxy.is_empty() {
                "not configured"
            } else {
                "set to 'none'"
            }
        );
        return ProxyType::Disabled;
    }
    if proxy == "env" {
        return from_environment().unwrap_or_else(|| {
            if explicit {
                nd_log!(
                    Source::Daemon,
                    Priority::Warning,
                    "ACLK: proxy is explicitly set to 'env' but neither 'http_proxy' nor 'https_proxy' environment \
                     variables are set. Will connect directly without proxy."
                );
            }
            ProxyType::Disabled
        });
    }
    match verify(&proxy) {
        Some(proxy_type) => {
            log_using(&proxy, proxy_type, source);
            proxy_type
        }
        None => {
            log_error(
                "Config var \"proxy\" defined but of unknown format. Supported syntax: \
                 \"http://[user:pass@]host:port\" or \"socks5[h]://[user:pass@]host:port\".",
                &proxy,
            );
            ProxyType::Disabled
        }
    }
}

/// `aclk_get_proxy()`: resolved on the first call, the same afterwards.
pub fn get(netdata: &mut Config, cloud: &mut Config) -> ProxyType {
    static PROXY: OnceLock<ProxyType> = OnceLock::new();
    *PROXY.get_or_init(|| resolve(netdata, cloud))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_classified_censored_and_shown_as_c() {
        assert_eq!(verify("  http://h:1"), Some(ProxyType::Http));
        assert_eq!(verify("socks5h://h:1"), Some(ProxyType::Socks5h));
        assert_eq!(verify("socks5://h:1"), Some(ProxyType::Socks5));
        assert_eq!(verify("https://h:1"), None);
        assert_eq!(verify("   "), None);
        assert_eq!(censor("http://user:pass@h:1"), "http://XXXXXXXXX@h:1");
        assert_eq!(censor("user:pass@h:1"), "XXXXXXXXX@h:1");
        assert_eq!(censor("@h:1"), "@h:1");
        assert_eq!(censor("http://h:1"), "http://h:1");
        assert_eq!(
            display("http://user:pass@h:1", ProxyType::Http),
            "http://h:1"
        );
        assert_eq!(display("socks5://h:1", ProxyType::Socks5), "socks5://h:1");
    }

    #[test]
    fn netdata_conf_gets_cloud_confs_proxy_unless_it_has_one() {
        let (mut netdata, mut cloud) = (Config::default(), Config::default());
        assert_eq!(
            configured(&mut netdata, &mut cloud),
            ("env".into(), "cloud.conf", false)
        );
        assert!(netdata.exists(SECTION_CLOUD, "proxy"));
        let (mut netdata, mut cloud) = (Config::default(), Config::default());
        netdata.set(SECTION_CLOUD, "proxy", "none");
        assert_eq!(
            configured(&mut netdata, &mut cloud),
            ("none".into(), "netdata.conf [cloud]", true)
        );
        assert_eq!(
            cloud.get(SECTION_GLOBAL, "proxy", None),
            Some(b"none".to_vec())
        );
    }
}
