//! Typed view over `health_alarm_notify.conf`.
//!
//! Values stay in a map rather than a struct with 120 fields: the shell script read
//! them dynamically, users add keys the stock file never mentions, and a map keeps
//! the sender code reading exactly like the configuration documentation.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::conf_parser::{self, ConfigData};
use crate::exec;
use crate::paths::Paths;

/// Methods that resolve recipients per role (`role_recipients_<method>[role]` and
/// `DEFAULT_RECIPIENT_<METHOD>`). Same list, same order as the script's
/// `method_names`.
pub const METHOD_NAMES: &[&str] = &[
    "alerta",
    "awssns",
    "custom",
    "discord",
    "dynatrace",
    "email",
    "fleep",
    "flock",
    "gotify",
    "hipchat",
    "irc",
    "kavenegar",
    "matrix",
    "messagebird",
    "msteams",
    "ntfy",
    "pd",
    "prowl",
    "pushbullet",
    "pushover",
    "rocketchat",
    "slack",
    "sms",
    "syslog",
    "telegram",
    "twilio",
    "smseagle",
];

/// Methods addressed at host level, with no recipient list.
pub const HOST_LEVEL_METHODS: &[&str] = &["kafka", "opsgenie", "ilert", "signl4"];

pub struct Config {
    pub data: ConfigData,
    /// `SEND_<METHOD>` state, keyed by the upper-case method name. Kept separate
    /// from `data` because screening mutates it.
    send: BTreeMap<String, String>,
    /// External programs the remaining shell-delegating senders need.
    pub sendmail: Option<PathBuf>,
    pub aws: Option<PathBuf>,
    pub sendsms: Option<PathBuf>,
}

impl Config {
    /// Load stock then user configuration.
    pub fn load(paths: &Paths) -> Self {
        let mut data = Self::defaults();
        for path in paths.config_files() {
            if !path.is_file() {
                tracing::debug!(path = %path.display(), "config file not found");
                continue;
            }
            match conf_parser::parse_file(&path, &data.vars) {
                Ok(parsed) => {
                    tracing::debug!(path = %path.display(), "loaded config file");
                    data.merge(parsed);
                }
                Err(e) => {
                    tracing::error!(path = %path.display(), "failed to load config file: {e}")
                }
            }
        }
        Self::from_data(data)
    }

    /// Load a single explicit file - the `unittest` mode.
    pub fn load_file(path: &std::path::Path) -> Self {
        let mut data = Self::defaults();
        match conf_parser::parse_file(path, &data.vars) {
            Ok(parsed) => data.merge(parsed),
            Err(e) => tracing::error!(path = %path.display(), "failed to load config file: {e}"),
        }
        Self::from_data(data)
    }

    pub fn from_data(mut data: ConfigData) -> Self {
        for (path, line, text) in &data.unsupported {
            tracing::warn!(
                config = %path.display(),
                line,
                "unsupported configuration construct, ignored: {text}"
            );
        }

        apply_msteams_migration(&mut data);
        apply_post_load_defaults(&mut data);

        let mut send = BTreeMap::new();
        for method in METHOD_NAMES {
            send.insert(method.to_uppercase(), "YES".to_string());
        }
        // The script declares these two outside the loop; kafka defaults on,
        // dynatrace defaults off.
        send.insert("KAFKA".to_string(), "YES".to_string());
        send.insert("DYNATRACE".to_string(), String::new());
        // Anything the configuration mentions, including methods with no per-role
        // recipients such as opsgenie/ilert/signl4.
        for (key, value) in &data.vars {
            if let Some(method) = key.strip_prefix("SEND_") {
                send.insert(method.to_string(), value.clone());
            }
        }

        let mut cfg = Self {
            data,
            send,
            sendmail: None,
            aws: None,
            sendsms: None,
        };
        cfg.static_screening();
        cfg
    }

    /// Build a configuration from literal text - used by tests and by golden
    /// comparisons against the shell implementation.
    pub fn from_text(text: &str) -> Self {
        let mut data = Self::defaults();
        data.merge(conf_parser::parse_str(
            text,
            std::path::Path::new("<inline>"),
            &data.vars.clone(),
        ));
        Self::from_data(data)
    }

    /// Defaults applied before any file is read.
    fn defaults() -> ConfigData {
        let mut d = ConfigData::default();
        for (k, v) in [
            ("images_base_url", "https://registry.my-netdata.io"),
            ("curl_options", ""),
            ("use_fqdn", "NO"),
            ("date_format", ""),
            ("SMSEAGLE_MSG_TYPE", "sms"),
            ("SMSEAGLE_CALL_DURATION", "10"),
            ("SMSEAGLE_VOICE_ID", "1"),
            ("IRC_PORT", "6667"),
            ("EMAIL_CHARSET", "UTF-8"),
        ] {
            d.set(k, v);
        }
        for method in METHOD_NAMES {
            d.set(&format!("DEFAULT_RECIPIENT_{}", method.to_uppercase()), "");
        }
        d
    }

