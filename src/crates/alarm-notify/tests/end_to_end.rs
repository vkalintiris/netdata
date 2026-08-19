//! End-to-end tests: run the real binary the way the daemon runs it, and assert on
//! what it puts on the wire.
//!
//! These are the tests that would catch a regression in a payload, so they check
//! field values rather than just "a request happened".

mod mock_http;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use mock_http::MockServer;

const BIN: &str = env!("CARGO_BIN_EXE_alarm-notify");

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        for sub in ["stock", "user", "cache", "registry"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        std::fs::write(dir.path().join("stock/health_alarm_notify.conf"), config).unwrap();
        std::fs::write(
            dir.path().join("registry/netdata.public.unique.id"),
            "agent-guid",
        )
        .unwrap();
        Self { dir }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.env("NETDATA_STOCK_CONFIG_DIR", self.path("stock"))
            .env("NETDATA_USER_CONFIG_DIR", self.path("user"))
            .env("NETDATA_CACHE_DIR", self.path("cache"))
            .env("NETDATA_REGISTRY_DIR", self.path("registry"))
            .env("NETDATA_REGISTRY_UNIQUE_ID", "agent-guid")
            .env("NETDATA_REGISTRY_URL", "http://registry.example")
            .env("TZ", "UTC");
        cmd
    }

    /// One fixed transition, so no assertion depends on the clock.
    fn notify(&self, status: &str, old_status: &str) -> std::process::Output {
        self.command()
            .args(alert_args(status, old_status))
            .output()
            .expect("run notifier")
    }
}

fn alert_args(status: &str, old_status: &str) -> Vec<String> {
    [
        "sysadmin",
        "node1",
        "11",
        "22",
        "33",
        "1700000000",
        "disk_space_usage",
        "disk_space./",
        status,
        old_status,
        "91.5",
        "80",
        "health.d/disks.conf:12",
        "120",
        "3600",
        "%",
        "disk is almost full",
        "91.5%",
        "80%",
        "$used > 90",
        "used = 91.5",
        "2",
        "1",
        "",
        "",
        "Utilization",
        "sudo edit-config health.d/disks.conf=12=node1",
        "guid-child",
        "aaaa-bbbb-cccc",
        "disk_space_usage",
        "disk.space",
        "Disk",
        "System",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[test]
fn slack_payload_carries_the_expected_fields() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SLACK_WEBHOOK_URL=\"{}/slack\"\nDEFAULT_RECIPIENT_SLACK=\"alerts\"\n",
        mock.base_url
    ));

    let out = env.notify("WARNING", "CLEAR");
    assert!(out.status.success(), "exit code should report delivery");
    mock.wait_for(1);

    let req = mock.request_for("/slack");
    assert_eq!(req.method, "POST");
    let form = req.form();
    let payload: serde_json::Value =
        serde_json::from_str(&form["payload"]).expect("slack payload is JSON");

    // A bare recipient name is addressed as a channel.
    assert_eq!(payload["channel"], "#alerts");
    assert_eq!(payload["username"], "netdata on node1");
    assert_eq!(
        payload["text"],
        "node1 needs attention, `disk_space./`, *disk space usage = 91.5%*"
    );
    let attachment = &payload["attachments"][0];
    assert_eq!(attachment["color"], "warning");
    assert_eq!(attachment["title"], "disk space usage = 91.5%");
    assert_eq!(attachment["text"], "disk is almost full");
    assert_eq!(attachment["ts"], 1_700_000_000_i64);
    assert!(
        attachment["title_link"]
            .as_str()
            .unwrap()
            .contains("registry-alert-redirect.html")
    );
}

#[test]
fn status_specific_colours_and_wording() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SLACK_WEBHOOK_URL=\"{}/slack\"\nDEFAULT_RECIPIENT_SLACK=\"alerts\"\n",
        mock.base_url
    ));

    for (status, old, colour, wording) in [
        ("CRITICAL", "WARNING", "danger", "is critical"),
        ("CLEAR", "CRITICAL", "good", "recovered"),
    ] {
        let out = env.notify(status, old);
        assert!(out.status.success(), "{status} should be delivered");
        let expected = mock.requests_for("/slack").len() + 1;
        mock.wait_for(expected - 1);
        let req = mock.requests_for("/slack").pop().expect("a request");
        let payload: serde_json::Value = serde_json::from_str(&req.form()["payload"]).unwrap();
        assert_eq!(payload["attachments"][0]["color"], colour, "for {status}");
        assert!(
            payload["text"].as_str().unwrap().contains(wording),
            "expected '{wording}' in {}",
            payload["text"]
        );
    }
}

