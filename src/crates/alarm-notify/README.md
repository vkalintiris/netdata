# alarm-notify

The Netdata Agent's alert notification dispatcher: the daemon runs it once per alert
transition, it works out who should be told and over which of the 31 supported
services, and it reports back through its exit code.

It replaces `src/health/notifications/alarm-notify.sh`. That script needed bash 4,
`curl`, `sendmail`, `logger`, `nc`, `aws` and GNU `date`; Windows has none of them
once the bundled MSYS2 root is gone, so notifications there had no working
interpreter at all. This binary needs nothing but itself.

## Contract

Everything the outside world depends on is unchanged:

| Surface | Contract |
|---|---|
| Arguments | 33 positional arguments, in the daemon's order (`src/health/health_notifications.c`). |
| `test [role]` | Sends a WARNING, a CRITICAL and a CLEAR through the real dispatch path. |
| `dump_methods` | Prints the enabled `SEND_*` variable names, one per line, sorted. Read by `src/daemon/analytics.c`. |
| `unittest <role> <config> <status> <old_status>` | Prints `results: <method>: <recipients>` per method. |
| Exit code | `0` when at least one notification was delivered. Stored as the alert's `exec_code` and forwarded to Netdata Cloud. |
| Logging | journald records carrying `MESSAGE_ID=6db0018e83e34320ae2a659d78019fb7` and the `ND_ALERT_*` fields, as `systemd-cat-native --log-as-netdata` produced before. |
| Configuration | `health_alarm_notify.conf`, stock file first, user file second. |
| stdout | Silent during dispatch. The daemon opens a pipe for it and never reads it, so anything written there would eventually block. |

## Configuration

The existing file is parsed directly - there is no new format and no migration. The
supported shell subset covers what real configurations contain:

- assignments in all three quoting styles, `export`/`declare` prefixes, comments,
  line continuations, and values spanning lines;
- `${VAR}`, `$VAR`, `${VAR:-default}` and `${VAR:+alt}`, resolved against earlier
  assignments and then the environment;
- `role_recipients_<method>[<role>]="..."` tables;
- `unset` and `unset -v`;
- a `custom_sender()` function, captured for the compatibility shim.

Anything else - a conditional, a loop, another function - is reported with its file
and line number and skipped, rather than silently ignored. `$(command)` is evaluated
by handing that fragment to a POSIX shell where one exists; on Windows there is none,
so it expands to nothing and says so.

## The `custom` method

`custom_sender()` is user-written shell living inside the configuration file, so it
cannot be reimplemented. It is preserved instead, three ways:

1. **`CUSTOM_SENDER_COMMAND`** - any executable, run with the recipients as its first
   argument and every notification variable in its environment. Portable, works on
   Windows, and the recommended option. This is the shape Alertmanager, Zabbix and
   Sensu use for the same problem.
2. **An existing `custom_sender()`** - the shipped `custom-sender.sh` re-reads the
   configuration, restores the helpers the function expects (`urlencode`, `docurl`,
   `info`, `warning`, `error`, `debug`, `duration4human`, `$REPLY`) and calls it.
   Existing installations keep working with no edit.
3. **Windows** - the shipped `custom-sender.ps1` calls a `Custom-Sender` function
   from `health_alarm_notify_custom.ps1` next to the configuration.

## Deliberate differences from the shell script

Behaviour is otherwise identical; these are the exceptions, each verified against the
old script by comparing what both put on the wire.

**Fixes to malformed output**

- `ilert` emitted `"severity": WARNING` unquoted, which is not valid JSON. Now quoted.
- `kafka` emitted unquoted object keys with curl's default form content type. Now a
  well-formed JSON document sent as `application/json`; field names and value types
  are unchanged.
- `fleep` emitted single-quoted pseudo-JSON. Now well-formed JSON.
- `smseagle` emitted `"duration": ,` when the value was empty. Those keys are now
  omitted when they do not apply.
- `alerta`'s `rawData` read `BASH_ARGV`, which is empty unless `extdebug` is set. It
  now carries the notification's arguments.
- The e-mail `In-Reply-To`/`References` headers were one header containing a literal
  `\r\n`. They are now two headers.
- Payload text is JSON-encoded, so an alert whose text contains a quote or a
  backslash no longer produces a broken document.

**Payload shapes that changed with the fixes**

- `kafka` and `fleep` now send `application/json`. Their bodies were not JSON before
  and were sent with curl's default form content type; a receiver parsing them as JSON
  could not have succeeded either way.
- `kafka`'s `value`/`old_value` become JSON `null` when the alert value is `nan`. The
  script emitted the bare token `nan`, which is not valid JSON.
