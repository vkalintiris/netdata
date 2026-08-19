//! Team-chat and incident-feed webhooks.

use serde_json::json;

use crate::args::Status;
use crate::http::Request;
use crate::senders::{Ctx, log_failed, log_sent};
use crate::textutil::{truncate_with_ellipsis, underscores_to_spaces, urlencode};

/// Slack's legacy attachment colours, shared by several Slack-shaped webhooks.
fn attachment_color(status: Status) -> &'static str {
    match status {
        Status::Warning => "warning",
        Status::Critical => "danger",
        Status::Clear => "good",
        Status::Other => "#777777",
    }
}

fn status_emoji(status: Status) -> &'static str {
    match status {
        Status::Warning => "⚠️",
        Status::Critical => "🔴",
        Status::Clear => "✅",
        Status::Other => "⚪️",
    }
}

pub async fn slack(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("slack") {
        return false;
    }
    let webhook = ctx.cfg.str("SLACK_WEBHOOK_URL");
    let color = attachment_color(ctx.args.status());
    let m = ctx.msg;
    let mut sent = false;

    for channel in ctx.to("slack") {
        // A bare name means a channel; `@` addresses a user. A lone `#` means
        // "whatever the webhook is configured for", so the field is omitted.
        let channel = if channel.starts_with('#') || channel.starts_with('@') {
            channel
        } else {
            format!("#{channel}")
        };
        let (channel_field, label) = if channel == "#" {
            (None, "without specifying a channel".to_string())
        } else {
            (Some(channel.clone()), format!("to '{channel}'"))
        };

        let mut payload = json!({
            "username": format!("netdata on {}", m.host),
            "icon_url": format!("{}/images/banner-icon-144x144.png", m.images_base_url),
            "text": format!("{} {}, `{}`, *{}*", m.host, m.status_message, ctx.args.chart, m.alarm),
            "attachments": [{
                "fallback": format!("{} - {} - {}", m.alarm, ctx.args.chart, ctx.args.info),
                "color": color,
                "title": m.alarm,
                "title_link": m.goto_url,
                "text": ctx.args.info,
                "fields": [{ "title": ctx.args.chart, "value": "chart", "short": true }],
                "thumb_url": m.image,
                "footer": format!("by {}", m.host),
                "ts": ctx.args.when_secs(),
            }],
        });
        if let Some(ch) = channel_field {
            payload["channel"] = json!(ch);
        }

        let resp = ctx
            .http
            .send(Request::post(webhook).form(vec![("payload".into(), payload.to_string())]))
            .await;

        if resp.is(200) {
            tracing::info!("sent slack notification {label} for {}", ctx.what());
            sent = true;
        } else {
            tracing::error!(
                "failed to send slack notification {label} for {}, with HTTP response status code {}.",
                ctx.what(),
                resp.code_str()
            );
        }
    }
    sent
}

pub async fn msteams(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("msteams") {
        return false;
    }
    let webhook = ctx.cfg.str("MSTEAMS_WEBHOOK_URL");
    let (icon_key, color_key) = match ctx.args.status() {
        Status::Warning => ("MSTEAMS_ICON_WARNING", "MSTEAMS_COLOR_WARNING"),
        Status::Critical => ("MSTEAMS_ICON_CRITICAL", "MSTEAMS_COLOR_CRITICAL"),
        Status::Clear => ("MSTEAMS_ICON_CLEAR", "MSTEAMS_COLOR_CLEAR"),
        Status::Other => ("MSTEAMS_ICON_DEFAULT", "MSTEAMS_COLOR_DEFAULT"),
    };
    let icon = ctx.cfg.str(icon_key);
    let color = ctx.cfg.str(color_key);
    let m = ctx.msg;
    let mut sent = false;

    for channel in ctx.to("msteams") {
        // The webhook URL may contain the literal token CHANNEL as a placeholder.
        let url = webhook.replace("CHANNEL", &channel);
        let payload = json!({
            "@context": "http://schema.org/extensions",
            "@type": "MessageCard",
            "themeColor": color,
            "title": format!("{icon} Alert {} from netdata for {}", ctx.args.status, m.host),
            "text": format!("{} {}, {}, *{}*", m.host, m.status_message, ctx.args.chart, m.alarm),
            "potentialAction": [{
                "@type": "OpenUri",
                "name": "Netdata",
                "targets": [{ "os": "default", "uri": m.goto_url }],
            }],
        });

        let resp = ctx
            .http
            .send(Request::post(&url).json(payload.to_string()))
            .await;
        if resp.is(200) {
            log_sent("Microsoft team notification", &channel, ctx.what());
            sent = true;
        } else {
            log_failed("Microsoft team", &channel, ctx.what(), &resp.code_str());
        }
    }
    sent
}

