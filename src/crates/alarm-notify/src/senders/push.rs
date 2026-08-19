//! Push-notification and alerting-platform senders.

use base64::Engine;
use serde_json::json;

use crate::args::Status;
use crate::http::Request;
use crate::senders::{Ctx, log_failed, log_sent};
use crate::textutil::{truncate_with_ellipsis, underscores_to_spaces, urlencode};

/// Collapse every run of whitespace into a single space.
///
/// The shell passed the Pushbullet body through an unquoted `echo`, so word
/// splitting flattened it; the API therefore always received a single line and that
/// is what the integration was tested against.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title(ctx: &Ctx<'_>) -> String {
    format!(
        "{} {} - {} - {}",
        ctx.msg.host,
        ctx.msg.status_message,
        underscores_to_spaces(&ctx.args.name),
        ctx.args.chart
    )
}

pub async fn pushover(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("pushover") {
        return false;
    }
    let app_token = ctx.cfg.str("PUSHOVER_APP_TOKEN");
    let m = ctx.msg;

    let priority = match ctx.args.status() {
        // Low priority: no sound or vibration.
        Status::Clear => -1,
        // Normal priority: respects quiet hours.
        Status::Warning => 0,
        // High priority: bypasses quiet hours.
        Status::Critical => 1,
        Status::Other => -2,
    };

    let mut title = title(ctx);
    let mut message = format!(
        "\n<font color=\"{}\"><b>{}</b></font>{}<br/>&nbsp;\n<small><b>{}</b><br/>Chart<br/>&nbsp;</small>\n<small><b>{}</b><br/>Severity<br/>&nbsp;</small>\n<small><b>{}{}</b><br/>Time<br/>&nbsp;</small>\n<a href=\"{}\">View Netdata</a><br/>&nbsp;\n<small><small>The source of this alarm is line {}</small></small>\n",
        m.color,
        m.alarm,
        m.info_html,
        ctx.args.chart,
        m.severity,
        m.date,
        m.raised_for_html,
        m.goto_url,
        ctx.args.src
    );
    let mut url = m.goto_url.clone();

    // Pushover's documented limits: title 250, message 1024, url 512.
    title = truncate_with_ellipsis(&title, 250, 247);
    message = truncate_with_ellipsis(&message, 1024, 1021);
    if url.chars().count() > 512 {
        url = String::new();
    }

    let mut sent = false;
    for user in ctx.to("pushover") {
        let fields = vec![
            ("token".to_string(), app_token.to_string()),
            ("user".to_string(), user.clone()),
            ("html".to_string(), "1".to_string()),
            ("title".to_string(), title.clone()),
            ("message".to_string(), message.clone()),
            ("timestamp".to_string(), ctx.args.when.clone()),
            ("url".to_string(), url.clone()),
            (
                "url_title".to_string(),
                "Open netdata dashboard to view the alarm".to_string(),
            ),
            ("priority".to_string(), priority.to_string()),
        ];
        let resp = ctx
            .http
            .send(Request::post("https://api.pushover.net/1/messages.json").multipart(fields))
            .await;
        if resp.is(200) {
            log_sent("pushover notification", &user, ctx.what());
            sent = true;
        } else {
            log_failed("pushover notification", &user, ctx.what(), &resp.code_str());
        }
    }
    sent
}

pub async fn pushbullet(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("pushbullet") {
        return false;
    }
    let api_key = ctx.cfg.str("PUSHBULLET_ACCESS_TOKEN");
    let source_device = ctx.cfg.str("PUSHBULLET_SOURCE_DEVICE");
    let m = ctx.msg;
    let title = title(ctx);
    let body = collapse_whitespace(&format!(
        "{}\\n\nSeverity: {}\\n\nChart: {}\\n\n{}\\n\nThe source of this alarm is line {}",
        m.alarm, m.severity, ctx.args.chart, m.date, ctx.args.src
    ));
    let mut sent = false;

    for recipient in ctx.to("pushbullet") {
        // A leading '#' addresses a channel tag; anything else is an account e-mail.
        let (kind, target) = match recipient.strip_prefix('#') {
            Some(tag) => ("channel_tag", tag.to_string()),
            None => ("email", recipient.clone()),
        };
        let mut payload = json!({
            "title": title,
            "type": "link",
            "body": body,
            "url": m.goto_url,
            "source_device_iden": source_device,
        });
        payload[kind] = json!(target);

        let resp = ctx
            .http
            .send(
                Request::post("https://api.pushbullet.com/v2/pushes")
                    .header("Access-Token", api_key)
                    .json(payload.to_string()),
            )
            .await;
        if resp.is(200) {
            log_sent("pushbullet notification", &recipient, ctx.what());
            sent = true;
        } else {
            log_failed(
                "pushbullet notification",
                &recipient,
                ctx.what(),
                &resp.code_str(),
            );
        }
    }
    sent
}

