//! Incident-management and observability-platform senders.

use serde_json::json;

use crate::args::Status;
use crate::datefmt;
use crate::exec;
use crate::http::Request;
use crate::senders::Ctx;
use crate::senders::push::number_or_string;
use crate::textutil::{truncate_bytes, underscores_to_spaces};

pub async fn kafka(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("kafka") {
        return false;
    }
    let url = ctx.cfg.str("KAFKA_URL");
    let sender_ip = ctx.cfg.str("KAFKA_SENDER_IP");

    // The shell emitted unquoted keys here, which is not JSON. The field names and
    // value types are unchanged; only the document is now well formed.
    let payload = json!({
        "host_ip": sender_ip,
        "when": number_or_string(&ctx.args.when),
        "name": ctx.args.name,
        "chart": ctx.args.chart,
        "status": ctx.args.status,
        "old_status": ctx.args.old_status,
        "value": number_or_float(&ctx.args.value),
        "old_value": number_or_float(&ctx.args.old_value),
        "duration": number_or_string(&ctx.args.duration),
        "non_clear_duration": number_or_string(&ctx.args.non_clear_duration),
        "units": ctx.args.units,
        "info": ctx.args.info,
    });

    let resp = ctx
        .http
        .send(Request::post(url).json(payload.to_string()))
        .await;
    if resp.is(204) {
        tracing::info!("sent kafka data to '{sender_ip}' for {}", ctx.what());
        true
    } else {
        tracing::error!(
            "failed to send kafka data to '{sender_ip}' for {}, with HTTP response status code {}.",
            ctx.what(),
            resp.code_str()
        );
        false
    }
}

/// `nan` and empty values become JSON `null`; real numbers stay numbers.
fn number_or_float(s: &str) -> serde_json::Value {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("nan") {
        return serde_json::Value::Null;
    }
    // Parsed as JSON rather than as f64 so an integer stays an integer.
    match serde_json::from_str::<serde_json::Value>(t) {
        Ok(v) if v.is_number() => v,
        _ => json!(t),
    }
}

pub async fn pagerduty(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("pd") {
        return false;
    }
    // Only real transitions map onto PagerDuty actions.
    let (action, severity) = match ctx.args.status() {
        Status::Clear => ("resolve", "info"),
        Status::Warning => ("trigger", "warning"),
        Status::Critical => ("trigger", "critical"),
        Status::Other => return false,
    };

    let timestamp = datefmt::pagerduty_timestamp(ctx.args.when_secs());
    let use_v2 = ctx.cfg.str("USE_PD_VERSION") == "2";
    let m = ctx.msg;
    let mut sent = false;

    for routing_key in ctx.to("pd") {
        let details = json!({
            "value_w_units": ctx.args.value_string,
            "when": ctx.args.when,
            "duration": ctx.args.duration,
            "roles": ctx.args.roles,
            "alarm_id": ctx.args.alarm_id,
            "name": ctx.args.name,
            "chart": ctx.args.chart,
            "status": ctx.args.status,
            "old_status": ctx.args.old_status,
            "value": ctx.args.value,
            "old_value": ctx.args.old_value,
            "src": ctx.args.src,
            "non_clear_duration": ctx.args.non_clear_duration,
            "units": ctx.args.units,
            "info": ctx.args.info,
        });

        let (url, payload, expected) = if use_v2 {
            (
                "https://events.pagerduty.com/v2/enqueue",
                json!({
                    "payload": {
                        "summary": truncate_bytes(&ctx.args.info, 1024),
                        "source": ctx.args.args_host,
                        "severity": severity,
                        "timestamp": timestamp,
                        "class": ctx.args.chart,
                        "custom_details": details,
                    },
                    "routing_key": routing_key,
                    "event_action": action,
                    "dedup_key": ctx.args.unique_id,
                }),
                202u16,
            )
        } else {
            (
                "https://events.pagerduty.com/generic/2010-04-15/create_event.json",
                json!({
                    "service_key": routing_key,
                    "event_type": action,
                    "incident_key": ctx.args.alarm_id,
                    "description": format!(
                        "{} {} = {} - {}",
                        ctx.args.status, ctx.args.name, ctx.args.value_string, m.host
                    ),
                    "details": details,
                }),
                200u16,
            )
        };

        // Sent without an explicit content type, exactly as curl's `--data` did.
        let resp = ctx
            .http
            .send(Request::post(url).raw(payload.to_string()))
            .await;
        if resp.is(expected) {
            tracing::info!("sent pagerduty event for {}", ctx.what());
            sent = true;
        } else {
            tracing::error!(
                "failed to send pagerduty event for {}, with HTTP response status code {}.",
                ctx.what(),
                resp.code_str()
            );
        }
    }
    sent
}

