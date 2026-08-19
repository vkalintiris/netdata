# Handoff: native (Rust) alert notification dispatcher

Point-in-time state of the `alarm-notify` work, written so a new session can resume
without re-deriving anything. Last updated 2026-08-20.

Durable knowledge does **not** live here - it lives in
`.agents/skills/project-alert-notifications/SKILL.md` (how to work on this),
`.agents/sow/specs/alert-notification-dispatch.md` (the contract) and
`src/crates/alarm-notify/README.md` (the crate, and every deviation from the shell
script). Read those first; this file covers only what is not in them: current state,
decisions and their reasons, how the work was validated, and what is still open.

## The task

- Origin: <https://github.com/netdata/netdata/pull/22650> (Windows/UCRT64, "Part II"),
  specifically comment `5315401582` - the PR was put back to draft because
  `alarm-notify.sh` "still hasn't been converted to Go".
- The user asked for the conversion **in Rust instead of Go**, and delegated every
  design decision, including the `custom_sender()` compatibility question.
- Root cause the work addresses: the notification dispatcher is a 3851-line bash script
  that shells out to `curl`, `sendmail`, `logger`, `nc`, `aws` and GNU `date`. PR #22650
  drops the bundled MSYS2 root, so Windows has none of them - not even a shell.

## Current state

- Branch `anotify`, a git worktree of the `master` checkout, based on
  `853ec203e8` (the `compile_on_windows` PR tip).
- **Nothing is pushed.** No PR exists. The user authorises pushes explicitly, per
  their standing instruction.
- 12 commits, 107 files, +10872/-334. Each commit builds on its own (bisectable).

| Commit | Scope |
|---|---|
| `84306d7207` | spawn server: stop dropping empty arguments when exec'ing via argv |
| `4583ce653e` | health: exec the notification program without a shell (argv) |
| `1889e1fddb` | the `alarm-notify` crate (49 files, ~8.6k lines Rust) |
| `5b945f0e4f` | build: `ENABLE_ALARM_NOTIFY_NATIVE`, install rules, spec/CPack |
| `98ac713005` | docs: rename the dispatcher, document the hooks, 28 regenerated READMEs |
| `8d591712df` | project skill |
| `7d12728ca0` | AGENTS.md skill index entry |
| `ded91b50b9` | spawn server: same fix for Windows, plus codec hardening (review) |
| `c0ffccdfe5` | crate: review findings (parser, panics, redaction, exit code, syslog, ...) |
| `ad2ad2238b` | health: executability test Windows can answer (review) |
| `59c6860a0e` | build: installer permissions, spec predicate, Windows guard (review) |
| `eb0b3c8ed8` | notifications: custom-sender shim `curl` order, docs (review) |

Verification status at this commit: 127 tests pass, `cargo clippy --all-targets` clean,
`cargo check --target x86_64-pc-windows-gnu` clean, full `cmake --build` and
`cmake --install` clean, wire diff against the shell script shows only the two
documented JSON-validity fixes, and a live Agent run delivers notifications.

## What was built

`src/crates/alarm-notify/` - a binary installed to
`usr/libexec/netdata/plugins.d/alarm-notify` (`.exe` on Windows). All 31 notification
methods are implemented. **Zero new workspace dependencies**: `reqwest`+`rustls`,
`tokio`, `serde_json`, `anyhow`, `tracing`, `chrono`, `base64`, `libc` were already in
`Cargo.lock`.

| Module | Responsibility |
|---|---|
| `args.rs` | the 33-argument contract; `test`/`unittest`/`dump_methods` classification |
| `conf_parser.rs` | `health_alarm_notify.conf` as a documented bash subset |
| `config.rs` | typed config, defaults, availability screening, deferred templates |
| `recipients.rs` | role resolution and the `\|critical` severity filter's state machine |
| `message.rs` | every derived field, and the runtime-variable set |
| `senders/` | one module per family (`chat`, `push`, `sms`, `incident`, `email`, `syslog`, `irc`) |
| `http.rs` | the `docurl` replacement, including curl's content-type defaults |
| `logging.rs` | the journald record contract |
| `templates/` | plain-text and HTML e-mail bodies, extracted verbatim from the script |
| `shims/` | `custom-sender.sh` and `custom-sender.ps1` |

## Decisions taken, and why

The user delegated these. Each was recorded in the SOW before implementation.