- `opsgenie` does the same for an empty value, where the script emitted `"value" : ,`.
- `fleep`'s message carries a real newline. The script's `\n` was literal text inside
  a body that was not valid JSON, so a parsing receiver saw two characters.
- `alerta`'s `rawData` lists the notification's arguments in reverse order, which is
  what `"${BASH_ARGV[@]}"` would have expanded to had it been populated.
- `DYNATRACE_SERVER` and `OPSGENIE_API_URL` have a trailing slash trimmed, so a
  configuration ending in `/` no longer produces a double slash in the request path.

**Behaviour that had to change**

- `EMAIL_CHARSET` defaults to `UTF-8`. It used to come from `locale charmap`, which
  under the daemon's `LC_ALL=C` always reported US-ASCII while the body was UTF-8.
- `clear_alarm_always` now works. The script tested it before reading the
  configuration, so setting it had no effect.
- `unittest` works. The script's config-load check was inverted, so the mode exited
  with an error whenever the file loaded successfully.
- `SEND_EMAIL="AUTO"` probes for an MTA. It used to probe for `curl`.
- Recipients keep their configured order; the script emitted them in bash hash order.
- `syslog` and `irc` are implemented in-process, so they no longer need `logger` or
  `nc`, and `curl` is not required by any method. On Windows, `syslog` needs a remote
  target (`facility.level@host:port/prefix`) because there is no local syslog socket.
- Arguments reach the notifier verbatim. They used to pass through a shell command
  string, which replaced `$` with `_` and stripped leading `-`.
- `syslog` records keep the framing `logger` used, which differs by destination: a
  local datagram carries no timestamp or host, because the receiving daemon supplies
  both, and a remote one is RFC 5424, as util-linux has sent since 2.26. The record's
  tag is the account the Agent runs as, as `logger` reported it, not the recipient's
  prefix. `logger_options` no longer leaks from one recipient to the next, and
  bracketed or bare IPv6 targets now work.
- `ntfy` honours `role_recipients_ntfy[<role>]`. The script documented those entries
  but always used `DEFAULT_RECIPIENT_NTFY`, so severity modifiers never applied and a
  role-specific topic was never used. Its Basic credential is also no longer
  line-wrapped, which `base64` did past 57 bytes.
- `email` no longer truncates its own header block when `EMAIL_THREADING="NO"`. The
  script emitted an empty line there, which ended the header section and pushed the
  `X-Netdata-*` headers into the message body.
- The `-F` capability probe no longer shares stdin with the message, so it cannot
  consume part of the mail it is about to send.
- Header values have CR and LF replaced with a space, so alert text cannot inject a
  header. Nothing upstream can currently produce such a value; the guard does not rely
  on that.
- IRC lines end with CRLF, as RFC 1459 requires.
- Timestamps in payloads and the SMS length limit are unchanged, but the SMS cut is on
  bytes (as the script's `LC_ALL=C` cut was) while Pushover's and Discord's limits are
  applied in characters, which is how those vendors document them.

**Unchanged on purpose**

`sendmail`, the `aws` CLI and smstools3's `sendsms` are still invoked as external
programs: they own the user's mail routing and cloud credentials, and replacing them
would change the identity notifications are sent as. On Windows, point the `sendmail`
setting at a native mail submission program.

## Configuration values that are templates

A configuration value may reference the alert's variables - the shipped
`AWSSNS_MESSAGE_FORMAT` does - and the configuration documents the whole
`custom_sender()` variable set as available. The shell script got this for free by
sourcing the file after parsing its arguments. Here the parser leaves such references
alone and they are resolved once the alert is known, which also makes `${date}` work;
bash expanded it before the script had computed it, so it was always empty.

## Layout

| Path | What it holds |
|---|---|
| `src/args.rs` | The 33-argument contract and the mode classifier. |
| `src/conf_parser.rs` | The `health_alarm_notify.conf` reader. |
| `src/config.rs` | Typed configuration, defaults, availability screening. |
| `src/recipients.rs` | Role resolution and the `\|critical` severity filter. |
| `src/message.rs` | Every derived field: status wording, colours, URLs, durations. |
| `src/senders/` | One module per family of destinations. |
| `src/http.rs` | The `docurl` replacement, including curl's content-type defaults. |
| `src/logging.rs` | The journald record contract. |
| `templates/` | The plain-text and HTML e-mail bodies, taken verbatim from the script. |
| `shims/` | The `custom_sender()` compatibility shims. |

## Tests

```sh
cargo test -p alarm-notify
```

Unit tests pin the behaviours that have to match the shell script exactly - URL
encoding, duration wording, the severity filter's state machine, the configuration
parser against the real stock file. The integration tests in `tests/` run the built
binary against a recording HTTP server and assert on the payloads themselves.