pub async fn prowl(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("prowl") {
        return false;
    }
    let m = ctx.msg;
    let keys = ctx.to("prowl");
    if keys.is_empty() {
        return false;
    }

    let priority = match ctx.args.status() {
        Status::Critical => 2,
        Status::Warning => 1,
        _ => 0,
    };
    // One request carries every API key, comma separated.
    let fields = vec![
        ("apikey".to_string(), keys.join(",")),
        ("priority".to_string(), priority.to_string()),
        ("url".to_string(), m.goto_url.clone()),
        ("application".to_string(), "Netdata".to_string()),
        (
            "event".to_string(),
            format!("{} {}", m.host, m.status_message),
        ),
        (
            "description".to_string(),
            format!(
                "{} {}, `{}`, *{}*\\n{}",
                m.host, m.status_message, ctx.args.chart, m.alarm, ctx.args.info
            ),
        ),
    ];

    let resp = ctx
        .http
        .send(Request::post("https://api.prowlapp.com/publicapi/add").form(fields))
        .await;
    if resp.is(200) {
        tracing::info!("sent prowl event for {}", ctx.what());
        true
    } else {
        tracing::error!(
            "failed to send prowl event for {}, with HTTP response status code {}.",
            ctx.what(),
            resp.code_str()
        );
        false
    }
}

pub async fn gotify(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("gotify") {
        return false;
    }
    let app_url = ctx.cfg.str("GOTIFY_APP_URL");
    let app_token = ctx.cfg.str("GOTIFY_APP_TOKEN");
    if app_token.is_empty() {
        tracing::info!("Can't send Gotify notification, because GOTIFY_APP_TOKEN is not defined");
        return false;
    }
    // Android client behaviour: 10 rings, 4 makes a sound, 1 is silent.
    let priority = match ctx.args.status() {
        Status::Critical => 10,
        Status::Warning => 4,
        _ => 1,
    };
    let m = ctx.msg;
    let payload = json!({
        "title": format!("{}, {} = {}, on {}", ctx.args.status, ctx.args.name, ctx.args.value_string, m.host),
        "message": format!("{}: {} {}", m.date, ctx.args.chart, ctx.args.value_string),
        "priority": priority,
    });

    let resp = ctx
        .http
        .send(
            Request::post(format!("{app_url}/message?token={app_token}")).json(payload.to_string()),
        )
        .await;
    if resp.is(200) {
        tracing::info!("sent gotify event for {}", ctx.what());
        true
    } else {
        tracing::error!(
            "failed to send gotify event for {}, with HTTP response status code {}.",
            ctx.what(),
            resp.code_str()
        );
        false
    }
}

pub async fn ntfy(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("ntfy") {
        return false;
    }
    let (tag, priority) = match ctx.args.status() {
        Status::Warning => ("warning", "high"),
        Status::Critical => ("red_circle", "urgent"),
        Status::Clear => ("white_check_mark", "default"),
        Status::Other => ("white_circle", "default"),
    };

    let auth_header = ntfy_auth_header(ctx);
    let m = ctx.msg;
    let body = format!(
        "{} {}: {} - {}",
        m.host, m.status_message, m.alarm, ctx.args.info
    );
    let mut sent = false;

    // Each recipient is a full topic URL.
    for recipient in ctx.to("ntfy") {
        let mut req = Request::post(&recipient)
            .header(
                "Title",
                format!("{}: {}", m.host, underscores_to_spaces(&ctx.args.name)),
            )
            .header("Tags", tag)
            .header("Priority", priority)
            .header(
                "Actions",
                format!("view, View node, {}, clear=true;", m.goto_url),
            )
            // curl's `--data` sent this without an explicit type; keep that on the wire.
            .raw(body.clone());
        if let Some((name, value)) = &auth_header {
            req = req.header(name, value.clone());
        }

        let resp = ctx.http.send(req).await;
        if resp.is(200) {
            log_sent("ntfy notification", &recipient, ctx.what());
            sent = true;
        } else {
            log_failed(
                "ntfy notification",
                &recipient,
                ctx.what(),
                &resp.code_str(),
            );
        }
    }
    sent
}

