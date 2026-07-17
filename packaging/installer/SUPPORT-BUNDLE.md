# netdata-support-bundle — the Netdata support bundle

`netdata-support-bundle` collects a **sanitized diagnostic bundle** (zstd tarball on POSIX
systems, zip on Windows) that users attach to support tickets, so support gets
everything it needs on first contact instead of asking for it over multiple
round trips.

It is a single Rust binary (source: `src/crates/support-bundle/`), installed
with the agent on every platform:

```sh
# POSIX systems (in PATH, like netdatacli; static installs get it under the prefix):
sudo netdata-support-bundle

# static installs:
sudo /opt/netdata/usr/sbin/netdata-support-bundle
```

```powershell
# Windows (elevated prompt):
& "C:\Program Files\Netdata\usr\bin\netdata-support-bundle.exe"
```

One implementation serves every platform; the bundle contract below is
identical everywhere (only the collector sources differ per OS).

## Design contract (do not regress these)

| guarantee | implementation |
|---|---|
| Zero system impact | self-demotion to idle CPU/IO priority (`setpriority(19)` + `ioprio_set` idle class on Linux; `IDLE_PRIORITY_CLASS` on Windows); per-command timeout (10 s default) that SIGKILLs the child's whole process group on unix (the direct child on Windows); global deadline checked before each collector and inside composite/native collectors, so the hard runtime bound is deadline + one command timeout; size caps (5 MiB per log, 1 MiB per file, 2 MiB per command/API output) enforced with bounded reads — an oversized source file is never loaded whole into memory; read-only — writes only its private staging dir and the final artifacts, never restarts or reconfigures anything; artifacts are published with `O_EXCL` so pre-existing files or symlinks in shared tmp dirs are never followed |
| Works when the agent is dead | no hard dependency on a running agent; the most valuable crash artifacts (status file, logs, buildinfo via the binary) are collected from disk; a `07-runtime/AGENT-WAS-DOWN.txt` marker is written instead of API captures |
| Secrets always redacted | non-optional single-pass sanitizer; see "Sanitization" below |
| PII pseudonymized by default | IPs (v4+v6), MACs, emails, this host's names, the invoking user, child/mirrored node hostnames and stream destinations are replaced with **stable** pseudonyms (`ip-1`, `private-host-1`) so cross-file correlation still works; the private map is saved **next to** the bundle, never inside it; `--no-obfuscate` opts out |
| Caps cannot expose secrets | all caps cut at LINE boundaries, so a secret can never straddle the cut and dodge the line-based sanitizer; a capped tail with no line break at all is withheld entirely; a capture cut short by a timeout is withheld when it must stay parseable (API/JSON) or has its unterminated final line dropped (text); sanitizer failures withhold the file content (fail closed) |
| Legible to humans AND AI agents | triage-ordered numbered directories; sanitized file copies have no injected provenance headers; provenance headers only on command captures; `MANIFEST.json` indexes every file with safe origin + sanitization state; `summary.txt` opens with a triage read-order |

## Platform support

| platform | status | notes |
|---|---|---|
| Linux (glibc, systemd) | tested | full collection incl. journal namespace |
| Linux (musl/BusyBox, e.g. Alpine) | tested | static binary; file logs instead of journal |
| Docker (official image) | tested | agent logs live in `docker logs` on the host — bundle includes a marker with the exact command to run and attach; `/proc/1/environ` needs `CAP_SYS_PTRACE` (fallback to exec env) |
| Static builds (`/opt/netdata`) | tested paths | all paths resolved under the prefix |
| FreeBSD | best effort | `/usr/local/etc/netdata` + `/var/db/netdata` paths, `sockstat` fallback, `ps -H` threads; no `/proc` items |
| macOS (Homebrew) | best effort | `/usr/local` and `/opt/homebrew` prefixes, `sysctl`/`vm_stat` fallbacks, `ps -M` threads |
| Windows | native APIs | WMI for CIM classes, the Evt API for `NetdataWEL`/`Application` logs (scan bounded per channel), registry for MSI/proxy state, `GetExtendedTcpTable` for listeners; the install prefix is discovered from the Netdata service's `PathName` with the MSI default as fallback; config and state collection match the POSIX artifact set |

Portability rules the implementation obeys (keep them when editing):