pub async fn rocketchat(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("rocketchat") {
        return false;
    }
    let webhook = ctx.cfg.str("ROCKETCHAT_WEBHOOK_URL");
    let color = attachment_color(ctx.args.status());
    let m = ctx.msg;
    let mut sent = false;

    for channel in ctx.to("rocketchat") {
        let payload = json!({
            "channel": format!("#{channel}"),
            "alias": format!("netdata on {}", m.host),
            "avatar": format!("{}/images/banner-icon-144x144.png", m.images_base_url),
            "text": format!("{} {}, `{}`, *{}*", m.host, m.status_message, ctx.args.chart, m.alarm),
            "attachments": [{
                "color": color,
                "title": m.alarm,
                "title_link": m.goto_url,
                "text": ctx.args.info,
                "fields": [{ "title": ctx.args.chart, "short": true, "value": "chart" }],
                "thumb_url": m.image,
                // Rocket.Chat receives the timestamp as a string, as it always has.
                "ts": ctx.args.when,
            }],
        });

        let resp = ctx
            .http
            .send(Request::post(webhook).json(payload.to_string()))
            .await;
        if resp.is(200) {
            log_sent("rocketchat notification", &channel, ctx.what());
            sent = true;
        } else {
            log_failed(
                "rocketchat notification",
                &channel,
                ctx.what(),
                &resp.code_str(),
            );
        }
    }
    sent
}

pub async fn alerta(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("alerta") {
        return false;
    }
    let webhook = ctx.cfg.str("ALERTA_WEBHOOK_URL");
    let severity = match ctx.args.status() {
        Status::Critical => "critical",
        Status::Warning => "warning",
        Status::Clear => "cleared",
        Status::Other => "indeterminate",
    };
    // httpcheck alerts are per-endpoint, so the chart is the better resource id.
    let (resource, event) = if ctx.args.chart.starts_with("httpcheck") {
        (ctx.args.chart.clone(), ctx.args.name.clone())
    } else {
        (
            ctx.msg.host.clone(),
            format!("{}.{}", ctx.args.chart, ctx.args.name),
        )
    };
    let m = ctx.msg;
    let mut sent = false;

    for environment in ctx.to("alerta") {
        let payload = json!({
            "resource": resource,
            "event": event,
            "environment": environment,
            "severity": severity,
            "service": ["Netdata"],
            "group": "Performance",
            "value": ctx.args.value_string,
            "text": ctx.args.info,
            "tags": [format!("alarm_id:{}", ctx.args.alarm_id)],
            "attributes": {
                "roles": ctx.args.roles,
                "name": ctx.args.name,
                "chart": ctx.args.chart,
                "source": ctx.args.src,
                "moreInfo": format!("<a href=\"{}\">View Netdata</a>", m.goto_url),
            },
            "origin": format!("netdata/{}", m.host),
            "type": "netdataAlarm",
            // The shell read BASH_ARGV here, which is only populated under
            // `shopt -s extdebug`, so this field was always empty in practice.
            "rawData": raw_data(ctx),
        });

        let mut req = Request::post(format!("{webhook}/alert")).json(payload.to_string());
        let api_key = ctx.cfg.str("ALERTA_API_KEY");
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Key {api_key}"));
        }

        let resp = ctx.http.send(req).await;
        if resp.is_any(&[200, 201]) {
            log_sent("alerta notification", &environment, ctx.what());
            sent = true;
        } else if resp.is(202) {
            // Alerta answers 202 when it suppressed the alert; that is not a delivery.
            tracing::info!(
                "suppressed alerta notification to '{environment}' for {}",
                ctx.what()
            );
        } else {
            log_failed(
                "alerta notification",
                &environment,
                ctx.what(),
                &resp.code_str(),
            );
        }
    }
    sent
}

/// Alerta's `rawData`, reproducing what `"${BASH_ARGV[@]}"` expanded to: the
/// notification's arguments, in reverse order.
fn raw_data(ctx: &Ctx<'_>) -> String {
    let mut args = ctx.args.as_positional();
    args.reverse();
    args.join(" ")
}

pub async fn flock(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("flock") {
        return false;
    }
    let webhook = ctx.cfg.str("FLOCK_WEBHOOK_URL");
    let color = attachment_color(ctx.args.status());
    let m = ctx.msg;
    let mut sent = false;

    // The payload never referenced the channel; one identical POST per recipient is
    // what the script did, and Flock routes by webhook.
    for channel in ctx.to("flock") {
        let payload = json!({
            "sendAs": {
                "name": format!("netdata on {}", m.host),
                "profileImage": format!("{}/images/banner-icon-144x144.png", m.images_base_url),
            },
            "text": format!("{} *{}*", m.host, m.status_message),
            "timestamp": ctx.args.when,
            "attachments": [{
                "description": format!("{} - {}", ctx.args.chart, ctx.args.info),
                "color": color,
                "title": m.alarm,
                "url": m.goto_url,
                "text": ctx.args.info,
                "views": {
                    "image": {
                        "original": { "src": m.image, "width": 400, "height": 400 },
                        "thumbnail": { "src": m.image, "width": 50, "height": 50 },
                        "filename": m.image,
                    }
                },
            }],
        });

        let resp = ctx
            .http
            .send(Request::post(webhook).json(payload.to_string()))
            .await;
        if resp.is(200) {
            log_sent("flock notification", &channel, ctx.what());
            sent = true;
        } else {
            log_failed("flock notification", &channel, ctx.what(), &resp.code_str());
        }
    }
    sent
}

