//! Everything derived from the alert before any sender runs.
//!
//! This is the parity-critical module: every payload, subject line and message body
//! is built from these fields, so each one is a literal port of the corresponding
//! shell assignment, including the transition-specific rewrites and the quirks that
//! come with them (a CLEAR notification deliberately drops the value from `alarm`,
//! for instance).

use std::collections::HashMap;

use crate::args::{AlertArgs, Status};
use crate::config::Config;
use crate::datefmt;
use crate::hostname;
use crate::paths::Paths;
use crate::textutil::{
    dots_and_underscores_to_dashes, duration4human, underscores_to_spaces, urlencode,
};

const WARN_ROW_TEMPLATE: &str = include_str!("../templates/alarm_row_warning.tpl");
const CRIT_ROW_TEMPLATE: &str = include_str!("../templates/alarm_row_critical.tpl");

pub struct Message {
    pub host: String,
    pub notification_description: String,
    pub url_host: String,
    pub url_chart: String,
    pub url_name: String,
    pub url_value_string: String,
    pub goto_url: String,
    pub date: String,
    pub date_utc: String,
    pub severity: String,
    pub duration_txt: String,
    pub non_clear_duration_txt: String,
    pub raised_for: String,
    pub status_message: String,
    pub color: String,
    pub alarm: String,
    pub image: String,
    pub alarm_badge: String,
    pub status_email_subject: String,
    pub rich_status_raised_for: String,
    pub background_color: String,
    pub border_color: String,
    pub text_color: String,
    pub action_text_color: String,
    pub html_email_subject: String,
    pub info_html: String,
    pub raised_for_html: String,
    pub edit_command: String,
    pub line: String,
    pub s_host: String,
    pub extra_alarms_list_text: String,
    pub warn_alarms_html: String,
    pub crit_alarms_html: String,
    pub images_base_url: String,
}