- Feature-detect, never assume: `journalctl`, `coredumpctl`, `ss`/`sockstat`/`netstat`,
  `free`/`sysctl`, `/proc` availability are all probed before use.
- Every external command is optional: a missing tool degrades that one file,
  never the run.
- External commands are real CLI tools only (`journalctl`, `ss`, `w32tm`,
  `netsh`); everything else uses native APIs. Local agent API reads go over a
  raw TCP connection to `127.0.0.1:19999`, so a configured proxy can never
  see or route diagnostic data. The Netdata Cloud reachability probe performs
  an in-process certificate-validating TLS handshake (rustls + webpki roots);
  no external HTTP client is required or consulted.

## Why this exists (evidence)

Analysis of the full Freshdesk ticket history (443 tickets, 2026-07) and 287
maintainer comments across 79 GitHub bug threads showed:

- 24% of support tickets required at least one "please provide X" round trip
  (average 1.7 per ticket; some needed 3+), each adding a day or more of
  latency and eroding customer confidence.
- The asks are highly repetitive. Everything ranked below the top-20 asks fits
  in one automated collection pass.

Every item collected maps to a recurring support ask. That mapping is the
"why" column in the tables below. When adding a new item, add its why.

## What is collected, and why

### `summary.txt`, `MANIFEST.json`, `README.md` (bundle root)

| item | why |
|---|---|
| `summary.txt` | one-page human overview; opens with agent state and a "read order for triage" per issue class, so support (or an AI agent) starts at the right file |
| `MANIFEST.json` | machine-readable index: every file with its origin (command / source path / API endpoint), size, and sanitization state; lets AI tooling navigate the bundle without guessing |
| `README.md` | self-documentation for whoever receives the bundle |

### `01-system/` — platform context

| item | why |
|---|---|
| kernel/OS/architecture, distro (first readable of `/etc/os-release`, `/usr/lib/os-release`, `/etc/lsb-release`, with a `# source:` header) | first question in the bug template; kernel regressions have been root causes (two GitHub issues traced to kernel changes) |
| memory, disks, CPU count, uptime | capacity questions asked in most performance tickets |
| virtualization / container detection, cgroup version | OpenVZ/LXC/CageFS visibility problems are a recurring collector-failure class |
| **clock/time sync** | clock drift on children silently breaks streaming and cloud auth — maintainers explicitly ask ("check if the clock on child nodes is drifting") |
| `/proc/self/mountinfo` (POSIX) | namespace visibility issues ("cannot open /proc/diskstats") are diagnosed from the mount table |
| kernel OOM/segfault messages | evidence of the kernel killing netdata — distinguishes crashes from kills |
| SELinux/AppArmor state | MAC denials cause silent collector failures |

### `02-install/` — how netdata got here

| item | why |
|---|---|
| `.environment` file | install method, flags, release channel, custom CFLAGS (`-ffast-math` alone broke dbengine once); contains no secrets |
| `.install-type` marker | `kickstart-build` / `kickstart-static` / `oci` / `binpkg-*` — determines which update/troubleshoot paths apply |
| package manager info (POSIX) / MSI uninstall registry info (Windows; never `Win32_Product`, querying it triggers MSI reconfiguration) | version skew between repo package and expectation is a recurring theme |
| container context (env, cgroup, pid 1) | missing `init: true`, missing `pid: host`, and wrong images are recurring Docker-ticket root causes; `NETDATA_*` env values pass through the sanitizer |

### `03-process/` — the running agent

| item | why |
|---|---|
| netdata process tree with CPU/memory | "netdata is eating my CPU/RAM" tickets need this first |
| **per-thread CPU** (POSIX) | maintainers ask users to find the hot thread in htop; this captures it non-interactively |
| `/proc/PID/status`, `limits`, fd count | leak and limit diagnosis |
| agent process environment (sanitized) | proxy/claiming issues: the env the service sees differs from the user's shell — asked explicitly in GitHub threads |
| zombie process check | plugin-reaping failures in containers (`init: true` guidance) |
| service state (Windows) | start mode, run account, and exit code of the Netdata service |

### `04-config/` — configuration