#[test]
fn several_methods_dispatch_from_one_invocation() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        r##"
SEND_EMAIL=NO
SEND_SYSLOG=NO
SEND_SMS=NO
SEND_AWSSNS=NO
SLACK_WEBHOOK_URL="{base}/slack"
DEFAULT_RECIPIENT_SLACK="alerts"
DISCORD_WEBHOOK_URL="{base}/discord"
DEFAULT_RECIPIENT_DISCORD="alerts"
ROCKETCHAT_WEBHOOK_URL="{base}/rocketchat"
DEFAULT_RECIPIENT_ROCKETCHAT="alerts"
ALERTA_WEBHOOK_URL="{base}/alerta"
DEFAULT_RECIPIENT_ALERTA="production"
GOTIFY_APP_URL="{base}/gotify"
GOTIFY_APP_TOKEN="token"
DEFAULT_RECIPIENT_GOTIFY="gotify"
SEND_ILERT=YES
ILERT_ALERT_SOURCE_URL="{base}/ilert"
SEND_SIGNL4=YES
SIGNL4_WEBHOOK_URL="{base}/signl4"
"##,
        base = mock.base_url
    ));

    let out = env.notify("CRITICAL", "WARNING");
    assert!(out.status.success());
    mock.wait_for(6);

    // Discord uses Slack's compatibility endpoint.
    assert_eq!(mock.request_for("/discord/slack").method, "POST");

    let rocketchat = mock.request_for("/rocketchat").json();
    assert_eq!(rocketchat["channel"], "#alerts");
    assert_eq!(rocketchat["alias"], "netdata on node1");
    // Rocket.Chat has always received the timestamp as a string.
    assert_eq!(rocketchat["attachments"][0]["ts"], "1700000000");

    let alerta = mock.request_for("/alerta/alert").json();
    assert_eq!(alerta["severity"], "critical");
    assert_eq!(alerta["resource"], "node1");
    assert_eq!(alerta["event"], "disk_space./.disk_space_usage");
    assert_eq!(alerta["environment"], "production");
    assert_eq!(alerta["service"][0], "Netdata");

    let gotify = mock.request_for("/gotify/message").json();
    assert_eq!(gotify["priority"], 10);
    assert_eq!(
        gotify["title"],
        "CRITICAL, disk_space_usage = 91.5%, on node1"
    );

    let ilert = mock.request_for("/ilert").json();
    assert_eq!(ilert["severity"], "CRITICAL");
    assert_eq!(ilert["alert"], "disk_space_usage");

    let signl4 = mock.request_for("/signl4").json();
    assert_eq!(signl4["X-S4-Status"], "new");
    assert_eq!(signl4["X-S4-SourceSystem"], "Netdata");
}

#[test]
fn a_recovery_marks_signl4_resolved() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SEND_SIGNL4=YES\nSIGNL4_WEBHOOK_URL=\"{}/signl4\"\n",
        mock.base_url
    ));
    assert!(env.notify("CLEAR", "CRITICAL").status.success());
    mock.wait_for(1);
    assert_eq!(
        mock.request_for("/signl4").json()["X-S4-Status"],
        "resolved"
    );
}

#[test]
fn telegram_addresses_topics_and_silences_recoveries() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         TELEGRAM_BOT_TOKEN=\"bot-token\"\nTELEGRAM_API_URL=\"{}/tg\"\n\
         DEFAULT_RECIPIENT_TELEGRAM=\"-1001234:42\"\n",
        mock.base_url
    ));

    assert!(env.notify("WARNING", "CLEAR").status.success());
    mock.wait_for(1);
    let req = mock.request_for("/sendMessage");
    assert!(req.path.contains("chat_id=-1001234"), "{}", req.path);
    assert!(req.path.contains("message_thread_id=42"), "{}", req.path);
    let form = req.form();
    assert_eq!(form["parse_mode"], "HTML");
    assert!(!form.contains_key("disable_notification"));
    assert!(
        form["text"].starts_with('\u{26a0}'),
        "warning emoji expected"
    );

    assert!(env.notify("CLEAR", "WARNING").status.success());
    mock.wait_for(2);
    let clear = mock.requests_for("/sendMessage").pop().unwrap();
    assert_eq!(clear.form()["disable_notification"], "true");
}

