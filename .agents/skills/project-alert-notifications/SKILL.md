---
name: project-alert-notifications
description: How the Netdata Agent dispatches alert notifications - the `alarm-notify` Rust dispatcher, its 33-argument contract with the health engine, `health_alarm_notify.conf` parsing, the `custom_sender()` compatibility shims, and how to validate a change to any of it. Use when adding or changing a notification method, touching src/crates/alarm-notify, health_notifications.c, health.c's notification program resolution, analytics' dump_methods call, health_alarm_notify.conf, or the per-method metadata.yaml/README pairs under src/health/notifications/.
---

# Alert notifications

## The path, end to end

1. The health engine decides an alert transitioned and calls
   `health_send_notification()` (`src/health/health_notifications.c`).
2. `prepare_command()` fills an argv array: the program plus **33 positional
   arguments** in a fixed order. Nothing is quoted or escaped - it is argv, not a
   shell line.
3. `spawn_popen_run_argv()` execs it. **No shell is involved**, which is what lets
   notifications work on Windows.
4. The daemon waits up to `[health] notification execution timeout` (default 120s),
   then kills the child with code 128.
5. The exit code is stored as the alert's `exec_code` and forwarded to Netdata Cloud
   (`sqlite_health.c`, `sqlite_aclk_alert.c`). **0 means at least one notification was
   delivered.**

## Which program runs

Exactly one notifier is installed per build:

| Build | Installed | Chosen by |
|---|---|---|
| With a Rust toolchain | `plugins.d/alarm-notify` (`.exe` on Windows) | `health_notification_program_default()` in `src/health/health.c` |
| Without one | `plugins.d/alarm-notify.sh` | the same function, by falling through |

`ENABLE_ALARM_NOTIFY_NATIVE` (root `CMakeLists.txt`) probes for `cargo`/`rustc` and
the workspace MSRV. Missing toolchain: warn and install the shell notifier; on
Windows, fail the configure, because the shell notifier cannot run there.

`[health] script to execute on alarm` still overrides everything. If it names an
`alarm-notify.sh` that no longer exists, the sibling native program is used and a
warning tells the operator to update `netdata.conf`.

## Contracts you must not break

- **The 33 arguments** and their order. Adding one means changing
  `health_notifications.c`, `src/crates/alarm-notify/src/args.rs`, and the shell
  notifier together.
- **`dump_methods`** - `src/daemon/analytics.c` runs it and reads stdout. Output is
  the enabled `SEND_*` variable names, one per line, sorted.
- **`test [role]`** - documented in `integrations/templates/troubleshooting.md` and
  therefore in all 28 generated `src/health/notifications/*/README.md`.
- **Exit code 0 = delivered.**
- **journald fields** - `MESSAGE_ID=6db0018e83e34320ae2a659d78019fb7` plus the
  `ND_ALERT_*` set. Operators filter on these.
- **Silence on stdout during dispatch.** The daemon opens a stdout pipe and never
  reads it; a chatty notifier eventually blocks and gets killed at the timeout.

## Adding a notification method

1. `src/crates/alarm-notify/src/senders/` - add the sender to the right module and to
   `dispatch_all()`, keeping the existing dispatch order.
2. `src/crates/alarm-notify/src/config.rs` - add the method to `METHOD_NAMES` (per-role recipients) or
   `HOST_LEVEL_METHODS`, and add its required keys to `static_screening()`.
3. `src/health/notifications/health_alarm_notify.conf` - the `SEND_*`,
   `DEFAULT_RECIPIENT_*` and credential keys. A method in `HOST_LEVEL_METHODS` only
   activates when the configuration sets its `SEND_*`, so ship that line.
4. `src/health/notifications/<method>/metadata.yaml` - then regenerate:
   `python3 integrations/gen_integrations.py && python3 integrations/gen_docs_integrations.py`.
   The generator also refreshes unrelated collector docs that have drifted; restore
   those before committing.
5. Add an end-to-end test in `tests/end_to_end.rs` asserting the payload fields.

## Validating a change

Unit and integration tests:

```sh
cd src/crates && cargo test -p alarm-notify && cargo clippy -p alarm-notify --all-targets
```

Differential test against the shell notifier - the strongest evidence, and how the
port was verified. The shape:

1. Generate a runnable script: `sed` the `@..._POST@` placeholders in
   `alarm-notify.sh.in` to temporary directories.
2. Start a recording HTTP server and point every configurable endpoint at it
   (`SLACK_WEBHOOK_URL`, `TELEGRAM_API_URL`, `MATRIX_HOMESERVER`, `OPSGENIE_API_URL`,
   `GOTIFY_APP_URL`, `DEFAULT_RECIPIENT_NTFY`, ... - 16 of them can be redirected).
3. Point `sendmail`, `aws`, `sendsms`, `logger` and `nc` at recorder scripts; the
   config keys of those names exist precisely for this.
4. Run both binaries with the same 33 arguments and a fixed `when`, then compare
   requests field by field, and the captured e-mail with `diff`.

Live test: install to a temporary prefix, run `netdata -D` with a
`CUSTOM_SENDER_COMMAND` that writes a file, and confirm the notification arrives and
the journal records carry the right fields. Start the daemon with
`systemd-run --user`; a plain background process gets reaped.

## Pitfalls

- **Empty arguments used to vanish.** The spawn server's argv encoding dropped
  zero-length strings, shifting every later argument. Fixed in
  `spawn_server_nofork.c` (length-prefixed format). Alert fields such as
  `total_warn_alarms`, `context` and `component` are routinely empty, so any new argv
  caller depends on this.
- **`health_alarm_notify.conf` is shell, not data.** The parser
  (`src/crates/alarm-notify/src/conf_parser.rs`) covers a documented subset and
  reports anything else with file and line. Extend the parser rather than asking
  users to change their files.
- **`custom_sender()` is user-written shell.** It runs through the shipped
  `custom-sender.sh` shim; `CUSTOM_SENDER_COMMAND` is the portable path and the only
  one on Windows. Never break either.
- **Do not "fix" curl's defaults.** `--data` without a content type sends
  `application/x-www-form-urlencoded`, and several receivers were configured against
  that. `http::Body::Raw` with no content type reproduces it deliberately.
- Payload deviations from the shell script are listed in
  `src/crates/alarm-notify/README.md`. Add to that list rather than silently
  diverging.
