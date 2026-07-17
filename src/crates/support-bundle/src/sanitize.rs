//! Single-pass line sanitizer: the port of the awk (POSIX) and PowerShell
//! sanitizers from the original scripts. Two passes over every line:
//!   pass 1 (always on): credential redaction
//!   pass 2 (default on): PII pseudonymization
//! Where the two script implementations diverged, the stricter (more
//! redacting) variant was kept; the divergence is noted inline.

use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

/// Values of keys whose punctuation-normalized name contains one of these are
/// redacted (substring match on the normalized key).
const SECRET_KEY_WORDS: &[&str] = &[
    "api key",
    "apikey",
    "token",
    "password",
    "passwd",
    "secret",
    "community",
    "bearer",
    "webhook",
    "license key",
    "auth",
    "credential",
    "cookie",
    "passphrase",
    "proxy user",
    "proxy pass",
    "username",
    "dsn",
    "private key",
    "access key",
    "session",
    "recipient",
    "account sid",
    "priv key",
    // compact/camelCase spellings of the two-word phrases above
    // (accessKey/access_key both normalize into these)
    "accesskey",
    "licensekey",
    "privatekey",
    "privkey",
    "proxyuser",
    "proxypass",
    "accountsid",
];

/// Short credential aliases that appear as real keys in collector configs
/// (python.d modules use `pass:`). Matched as WHOLE words of the normalized
/// key, never as substrings — `bypass`, `compass` and `pattern` must survive.
const SECRET_KEY_EXACT_WORDS: &[&str] = &["pass", "pwd", "pat"];

/// Keys ENDING in these words describe secrets rather than being secrets
/// ("bearer token protection", "api key file"). Exemption is decided by the
/// KEY, never the value: TOKEN=false and PASSWORD=/x must still be redacted.
const DIAGNOSTIC_NOUNS: &[&str] = &[
    "file",
    "path",
    "dir",
    "directory",
    "protection",
    "support",
    "mode",
    "level",
    "port",
    "timeout",
    "cookies",
    "secure",
    "log",
    "size",
    "options",
    // toggle suffixes: the value of "X enabled" is yes/no, never a secret
    "enabled",
    "disabled",
];

/// Pseudonym mappings past this cap get a constant non-correlating
/// placeholder so hostile high-cardinality input cannot grow memory or the
/// private map without bound.
const PSEUDONYM_CAP: usize = 4096;

pub const WITHHELD_NUL: &str = "[content withheld: file contains NUL bytes (binary or UTF-16?)]";
pub const WITHHELD_FAILED: &str =
    "[netdata-support-bundle] sanitization failed for this file - content withheld for safety";

/// The names that identify this host and the invoking user.
#[derive(Clone, Default)]
pub struct Identity {
    pub host_short: String,
    pub host_fqdn: String,
    pub run_user: String,
}

impl Identity {
    /// Apply the gating both scripts use: hostname dropped when it cannot be
    /// PII (localhost, too short); username dropped for service accounts.
    pub fn gated(host_short: &str, host_fqdn: &str, run_user: &str) -> Self {
        let gate_host = |h: &str| {
            if h.eq_ignore_ascii_case("localhost") || h.len() < 4 {
                String::new()
            } else {
                h.to_string()
            }
        };
        // Windows account names are case-insensitive; match accordingly
        let user_blocked = ["root", "netdata", "system", "administrator"];
        let run_user = if run_user.len() < 3
            || user_blocked.contains(&run_user.to_ascii_lowercase().as_str())
        {
            String::new()
        } else {
            run_user.to_string()
        };
        Identity {
            host_short: gate_host(host_short),
            host_fqdn: gate_host(host_fqdn),
            run_user,
        }
    }
}

/// The closed set of pseudonym kinds in the private map's TSV format.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum MapKind {
    Ip,
    Ip6,
    Fqdn,
    Host,
    User,
}

impl MapKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MapKind::Ip => "ip",
            MapKind::Ip6 => "ip6",
            MapKind::Fqdn => "fqdn",
            MapKind::Host => "host",
            MapKind::User => "user",
        }
    }
}

/// One pseudonym map row destined for the private `*.pseudonym-map.tsv`.
pub struct MapRow {
    pub kind: MapKind,
    pub real: String,
    pub pseudonym: String,
}

