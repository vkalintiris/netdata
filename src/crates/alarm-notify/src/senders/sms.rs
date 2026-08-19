//! SMS, voice and messaging-gateway senders.

use serde_json::json;

use crate::exec;
use crate::http::Request;
use crate::senders::{Ctx, log_failed, log_sent};
use crate::textutil::{truncate, underscores_to_spaces};

/// The multi-line body the SMS gateways receive.
fn sms_body(ctx: &Ctx<'_>) -> String {
    format!(
        "{}\nSeverity: {}\nChart: {}\n{}",
        ctx.msg.alarm, ctx.msg.severity, ctx.args.chart, ctx.args.info
    )
}

fn sms_title(ctx: &Ctx<'_>) -> String {
    format!(
        "{} {} - {} - {}",
        ctx.msg.host,
        ctx.msg.status_message,
        underscores_to_spaces(&ctx.args.name),
        ctx.args.chart
    )
}

pub async fn twilio(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("twilio") {
        return false;
    }
    let sid = ctx.cfg.str("TWILIO_ACCOUNT_SID");
    let token = ctx.cfg.str("TWILIO_ACCOUNT_TOKEN");
    let from = ctx.cfg.str("TWILIO_NUMBER");
    let body = format!("{} {}", sms_title(ctx), sms_body(ctx));
    let mut sent = false;

    for user in ctx.to("twilio") {
        let resp = ctx
            .http
            .send(
                Request::post(format!(
                    "https://api.twilio.com/2010-04-01/Accounts/{sid}/Messages.json"
                ))
                .basic_auth(sid, token)
                .form(vec![
                    ("From".to_string(), from.to_string()),
                    ("To".to_string(), user.clone()),
                    ("Body".to_string(), body.clone()),
                ]),
            )
            .await;
        if resp.is(201) {
            log_sent("Twilio SMS", &user, ctx.what());
            sent = true;
        } else {
            log_failed("Twilio SMS", &user, ctx.what(), &resp.code_str());
        }
    }
    sent
}

pub async fn messagebird(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("messagebird") {
        return false;
    }
    let key = ctx.cfg.str("MESSAGEBIRD_ACCESS_KEY");
    let originator = ctx.cfg.str("MESSAGEBIRD_NUMBER");
    let body = format!("{} {}", sms_title(ctx), sms_body(ctx));
    let mut sent = false;

    for user in ctx.to("messagebird") {
        let resp = ctx
            .http
            .send(
                Request::post("https://rest.messagebird.com/messages")
                    .header("Authorization", format!("AccessKey {key}"))
                    .form(vec![
                        ("originator".to_string(), originator.to_string()),
                        ("recipients".to_string(), user.clone()),
                        ("body".to_string(), body.clone()),
                        ("datacoding".to_string(), "auto".to_string()),
                    ]),
            )
            .await;
        if resp.is(201) {
            log_sent("Messagebird SMS", &user, ctx.what());
            sent = true;
        } else {
            log_failed("Messagebird SMS", &user, ctx.what(), &resp.code_str());
        }
    }
    sent
}

pub async fn smseagle(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("smseagle") {
        return false;
    }
    let address = ctx.cfg.str("SMSEAGLE_API_URL").trim_end_matches('/');
    let token = ctx.cfg.str("SMSEAGLE_API_ACCESSTOKEN");
    let msg_type = ctx.cfg.str("SMSEAGLE_MSG_TYPE");
    let recipients = ctx.to("smseagle");
    if recipients.is_empty() {
        return false;
    }

    let endpoint = match msg_type {
        "mms" => "messages/mms",
        "ring" => "calls/ring",
        "tts" => "calls/tts",
        "tts_advanced" => "calls/tts_advanced",
        _ => "messages/sms",
    };

    // The voice endpoints need a ring duration; only tts_advanced needs a voice.
    let voice_call = matches!(msg_type, "ring" | "tts" | "tts_advanced");
    let duration = {
        let configured = ctx.cfg.str("SMSEAGLE_CALL_DURATION").trim();
        if configured.is_empty() && voice_call {
            Some(10)
        } else {
            configured.parse::<i64>().ok()
        }
    };
    let voice_id = {
        let configured = ctx.cfg.str("SMSEAGLE_VOICE_ID").trim();
        if configured.is_empty() && msg_type == "tts_advanced" {
            Some(1)
        } else {
            configured.parse::<i64>().ok()
        }
    };

    let mut payload = json!({
        "to": recipients,
        "text": format!("{} {}: {}, {}", ctx.msg.host, ctx.msg.status_message, ctx.args.chart, ctx.msg.alarm),
    });
    // The shell always emitted both keys, producing `"duration": ,` - invalid JSON -
    // whenever they were empty. They are omitted instead when they do not apply.
    if let Some(d) = duration {
        payload["duration"] = json!(d);
    }
    if let Some(v) = voice_id {
        payload["voice_id"] = json!(v);
    }

    let resp = ctx
        .http
        .send(
            Request::post(format!("{address}/api/v2/{endpoint}"))
                .header("access-token", token)
                .header("Accept", "application/json")
                .json(payload.to_string()),
        )
        .await;

    if resp.is(200) {
        tracing::info!("Sending successful for {}", ctx.what());
        true
    } else {
        tracing::error!(
            "Sending failed for {}, with response code {}.",
            ctx.what(),
            resp.code_str()
        );
        false
    }
}

pub async fn kavenegar(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("kavenegar") {
        return false;
    }
    let api_key = ctx.cfg.str("KAVENEGAR_API_KEY");
    let sender = ctx.cfg.str("KAVENEGAR_SENDER");
    let message = format!("{} {}", sms_title(ctx), sms_body(ctx));
    let mut sent = false;

    for user in ctx.to("kavenegar") {
        let resp = ctx
            .http
            .send(
                Request::post(format!(
                    "http://api.kavenegar.com/v1/{api_key}/sms/send.json"
                ))
                .form(vec![
                    ("sender".to_string(), sender.to_string()),
                    ("receptor".to_string(), user.clone()),
                    ("message".to_string(), message.clone()),
                ]),
            )
            .await;
        if resp.is(200) {
            log_sent("Kavenegar SMS", &user, ctx.what());
            sent = true;
        } else {
            log_failed("Kavenegar SMS", &user, ctx.what(), &resp.code_str());
        }
    }
    sent
}

/// smstools3: hands each message to the local `sendsms` helper.
pub fn smstools3(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("sms") {
        return false;
    }
    let Some(program) = &ctx.cfg.sendsms else {
        return false;
    };
    // Kept to one SMS worth of text, as before.
    let message = truncate(
        &format!(
            "{} {}: {}, {}",
            ctx.msg.host, ctx.msg.status_message, ctx.args.chart, ctx.msg.alarm
        ),
        160,
    );
    let mut sent = false;

    for phone in ctx.to("sms") {
        match exec::run(program, [phone.as_str(), message.as_str()], None, &[]) {
            Ok(out) if out.success() => {
                log_sent("smstools3 SMS", &phone, ctx.what());
                sent = true;
            }
            Ok(out) => tracing::error!(
                "failed to send smstools3 SMS to '{phone}' for {}, with error code {:?}: {}.",
                ctx.what(),
                out.status,
                out.combined()
            ),
            Err(e) => tracing::error!(
                "failed to send smstools3 SMS to '{phone}' for {}: {e}",
                ctx.what()
            ),
        }
    }
    sent
}