| item | why |
|---|---|
| **effective running config** (`GET /netdata.conf`) | the #1 GitHub maintainer ask; shows the merged config the agent actually uses and annotates unrecognized options — resolves "my config is ignored" outright; authoritative over on-disk files |
| on-disk `netdata.conf`, `stream.conf`, cloud/claim conf, `go.d.conf`, go.d/health.d/python.d/charts.d/statsd.d user files, `exporting.conf` | the files users were asked to paste, ticket after ticket (child stream.conf + parent `[web]`/`[stream]` sections is a canned Freshdesk ask); **all pass the sanitizer**, and their bundle paths mirror their paths relative to the config directory |

### `05-logs/` — history

| item | why |
|---|---|
| systemd journal, **including `--namespace=netdata`** | the agent logs to its own journal namespace on systemd installs — plain `journalctl -u netdata` misses almost everything; support asks for "a complete log from start until the problem" |
| `/var/log/netdata/*.log` tails (size-capped) | non-systemd installs, static builds, macOS/BSD |
| Windows Event Log (`NetdataWEL` + Application, Netdata providers) | Windows agents log to the Event Log; Windows is a top-3 support theme |
| updater service journal | update failures; the updater keeps no persistent log file |
| **coredump metadata** (`coredumpctl list`, never the dumps) | tells support a dump exists and matches the crash time — the dump itself is fetched later only if needed |
| docker marker file | in containers the log "files" are symlinks to stdout — history only exists in `docker logs` on the host; the bundle says raw logs must not be attached and gives a private capture/review/redaction workflow using the requested time window |

### `06-state/` — persistent state