struct Regexes {
    uuid_section: Regex,
    json_quoted: Regex,
    json_scalar: Regex,
    json_container: Regex,
    short_secret: Regex,
    url_creds: Regex,
    go_dsn: Regex,
    jwt: Regex,
    bearer: Regex,
    basic: Regex,
    query_secret: Regex,
    argv_secret: Regex,
    yaml_block_header: Regex,
    pem_begin: Regex,
    pem_end: Regex,
    email: Regex,
    mac: Regex,
    ipv4: Regex,
    ip6_run: Regex,
    private_fqdn: Regex,
    destination_line: Regex,
    dest_pseudonym: Regex,
    dest_all_v4: Regex,
}

impl Regexes {
    fn new() -> Self {
        let n = |p: &str| Regex::new(p).expect("static sanitizer regex must compile");
        Regexes {
            uuid_section: n(
                r"^[ \t]*\[[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\][ \t]*$",
            ),
            // escape-aware string bodies: an escaped quote inside a secret
            // value must not terminate the match and leave a tail behind
            json_quoted: n(r#""((?:[^"\\]|\\.)+)"[ \t]*:[ \t]*"(?:[^"\\]|\\.)*""#),
            // union of the sh and ps1 scalar shapes (ps1 adds the leading '-')
            json_scalar: n(
                r#""((?:[^"\\]|\\.)+)"[ \t]*:[ \t]*(-?[0-9][0-9.eE+-]*|true|false|null)"#,
            ),
            // secret keys whose value is an object/array: the whole container
            // is redacted (across lines when needed)
            json_container: n(r#""((?:[^"\\]|\\.)+)"[ \t]*:[ \t]*[\[{]"#),
            // short credential aliases (pass/pwd/pat) mid-line, guarded by a
            // non-alphanumeric boundary so bypass/pattern/path never match
            short_secret: n(r#"(?i)((?:^|[^A-Za-z0-9])(?:pass|pwd|pat)[ ]?[=:][ ]?)([^&" \t\[]+)"#),
            url_creds: n(r"://[^:/@ \t]+:[^@ \t]+@"),
            go_dsn: n(r"\b\w+:[^@ \t]+@(tcp|unix)\("),
            // sh accepts 1+ chars per segment (stricter than ps1's {10,})
            jwt: n(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"),
            // the value must contain a digit: real tokens do, English prose
            // after "bearer" ("bearer token protection = no") does not
            bearer: n(r"[Bb]earer[ \t]+[A-Za-z._~+/=-]*[0-9][A-Za-z0-9._~+/=-]*"),
            basic: n(r"[Bb]asic[ \t]+[A-Za-z0-9+/=]{8,}"),
            query_secret: n(
                r#"([?&][A-Za-z0-9_.-]*(?:token|apikey|api_key|access_key|private_key|secret_key|password|passwd|secret|bearer|claim_token|claim_rooms|key|auth)=)[^&" \t]+"#,
            ),
            // combined argv/env + two-word-key rule (ps1 shape, case-insensitive
            // superset of the sh case-variant list). The captured key is only
            // the leading word-cluster, so a trailing diagnostic noun
            // (--token-file=/x) is NOT exempted here - over-redaction is the
            // safe direction; do not "fix" this into an under-redaction.
            argv_secret: n(
                r#"(?i)(([\w.-]*(?:token|password|passwd|secret|apikey|api_key|community|bearer)|(?:api|license|auth|access) key|proxy (?:user|pass|password)) ?[=:] ?)([^&" \t\[]+)"#,
            ),
            // tabs count as indentation; the indent indicator ("|2") and
            // chomping indicator ("|+") are valid in EITHER order per YAML,
            // so both "|2+" and "|+2" open a block
            yaml_block_header: n(
                r"^[ \t]*[A-Za-z0-9_. -]+:[ \t]*[|>](?:[0-9][+-]?|[+-][0-9]?)?[ \t]*$",
            ),
            // union: sh allows [A-Z ], ps1 [A-Z0-9 ]
            pem_begin: n(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY"),
            pem_end: n(r"-----END [A-Z0-9 ]*PRIVATE KEY"),
            email: n(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"),
            // union: ps1 adds '-' separated MACs and word boundaries
            mac: n(r"\b(?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}\b"),
            ipv4: n(r"\b[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\b"),
            ip6_run: n(r"[0-9A-Fa-f:]+"),
            private_fqdn: n(
                r"[A-Za-z0-9][A-Za-z0-9.-]*\.(?:internal|local|lan|corp|intranet|localdomain)",
            ),
            destination_line: n(r"^[ \t#;]*(proxy )?destination[ \t]*="),
            dest_pseudonym: n(r"^(?:ip|ip6|private-host)-[0-9]+$"),
            dest_all_v4: n(r"^[0-9.]+$"),
        }
    }
}

fn normalize_key(key: &str) -> String {
    key.replace(['-', '_'], " ")
        .to_ascii_lowercase()
        .trim_matches([' ', '#', '\t'])
        .to_string()
}

fn is_secret_key(key: &str) -> bool {
    let k = normalize_key(key);
    SECRET_KEY_WORDS.iter().any(|w| k.contains(w))
        || k.split(' ')
            .any(|tok| SECRET_KEY_EXACT_WORDS.contains(&tok))
}

fn is_diagnostic_key(key: &str) -> bool {
    let k = normalize_key(key);
    DIAGNOSTIC_NOUNS
        .iter()
        .any(|w| k == *w || k.ends_with(&format!(" {w}")))
}

pub struct Sanitizer {
    obfuscate: bool,
    id: Identity,
    rx: Regexes,
    ip_map: HashMap<String, String>,
    ip6_map: HashMap<String, String>,
    fq_map: HashMap<String, String>,
    host_map: HashMap<String, String>,
    user_map: HashMap<String, String>,
    // /home/<name> and /Users/<name> segments -> user-N
    home_user_map: HashMap<String, String>,
    rows: Vec<MapRow>,
    // longest-first replacement order for mapped hostnames, rebuilt only
    // when the map grows (never per line)
    fq_sorted: Vec<String>,
    fq_dirty: bool,
    // multi-line withholding state, reset per file
}

/// Multi-line withholding state (PEM blocks, YAML block scalars, JSON
/// containers). Owned by `sanitize_text` as a per-file local, so the
/// per-line `sanitize_line` API provably cannot carry block context across
/// calls — dropping it on a panic also resets it for free.
#[derive(Default)]
struct BlockState {
    in_pem: bool,
    in_yaml: bool,
    yaml_indent: usize,
    in_json: bool,
    json_depth: i32,
}

impl Sanitizer {
    pub fn new(obfuscate: bool, id: Identity) -> Self {
        Sanitizer {
            obfuscate,
            id,
            rx: Regexes::new(),
            ip_map: HashMap::new(),
            ip6_map: HashMap::new(),
            fq_map: HashMap::new(),
            host_map: HashMap::new(),
            user_map: HashMap::new(),
            home_user_map: HashMap::new(),
            rows: Vec::new(),
            fq_sorted: Vec::new(),
            fq_dirty: false,
        }
    }

    pub fn map_rows(&self) -> &[MapRow] {
        &self.rows
    }

    pub fn obfuscate(&self) -> bool {
        self.obfuscate
    }

    /// Pre-seed a child/mirrored node hostname so it pseudonymizes
    /// consistently in every file. Skips this host and unusable names.
    pub fn seed_fqdn(&mut self, host: &str) {
        let h = host.trim();
        // the replacer only handles ASCII; a non-ASCII seed would sit in the
        // private map without ever being applied
        if !h.is_ascii()
            || h.len() < 4
            || h.eq_ignore_ascii_case("localhost")
            || h.eq_ignore_ascii_case(&self.id.host_short)
            || h.eq_ignore_ascii_case(&self.id.host_fqdn)
        {
            return;
        }
        self.pseudo_fqdn(h);
    }

    fn pseudo_ip(&mut self, ip: &str) -> String {
        if ip.starts_with("127.") || ip == "0.0.0.0" || ip.starts_with("255.") {
            return ip.to_string();
        }
        if !self.ip_map.contains_key(ip) {
            if self.ip_map.len() >= PSEUDONYM_CAP {
                return "redacted-ip".to_string();
            }
            let p = format!("ip-{}", self.ip_map.len() + 1);
            self.ip_map.insert(ip.to_string(), p.clone());
            self.rows.push(MapRow {
                kind: MapKind::Ip,
                real: ip.to_string(),
                pseudonym: p,
            });
        }
        self.ip_map[ip].clone()
    }

    fn pseudo_ip6(&mut self, ip: &str) -> String {
        if !self.ip6_map.contains_key(ip) {
            if self.ip6_map.len() >= PSEUDONYM_CAP {
                return "redacted-ip6".to_string();
            }
            let p = format!("ip6-{}", self.ip6_map.len() + 1);
            self.ip6_map.insert(ip.to_string(), p.clone());
            self.rows.push(MapRow {
                kind: MapKind::Ip6,
                real: ip.to_string(),
                pseudonym: p,
            });
        }
        self.ip6_map[ip].clone()
    }

    fn pseudo_fqdn(&mut self, host: &str) -> String {
        if !self.fq_map.contains_key(host) {
            if self.fq_map.len() >= PSEUDONYM_CAP {
                return "redacted-host-overflow".to_string();
            }
            let p = format!("private-host-{}", self.fq_map.len() + 1);
            self.fq_map.insert(host.to_string(), p.clone());
            self.fq_dirty = true;
            self.rows.push(MapRow {
                kind: MapKind::Fqdn,
                real: host.to_string(),
                pseudonym: p,
            });
        }
        self.fq_map[host].clone()
    }

    // --- pass 1: credentials (always on) -----------------------------------

    fn redact_kv(&self, line: &str) -> Option<String> {
        // JSON-shaped lines are owned by the json rules, which preserve quoting
        if line.trim_start_matches([' ', '\t']).starts_with('"') {
            return None;
        }
        let pose = line.find('=');
        let posc = line.find(':');
        let pos = match (pose, posc) {
            (Some(e), Some(c)) => e.min(c),
            (Some(e), None) => e,
            (None, Some(c)) => c,
            (None, None) => return None,
        };
        if pos < 1 {
            return None;
        }
        let key = line[..pos]
            .trim_start_matches([' ', '\t', '#', ';'])
            .trim_end_matches([' ', '\t']);
        // only plausible config keys: short, starts alphanumeric, no
        // sentence/shell punctuation (prevents prose matching as a key)
        if key.len() > 64
            || !key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            || key.contains(['"', '`', ';', '|', '(', ')', '/'])
        {
            return None;
        }
        if is_diagnostic_key(key) {
            return None;
        }
        if is_secret_key(key) && line[pos + 1..].chars().any(|c| c != ' ' && c != '\t') {
            return Some(format!("{} [REDACTED]", &line[..=pos]));
        }
        None
    }

    fn redact_json(&self, line: &str) -> String {
        let pass = |rx: &Regex, line: &str| -> String {
            rx.replace_all(line, |caps: &regex::Captures| {
                let key = &caps[1];
                if is_secret_key(key) && !is_diagnostic_key(key) {
                    format!("\"{key}\": \"[REDACTED]\"")
                } else {
                    caps[0].to_string()
                }
            })
            .into_owned()
        };
        let line = pass(&self.rx.json_quoted, line);
        pass(&self.rx.json_scalar, &line)
    }

    /// Redact secret-keyed JSON objects/arrays. A balanced container on the
    /// line is replaced whole; an unbalanced one truncates the line and arms
    /// the multi-line withholding state (fail closed to EOF if never closed).
    fn redact_json_containers(&self, line: &str, state: &mut BlockState) -> String {
        let mut out = String::with_capacity(line.len());
        let mut rest = line;
        while let Some(caps) = self.rx.json_container.captures(rest) {
            let whole = caps.get(0).unwrap();
            let key = caps.get(1).unwrap().as_str().to_string();
            if !is_secret_key(&key) || is_diagnostic_key(&key) {
                out.push_str(&rest[..whole.end()]);
                rest = &rest[whole.end()..];
                continue;
            }
            let bracket = whole.end() - 1;
            out.push_str(&rest[..whole.start()]);
            match scan_balanced(&rest[bracket..]) {
                Ok(end) => {
                    out.push_str(&format!("\"{key}\": \"[REDACTED]\""));
                    rest = &rest[bracket + end..];
                }
                Err(depth) => {
                    out.push_str(&format!("\"{key}\": [REDACTED-BLOCK]"));
                    state.in_json = true;
                    state.json_depth = depth;
                    rest = "";
                    break;
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn redact_secret_line(&self, line: &str, state: &mut BlockState) -> String {
        // stream.conf parent side: [<API_KEY>] / [<MACHINE_GUID>] section
        // headers ARE secrets
        if self.rx.uuid_section.is_match(line) {
            return "[REDACTED-KEY-SECTION]".to_string();
        }
        let mut line = match self.redact_kv(line) {
            Some(l) => l,
            None => line.to_string(),
        };
        if line.contains('"') {
            line = self.redact_json(&line);
            line = self.redact_json_containers(&line, state);
        }
        line = self
            .rx
            .url_creds
            .replace_all(&line, "://[REDACTED]@")
            .into_owned();
        line = self
            .rx
            .go_dsn
            .replace_all(&line, "[REDACTED]@${1}(")
            .into_owned();
        line = self
            .rx
            .jwt
            .replace_all(&line, "[REDACTED-JWT]")
            .into_owned();
        line = self
            .rx
            .bearer
            .replace_all(&line, "Bearer [REDACTED]")
            .into_owned();
        line = self
            .rx
            .basic
            .replace_all(&line, "Basic [REDACTED]")
            .into_owned();
        line = self
            .rx
            .query_secret
            .replace_all(&line, "${1}[REDACTED]")
            .into_owned();
        line = self
            .rx
            .argv_secret
            .replace_all(&line, |caps: &regex::Captures| {
                if is_diagnostic_key(&caps[2]) {
                    caps[0].to_string()
                } else {
                    format!("{}[REDACTED]", &caps[1])
                }
            })
            .into_owned();
        line = self
            .rx
            .short_secret
            .replace_all(&line, "${1}[REDACTED]")
            .into_owned();
        line
    }

    // --- pass 2: PII pseudonymization (default on) -------------------------

    fn redact_destination(&mut self, line: &str) -> String {
        // stream.conf destination values are user infrastructure hostnames
        // regardless of TLD. Token syntax: [PROTOCOL:]HOST[%IFACE][:PORT][:SSL]
        let Some(pos) = line.find('=') else {
            return line.to_string();
        };
        let head = &line[..=pos];
        let mut valpart = String::new();
        for tok in line[pos + 1..].split_whitespace() {
            let mut proto = "";
            let mut tok = tok;
            for p in ["tcp:", "udp:", "unix:"] {
                if let Some(rest) = tok.strip_prefix(p) {
                    proto = p;
                    tok = rest;
                    break;
                }
            }
            // bracketed IPv6 belongs to the IP rules; unix socket paths are
            // not hostnames
            if tok.starts_with('[') || tok.starts_with('/') {
                valpart.push(' ');
                valpart.push_str(proto);
                valpart.push_str(tok);
                continue;
            }
            let (mut hostp, mut rest2) = match tok.find(':') {
                Some(c) if c > 0 => (&tok[..c], tok[c..].to_string()),
                _ => (tok, String::new()),
            };
            if let Some(c) = hostp.find('%') {
                if c > 0 {
                    rest2 = format!("{}{}", &hostp[c..], rest2);
                    hostp = &hostp[..c];
                }
            }
            // leave IPs to the IP rules, and never map an existing pseudonym
            // the split at ':' means hostp can never be a full IPv6 literal;
            // pure-hex leftovers ("beef", "fe80") are treated as hostnames -
            // over-pseudonymizing a mangled address is the safe direction
            let hostp = if !hostp.is_empty()
                && hostp.len() >= 4
                && !self.rx.dest_all_v4.is_match(hostp)
                && !self.rx.dest_pseudonym.is_match(hostp)
            {
                self.pseudo_fqdn(hostp)
            } else {
                hostp.to_string()
            };
            valpart.push(' ');
            valpart.push_str(proto);
            valpart.push_str(&hostp);
            valpart.push_str(&rest2);
        }
        format!("{head}{valpart}")
    }

    fn replace_ips(&mut self, line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut last = 0;
        // collect first: replace_all cannot borrow self mutably in the closure
        let matches: Vec<(usize, usize)> = self
            .rx
            .ipv4
            .find_iter(line)
            .map(|m| (m.start(), m.end()))
            .collect();
        for (s, e) in matches {
            out.push_str(&line[last..s]);
            out.push_str(&self.pseudo_ip(&line[s..e]));
            last = e;
        }
        out.push_str(&line[last..]);
        out
    }

    fn replace_ip6(&mut self, line: &str) -> String {
        // candidates are hex-and-colon runs; validated to avoid timestamps
        // (13:38:34), file:line refs and C++ :: tokens. ::1 and :: are kept.
        let mut out = String::with_capacity(line.len());
        let mut last = 0;
        let matches: Vec<(usize, usize)> = self
            .rx
            .ip6_run
            .find_iter(line)
            .map(|m| (m.start(), m.end()))
            .collect();
        for (s, e) in matches {
            let cand = &line[s..e];
            let pre_ok = line[..s]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')));
            let ncolons = cand.matches(':').count();
            let compressed = cand.contains("::");
            let has_hex_letter = cand.chars().any(|c| c.is_ascii_alphabetic());
            let valid = ncolons > 0
                && pre_ok
                && cand.len() >= 5
                && !cand.ends_with(':')
                && (ncolons >= 3 || compressed)
                && (has_hex_letter || compressed || ncolons >= 6)
                && cand != "::1"
                && cand != "0:0:0:0:0:0:0:1";
            out.push_str(&line[last..s]);
            if valid {
                out.push_str(&self.pseudo_ip6(cand));
            } else {
                out.push_str(cand);
            }
            last = e;
        }
        out.push_str(&line[last..]);
        out
    }

    fn replace_private_fqdns(&mut self, line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut last = 0;
        let matches: Vec<(usize, usize)> = self
            .rx
            .private_fqdn
            .find_iter(line)
            .map(|m| (m.start(), m.end()))
            .collect();
        for (s, e) in matches {
            out.push_str(&line[last..s]);
            let partial_word = line[e..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-');
            if partial_word {
                // e.g. ".locale" - keep as is
                out.push_str(&line[s..e]);
            } else {
                out.push_str(&self.pseudo_fqdn(&line[s..e]));
            }
            last = e;
        }
        out.push_str(&line[last..]);
        out
    }

    fn replace_mapped_fqdns(&mut self, line: &str) -> String {
        // replace every known (pre-seeded or discovered) hostname, longest
        // first, with word boundaries so "host1" never corrupts "host10".
        // Case-insensitive (the ps1 behavior; a superset of the sh one).
        // The sorted key list is rebuilt only when the map grew.
        if self.fq_dirty {
            self.fq_sorted = self
                .fq_map
                .keys()
                .filter(|k| k.len() >= 4 && k.is_ascii())
                .cloned()
                .collect();
            self.fq_sorted
                .sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
            self.fq_dirty = false;
        }
        let mut line = line.to_string();
        for key in &self.fq_sorted {
            line = replace_ci(&line, key, &self.fq_map[key], true);
        }
        line
    }

    fn obfuscate_pii_line(&mut self, line: &str) -> String {
        let mut line = self.rx.email.replace_all(line, "[EMAIL]").into_owned();
        line = self.rx.mac.replace_all(&line, "[MAC]").into_owned();
        if self.rx.destination_line.is_match(&line) {
            line = self.redact_destination(&line);
        }
        line = self.replace_ips(&line);
        line = self.replace_ip6(&line);
        line = self.replace_private_fqdns(&line);
        line = self.replace_mapped_fqdns(&line);
        let host_fqdn = self.id.host_fqdn.clone();
        if !host_fqdn.is_empty() {
            self.record_host(&host_fqdn);
            line = replace_ci(&line, &host_fqdn, "redacted-host", false);
        }
        let host_short = self.id.host_short.clone();
        if !host_short.is_empty() && !host_short.eq_ignore_ascii_case(&host_fqdn) {
            self.record_host(&host_short);
            line = replace_ci(&line, &host_short, "redacted-host", false);
        }
        let run_user = self.id.run_user.clone();
        if !run_user.is_empty() {
            if !self.user_map.contains_key(&run_user) {
                self.user_map
                    .insert(run_user.clone(), "redacted-user".to_string());
                self.rows.push(MapRow {
                    kind: MapKind::User,
                    real: run_user.clone(),
                    pseudonym: "redacted-user".to_string(),
                });
            }
            line = replace_ci(&line, &run_user, "redacted-user", false);
        }
        line = self.replace_home_users(&line);
        line
    }

    /// Other local users leak through mount tables and paths when the tool
    /// runs as root: `/home/<name>` and `/Users/<name>` segments become
    /// stable `user-N` pseudonyms (root and already-emitted pseudonyms are
    /// left alone).
    fn replace_home_users(&mut self, line: &str) -> String {
        let mut line = line.to_string();
        // the backslash form appears in Windows captures (C:\Users\name)
        for prefix in ["/home/", "/Users/", "\\Users\\"] {
            if !line.contains(prefix) {
                continue;
            }
            let mut out = String::with_capacity(line.len());
            let mut rest = line.as_str();
            while let Some(idx) = rest.find(prefix) {
                let after = &rest[idx + prefix.len()..];
                let seg_len = after
                    .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
                    .unwrap_or(after.len());
                out.push_str(&rest[..idx + prefix.len()]);
                let seg = &after[..seg_len];
                let is_pseudonym = seg
                    .to_ascii_lowercase()
                    .strip_prefix("user-")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    || seg.eq_ignore_ascii_case("user-overflow");
                if seg.is_empty() || seg == "root" || seg.starts_with("redacted-") || is_pseudonym {
                    out.push_str(seg);
                } else {
                    out.push_str(&self.pseudo_home_user(seg));
                }
                rest = &after[seg_len..];
            }
            out.push_str(rest);
            line = out;
        }
        line
    }

    fn pseudo_home_user(&mut self, user: &str) -> String {
        if !self.home_user_map.contains_key(user) {
            if self.home_user_map.len() >= PSEUDONYM_CAP {
                return "user-overflow".to_string();
            }
            let p = format!("user-{}", self.home_user_map.len() + 1);
            self.home_user_map.insert(user.to_string(), p.clone());
            self.rows.push(MapRow {
                kind: MapKind::User,
                real: user.to_string(),
                pseudonym: p,
            });
        }
        self.home_user_map[user].clone()
    }

    fn record_host(&mut self, h: &str) {
        if !self.host_map.contains_key(h) {
            self.host_map
                .insert(h.to_string(), "redacted-host".to_string());
            self.rows.push(MapRow {
                kind: MapKind::Host,
                real: h.to_string(),
                pseudonym: "redacted-host".to_string(),
            });
        }
    }

    /// Sanitize one line with no multi-line block context (PEM/YAML handling
    /// lives in `sanitize_text`).
    pub fn sanitize_line(&mut self, line: &str) -> String {
        // a discarded per-call state keeps the documented contract honest:
        // one line in, one line out, no block context survives the call
        let mut state = BlockState::default();
        self.sanitize_line_with(line, &mut state)
    }

    fn sanitize_line_with(&mut self, line: &str, state: &mut BlockState) -> String {
        let line = self.redact_secret_line(line, state);
        if self.obfuscate {
            self.obfuscate_pii_line(&line)
        } else {
            line
        }
    }

    /// Sanitize a whole text: per-line redaction plus whole-block withholding
    /// for multiline secrets (PEM private keys, YAML block scalars under a
    /// secret key). Fails closed: if the END marker never arrives, the rest
    /// of the file stays withheld.
    pub fn sanitize_text(&mut self, text: &str) -> String {
        let mut state = BlockState::default();
        if text.is_empty() {
            return String::new();
        }
        let mut out = String::with_capacity(text.len());
        for raw in text.split('\n') {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if state.in_pem {
                if self.rx.pem_end.is_match(line) {
                    state.in_pem = false;
                }
                continue;
            }
            if state.in_json {
                // JSON strings cannot contain raw newlines, so each line
                // starts outside a string; track depth until the container
                // closes (the closing line is withheld too, fail closed)
                state.json_depth = scan_json_depth(line, state.json_depth);
                if state.json_depth <= 0 {
                    state.in_json = false;
                }
                continue;
            }
            if state.in_yaml {
                if line.trim_matches([' ', '\t']).is_empty() {
                    continue;
                }
                let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                if indent > state.yaml_indent {
                    continue;
                }
                state.in_yaml = false;
            }
            if self.rx.yaml_block_header.is_match(line) {
                let key_end = line.find(':').unwrap_or(line.len());
                let key = line[..key_end].trim_start_matches([' ', '\t']);
                if is_secret_key(key) && !is_diagnostic_key(key) {
                    state.yaml_indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                    state.in_yaml = true;
                    out.push_str(&line[..key_end]);
                    out.push_str(": [REDACTED BLOCK]\n");
                    continue;
                }
            }
            if self.rx.pem_begin.is_match(line) {
                state.in_pem = true;
                out.push_str("[REDACTED PRIVATE KEY BLOCK]\n");
                continue;
            }
            out.push_str(&self.sanitize_line_with(line, &mut state));
            out.push('\n');
        }
        // split('\n') manufactures a trailing empty segment when the text
        // ends in a newline; drop the extra blank line it would produce
        if text.ends_with('\n') && out.ends_with('\n') {
            out.pop();
        }
        out
    }

    /// Sanitize raw bytes to sanitized bytes, never failing open. Buffers
    /// with NUL bytes (binary or BOM-less UTF-16) are withheld rather than
    /// run through byte-unsafe line redaction; a sanitizer panic withholds
    /// the content (fail closed; the per-text block state is created fresh
    /// inside `sanitize_text`, and pseudonym mappings only ever grow, so a
    /// caught panic cannot cause later under-redaction). The second return
    /// value names the withheld outcome so the caller can log it. The
    /// collectors cap what lands here, so scanning the whole buffer is
    /// bounded.
    pub fn sanitize_bytes(&mut self, raw: &[u8]) -> (Vec<u8>, Option<&'static str>) {
        if raw.contains(&0) {
            return (
                format!("{WITHHELD_NUL}\n").into_bytes(),
                Some("NUL bytes (binary or UTF-16?)"),
            );
        }
        let text = String::from_utf8_lossy(raw);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.sanitize_text(&text))) {
            Ok(s) => (s.into_bytes(), None),
            // fail CLOSED: never ship content the sanitizer could not process
            Err(_) => (
                format!("{WITHHELD_FAILED}\n").into_bytes(),
                Some("sanitizer failure"),
            ),
        }
    }

    /// Sanitize a file in place (same semantics as `sanitize_bytes`). Used
    /// only for files written outside the collection path (MANIFEST.json).
    pub fn sanitize_file(&mut self, path: &Path) -> std::io::Result<()> {
        if !path.is_file() {
            return Ok(());
        }
        let raw = std::fs::read(path)?;
        let (sanitized, _) = self.sanitize_bytes(&raw);
        std::fs::write(path, sanitized)
    }
}

/// Scan a JSON container starting at its opening bracket. String-aware
/// (escaped quotes and bracket characters inside strings do not count).
/// Returns Ok(byte index just past the matching close) when balanced on this
/// slice, or Err(open depth) when the container continues past its end.
fn scan_balanced(s: &str) -> Result<usize, i32> {
    scan_balanced_from_depth(s, 0)
}

/// Continue a multi-line container scan on a fresh line (JSON strings cannot
/// span raw newlines, so the line starts outside a string). Returns the new
/// open depth.
fn scan_json_depth(line: &str, depth: i32) -> i32 {
    match scan_balanced_from_depth(line, depth) {
        Ok(_) => 0,
        Err(d) => d,
    }
}

fn scan_balanced_from_depth(s: &str, mut depth: i32) -> Result<usize, i32> {
    let mut in_string = false;
    let mut escape = false;
    for (i, c) in s.char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth <= 0 {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
    }
    Err(depth)
}

/// Replace every occurrence of `needle` (ASCII case-insensitive) with
/// `replacement`. With `word_boundary`, occurrences adjacent to `[\w.-]`
/// characters are left alone.
fn replace_ci(haystack: &str, needle: &str, replacement: &str, word_boundary: bool) -> String {
    if needle.is_empty() || !needle.is_ascii() {
        return haystack.to_string();
    }
    let hay_lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let boundary = |c: Option<char>| {
        c.is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    };
    let mut out = String::with_capacity(haystack.len());
    let mut pos = 0;
    while let Some(found) = hay_lower[pos..].find(&needle_lower) {
        let s = pos + found;
        let e = s + needle.len();
        let ok = !word_boundary
            || (boundary(haystack[..s].chars().next_back())
                && boundary(haystack[e..].chars().next()));
        out.push_str(&haystack[pos..s]);
        if ok {
            out.push_str(replacement);
        } else {
            out.push_str(&haystack[s..e]);
        }
        pos = e;
    }
    out.push_str(&haystack[pos..]);
    out
}