1. **Drop-in binary, not in-process.** Keeps the daemon's timeout and exit-code model
   and the blast radius on the C side small.
2. **Keep `health_alarm_notify.conf`.** A new format would break every installation.
   The parser covers the subset real files use and *reports* what it cannot handle with
   file and line, rather than ignoring it.
3. **`custom_sender()` preserved three ways.** `CUSTOM_SENDER_COMMAND` (any executable,
   portable, works on Windows - the Alertmanager/Zabbix/Sensu shape);
   `custom-sender.sh`, which re-sources the config and restores `urlencode`, `docurl`,
   `info`, `$REPLY`, `duration4human` so an unmodified function keeps working;
   `custom-sender.ps1` calling a `Custom-Sender` function on Windows.
4. **`sendmail`, `aws` and `sendsms` still shell out.** They own the user's mail routing
   and cloud credentials; replacing them would change the identity mail is sent as.
   `logger` and `nc` were replaced by native syslog and IRC.
5. **`reqwest` + `rustls`, not `ureq`.** Already in the lock file, already shipped in two
   Netdata binaries including static builds with `+crt-static`, and its `rustls` feature
   brings `rustls-platform-verifier`, which is what makes the Windows certificate store
   work. A reviewer recommended `ureq`; rejected with that evidence.
6. **Sequential dispatch**, in the script's order, to preserve log ordering and parity.
7. **argv exec, no shell.** Required on Windows, and it removes the shell-quoting
   mangling (`$`→`_`, leading `-` stripped) that alert text used to suffer.
8. **The shell script stays as the build-time fallback.** Rust is optional at build time
   (`packaging/build-package.sh`, `netdata.spec.in`, armv6l static builds disable it), so
   deleting the script would remove notifications from those builds. Exactly one notifier
   is installed per configuration and the daemon picks whichever is present.

## How it was validated, and how to redo it

Automated, in-repo:

```sh
cd "$(git rev-parse --show-toplevel)/src/crates"
cargo test -p alarm-notify              # 127 tests
cargo clippy -p alarm-notify --all-targets
cargo check -p alarm-notify --target x86_64-pc-windows-gnu
```

`spawn-tester test` covers argv fidelity end to end (empty arguments, quotes, trailing
backslashes). Reintroducing either transport's defect makes it fail - that was verified,
not assumed.

Differential harness against the shell script (**the strongest evidence, and the method
to reuse for any further change**). It lived in `/tmp/an-test/` and is ephemeral;
recreate it as follows.

- Generate a runnable script: `sed` the `@..._POST@` placeholders in
  `alarm-notify.sh.in` to temporary directories.
- Start a recording HTTP server and point every redirectable endpoint at it. 16 of them
  can be redirected via config: `SLACK_WEBHOOK_URL`, `MSTEAMS_WEBHOOK_URL`,
  `ROCKETCHAT_WEBHOOK_URL`, `ALERTA_WEBHOOK_URL`, `FLOCK_WEBHOOK_URL`,
  `DISCORD_WEBHOOK_URL`, `TELEGRAM_API_URL`, `MATRIX_HOMESERVER`, `SMSEAGLE_API_URL`,
  `DYNATRACE_SERVER`, `OPSGENIE_API_URL`, `GOTIFY_APP_URL`, `DEFAULT_RECIPIENT_NTFY`,
  `ILERT_ALERT_SOURCE_URL`, `SIGNL4_WEBHOOK_URL`, `KAFKA_URL` (plus `HIPCHAT_SERVER`).
- Point `sendmail`, `aws`, `sendsms`, `logger` and `nc` at recorder scripts - the config
  keys of those names exist for exactly this.
- Run both binaries with the same 33 arguments and a fixed `when`, then compare requests
  field by field (parse JSON/form bodies before comparing) and `diff` the captured mail.
- Start a fresh mock server per case, or captures land in the previous case's file.

Live run of the daemon:

- Configure and build with Go-dependent features off (this workstation's Go is older
  than the tree requires):
  `cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Debug -DENABLE_PLUGIN_GO=Off
  -DENABLE_ND_MCP=Off -DENABLE_PLUGIN_PYTHON=Off -DENABLE_PLUGIN_OTEL=Off
  -DENABLE_PLUGIN_NETFLOW=Off -DENABLE_PLUGIN_SCRIPTS=Off -DENABLE_PLUGIN_EBPF=Off
  -DENABLE_PLUGIN_IBM=Off -DENABLE_CGROUP_NAME=Off -DENABLE_ML=Off`