pub async fn discord(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("discord") {
        return false;
    }
    // Discord's Slack-compatibility endpoint.
    let webhook = format!("{}/slack", ctx.cfg.str("DISCORD_WEBHOOK_URL"));
    let color = attachment_color(ctx.args.status());
    let m = ctx.msg;
    let mut sent = false;

    for channel in ctx.to("discord") {
        // Discord rejects usernames longer than 32 characters.
        let username = truncate_with_ellipsis(&format!("netdata on {}", m.host), 32, 29);
        let payload = json!({
            "channel": format!("#{channel}"),
            "username": username,
            "text": format!("{} {}, `{}`, *{}*", m.host, m.status_message, ctx.args.chart, m.alarm),
            "icon_url": format!("{}/images/banner-icon-144x144.png", m.images_base_url),
            "attachments": [{
                "color": color,
                "title": m.alarm,
                "title_link": m.goto_url,
                "text": ctx.args.info,
                "fields": [{ "title": ctx.args.chart }],
                "thumb_url": m.image,
                "footer_icon": format!("{}/images/banner-icon-144x144.png", m.images_base_url),
                "footer": m.host,
                "ts": ctx.args.when_secs(),
            }],
        });

        let resp = ctx
            .http
            .send(Request::post(&webhook).form(vec![("payload".into(), payload.to_string())]))
            .await;
        if resp.is(200) {
            log_sent("discord notification", &channel, ctx.what());
            sent = true;
        } else {
            log_failed(
                "discord notification",
                &channel,
                ctx.what(),
                &resp.code_str(),
            );
        }
    }
    sent
}

pub async fn fleep(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("fleep") {
        return false;
    }
    let sender = ctx.cfg.str("FLEEP_SENDER");
    let m = ctx.msg;
    // A real newline: the script's `\n` was literal text inside a body that was not
    // valid JSON, so a parsing receiver saw the two characters rather than a break.
    let message = format!(
        "{} {}, `{}`, *{}*\n{}",
        m.host, m.status_message, ctx.args.chart, m.alarm, ctx.args.info
    );
    let mut sent = false;

    for hook in ctx.to("fleep") {
        let payload = json!({ "message": message, "user": sender });
        let resp = ctx
            .http
            .send(Request::post(format!("https://fleep.io/hook/{hook}")).json(payload.to_string()))
            .await;
        if resp.is(200) {
            tracing::info!("sent fleep data to user '{sender}' for {}", ctx.what());
            sent = true;
        } else {
            tracing::error!(
                "failed to send fleep data to user '{sender}' for {}, with HTTP response status code {}.",
                ctx.what(),
                resp.code_str()
            );
        }
    }
    sent
}

pub async fn hipchat(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("hipchat") {
        return false;
    }
    let server = ctx.cfg.str("HIPCHAT_SERVER");
    let token = ctx.cfg.str("HIPCHAT_AUTH_TOKEN");
    if server.is_empty() || token.is_empty() {
        return false;
    }
    let color = match ctx.args.status() {
        Status::Warning => "yellow",
        Status::Critical => "red",
        Status::Clear => "green",
        Status::Other => "gray",
    };
    let m = ctx.msg;
    // HipChat renders no <small>, so the script stripped those tags.
    let message = format!(
        " {} {}<br/> <b>{}</b> {}<br/> <b>{}</b><br/> <b>{}{}</b><br/> <a href=\"{}\">View netdata dashboard</a> (source of alarm {}) ",
        m.host,
        m.status_message,
        m.alarm,
        m.info_html,
        ctx.args.chart,
        m.date,
        m.raised_for_html,
        m.goto_url,
        ctx.args.src
    )
    .replace("<small>", "")
    .replace("</small>", "");
    let mut sent = false;

    for room in ctx.to("hipchat") {
        let payload = json!({
            "color": color,
            "from": m.host,
            "message_format": "html",
            "message": message,
            "notify": "true",
        });
        let resp = ctx
            .http
            .send(
                Request::post(format!("https://{server}/v2/room/{room}/notification"))
                    .header("Content-type", "application/json")
                    .header("Authorization", format!("Bearer {token}"))
                    .json(payload.to_string()),
            )
            .await;
        if resp.is(204) {
            log_sent("HipChat notification", &room, ctx.what());
            sent = true;
        } else {
            log_failed("HipChat notification", &room, ctx.what(), &resp.code_str());
        }
    }
    sent
}

