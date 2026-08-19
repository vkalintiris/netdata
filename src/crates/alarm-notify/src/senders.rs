//! The notification senders.
//!
//! One function per method, dispatched in the same order the shell script used, so
//! log streams and delivery ordering are unchanged. Each returns `true` when at
//! least one recipient was reached - that is what the daemon's exit code means.
//!
//! Payloads are built with `serde_json` rather than string concatenation. That is
//! the one deliberate departure from a literal port: the script hand-assembled JSON
//! and produced invalid documents for some inputs (an unquoted `nan`, an alert
//! description containing a quote). Every such fix is listed in the crate README.

pub mod chat;
pub mod email;
pub mod incident;
pub mod irc;
pub mod push;
pub mod sms;
pub mod syslog;

use crate::args::AlertArgs;
use crate::config::Config;
use crate::custom::{self, CustomSender};
use crate::http::HttpClient;
use crate::message::Message;
use crate::recipients::Recipients;

pub struct Ctx<'a> {
    pub args: &'a AlertArgs,
    pub cfg: &'a Config,
    pub msg: &'a Message,
    pub http: &'a HttpClient,
    pub recipients: &'a Recipients,
    pub debug: bool,
}

impl Ctx<'_> {
    /// Recipients for a method, whitespace separated as the senders expect.
    pub fn to(&self, method: &str) -> Vec<String> {
        self.recipients.get(method).to_vec()
    }

    pub fn enabled(&self, method: &str) -> bool {
        self.cfg.enabled(method)
    }

    /// The suffix every log line carries, so operators can grep one transition.
    pub fn what(&self) -> &str {
        &self.msg.notification_description
    }
}

/// Run every enabled method. Returns true if any of them delivered something.
pub async fn dispatch_all(ctx: &Ctx<'_>, custom_sender: Option<&CustomSender>) -> bool {
    let mut sent = false;

    // Order matches the shell script's dispatch sequence.
    sent |= chat::slack(ctx).await;
    sent |= chat::msteams(ctx).await;
    sent |= chat::rocketchat(ctx).await;
    sent |= chat::alerta(ctx).await;
    sent |= chat::flock(ctx).await;
    sent |= chat::discord(ctx).await;
    sent |= push::pushover(ctx).await;
    sent |= push::pushbullet(ctx).await;
    sent |= sms::twilio(ctx).await;
    sent |= sms::messagebird(ctx).await;
    sent |= sms::smseagle(ctx).await;
    sent |= sms::kavenegar(ctx).await;
    sent |= chat::telegram(ctx).await;
    sent |= incident::kafka(ctx).await;
    sent |= incident::pagerduty(ctx).await;
    sent |= chat::fleep(ctx).await;
    sent |= push::prowl(ctx).await;
    sent |= irc::irc(ctx).await;
    sent |= sms::smstools3(ctx);
    sent |= dispatch_custom(ctx, custom_sender);
    sent |= chat::hipchat(ctx).await;
    sent |= incident::awssns(ctx);
    sent |= chat::matrix(ctx).await;
    sent |= syslog::syslog(ctx);
    sent |= email::email(ctx);
    sent |= incident::dynatrace(ctx).await;
    sent |= incident::opsgenie(ctx).await;
    sent |= push::gotify(ctx).await;
    sent |= push::ntfy(ctx).await;
    sent |= push::ilert(ctx).await;
    sent |= push::signl4(ctx).await;

    sent
}

fn dispatch_custom(ctx: &Ctx<'_>, sender: Option<&CustomSender>) -> bool {
    if !ctx.enabled("custom") {
        return false;
    }
    let recipients = ctx.recipients.joined("custom");
    if recipients.is_empty() {
        return false;
    }
    let Some(sender) = sender else {
        return false;
    };

    // The user's function expects the documented variable names in scope; exporting
    // them is what makes an unmodified `custom_sender()` body keep working.
    let env: Vec<(String, String)> = ctx
        .msg
        .template_vars(ctx.args, ctx.cfg)
        .into_iter()
        .filter(|(k, _)| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect();

    tracing::debug!("custom notification via {}", sender.describe());
    let ok = custom::dispatch(sender, &recipients, &env);
    if ok {
        tracing::info!(
            "sent custom notification to '{recipients}' for {}",
            ctx.what()
        );
    }
    ok
}

/// Log helper shared by every sender: one line per recipient, matching the wording
/// operators already grep for.
pub fn log_sent(method_label: &str, recipient: &str, what: &str) {
    tracing::info!("sent {method_label} to '{recipient}' for {what}");
}

pub fn log_failed(method_label: &str, recipient: &str, what: &str, code: &str) {
    tracing::error!(
        "failed to send {method_label} to '{recipient}' for {what}, with HTTP response status code {code}."
    );
}