- Install to a temporary prefix, set `[directories] stock data`, and use a
  `CUSTOM_SENDER_COMMAND` that appends to a file as the observable.
- **Start the daemon with `systemd-run --user`.** A plain background process is reaped,
  which cost time before this was understood.
- Two stock alerts (`1hour_memory_hw_corrupted`, `used_swap`) transition within seconds
  of a clean start, so no synthetic alert is needed. Wipe `var/lib/netdata` between runs
  or they will not transition again.
- Check the journal contract with
  `journalctl --user MESSAGE_ID=6db0018e83e34320ae2a659d78019fb7 -o json`.

## Review outcome

Six independent reviews were run over the committed change: spawn-server wire format;
C daemon wiring; two sender-parity audits covering all 31 methods against the script;
Rust core and security; build, packaging and docs. They found **11 shipping blockers**,
all verified and all fixed. The full list with reasons is in the SOW's Validation
section; the ones worth remembering as a class:

- Three were **Windows-only and invisible without that platform**: `argv_to_windows()`
  dropping empty arguments, `access(X_OK)` being unanswerable by the Windows CRT, and
  `CUSTOM_SENDER_COMMAND` documented in a config every build installs but implemented
  only in the native notifier.
- Two were **silent-success** failures: the shim resolving `curl` before sourcing a
  config that contains `curl=""`, and the shipped `AWSSNS_MESSAGE_FORMAT` expanding to
  six spaces. Both reported delivery.
- One was a **permissions** failure: `netdata-installer.sh` restores the execute bit only
  for `*plugin` and `*.sh`, so the binary installed 0644 on source, static and Docker.
- Two were **parser** failures that produced a wrong value rather than an error: a `}`
  inside a string ending a `custom_sender()` body (the remainder then parsed as
  configuration, able to enable a disabled method), and one unterminated quote
  discarding the rest of the file.

One reviewer claim was **rejected after measurement**: that curl's `--data-urlencode`
writes `%20` for a space. It writes `+` with upper-case escapes, and `reqwest`'s
`.form()` is byte-identical. A change made on that advice was reverted. Measure before
believing a review, including these.

Regression coverage was added for the two areas that had none: the argv-fidelity case in
`spawn-tester`, and two end-to-end tests that drive the **shipped**
`health_alarm_notify.conf` rather than a synthetic one - the gap that hid the shim bug.

## Open items

- **Windows end-to-end is unverified.** No Windows host was available. Three of the
  eleven blockers were Windows-only, so treat a UCRT64 CI run as the next required step.
  This change also enables a Rust target on Windows for the first time.
- **No native SMTP.** E-mail still goes through `sendmail`; on Windows the `sendmail`
  setting must point at a native mail submission program. Adding SMTP means new config
  keys and a new dependency (`lettre`) - a feature, not part of the port.
- **`alarm-notify.sh` is retained** as the no-toolchain fallback. It can be deleted once
  a Rust toolchain is mandatory for every build.
- **Dispatch is sequential.** Worth revisiting only if notification latency is raised as
  a complaint.
- **`sanitize_command_argument_string()` kept** although this change removed its last
  production caller. It is a tested security helper in a general-purpose library and
  other `spawn_popen_run()` callers still build shell strings from configuration. This is
  a disclosed exclusion from the clean end state, not an oversight.
- Reviewers noted pre-existing issues deliberately left alone: `posix_spawn` with
  `argv[0] == NULL` when the decoded count is zero, unbounded `mallocz` on wire-supplied
  sizes (both gated behind the spawn server's magic-UUID check), the CodeQL Rust job
  analysing only the excluded `jf` workspace, and no `cargo test`/`clippy`/`fmt` gate in
  any workflow.

## Working rules that shaped this

- The SOW for this work is `.agents/sow/q/current/SOW-20260819-alarm-notify-rust.md` -
  local-only and never committed. Read it for the recorded decisions and the full
  validation evidence.
- Commit messages must not mention any AI tool or assistant. Commit specific files;
  never `git add -A`.
- Never push, open a PR, or trigger remote CI without the user asking for that specific
  action.
- The integrations generator (`integrations/gen_integrations.py`,
  `gen_docs_integrations.py`) also refreshes unrelated collector docs that have drifted
  in the tree. Restore those before committing; only the 28 notification READMEs belong
  to this change.