#[test]
fn ntfy_uses_headers_and_basic_auth() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         DEFAULT_RECIPIENT_NTFY=\"{}/ntfy/alerts\"\nNTFY_USERNAME=\"u\"\nNTFY_PASSWORD=\"p\"\n",
        mock.base_url
    ));

    assert!(env.notify("CRITICAL", "WARNING").status.success());
    mock.wait_for(1);
    let req = mock.request_for("/ntfy/alerts");
    assert_eq!(req.headers["tags"], "red_circle");
    assert_eq!(req.headers["priority"], "urgent");
    assert_eq!(req.headers["title"], "node1: disk space usage");
    // base64("u:p")
    assert_eq!(req.headers["authorization"], "Basic dTpw");
    assert!(req.body.contains("disk is almost full"));
}

#[test]
fn a_bearer_token_is_used_when_there_is_no_password() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         DEFAULT_RECIPIENT_NTFY=\"{}/ntfy/alerts\"\nNTFY_ACCESS_TOKEN=\"tk_123\"\n",
        mock.base_url
    ));
    assert!(env.notify("WARNING", "CLEAR").status.success());
    mock.wait_for(1);
    assert_eq!(
        mock.request_for("/ntfy/alerts").headers["authorization"],
        "Bearer tk_123"
    );
}

#[test]
fn opsgenie_maps_nan_to_null() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SEND_OPSGENIE=YES\nOPSGENIE_API_KEY=\"key\"\nOPSGENIE_API_URL=\"{}/og\"\n",
        mock.base_url
    ));

    let mut args = alert_args("WARNING", "CLEAR");
    args[10] = "nan".to_string(); // value
    args[11] = "80".to_string(); // old_value
    let out = env.command().args(&args).output().unwrap();
    assert!(out.status.success());
    mock.wait_for(1);

    let body = mock.request_for("/og/v1/json").json();
    assert!(body["value"].is_null(), "nan must serialise as null");
    // An integer must not become a float on the way through.
    assert_eq!(body["old_value"], 80);
    assert_eq!(body["priority"], "P3");
    assert_eq!(body["alarmId"], 22);
}

#[test]
fn a_failing_endpoint_yields_a_non_zero_exit_code() {
    let mock = MockServer::start(500);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SLACK_WEBHOOK_URL=\"{}/slack\"\nDEFAULT_RECIPIENT_SLACK=\"alerts\"\n",
        mock.base_url
    ));
    let out = env.notify("WARNING", "CLEAR");
    assert!(
        !out.status.success(),
        "a rejected notification must not report success"
    );
}

#[test]
fn statuses_that_are_not_transitions_are_ignored() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SLACK_WEBHOOK_URL=\"{}/slack\"\nDEFAULT_RECIPIENT_SLACK=\"alerts\"\n",
        mock.base_url
    ));
    assert!(!env.notify("REMOVED", "WARNING").status.success());
    // A CLEAR that follows a non-alarm state is not worth a notification.
    assert!(!env.notify("CLEAR", "UNDEFINED").status.success());
    assert!(mock.requests().is_empty(), "nothing should have been sent");
}

#[test]
fn clear_alarm_always_re_enables_those_recoveries() {
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         clear_alarm_always=\"YES\"\n\
         SLACK_WEBHOOK_URL=\"{}/slack\"\nDEFAULT_RECIPIENT_SLACK=\"alerts\"\n",
        mock.base_url
    ));
    assert!(env.notify("CLEAR", "UNDEFINED").status.success());
    mock.wait_for(1);
    assert_eq!(mock.requests_for("/slack").len(), 1);
}

#[test]
fn dump_methods_lists_enabled_methods_sorted() {
    let env = Env::new(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SLACK_WEBHOOK_URL=\"https://hooks.example/x\"\n\
         DISCORD_WEBHOOK_URL=\"https://discord.example/x\"\n",
    );
    let out = env.command().arg("dump_methods").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["SEND_DISCORD", "SEND_SLACK"]);
}