/// Basic auth wins over a token, as it did before.
fn ntfy_auth_header(ctx: &Ctx<'_>) -> Option<(String, String)> {
    let user = ctx.cfg.str("NTFY_USERNAME");
    let password = ctx.cfg.str("NTFY_PASSWORD");
    if !user.is_empty() && !password.is_empty() {
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        return Some(("Authorization".to_string(), format!("Basic {encoded}")));
    }
    let token = ctx.cfg.str("NTFY_ACCESS_TOKEN");
    if !token.is_empty() {
        return Some(("Authorization".to_string(), format!("Bearer {token}")));
    }
    None
}

pub async fn ilert(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("ilert") {
        return false;
    }
    let url = ctx.cfg.str("ILERT_ALERT_SOURCE_URL");
    if url.is_empty() {
        tracing::info!(
            "Can't send ilert notification, because ILERT_ALERT_SOURCE_URL is not defined"
        );
        return false;
    }
    let m = ctx.msg;
    let payload = json!({
        "alert": ctx.args.name,
        "alert_url": m.goto_url,
        "alarm_id": number_or_string(&ctx.args.alarm_id),
        "chart": ctx.args.chart,
        "date": ctx.args.when,
        "duration": m.duration_txt,
        "host": m.host,
        "info": ctx.args.info,
        "message": m.status_message,
        // The shell emitted this unquoted, which made the document invalid JSON.
        "severity": ctx.args.status,
        "total_critical": ctx.args.total_critical,
        "total_warnings": ctx.args.total_warnings,
        "value": ctx.args.value_string,
        "image_url": m.image,
        "src": ctx.args.src,
    });

    let resp = ctx
        .http
        .send(Request::post(url).json(payload.to_string()))
        .await;
    if resp.is_any(&[200, 202]) {
        tracing::info!("sent ilert event for {}", ctx.what());
        true
    } else {
        tracing::error!(
            "failed to send ilert event for {}, with HTTP response status code {}.",
            ctx.what(),
            resp.code_str()
        );
        false
    }
}

pub async fn signl4(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("signl4") {
        return false;
    }
    let url = ctx.cfg.str("SIGNL4_WEBHOOK_URL");
    let m = ctx.msg;
    let status = if ctx.args.status() == Status::Clear {
        "resolved"
    } else {
        "new"
    };
    let payload = json!({
        "title": ctx.args.name,
        "message": m.status_message,
        "alert_url": m.goto_url,
        "alarm_id": ctx.args.alarm_id,
        "chart": ctx.args.chart,
        "date": ctx.args.when,
        "duration": m.duration_txt,
        "host": m.host,
        "info": ctx.args.info,
        "severity": ctx.args.status,
        "total_critical": ctx.args.total_critical,
        "total_warnings": ctx.args.total_warnings,
        "value": ctx.args.value_string,
        "image_url": m.image,
        "src": ctx.args.src,
        "X-S4-ExternalID": ctx.args.unique_id,
        "X-S4-Status": status,
        "X-S4-SourceSystem": "Netdata",
    });

    let resp = ctx
        .http
        .send(Request::post(url).json(payload.to_string()))
        .await;
    if resp.is_any(&[200, 201, 202]) {
        tracing::info!("sent signl4 event for {}", ctx.what());
        true
    } else {
        tracing::error!(
            "failed to send signl4 event for {}, with HTTP response status code {}.",
            ctx.what(),
            resp.code_str()
        );
        false
    }
}

/// Emit a numeric JSON value when the string really is a number, so payload shapes
/// that were numeric before stay numeric.
pub fn number_or_string(s: &str) -> serde_json::Value {
    match s.trim().parse::<i64>() {
        Ok(n) => json!(n),
        Err(_) => json!(s),
    }
}

/// URL-encode helper kept here so senders that hand-build query strings share one
/// implementation with the rest of the crate.
pub fn encode(s: &str) -> String {
    urlencode(s)
}
