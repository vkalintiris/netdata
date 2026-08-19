//! E-mail delivery.
//!
//! Still handed to the local MTA through `sendmail -t`: that is where the user's
//! relay, authentication and rewriting rules live, and reimplementing SMTP here
//! would silently change which identity mail is sent as. On Windows, point the
//! `sendmail` setting at any native mail submission program.

use crate::exec;
use crate::senders::Ctx;
use crate::textutil::expand;

const PLAINTEXT_TEMPLATE: &str = include_str!("../../templates/email_plaintext.tpl");
const HTML_TEMPLATE: &str = include_str!("../../templates/email_html.tpl");
const BOUNDARY: &str = "multipart-boundary";

pub fn email(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("email") {
        return false;
    }
    let Some(sendmail) = &ctx.cfg.sendmail else {
        return false;
    };
    let recipients = ctx.to("email");
    if recipients.is_empty() {
        return false;
    }
    // RFC 822 address list.
    let to = recipients.join(", ");

    let message = build_message(ctx, &to);
    let (sender_email, sender_name) = parse_email_sender(ctx.cfg.str("EMAIL_SENDER"));

    let mut args: Vec<String> = vec!["-t".to_string()];
    if !sender_email.is_empty() {
        args.push("-f".to_string());
        args.push(sender_email);
    }
    if !sender_name.is_empty() && supports_f_flag(sendmail) {
        args.push("-F".to_string());
        args.push(sender_name);
    }

    if ctx.debug {
        tracing::debug!("running {} {:?}", sendmail.display(), args);
    }

    match exec::run(sendmail, &args, Some(message.as_bytes()), &[]) {
        Ok(out) if out.success() => {
            tracing::info!("sent email to '{to}' for {}", ctx.what());
            true
        }
        Ok(out) => {
            tracing::error!(
                "failed to send email to '{to}' for {}, with error code {:?} ({}).",
                ctx.what(),
                out.status,
                out.combined()
            );
            false
        }
        Err(e) => {
            tracing::error!("failed to send email to '{to}' for {}: {e}", ctx.what());
            false
        }
    }
}

/// Make a value safe to place in a header.
///
/// A CR or LF inside a header value would end the header and start a new one, so a
/// value carrying one could inject headers or body content. Nothing upstream can
/// currently deliver such a value - alert labels and streamed host names are both
/// filtered before they reach here - but this assembles headers from alert text, so
/// it does not rely on that.
fn header_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

/// Assemble the full RFC 822 message that goes to `sendmail -t`.
pub fn build_message(ctx: &Ctx<'_>, to: &str) -> String {
    let vars = ctx.msg.template_vars(ctx.args, ctx.cfg);
    let lookup = |k: &str| vars.get(k).map(String::as_str);

    let plaintext_only = ctx.cfg.str("EMAIL_PLAINTEXT_ONLY") == "YES";
    let subject = if plaintext_only {
        format!(
            "{} {} - {} - {}",
            ctx.msg.host,
            ctx.msg.status_message,
            crate::textutil::underscores_to_spaces(&ctx.args.name),
            ctx.args.chart
        )
    } else {
        ctx.msg.html_email_subject.clone()
    };

    let mut out = String::new();
    out.push_str(&format!("To: {}\n", header_value(to)));
    out.push_str(&format!("Subject: {}\n", header_value(&subject)));
    out.push_str("MIME-Version: 1.0\n");
    out.push_str(&format!(
        "Content-Type: multipart/alternative; boundary=\"{BOUNDARY}\"\n"
    ));
    // Threading keeps every transition of one alert in a single mail conversation.
    // The shell wrote a literal `\r\n` inside one header value here, producing a
    // single malformed header; these are two proper headers.
    if ctx.cfg.str("EMAIL_THREADING") != "NO" {
        let reference = format!("<{}-{}@{}>", ctx.args.chart, ctx.args.name, ctx.msg.host);
        let reference = header_value(&reference);
        out.push_str(&format!("In-Reply-To: {reference}\n"));
        out.push_str(&format!("References: {reference}\n"));
    }
    out.push_str(&format!(
        "X-Netdata-Severity: {}\n",
        header_value(&ctx.args.status.to_lowercase())
    ));
    out.push_str(&format!(
        "X-Netdata-Alert-Name: {}\n",
        header_value(&ctx.args.name)
    ));
    out.push_str(&format!(
        "X-Netdata-Chart: {}\n",
        header_value(&ctx.args.chart)
    ));
    out.push_str(&format!(
        "X-Netdata-Classification: {}\n",
        header_value(&ctx.args.classification)
    ));
    out.push_str(&format!(
        "X-Netdata-Host: {}\n",
        header_value(&ctx.msg.host)
    ));
    out.push_str(&format!(
        "X-Netdata-Role: {}\n",
        header_value(&ctx.args.roles)
    ));
    out.push('\n');
    out.push_str("This is a MIME-encoded multipart message\n");
    out.push('\n');
    out.push_str(&format!("--{BOUNDARY}\n"));
    out.push_str(&expand(PLAINTEXT_TEMPLATE, lookup));
    // A blank line closes each part before the next boundary.
    out.push('\n');
    if !plaintext_only {
        out.push_str(&format!("--{BOUNDARY}\n"));
        out.push_str(&expand(HTML_TEMPLATE, lookup));
        out.push('\n');
    }
    out.push_str(&format!("--{BOUNDARY}--\n"));
    out
}

/// Split `EMAIL_SENDER` into an address and an optional display name.
///
/// Accepted forms: `addr`, `Name <addr>`, `"Name" <addr>`, `'Name' <addr>`.
pub fn parse_email_sender(raw: &str) -> (String, String) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), String::new());
    }
    let Some(open) = raw.rfind('<') else {
        return (raw.to_string(), String::new());
    };
    let Some(close) = raw[open..].find('>') else {
        return (raw.to_string(), String::new());
    };

    let address = raw[open + 1..open + close].trim().to_string();
    let name = raw[..open]
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_string();
    (address, name)
}

/// Not every MTA implements `-F`; ask before using it, as the script did.
fn supports_f_flag(sendmail: &std::path::Path) -> bool {
    match exec::run(sendmail, ["-F"], None, &[]) {
        Ok(out) => !out.combined().contains("unrecognized option"),
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "email_tests.rs"]
mod tests;