#[test]
fn unittest_reports_resolved_recipients_per_method() {
    let env = Env::new("");
    let config = env.path("user/health_alarm_notify.conf");
    std::fs::write(
        &config,
        "SLACK_WEBHOOK_URL=\"https://hooks.example/x\"\n\
         role_recipients_slack[dba]=\"#dba\"\n\
         DEFAULT_RECIPIENT_EMAIL=\"root\"\n",
    )
    .unwrap();

    let out = env
        .command()
        .args([
            "unittest",
            "dba",
            config.to_str().unwrap(),
            "WARNING",
            "CLEAR",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let by_method: HashMap<&str, &str> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("results: "))
        .map(|l| {
            let (m, r) = l.split_once(':').unwrap();
            (m, r.trim())
        })
        .collect();
    assert_eq!(by_method["slack"], "#dba");
    assert_eq!(by_method["email"], "root");
    assert_eq!(by_method["telegram"], "");
    // Every method with per-role recipients is reported.
    assert_eq!(by_method.len(), 27);
}

#[test]
fn nothing_is_written_to_stdout_during_normal_dispatch() {
    // The daemon creates a stdout pipe for the notifier but never drains it.
    let mock = MockServer::start(200);
    let env = Env::new(&format!(
        "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
         SLACK_WEBHOOK_URL=\"{}/slack\"\nDEFAULT_RECIPIENT_SLACK=\"alerts\"\n",
        mock.base_url
    ));
    let out = env.notify("WARNING", "CLEAR");
    assert!(
        out.stdout.is_empty(),
        "stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn the_custom_sender_shim_runs_an_existing_shell_function() {
    if cfg!(not(unix)) {
        return;
    }
    let mock = MockServer::start(200);
    let env = Env::new("");
    let marker = env.path("custom-ran.txt");

    // A plugins directory holding the shipped shim, as installed.
    let plugins = env.path("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let shim_source = Path::new(env!("CARGO_MANIFEST_DIR")).join("shims/custom-sender.sh");
    std::fs::copy(&shim_source, plugins.join("custom-sender.sh")).unwrap();

    std::fs::write(
        env.path("stock/health_alarm_notify.conf"),
        format!(
            r##"
SEND_EMAIL=NO
SEND_SYSLOG=NO
SEND_SMS=NO
SEND_AWSSNS=NO
SEND_CUSTOM=YES
DEFAULT_RECIPIENT_CUSTOM="ops-team"
custom_sender() {{
    printf '%s|%s|%s|%s\n' "${{1}}" "${{host}}" "${{status}}" "${{alarm}}" > {marker}
    info "custom sender ran"
    return 0
}}
"##,
            marker = marker.display()
        ),
    )
    .unwrap();

    let out = env
        .command()
        .env("NETDATA_PLUGINS_DIR", &plugins)
        .args(alert_args("WARNING", "CLEAR"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "custom sender should report delivery; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let recorded = std::fs::read_to_string(&marker).expect("custom_sender() wrote its marker");
    assert_eq!(
        recorded.trim(),
        "ops-team|node1|WARNING|disk space usage = 91.5%"
    );
    let _ = mock;
}

#[test]
fn a_custom_sender_command_receives_recipients_and_variables() {
    if cfg!(not(unix)) {
        return;
    }
    let env = Env::new("");
    let marker = env.path("exec-ran.txt");
    let program = env.path("sender.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$1\" \"$host\" \"$status\" > {}\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    std::fs::write(
        env.path("stock/health_alarm_notify.conf"),
        format!(
            "SEND_EMAIL=NO\nSEND_SYSLOG=NO\nSEND_SMS=NO\nSEND_AWSSNS=NO\n\
             SEND_CUSTOM=YES\nDEFAULT_RECIPIENT_CUSTOM=\"a b\"\n\
             CUSTOM_SENDER_COMMAND=\"{}\"\n",
            program.display()
        ),
    )
    .unwrap();

    let out = env
        .command()
        .args(alert_args("WARNING", "CLEAR"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().trim(),
        "a b|node1|WARNING"
    );
}