| item | why |
|---|---|
| **`status-netdata.json`** (the agent's fallback write locations, newest file wins; a symlinked file is refused) | the single most valuable crash artifact: last exit reason, fatal line/file/function, signal, **stack trace** — same data that feeds agent-events crash telemetry; support gets crash forensics with zero extra round trips |
| state dir aggregate inventory | unexpected file counts/sizes without exposing filenames that may themselves be live tokens, hostnames, or job identifiers; **contents of secret files are never read** (see exclusions) |
| claim state (`claimed_id` only) | claim id is the identifier support needs to find the node in Cloud; a non-persisted `cloud.d` across restarts is a known Freshdesk root cause |
| db disk usage per tier + sqlite sizes | retention questions ("why do I only have N days") are answered by tier sizes vs configured limits |
| dyncfg files (sanitized) | jobs created via UI live here, not in `/etc/netdata` — invisible in classic config collection |
| go.d job statuses, health silencers | which collector jobs exist/fail; why alerts are silent |

### `07-runtime/` — live agent state (only when API responds)

| item | why |
|---|---|
| `/api/v3/info` | best single call: structured buildinfo, features, cloud status, per-tier retention — and it works even under bearer protection |
| `/api/v1/info`, `/api/v2/node_instances` | children, streaming state, `db_size`, metric counts — the exact endpoint maintainers ask for in retention/memory tickets |
| `/api/v3/stream_info`, `/api/v1/aclk` | streaming and cloud-connection diagnostics |
| active alerts + alert instances | alert tickets are the single biggest Freshdesk theme |
| `/api/v1/functions`, `/api/v1/ml_info` | which plugins expose what; ML state |
| `netdata -W buildinfo` + `buildinfojson` | required by the bug template; the paths section proves which config dirs the binary uses; works with the daemon **down** |
| `netdatacli aclk-state json` | canned Freshdesk ask for cloud issues |
| netdata self CPU/memory/clients CSVs (10 min, bounded) | replaces the "please send a screenshot of the Netdata memory charts" round trip |

Local API reads target `127.0.0.1:19999` directly over raw TCP and can never
route through a configured proxy, so diagnostic data cannot leave the host
through a forced proxy. The reachability probe tries `/api/v3/info` first —
it stays reachable under bearer protection, where `/api/v1/*` is locked —
so a protected-but-running agent is not mis-flagged as down.

### `08-network/` — connectivity

| item | why |
|---|---|
| listening sockets (netdata-related) | "dashboard unreachable" and port-conflict tickets |
| DNS config, proxy env/config (sanitized) | claiming-behind-proxy is a recurring theme; DNS misconfiguration breaks cloud connectivity |
| Netdata Cloud reachability (DNS + TCP + certificate-validating TLS probe + HTTP status; in-process, no bundle data sent) | separates network problems from agent problems in one step |

## What is NEVER collected

These are excluded by design. **Do not add them.**

- `cloud.d/private.pem` (ACLK private key), `cloud.d/token` (claim token)
- `bearer_tokens/` (the **filenames** are live API tokens), `netdata.api.key`,
  `mcp_dev_preview_api_key`, `netdata_random_session_id`
- `/etc/netdata/ssl/` and any `*.pem` / `*.key`
- dbengine data files (metric data, GBs), `ml.db`, `registry.db` (person GUIDs
  and dashboard URLs)
- metric values other than netdata's own bounded self-monitoring charts
- anything outside netdata's own scope (no full system journals, no other
  services' logs, no packet captures)

## Sanitization

Two passes, one sweep, applied to **every** collected file
(implementation: `src/crates/support-bundle/src/sanitize.rs`):

1. **Secrets — always on, not configurable:**
   - values of any key whose punctuation-normalized name contains a complete
     secret word or phrase:
     `api key, apikey, token, password, passwd, secret, community, bearer,
     webhook, license key, auth, credential, cookie, passphrase, proxy user,
     proxy pass, username, dsn, private key, access key, session, recipient,
     account sid, priv key` — plus the short aliases `pass, pwd, pat`, matched
     as whole words of the normalized key so `bypass`, `compass` and `pattern`
     are never touched (substring match on the normalized key otherwise, so
     `claim_token`, `access token`, `TELEGRAM_BOT_TOKEN`, camelCase and
     compact spellings all match) — in ini (`k = v`), yaml (`k: v`), env
     (`K=V`) and JSON (`"k": "v"`) forms, including escaped strings,
     numeric/scalar values, and nested values. Keys must look like real
     config keys (≤64 chars, no sentence punctuation) so prose containing
     "token" is not mangled. Multi-line JSON objects/arrays and YAML
     block-scalar secret values are withheld through their closing boundary
     (or to EOF if malformed). Exemptions are decided by the KEY, never the
     value: keys ending in `file path dir directory protection support mode
     level port timeout cookies secure log size options` describe secrets
     rather than being secrets, so `bearer token protection = no` and
     `api key file = /path` stay readable while `TOKEN=false` and
     `PASSWORD=/x` are redacted;
   - argv/env-style secrets mid-line (`-token=X`, `--password "X"`,
     `CLAIM_TOKEN=X`, `api key = X` inside captured process command lines);
   - URL-embedded credentials (`scheme://user:pass@`) and Go DSN credentials
     (`user:pass@tcp(...)`);
   - JWTs; `Bearer <value>` (the value must contain a digit, so config prose
     after the word "bearer" survives), `Basic <value>`, and `Authorization`
     header values (the header name matches the `auth` secret word);
   - secrets in URL query parameters (`?token=`, `&api_key=`, ... — request
     lines in access logs);
   - private-key PEM blocks — the WHOLE multi-line block is withheld from the
     BEGIN marker through the END marker (fail closed if END never arrives);
   - `stream.conf` parent-side `[<UUID>]` section headers (they ARE the API
     keys);
   - `bearer_tokens/` directory listings show a file COUNT only — the
     filenames are the tokens.
2. **PII — on by default, `--no-obfuscate` to disable:**
   - non-loopback IPv4 addresses → `ip-N` and IPv6 → `ip6-N` (stable per
     bundle; compressed, lettered, and numeric-only uncompressed forms;
     validated so timestamps, `file.c:123` refs and `::1` are left alone);
   - MAC addresses → `[MAC]`; email addresses → `[EMAIL]`;
   - this host's hostname/FQDN → `redacted-host`; the invoking user's name →
     `redacted-user` (usernames shorter than 3 characters and service
     accounts are left alone — replacing a 2-character string everywhere
     would corrupt unrelated content);
   - other users' home-directory segments (`/home/<name>`, `/Users/<name>`)
     → stable `user-N` pseudonyms (they leak through mount tables and paths
     when the tool runs as root);
   - hostnames under clearly-private TLDs (`.internal .local .lan .corp
     .intranet .localdomain`) → `private-host-N`;
   - child/mirrored node hostnames (pre-seeded from the local API before
     collection, so they pseudonymize consistently in every file) and
     `stream.conf` `destination` hosts regardless of TLD → `private-host-N`;
   - resolv.conf `search`/`domain` values → `[SEARCH-DOMAINS-WITHHELD]`
     (corporate search domains are rarely under private TLDs).

Hostnames that reach the bundle only as generic public FQDNs (not this host,
not a stream destination, not a child, not under a private TLD) are NOT
pseudonymized; emails, IPs and credentials on such lines still are.

The private map is written next to the bundle (`*.pseudonym-map.tsv`) so the
**user** can decode references if support asks "what is private-host-2?" — it
is never included in the bundle itself. Pseudonym mappings are capped at 4096
entries per kind; past the cap, values get a non-correlating placeholder so
hostile high-cardinality input cannot grow memory or the private map without
bound.

Redaction here is defense in depth, not a substitute for exclusion: files that
are pure secrets (see exclusion list) are never read at all. Content is
sanitized IN MEMORY and only sanitized bytes are ever written to the staging
tree — a crash or kill mid-run cannot leave unsanitized content on disk.
Buffers containing NUL bytes (binary or BOM-less UTF-16 input) are withheld
rather than run through byte-unsafe line redaction. A source file whose leaf is itself a
symlink (or reparse point on Windows) is withheld (a swapped link must not
redirect collection to another target; on unix the open uses `O_NOFOLLOW` and
all reads go through that descriptor, so no check-to-read race exists);
symlinked parent directories resolve normally. Do not extend collection to a directory writable by any other
identity. Each run uses a newly created, unpredictable staging directory and
never reuses a pre-existing path (0700 on POSIX; under the per-user `%TEMP%`
tree on Windows). Final artifacts (tarball, zip, pseudonym map) are published
with a no-overwrite `O_EXCL` create so a pre-existing file or symlink at the
target is never followed or clobbered.

**The agent itself provides no redaction anywhere** — `GET /netdata.conf` and
`netdatacli dumpconfig` print secrets verbatim. Everything must be sanitized
by this tool.

## How to extend it (checklist for future contributors)

1. Map the new item to a real support ask (link the ticket/issue class) and
   add it to the right section table above **with its why**.
2. Declare the item in the platform's plan builder (`build_items` in
   `unix.rs` / `windows.rs`, or `runtime.rs` for the shared API section)
   using the `Item` constructors — `cmd` / `file` / `api` / `cmd_raw` /
   `generated`, or `native` with a named producer function for composites
   (`src/crates/support-bundle/src/item.rs`). One executor
   (`Ctx::execute` in `collect.rs`) runs the declared plan and enforces
   timeouts, size caps, sanitization, and manifest registration. Never write
   into the bundle directly. Check your item appears in
   `netdata-support-bundle --list` (the resolved plan for the host, printed
   without collecting anything).