pub async fn dynatrace(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("dynatrace") {
        return false;
    }
    let server = ctx.cfg.str("DYNATRACE_SERVER").trim_end_matches('/');
    let space = ctx.cfg.str("DYNATRACE_SPACE");
    let token = ctx.cfg.str("DYNATRACE_TOKEN");
    let description = format!(
        "Netdata Notification for: {} {}.{} is {}",
        ctx.msg.host, ctx.args.chart, ctx.args.name, ctx.args.status
    );

    let payload = json!({
        "title": format!("Netdata Alarm from {}", ctx.msg.host),
        "source": ctx.cfg.str("DYNATRACE_ANNOTATION_TYPE"),
        "description": description,
        "eventType": ctx.cfg.str("DYNATRACE_EVENT"),
        "attachRules": {
            "tagRule": [{
                "meTypes": ["HOST"],
                "tags": [ctx.cfg.str("DYNATRACE_TAG_VALUE")],
            }],
        },
        "customProperties": { "description": description },
    });

    let resp = ctx
        .http
        .send(
            Request::post(format!("{server}/e/{space}/api/v1/events"))
                .header("Authorization", format!("Api-token {token}"))
                .json(payload.to_string()),
        )
        .await;

    let event = ctx.cfg.str("DYNATRACE_EVENT");
    match resp.status {
        Some(200) => {
            tracing::info!(
                "sent Dynatrace event '{event}' to '{server}' for {}",
                ctx.what()
            );
            true
        }
        Some(_) => {
            tracing::warn!(
                "failed to send Dynatrace event to '{server}' for {}, with HTTP response status code {}",
                ctx.what(),
                resp.code_str()
            );
            false
        }
        None => {
            tracing::error!(
                "failed to sent Dynatrace '{event}' to '{server}' for {}.",
                ctx.what()
            );
            false
        }
    }
}

pub async fn opsgenie(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("opsgenie") {
        return false;
    }
    let api_key = ctx.cfg.str("OPSGENIE_API_KEY");
    if api_key.is_empty() {
        tracing::info!("Can't send Opsgenie notification, because OPSGENIE_API_KEY is not defined");
        return false;
    }
    let api_url = ctx.cfg.str("OPSGENIE_API_URL").trim_end_matches('/');

    // Opsgenie's priority scale, per its alert-priority documentation.
    let priority = match ctx.args.status() {
        Status::Critical => "P1",
        Status::Clear => "P5",
        _ => "P3",
    };

    let payload = json!({
        "host": ctx.msg.host,
        "unique_id": ctx.args.unique_id,
        "alarmId": number_or_string(&ctx.args.alarm_id),
        "eventId": number_or_string(&ctx.args.event_id),
        "chart": ctx.args.chart,
        "when": number_or_string(&ctx.args.when),
        "name": ctx.args.name,
        "priority": priority,
        "status": ctx.args.status,
        "old_status": ctx.args.old_status,
        // `nan` becomes null so the document stays parseable.
        "value": number_or_float(&ctx.args.value),
        "old_value": number_or_float(&ctx.args.old_value),
        "duration": number_or_string(&ctx.args.duration),
        "non_clear_duration": number_or_string(&ctx.args.non_clear_duration),
        "units": ctx.args.units,
        "info": format!("{}, {}", ctx.msg.status_message, ctx.args.info),
        "calc_expression": ctx.args.calc_expression,
        "total_warnings": ctx.args.total_warnings,
        "total_critical": ctx.args.total_critical,
        "src": ctx.args.src,
    });

    let resp = ctx
        .http
        .send(
            Request::post(format!(
                "{api_url}/v1/json/integrations/webhooks/netdata?apiKey={api_key}"
            ))
            .json(payload.to_string()),
        )
        .await;

    if resp.is(200) {
        tracing::info!("sent opsgenie event for {}", ctx.what());
        true
    } else {
        tracing::error!(
            "failed to send opsgenie event for {}, with HTTP response status code {}.",
            ctx.what(),
            resp.code_str()
        );
        false
    }
}

/// Amazon SNS through the `aws` CLI, so the user's profiles, roles and credential
/// chain keep working exactly as configured.
pub fn awssns(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("awssns") {
        return false;
    }
    let Some(aws) = &ctx.cfg.aws else {
        return false;
    };

    let default_format = format!(
        "{} on {} at {}: {} {}",
        ctx.args.status, ctx.msg.host, ctx.msg.date, ctx.args.chart, ctx.args.value_string
    );
    let configured = ctx.cfg.str("AWSSNS_MESSAGE_FORMAT");
    let message = if configured.is_empty() {
        default_format
    } else {
        configured.to_string()
    };
    let subject = format!(
        "{} {} - {} - {}",
        ctx.msg.host,
        ctx.msg.status_message,
        underscores_to_spaces(&ctx.args.name),
        ctx.args.chart
    );
    let mut sent = false;

    for target in ctx.to("awssns") {
        // The region has to be explicit and has to match the target ARN's region,
        // which is its fourth colon-separated field.
        let region = target.split(':').nth(3).unwrap_or_default().to_string();
        let args = [
            "sns",
            "publish",
            "--region",
            &region,
            "--subject",
            &subject,
            "--message",
            &message,
            "--target-arn",
            &target,
        ];
        match exec::run(aws, args, None, &[]) {
            Ok(out) if out.success() => {
                tracing::info!(
                    "sent Amazon SNS notification to '{target}' for {}",
                    ctx.what()
                );
                sent = true;
            }
            Ok(_) => tracing::error!(
                "failed to send Amazon SNS notification to '{target}' for {}",
                ctx.what()
            ),
            Err(e) => tracing::error!(
                "failed to send Amazon SNS notification to '{target}' for {}: {e}",
                ctx.what()
            ),
        }
    }
    sent
}

#[cfg(test)]
#[path = "incident_tests.rs"]
mod tests;