    /// Resolve the alert placeholders the parser deferred.
    ///
    /// Called once the alert is known, so a configuration value written as a template
    /// (the stock `AWSSNS_MESSAGE_FORMAT` is one) carries real values, as it did when
    /// bash sourced the file after parsing its arguments.
    pub fn expand_runtime_placeholders(&mut self, vars: &HashMap<String, String>) {
        for value in self.data.vars.values_mut() {
            if value.contains("${") {
                *value = crate::textutil::expand(value, |k| vars.get(k).map(String::as_str));
            }
        }
        for table in self.data.arrays.values_mut() {
            for value in table.values_mut() {
                if value.contains("${") {
                    *value = crate::textutil::expand(value, |k| vars.get(k).map(String::as_str));
                }
            }
        }
    }

    pub fn str(&self, key: &str) -> &str {
        self.data.str(key)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key)
    }

    /// Is this method enabled? `method` is the lower-case name.
    pub fn enabled(&self, method: &str) -> bool {
        self.send
            .get(&method.to_uppercase())
            .is_some_and(|v| v == "YES")
    }

    pub fn disable(&mut self, method: &str) {
        self.send.insert(method.to_uppercase(), "NO".to_string());
    }

    fn enable(&mut self, method: &str) {
        self.send.insert(method.to_uppercase(), "YES".to_string());
    }

    /// `SEND_*` variable names that are `YES`, alphabetically - the `dump_methods`
    /// output, matching bash's sorted `${!SEND_@}` expansion.
    pub fn enabled_send_variables(&self) -> Vec<String> {
        self.send
            .iter()
            .filter(|(_, v)| v.as_str() == "YES")
            .map(|(k, _)| format!("SEND_{k}"))
            .collect()
    }

    pub fn any_enabled(&self) -> bool {
        self.send.values().any(|v| v == "YES")
    }

    /// Per-role recipients for a method, if the configuration defines any.
    pub fn role_recipients(&self, method: &str, role: &str) -> Option<&str> {
        self.data
            .array(&format!("role_recipients_{method}"))
            .and_then(|m| m.get(role))
            .map(String::as_str)
    }

    pub fn default_recipient(&self, method: &str) -> &str {
        self.str(&format!("DEFAULT_RECIPIENT_{}", method.to_uppercase()))
    }

    /// Disable methods whose mandatory credentials are missing. Mirrors the
    /// script's screening block, including which key each method insists on.
    fn static_screening(&mut self) {
        // `AUTO` means "enable if we can actually deliver". The script probed curl
        // here, which was a long-standing bug: e-mail needs an MTA, not curl.
        if self.send.get("EMAIL").map(String::as_str) == Some("AUTO") {
            if exec::which("sendmail").is_some() {
                self.enable("email");
            } else {
                self.disable("email");
            }
        }

        let requires_all: &[(&str, &[&str])] = &[
            ("slack", &["SLACK_WEBHOOK_URL"]),
            ("rocketchat", &["ROCKETCHAT_WEBHOOK_URL"]),
            ("alerta", &["ALERTA_WEBHOOK_URL"]),
            ("flock", &["FLOCK_WEBHOOK_URL"]),
            ("discord", &["DISCORD_WEBHOOK_URL"]),
            ("pushover", &["PUSHOVER_APP_TOKEN"]),
            ("pushbullet", &["PUSHBULLET_ACCESS_TOKEN"]),
            (
                "twilio",
                &[
                    "TWILIO_ACCOUNT_TOKEN",
                    "TWILIO_ACCOUNT_SID",
                    "TWILIO_NUMBER",
                ],
            ),
            ("hipchat", &["HIPCHAT_AUTH_TOKEN"]),
            (
                "messagebird",
                &["MESSAGEBIRD_ACCESS_KEY", "MESSAGEBIRD_NUMBER"],
            ),
            (
                "smseagle",
                &[
                    "SMSEAGLE_API_URL",
                    "SMSEAGLE_API_ACCESSTOKEN",
                    "SMSEAGLE_MSG_TYPE",
                ],
            ),
            ("kavenegar", &["KAVENEGAR_API_KEY", "KAVENEGAR_SENDER"]),
            ("telegram", &["TELEGRAM_BOT_TOKEN"]),
            ("kafka", &["KAFKA_URL", "KAFKA_SENDER_IP"]),
            ("irc", &["IRC_NETWORK"]),
            ("fleep", &["FLEEP_SERVER", "FLEEP_SENDER"]),
            (
                "dynatrace",
                &[
                    "DYNATRACE_SPACE",
                    "DYNATRACE_SERVER",
                    "DYNATRACE_TOKEN",
                    "DYNATRACE_TAG_VALUE",
                    "DYNATRACE_EVENT",
                ],
            ),
            ("opsgenie", &["OPSGENIE_API_KEY"]),
            ("matrix", &["MATRIX_HOMESERVER", "MATRIX_ACCESSTOKEN"]),
            ("gotify", &["GOTIFY_APP_TOKEN", "GOTIFY_APP_URL"]),
            ("ntfy", &["DEFAULT_RECIPIENT_NTFY"]),
            ("msteams", &["MSTEAMS_WEBHOOK_URL"]),
            ("pd", &["DEFAULT_RECIPIENT_PD"]),
            ("prowl", &["DEFAULT_RECIPIENT_PROWL"]),
            ("custom", &["DEFAULT_RECIPIENT_CUSTOM"]),
            ("ilert", &["ILERT_ALERT_SOURCE_URL"]),
            ("signl4", &["SIGNL4_WEBHOOK_URL"]),
        ];

        for (method, keys) in requires_all {
            if keys.iter().any(|k| self.str(k).is_empty()) {
                self.disable(method);
            }
        }
    }

    /// Resolve the external programs still needed, disabling what cannot run.
    ///
    /// Unlike the script this no longer looks for `curl`, `logger` or `nc`: HTTP,
    /// syslog and IRC are implemented in-process, so those methods stay available
    /// on a machine that ships none of those tools - which is the whole point on
    /// Windows.
    pub fn check_supported_targets(&mut self, quiet: bool) {
        if self.enabled("email") {
            let configured = self.str("sendmail").to_string();
            let found = if configured.is_empty() {
                exec::which("sendmail")
            } else {
                exec::which(&configured)
            };
            match found {
                Some(p) => self.sendmail = Some(p),
                None => {
                    if !quiet {
                        tracing::error!(
                            "Cannot find sendmail command in the system path. Disabling email notifications."
                        );
                    }
                    self.disable("email");
                }
            }
        }

        if self.enabled("awssns") {
            let configured = self.str("aws").to_string();
            let found = if configured.is_empty() {
                exec::which("aws")
            } else {
                exec::which(&configured)
            };
            match found {
                Some(p) => self.aws = Some(p),
                None => {
                    if !quiet {
                        tracing::error!(
                            "Cannot find aws command in the system path. Disabling Amazon SNS notifications."
                        );
                    }
                    self.disable("awssns");
                }
            }
        }

        if self.enabled("sms") {
            let configured = self.str("sendsms").to_string();
            let found = if configured.is_empty() {
                exec::which("sendsms")
            } else {
                exec::which(&configured)
            };
            match found {
                Some(p) => self.sendsms = Some(p),
                None => self.disable("sms"),
            }
        }

        if self.enabled("custom") && crate::custom::resolve_sender(self).is_none() {
            if !quiet {
                tracing::error!(
                    "custom notifications are enabled but no sender is usable: set \
                     CUSTOM_SENDER_COMMAND, or provide a custom_sender() function and a shell \
                     that can run it. Disabling custom notifications."
                );
            }
            self.disable("custom");
        }
    }
}