3. Respect the cost budget: nothing unbounded, nothing that queries metric
   data without a tight window, nothing that can block longer than the
   per-command timeout.
4. If the item can contain credentials of a NEW shape, extend the sanitizer
   (`sanitize.rs`) and add the pattern to the Sanitization section above.
5. If the item is platform-specific, say so in its table row; the 07-runtime
   API section is shared code (`runtime.rs`) precisely so the platforms
   cannot drift.
6. Test the redaction: add a vector to the regression suite
   (`src/crates/support-bundle/src/selftest.rs`) and run
   `cargo test -p support-bundle` — the same suite ships in the binary as
   `netdata-support-bundle --selftest`. For new collection sources also
   plant a sentinel secret in the source, run a collection, and `grep -r`
   the extracted bundle. Zero hits or it does not ship.
7. Never add anything from the "What is NEVER collected" list, and never make
   the tool write, restart, reconfigure, or otherwise mutate the system.

## Bundle format contract

- Schema id: `netdata-support-bundle/v1` (in `MANIFEST.json`). Bump the suffix on
  breaking layout changes; downstream ticket tooling may parse it.
- Command captures are `.txt` files starting with a
  `# netdata-support-bundle v<version> | command: ... | captured: <utc>` header
  and ending with an `# exit: N | duration: Ns` trailer, on every platform.
- Copied files and API responses are sanitized without provenance headers
  (and remain parseable when they fit their cap); their
  provenance lives in `MANIFEST.json`, not in the files.
- `MANIFEST.json` paths always use forward slashes, on Windows too.
- POSIX archives are zstd-compressed tarballs (`.tar.zst`) with anonymized
  ownership in the tar headers (uid/gid 0, no account names); Windows
  produces a zip.
- If manifest sanitization itself fails, `MANIFEST.json` degrades to a
  minimal document with the `schema` id, an `error` string, and an empty
  `files` array — always valid JSON for downstream tooling.