pub async fn matrix(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("matrix") {
        return false;
    }
    let homeserver = ctx.cfg.str("MATRIX_HOMESERVER");
    let token = ctx.cfg.str("MATRIX_ACCESSTOKEN");
    if token.is_empty() {
        return false;
    }
    let emoji = status_emoji(ctx.args.status());
    let m = ctx.msg;
    let name_spaced = underscores_to_spaces(&ctx.args.name);
    let mut sent = false;

    for (i, room) in ctx.to("matrix").into_iter().enumerate() {
        // Matrix needs a unique transaction id per event; the process id and an
        // index keep it unique for concurrent notifications of the same second.
        let txnid = format!(
            "nd_{}_{}_{}",
            crate::datefmt::now_secs(),
            std::process::id(),
            i
        );
        let url = format!(
            "{homeserver}/_matrix/client/r0/rooms/{}/send/m.room.message/{txnid}",
            urlencode(&room)
        );
        let payload = json!({
            "msgtype": "m.notice",
            "format": "org.matrix.custom.html",
            "formatted_body": format!(
                "{emoji} {} {} - <b>{name_spaced}</b><br>{}<br><a href=\"{}\">{}</a><br><i>{}</i>",
                m.host, m.status_message, ctx.args.chart, m.goto_url, m.alarm, ctx.args.info
            ),
            "body": format!(
                "{emoji} {} {} - {name_spaced} {} {} {} {}",
                m.host, m.status_message, ctx.args.chart, m.goto_url, m.alarm, ctx.args.info
            ),
        });

        let resp = ctx
            .http
            .send(
                Request::put(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .json(payload.to_string()),
            )
            .await;
        if resp.is(200) {
            log_sent("Matrix notification", &room, ctx.what());
            sent = true;
        } else {
            log_failed("Matrix notification", &room, ctx.what(), &resp.code_str());
        }
    }
    sent
}

pub async fn telegram(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("telegram") {
        return false;
    }
    let token = ctx.cfg.str("TELEGRAM_BOT_TOKEN");
    let api = ctx.cfg.str("TELEGRAM_API_URL");
    let retries: u32 = ctx
        .cfg
        .str("TELEGRAM_RETRIES_ON_LIMIT")
        .trim()
        .parse()
        .unwrap_or(0);
    let emoji = status_emoji(ctx.args.status());
    let m = ctx.msg;
    let message = format!(
        "{} {} - <b>{}</b>\n{}\n<a href=\"{}\">{}</a>\n<i>{}</i>",
        m.host,
        m.status_message,
        underscores_to_spaces(&ctx.args.name),
        ctx.args.chart,
        m.goto_url,
        m.alarm,
        ctx.args.info
    );
    let mut sent = false;

    for chat in ctx.to("telegram") {
        // `chat_id[:thread_id]` addresses a topic inside a group.
        let (chat_id, thread_id) = match chat.split_once(':') {
            Some((c, t)) => (c.to_string(), Some(t.to_string())),
            None => (chat.clone(), None),
        };
        let mut url = format!("{api}/bot{token}/sendMessage?chat_id={chat_id}");
        if let Some(thread) = &thread_id {
            url.push_str(&format!("&message_thread_id={thread}"));
        }

        let mut attempts_left = retries;
        loop {
            let mut fields = vec![
                ("parse_mode".to_string(), "HTML".to_string()),
                ("disable_web_page_preview".to_string(), "true".to_string()),
                ("text".to_string(), format!("{emoji} {message}")),
            ];
            // Recoveries should not buzz anyone's phone.
            if ctx.args.status() == Status::Clear {
                fields.push(("disable_notification".to_string(), "true".to_string()));
            }

            let resp = ctx.http.send(Request::post(&url).form(fields)).await;
            match resp.status {
                Some(200) => {
                    log_sent("telegram notification", &chat, ctx.what());
                    sent = true;
                }
                Some(401) => tracing::error!(
                    "failed to send telegram notification to '{chat}' for {}, wrong bot token.",
                    ctx.what()
                ),
                Some(429) if attempts_left > 0 => {
                    tracing::error!(
                        "failed to send telegram notification to '{chat}' for {}, rate limit exceeded, retrying after 1s.",
                        ctx.what()
                    );
                    attempts_left -= 1;
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
                Some(429) => tracing::error!(
                    "failed to send telegram notification to '{chat}' for {}, rate limit exceeded.",
                    ctx.what()
                ),
                _ => log_failed("telegram notification", &chat, ctx.what(), &resp.code_str()),
            }
            break;
        }
    }
    sent
}
