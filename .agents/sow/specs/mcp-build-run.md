# MCP build/run tool contracts

Durable contracts for the local MCP server under
`packaging/tools/automation/mcp/` that lets an LLM configure, build, and run the
Netdata Agent from a worktree. This file records the **decisions and accepted
trade-offs** behind that subsystem — the parts a future reader cannot infer from
the code alone. Mechanics that are obvious from the code (the tool list, the
async plumbing) are intentionally not restated here.

## Build profiles

- The tool exposes exactly **two** profiles, not arbitrary CMake toggles:
  - `debug` — `Debug` + internal runtime checks.
  - `optimized` — `RelWithDebInfo`.
- Both share one **curated, user-reviewed plugin set** (heavy/rare plugins off:
  go.d, ML, eBPF, NetFlow, exporters, etc.; kept on: systemd journal/units,
  journal-file reader, local-listeners, debugfs).
- **The otel plugin is the deliberate exception: always ON** despite its build
  cost, because this tool exists for OTel-logs development (build/run/verify the
  otel subsystem). `ENABLE_PLUGIN_OTEL=On` in both profiles.
- Rationale: a deliberately small surface for LLM-driven builds. Extra knobs
  (ASan, per-plugin toggles, a runtime-overrides layer) were **deferred**, not
  rejected — re-addable later if a concrete need appears.

## Single build directory per worktree

- Each worktree builds into a single `<worktree>/build/`. There is **one install
  per worktree** and **one build type per worktree** at a time.
- Switching a worktree's profile **reconfigures in place** (driven by
  `CMAKE_BUILD_TYPE` in `CMakeCache.txt`), it does not create a second build dir.
- To run two profiles in parallel, use **two worktrees** — this is the supported
  model, not a limitation to work around.
- `clangd` finds `<worktree>/build/compile_commands.json` natively, so there is
  **no compile-commands symlink** to maintain. Editor/clangd errors that
  contradict a successful build are stale-database false positives.

## Dedicated-tree ownership contract

- The tool **owns** `<worktree>/build/`. A worktree handed to the tool is a
  dedicated build tree, not a user's working build dir.
- Ownership is marked by a `build/.mcp-managed` file. The tool **refuses** to
  build into a pre-existing `build/` that lacks this marker (foreign-build guard),
  so it never clobbers a user's own CMake build.
- The build dir is **claimed before configure** (assert-ownable then stamp the
  marker), so a failed or killed configure leaves a recoverable, owned dir.
- Accepted trade-off (TOCTOU): the claim runs **before** the cross-process
  build-dir lock, so the assert-ownable → stamp window is not atomic. Under the
  one-build-type-per-worktree model this is acceptable; hardening it (claim under
  the lock) is optional future work, not a defect.

## Install paths

- An agent installs to `~/opt/netdata-builds/<worktree-basename>/netdata`.
- The path keys on the worktree **basename only**. Two worktrees with the same
  leaf name would collide.
- This is **avoided by convention, not enforced**: worktrees are expected to live
  as uniquely-named children of one top-level directory. The collision is an
  accepted residual; the tool does not guard against it.

## Build-dir lock placement

- The cross-process build lock lives at `<worktree>/.netdata-mcp-build.lock` — a
  gitignored sibling of `build/`, not inside it.
- Rationale: a lock inside `build/` would be destroyed by `rm -rf build/` while a
  build is queued on it. The sibling location survives a build-dir wipe. The
  `.netdata-mcp-build.lock` pattern is in the repo-root `.gitignore`.

## Transport

- Default transport is **stdio** (zero-config for a single user/editor).
- A persistent shared server uses `--transport http` (`--host`/`--port`).

## Cloud claiming

- Auto-claim is **opt-in via credentials, default-on when present**: when
  `NETDATA_CLAIM_TOKEN` (optional `NETDATA_CLAIM_ROOMS`, `NETDATA_CLAIM_URL`) is in
  the server's environment, every launched agent claims to Netdata Cloud; with no
  token, agents launch **unclaimed** and local/MCP access is unaffected.
- Credentials are passed via the launch **process env**, never the command line,
  never written to the generated `netdata.conf`, never logged.
- Each agent is a unique, stable, ephemeral cloud node:
  - `[global] hostname = mcp-<agent_id>` → unique display name; one agent_id maps
    to one stable machine_guid (its run dir), so a restart reuses the same node
    and distinct agents are distinct nodes.
  - `[global] is ephemeral node = yes` → Cloud auto-cleans the node once offline,
    so stopped dev agents don't accumulate.