/// Backfill the current Microsoft Teams keys from the legacy singular spellings.
fn apply_msteams_migration(data: &mut ConfigData) {
    let pairs = [
        ("SEND_MSTEAM", "SEND_MSTEAMS"),
        ("DEFAULT_RECIPIENT_MSTEAM", "DEFAULT_RECIPIENT_MSTEAMS"),
        ("MSTEAM_WEBHOOK_URL", "MSTEAMS_WEBHOOK_URL"),
        ("MSTEAM_ICON_DEFAULT", "MSTEAMS_ICON_DEFAULT"),
        ("MSTEAM_ICON_CLEAR", "MSTEAMS_ICON_CLEAR"),
        ("MSTEAM_ICON_WARNING", "MSTEAMS_ICON_WARNING"),
        ("MSTEAM_ICON_CRITICAL", "MSTEAMS_ICON_CRITICAL"),
        ("MSTEAM_COLOR_DEFAULT", "MSTEAMS_COLOR_DEFAULT"),
        ("MSTEAM_COLOR_CLEAR", "MSTEAMS_COLOR_CLEAR"),
        ("MSTEAM_COLOR_WARNING", "MSTEAMS_COLOR_WARNING"),
        ("MSTEAM_COLOR_CRITICAL", "MSTEAMS_COLOR_CRITICAL"),
    ];
    for (legacy, current) in pairs {
        // `${LEGACY:-$CURRENT}`: a non-empty legacy value wins.
        if let Some(v) = data
            .get(legacy)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
        {
            data.set(current, &v);
        }
    }
    data.vars.remove("SEND_MSTEAM");

    if let Some(legacy) = data.arrays.remove("role_recipients_msteam") {
        let target = data
            .arrays
            .entry("role_recipients_msteams".to_string())
            .or_default();
        for (role, recipients) in legacy {
            target.insert(role, recipients);
        }
    }
}

/// Defaults that the script applied after sourcing the configuration.
fn apply_post_load_defaults(data: &mut ConfigData) {
    if data.str("EMAIL_CHARSET").is_empty() {
        data.set("EMAIL_CHARSET", "UTF-8");
    }
    if data.str("OPSGENIE_API_URL").is_empty() {
        data.set("OPSGENIE_API_URL", "https://api.opsgenie.com");
    }
    if data.str("TELEGRAM_API_URL").is_empty() {
        data.set("TELEGRAM_API_URL", "https://api.telegram.org");
    }
}

/// Deduplicate while preserving first-seen order.
pub fn dedup_preserving_order(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashMap::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone(), ()).is_none() {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
