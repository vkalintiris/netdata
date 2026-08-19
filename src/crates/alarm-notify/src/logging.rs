//! Logging.
//!
//! The shell script piped every record into `systemd-cat-native --log-as-netdata`,
//! which is how alert-notification logs acquire their `ND_ALERT_*` fields and the
//! `MESSAGE_ID` operators filter on. Those fields are a contract - dashboards and
//! `journalctl` queries depend on them - so the same record is emitted here, written
//! straight to the journal socket instead of through a helper process.
//!
//! Where there is no journal (any non-Linux platform, or a container without one),
//! records go to stderr; the daemon captures and re-logs the notifier's stderr, so
//! they still reach the Agent's log.

use std::sync::Arc;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// The alert identity attached to every record.
#[derive(Default)]
pub struct LogContext {
    pub invocation_id: String,
    pub program_name: String,
    pub node: String,
    pub instance: String,
    pub context: String,
    pub alert_name: String,
    pub alert_id: String,
    pub unique_id: String,
    pub event_id: String,
    pub transition_id: String,
    pub class: String,
    pub component: String,
    pub alert_type: String,
    pub recipient: String,
    pub value: String,
    pub value_old: String,
    pub status: String,
    pub status_old: String,
    pub units: String,
    pub summary: String,
    pub info: String,
    pub duration: String,
    pub request: String,
}

impl LogContext {
    pub fn from_args(argv: &[String], program_name: &str) -> Self {
        let mut ctx = Self {
            invocation_id: std::env::var("NETDATA_INVOCATION_ID").unwrap_or_default(),
            program_name: program_name.to_string(),
            // The full command line, for correlating a record with its transition.
            request: format!(
                "'{program_name}' {}",
                argv.iter().map(|a| format!("'{a}' ")).collect::<String>()
            ),
            ..Default::default()
        };

        if let crate::args::Invocation::Notify(a) = crate::args::parse(argv) {
            ctx.node = a.args_host.clone();
            ctx.instance = a.chart.clone();
            ctx.context = a.context.clone();
            ctx.alert_name = a.name.clone();
            ctx.alert_id = a.alarm_id.clone();
            ctx.unique_id = a.unique_id.clone();
            ctx.event_id = a.event_id.clone();
            ctx.transition_id = a.transition_id_compact();
            ctx.class = a.classification.clone();
            ctx.component = a.component.clone();
            ctx.alert_type = a.alert_type.clone();
            ctx.recipient = a.roles.clone();
            ctx.value = a.value.clone();
            ctx.value_old = a.old_value.clone();
            ctx.status = a.status.clone();
            ctx.status_old = a.old_status.clone();
            ctx.units = a.units.clone();
            ctx.summary = a.summary.clone();
            ctx.info = a.info.clone();
            ctx.duration = a.duration.clone();
        }
        ctx
    }

    fn fields(&self, priority: u8, message: &str) -> Vec<(&'static str, String)> {
        vec![
            ("INVOCATION_ID", self.invocation_id.clone()),
            ("SYSLOG_IDENTIFIER", self.program_name.clone()),
            ("PRIORITY", priority.to_string()),
            ("THREAD_TAG", "alarm-notify".to_string()),
            ("ND_LOG_SOURCE", "health".to_string()),
            ("ND_NIDL_NODE", self.node.clone()),
            ("ND_NIDL_INSTANCE", self.instance.clone()),
            ("ND_NIDL_CONTEXT", self.context.clone()),
            ("ND_ALERT_NAME", self.alert_name.clone()),
            ("ND_ALERT_ID", self.alert_id.clone()),
            ("ND_ALERT_UNIQUE_ID", self.unique_id.clone()),
            ("ND_ALERT_EVENT_ID", self.event_id.clone()),
            ("ND_ALERT_TRANSITION_ID", self.transition_id.clone()),
            ("ND_ALERT_CLASS", self.class.clone()),
            ("ND_ALERT_COMPONENT", self.component.clone()),
            ("ND_ALERT_TYPE", self.alert_type.clone()),
            ("ND_ALERT_RECIPIENT", self.recipient.clone()),
            ("ND_ALERT_VALUE", self.value.clone()),
            ("ND_ALERT_VALUE_OLD", self.value_old.clone()),
            ("ND_ALERT_STATUS", self.status.clone()),
            ("ND_ALERT_STATUS_OLD", self.status_old.clone()),
            ("ND_ALERT_UNITS", self.units.clone()),
            ("ND_ALERT_SUMMARY", self.summary.clone()),
            ("ND_ALERT_INFO", self.info.clone()),
            ("ND_ALERT_DURATION", self.duration.clone()),
            ("ND_REQUEST", self.request.clone()),
            ("MESSAGE_ID", "6db0018e83e34320ae2a659d78019fb7".to_string()),
            (
                "MESSAGE",
                format!("[ALERT NOTIFICATION]: {}", message.replace('\n', "\\n")),
            ),
        ]
    }
}