- Claiming is **best-effort and never gates the run**. It has two phases with
  different timing, both reported (never waited on) as `claimed` / `cloud_connected`
  on `RunInfo`:
  - Phase 1, registration (gets a `claimed_id`): the daemon does this at startup,
    **before** the web server — a blocking curl loop bounded by netdata's own
    retries (~50s worst case on an unreachable Cloud). It delays readiness on a
    Cloud outage but never fails the run (the agent comes up unclaimed).
  - Phase 2, ACLK connection (the node goes live in the Cloud UI): asynchronous,
    background, retries indefinitely. We do not wait for it; `cloud_connected`
    typically flips true a few seconds after the agent is `ready`.
- Rationale for not waiting on Cloud: runs must not depend on Cloud responsiveness
  for success. The phase-1 startup tail is an accepted residual (no external knob
  to shorten it); claiming after launch to remove it was rejected as not worth the
  machinery.
- Credential provisioning: the client-wiring helper `scripts/setup_mcp.py` injects
  the claim creds into each client's per-server env (opencode `environment` map,
  Claude `mcp add --env`) so the server process has them. Each credential resolves
  CLI flag (`--claim-token`/`--claim-rooms`/`--claim-url`) → matching env var; the
  token is **mandatory at setup time** and setup fails if it resolves via neither.
  This is a stricter setup-time guard than the runtime, which still launches
  unclaimed when the server env carries no token. The token is written into the
  user-global client configs (accepted, never committed).

## Agent MCP wrapper

- A `ready` agent's own Netdata MCP (`http://127.0.0.1:<port>/mcp`) is re-exposed
  through the build-MCP so an LLM can query the agent it just built. The agent's
  13 `/mcp` tools are **vendored** (baked typed schemas, `agent_tools.py`) and
  registered as native `netdata_agent_<name>` tools, each with an injected required
  `agent_id` (chosen over a generic dispatcher for LLM tool-use accuracy).
- Registration uses the FastMCP `parameters`/`fn_metadata` split: a `Tool` carries
  the vendored `parameters` (what the LLM sees) backed by a permissive arg model +
  one generic forwarder (`tools/vendored.register_forwarding_tool`). Leans on SDK
  internals (`mcp>=1.27.2`); guarded by tests (register+list+call) rather than a
  hard pin, so a breaking SDK upgrade fails CI rather than ships silently.
- A call resolves `agent_id` → the ready run's port via `RunRegistry` (unknown /
  not-ready → a clear message, never a crash) and forwards opaquely to the agent's
  `/mcp` per-call (stateless transport, no cached session, no auth on localhost).
- Errors are forwarded verbatim: tool-execution failures arrive as content;
  protocol-level rejections (e.g. a missing required argument) are raised by the
  client as `McpError` and turned into text; an unreachable agent yields a clean
  string. (So e.g. `find_anomalous_metrics`, which needs ML — off in these
  profiles — just returns the agent's error.)
- The vendored schemas are a **pinned** surface (drift from the live agent is
  accepted); refresh with `scripts/snapshot_agent_tools.py` against a live agent.

## Job and run lifecycle

- State is **in-memory only**: jobs and runs do not survive a server restart, and
  there is **no eviction** — `_jobs`/`_runs`/`_start_locks` grow with distinct
  ids for the server's lifetime. Acceptable for a localhost dev tool with few
  agents; not a persistence layer.
- There are **two registries by design**, sharing primitives but not merged:
  - `JobRegistry` — finite build/configure jobs (run an ordered phase list to a
    terminal state; dedup/refuse-if-busy per build dir).
  - `RunRegistry` — long-lived `netdata` launches whose readiness is probed and
    whose terminal state is derived from process exit, with a per-agent restart
    that tears down and relaunches to pick up source edits.
- Shared lifecycle primitives live in `runner.py`
  (`escalate_cancel`/`drain_all`/`await_task` over a `Cancellable` protocol;
  `run_phases` over a `PhaseHost` protocol). Cancellation is always
  SIGTERM → grace → SIGKILL on the **process group** (children spawn in a new
  session) so build descendants cannot survive holding the output pipe.
- A run records `netdata`'s exit code as `returncode` once the launch ends;
  **negative means killed by a signal** (e.g. `-9` for SIGKILL).