impl Message {
    pub fn build(args: &AlertArgs, cfg: &Config, paths: &Paths) -> Self {
        let images_base_url = cfg.str("images_base_url").to_string();
        let host = resolve_host(args, cfg);

        let notification_description = format!(
            "notification to '{}' for transition from {} to {}, of alert '{}' = '{}', of instance '{}', context '{}' on host '{}'",
            args.roles,
            args.old_status,
            args.status,
            args.name,
            args.value_string,
            args.chart,
            args.context,
            host
        );

        let url_host = urlencode(&args.args_host);
        let url_chart = urlencode(&args.chart);
        let url_name = urlencode(&args.name);
        let url_value_string = urlencode(&args.value_string);

        let redirect_params = format!(
            "host={url_host}&chart={url_chart}&alarm={url_name}&alarm_unique_id={}&alarm_id={}&alarm_event_id={}&alarm_when={}&alarm_status={}&alarm_chart={}&alarm_value={url_value_string}",
            args.unique_id, args.alarm_id, args.event_id, args.when, args.status, args.chart
        );

        let registry_unique_id = registry_unique_id(paths);
        let registry_url = std::env::var("NETDATA_REGISTRY_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "https://registry.my-netdata.io".to_string());
        let goto_url = format!(
            "{registry_url}/registry-alert-redirect.html?agent_machine_guid={registry_unique_id}&host_machine_guid={}&transition_id={}&{redirect_params}",
            args.child_machine_guid, args.transition_id
        );

        let date_format = cfg.str("date_format");
        let when = args.when_secs();
        let date = datefmt::format(when, date_format, false);
        let date_utc = datefmt::format(when, date_format, true);

        let duration_txt = duration4human(args.duration_secs());
        let non_clear_duration_txt = duration4human(args.non_clear_duration_secs());

        let mut severity = args.status.clone();
        let mut raised_for = format!(
            "(was {} for {duration_txt})",
            args.old_status.to_lowercase()
        );
        let summary_spaced = underscores_to_spaces(&args.summary);
        let mut alarm = format!("{summary_spaced} = {}", args.value_string);

        // Defaults for a status the per-status table does not cover.
        let mut status_message = "status unknown".to_string();
        let mut color = "grey".to_string();
        let mut image = format!("{images_base_url}/images/banner-icon-144x144.png");
        let mut status_email_subject = args.status.clone();
        let mut alarm_badge = String::new();
        let mut rich_status_raised_for = String::new();
        let mut background_color = String::new();
        let mut border_color = String::new();
        let mut text_color = String::new();
        let mut action_text_color = String::new();

        match args.status() {
            Status::Critical => {
                image = format!("{images_base_url}/images/alert-128-red.png");
                alarm_badge =
                    "https://app.netdata.cloud/static/email/img/label_critical.png".to_string();
                status_message = "is critical".to_string();
                status_email_subject = "Critical".to_string();
                color = "#ca414b".to_string();
                rich_status_raised_for =
                    format!("Raised to critical, for {non_clear_duration_txt}");
                background_color = "#FFEBEF".to_string();
                border_color = "#FF4136".to_string();
                text_color = "#FF4136".to_string();
                action_text_color = "#FFFFFF".to_string();
            }
            Status::Warning => {
                image = format!("{images_base_url}/images/alert-128-orange.png");
                alarm_badge =
                    "https://app.netdata.cloud/static/email/img/label_warning.png".to_string();
                status_message = "needs attention".to_string();
                status_email_subject = "Warning".to_string();
                color = "#ffc107".to_string();
                rich_status_raised_for = format!("Raised to warning, for {non_clear_duration_txt}");
                background_color = "#FFF8E1".to_string();
                border_color = "#FFC300".to_string();
                text_color = "#536775".to_string();
                action_text_color = "#35414A".to_string();
            }
            Status::Clear => {
                image = format!("{images_base_url}/images/check-mark-2-128-green.png");
                alarm_badge =
                    "https://app.netdata.cloud/static/email/img/label_recovered.png".to_string();
                status_message = "recovered".to_string();
                status_email_subject = "Clear".to_string();
                color = "#77ca6d".to_string();
                rich_status_raised_for = String::new();
                background_color = "#E5F5E8".to_string();
                border_color = "#68C47D".to_string();
                text_color = "#00AB44".to_string();
                action_text_color = "#FFFFFF".to_string();
            }
            Status::Other => {}
        }

        let mut html_email_subject = format!(
            "{status_email_subject}, {} = {}, on {host}",
            args.summary, args.value_string
        );

        let longer_non_clear = args.non_clear_duration_secs() > args.duration_secs();
        match (args.old_status(), args.status()) {
            (_, Status::Clear) => {
                severity = format!("Recovered from {}", args.old_status);
                if longer_non_clear {
                    raised_for = format!("(alarm was raised for {non_clear_duration_txt})");
                }
                rich_status_raised_for = format!(
                    "Recovered from {}, {raised_for}",
                    args.old_status.to_lowercase()
                );
                // A CLEAR notification intentionally omits the value: for many alerts
                // the value at recovery carries no meaning.
                alarm = format!("{summary_spaced} {raised_for}");
                html_email_subject = format!(
                    "{status_email_subject}, {} {raised_for}, on {host}",
                    args.summary
                );
            }
            (Status::Warning, Status::Critical) => {
                severity = format!("Escalated to {}", args.status);
                if longer_non_clear {
                    raised_for = format!("(alarm is raised for {non_clear_duration_txt})");
                }
                rich_status_raised_for = format!("Escalated to critical, {raised_for}");
            }
            (Status::Critical, Status::Warning) => {
                severity = format!("Demoted to {}", args.status);
                if longer_non_clear {
                    raised_for = format!("(alarm is raised for {non_clear_duration_txt})");
                }
                rich_status_raised_for = format!("Demoted to warning, {raised_for}");
            }
            _ => raised_for = String::new(),
        }

        let info_html = if args.info.is_empty() {
            String::new()
        } else {
            format!(" <small><br/>{}</small>", args.info)
        };
        let raised_for_html = if raised_for.is_empty() {
            String::new()
        } else {
            format!("<br/><small>{raised_for}</small>")
        };

        let (edit_command, line, s_host) = split_edit_command(&args.edit_command_line);

        let extra_alarms_list_text = if args.total_warnings_num() + args.total_critical_num() > 15 {
            "(Showing latest 15 alerts)".to_string()
        } else {
            String::new()
        };

        let now = datefmt::now_secs();
        let warn_alarms_html = render_alarm_rows(
            &args.total_warn_alarms,
            WARN_ROW_TEMPLATE,
            date_format,
            now,
            "date_w",
        );
        let crit_alarms_html = render_alarm_rows(
            &args.total_crit_alarms,
            CRIT_ROW_TEMPLATE,
            date_format,
            now,
            "date_c",
        );

        Self {
            host,
            notification_description,
            url_host,
            url_chart,
            url_name,
            url_value_string,
            goto_url,
            date,
            date_utc,
            severity,
            duration_txt,
            non_clear_duration_txt,
            raised_for,
            status_message,
            color,
            alarm,
            image,
            alarm_badge,
            status_email_subject,
            rich_status_raised_for,
            background_color,
            border_color,
            text_color,
            action_text_color,
            html_email_subject,
            info_html,
            raised_for_html,
            edit_command,
            line,
            s_host,
            extra_alarms_list_text,
            warn_alarms_html,
            crit_alarms_html,
            images_base_url,
        }
    }

    /// Every variable the e-mail templates and the custom sender may reference.
    pub fn template_vars(&self, args: &AlertArgs, cfg: &Config) -> HashMap<String, String> {
        let mut v = HashMap::new();
        let mut set = |k: &str, val: &str| {
            v.insert(k.to_string(), val.to_string());
        };

        set("host", &self.host);
        set("url_host", &self.url_host);
        set("url_chart", &self.url_chart);
        set("url_name", &self.url_name);
        set("url_value_string", &self.url_value_string);
        set("goto_url", &self.goto_url);
        set("date", &self.date);
        set("date_utc", &self.date_utc);
        set("severity", &self.severity);
        set("duration_txt", &self.duration_txt);
        set("non_clear_duration_txt", &self.non_clear_duration_txt);
        set("raised_for", &self.raised_for);
        set("status_message", &self.status_message);
        set("color", &self.color);
        set("alarm", &self.alarm);
        set("image", &self.image);
        set("alarm_badge", &self.alarm_badge);
        set("rich_status_raised_for", &self.rich_status_raised_for);
        set("background_color", &self.background_color);
        set("border_color", &self.border_color);
        set("text_color", &self.text_color);
        set("action_text_color", &self.action_text_color);
        set("info_html", &self.info_html);
        set("raised_for_html", &self.raised_for_html);
        set("edit_command", &self.edit_command);
        set("line", &self.line);
        set("s_host", &self.s_host);
        set("images_base_url", &self.images_base_url);

        set("roles", &args.roles);
        set("unique_id", &args.unique_id);
        set("alarm_id", &args.alarm_id);
        set("event_id", &args.event_id);
        set("when", &args.when);
        set("name", &args.name);
        set("chart", &args.chart);
        set("status", &args.status);
        set("old_status", &args.old_status);
        set("value", &args.value);
        set("old_value", &args.old_value);
        set("src", &args.src);
        set("duration", &args.duration);
        set("non_clear_duration", &args.non_clear_duration);
        set("units", &args.units);
        set("info", &args.info);
        set("value_string", &args.value_string);
        set("old_value_string", &args.old_value_string);
        set("calc_expression", &args.calc_expression);
        set("calc_param_values", &args.calc_param_values);
        set("total_warnings", &args.total_warnings);
        set("total_critical", &args.total_critical);
        set("classification", &args.classification);
        set("summary", &args.summary);
        set("context", &args.context);
        set("component", &args.component);
        set("type", &args.alert_type);
        set("transition_id", &args.transition_id);
        set("child_machine_guid", &args.child_machine_guid);

        // Template-only keys.
        set("EMAIL_CHARSET", cfg.str("EMAIL_CHARSET"));
        set("EXTRA_ALARMS_LIST_TEXT", &self.extra_alarms_list_text);
        set("WARN_ALARMS", &self.warn_alarms_html);
        set("CRIT_ALARMS", &self.crit_alarms_html);
        set("copyright_year", &datefmt::current_year());
        set("name//[._]/-", &dots_and_underscores_to_dashes(&args.name));

        v
    }
}

fn resolve_host(args: &AlertArgs, cfg: &Config) -> String {
    if args.args_host.is_empty() {
        return hostname::short();
    }
    // Only rewrite to the FQDN for the local node: a child's FQDN is not knowable
    // from here.
    if cfg.str("use_fqdn") == "YES" && args.args_host == hostname::short() {
        let full = hostname::full();
        if !full.is_empty() {
            return full;
        }
    }
    args.args_host.clone()
}

fn registry_unique_id(paths: &Paths) -> String {
    if let Ok(v) = std::env::var("NETDATA_REGISTRY_UNIQUE_ID") {
        if !v.is_empty() {
            return v;
        }
    }
    let file = paths.machine_guid_file();
    match std::fs::read_to_string(&file) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            tracing::error!("failed to identify this agent via its NETDATA_REGISTRY_UNIQUE_ID.");
            String::new()
        }
    }
}

/// `edit_command_line` is `<command>=<line>=<host>`; the host may itself contain `=`.
fn split_edit_command(raw: &str) -> (String, String, String) {
    let mut parts = raw.splitn(3, '=');
    (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    )
}

/// Render the "other active alerts" rows for the HTML e-mail.
///
/// Input is the daemon's `name=timestamp,name=timestamp` list.
fn render_alarm_rows(
    list: &str,
    template: &str,
    date_format: &str,
    now: i64,
    date_key: &str,
) -> String {
    let mut out = String::new();
    for pair in list.split(',').filter(|p| !p.trim().is_empty()) {
        let (key, val) = match pair.split_once('=') {
            Some(kv) => kv,
            None => (pair, ""),
        };
        let ts: i64 = val.trim().parse().unwrap_or(0);
        let when = datefmt::format(ts, date_format, false);
        let elapsed_txt = duration4human(now - ts);
        out.push_str(&crate::textutil::expand(template, |k| {
            if k == "key" {
                Some(key)
            } else if k == date_key {
                Some(&when)
            } else if k == "elapsed_txt" {
                Some(&elapsed_txt)
            } else {
                None
            }
        }));
    }
    out
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