/// Netdata's eight syslog priorities.
fn priority_of(level: &Level) -> u8 {
    match *level {
        Level::ERROR => 3,
        Level::WARN => 4,
        Level::INFO => 6,
        Level::DEBUG | Level::TRACE => 7,
    }
}

/// Map `NETDATA_LOG_LEVEL` onto a tracing level, as `set_log_min_priority()` did.
fn level_from_env(debug: bool) -> Level {
    if debug {
        return Level::DEBUG;
    }
    match std::env::var("NETDATA_LOG_LEVEL")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "emerg" | "emergency" | "alert" | "crit" | "critical" | "err" | "error" => Level::ERROR,
        "warn" | "warning" => Level::WARN,
        "debug" => Level::DEBUG,
        // notice and info both surface at info; anything unset defaults to info.
        _ => Level::INFO,
    }
}

/// Install the logger. Returns true when records go to the journal.
pub fn init(ctx: LogContext, debug: bool) -> bool {
    let level = level_from_env(debug);
    let ctx = Arc::new(ctx);

    #[cfg(target_os = "linux")]
    if let Some(journal) = journal::Sink::open() {
        let layer = JournalLayer {
            ctx: ctx.clone(),
            sink: journal,
        };
        let _ = tracing_subscriber::registry()
            .with(layer.with_filter(tracing_subscriber::filter::LevelFilter::from_level(level)))
            .try_init();
        return true;
    }

    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(std::io::stderr)
                .with_filter(tracing_subscriber::filter::LevelFilter::from_level(level)),
        )
        .try_init();
    let _ = ctx;
    false
}

/// Extracts an event's `message` field.
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
            // `record_debug` quotes strings; the message is emitted verbatim.
            if self.message.starts_with('"') && self.message.ends_with('"') {
                self.message = self.message[1..self.message.len() - 1].replace("\\\"", "\"");
            }
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }
}

#[cfg(target_os = "linux")]
struct JournalLayer {
    ctx: Arc<LogContext>,
    sink: journal::Sink,
}

#[cfg(target_os = "linux")]
impl<S: Subscriber> Layer<S> for JournalLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let priority = priority_of(event.metadata().level());
        self.sink.send(&self.ctx.fields(priority, &visitor.message));
    }
}

#[cfg(target_os = "linux")]
mod journal {
    use std::os::unix::net::UnixDatagram;
    use std::path::Path;

    /// A connection to the local journal, speaking its native protocol.
    pub struct Sink {
        socket: UnixDatagram,
        path: String,
    }

    impl Sink {
        pub fn open() -> Option<Self> {
            // The daemon publishes the socket it uses; fall back to the standard one.
            let candidates = [
                std::env::var("NETDATA_SYSTEMD_JOURNAL_PATH").unwrap_or_default(),
                "/run/systemd/journal/socket".to_string(),
                "/var/run/systemd/journal/socket".to_string(),
            ];
            for path in candidates.into_iter().filter(|p| !p.is_empty()) {
                if !Path::new(&path).exists() {
                    continue;
                }
                if let Ok(socket) = UnixDatagram::unbound() {
                    return Some(Self { socket, path });
                }
            }
            None
        }

        pub fn send(&self, fields: &[(&'static str, String)]) {
            let mut buf = Vec::with_capacity(1024);
            for (name, value) in fields {
                if value.contains('\n') {
                    // Multi-line values use the length-prefixed binary form.
                    buf.extend_from_slice(name.as_bytes());
                    buf.push(b'\n');
                    buf.extend_from_slice(&(value.len() as u64).to_le_bytes());
                    buf.extend_from_slice(value.as_bytes());
                    buf.push(b'\n');
                } else {
                    buf.extend_from_slice(name.as_bytes());
                    buf.push(b'=');
                    buf.extend_from_slice(value.as_bytes());
                    buf.push(b'\n');
                }
            }
            // A failed log write must never take down a notification.
            let _ = self.socket.send_to(&buf, &self.path);
        }
    }
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;
