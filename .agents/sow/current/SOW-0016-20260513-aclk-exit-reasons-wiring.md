# SOW-0016 - ACLK Exit Reasons and Node Instance Connectivity Reason wiring

## Status

Status: in-progress

Sub-state: implementing against aclk-schemas PR #54 branch tip (`80a8daa8`) checked out in the submodule. Agent PR cannot merge until #54 lands, but development proceeds in parallel.

## Requirements

### Purpose

Let Netdata Cloud know **why** an Agent stopped or disconnected, and why a node instance disconnected, so Cloud can:

- attach exit reasons to feed events for incident investigations;
- skip reachability notifications when a node went offline because the agent is updating;
- remove a node instance immediately when it was rotated out of the agent's DB (no retention) instead of marking it pruned and waiting 60 days.

### User Request

> The backend team complained that we (agent team) haven't followed-up for a long time with https://github.com/netdata/aclk-schemas/pull/54. ... I guess the high-level goal is to allow the backend (ie. cloud) to be able to know about reason why some agent has exited (or disconnected).

Design decision recorded with the user 2026-05-13: `NodeInstanceConnectivityReason.AGENT_UPDATE` is **cloud-derived** from `UpdateAgentConnection.exit_reasons`; the agent never emits `AGENT_UPDATE` on a per-node message.

### Assistant Understanding

Facts:

- aclk-schemas PR #54 (state: OPEN as of 2026-05-13, last activity 2025-06-25) adds:
  - `enum AgentExitReason` with 21 values mirroring `EXIT_REASON` in `src/libnetdata/exit/exit_initiated.h` by **name and semantics**; proto values are sequential 0..20 (commit `ae84839` deliberately dropped the bitmask numbering).
  - `repeated AgentExitReason exit_reasons = 18` on `agent.v1.UpdateAgentConnection`.
  - `enum NodeInstanceConnectivityReason { UNSPECIFIED = 0; NO_RETENTION = 1; AGENT_UPDATE = 2; }` and `reason = 11` on `nodeinstance.v1.UpdateNodeInstanceConnection`.
- The agent's `EXIT_REASON` is a bitmap (`1 << n`) defined in `src/libnetdata/exit/exit_initiated.h`. The previous run's value is persisted in the status file and re-loaded into the file-static `last_session_status.exit_reason` by `daemon_status_file_init()` very early in startup (`src/daemon/status-file.c:1063`).
- `aclk_send_agent_connection_update()` in `src/aclk/aclk_tx_msgs.c:199` is the single producer of `UpdateAgentConnection`. It is called twice: once with `reachable=1` at `src/aclk/aclk.c:399` right after `aclk_set_connected()`, and once with `reachable=0` at `src/aclk/aclk.c:407` during `aclk_graceful_disconnect()`.
- `aclk_generate_lwt()` in `src/aclk/aclk_tx_msgs.c:236` produces the MQTT Last Will and Testament, registered at CONNECT time before any exit reason exists.
- `UpdateNodeInstanceConnection` is built by `generate_node_instance_connection()` in `src/aclk/schema-wrappers/node_connection.cc`, fed by `aclk_update_node_instance_job()` at `src/aclk/aclk.c:972`, which is reached from `aclk_host_state_update()`. The only callers that pass `queryable=0` today (i.e., "node has no retention / has been removed") are:
  - `src/daemon/service.c:269` — orphan-host cleanup when an archived host has zero retention (`from_s==0 && to_s==0`). **`NO_RETENTION` site.**
  - `src/daemon/commands.c:386` — `remove-stale-node` netdatacli action that unregisters an ephemeral host.
- The aclk-schemas submodule is pinned at `ec319a14` (current master tip, post-#54-base). The submodule will need a bump to a commit that contains PR #54 once it is merged.
- PR #54 still requests review from `ktsaou` and `stelfrag`; the title still has the `WIP:` prefix although the PR is functionally final per backend reviewers (juacker, car12o approved on 2025-06-20 and 2025-06-25).

Inferences:

- Mapping C `EXIT_REASON` bits to proto enum values is by **name**, not numeric value (backend deliberately renumbered to sequential).
- For the LWT path the agent cannot meaningfully populate `exit_reasons`: MQTT LWT is fixed at CONNECT and no exit reason has been initiated yet. Cloud sees an LWT-delivered `UpdateAgentConnection` with an empty `exit_reasons` set and treats it as "abrupt loss".
- `commands.c:386` (`remove-stale-node`) semantically removes the node from the agent's view; the closest matching reason is `NO_RETENTION` but `UNSPECIFIED` is also defensible. Confirm with backend before coding (see Open decisions).

Unknowns:

- None blocking. PR #54 merge timing is an external dependency, not an unknown.

### Acceptance Criteria

- AC1: `UpdateAgentConnection(reachable=true)` emitted on initial Cloud connect carries the **previous run's** exit reasons (translated from `last_session_status.exit_reason`).
- AC2: `UpdateAgentConnection(reachable=false)` emitted by `aclk_graceful_disconnect()` carries the **current** exit reasons (translated from `exit_initiated_get()`).
- AC3: LWT `UpdateAgentConnection` carries an empty `exit_reasons` set, by design.
- AC4: `UpdateNodeInstanceConnection` emitted by `service.c:269` carries `reason = NODE_INSTANCE_CONNECTIVITY_REASON_NO_RETENTION`.
- AC5: `UpdateNodeInstanceConnection` emitted by `commands.c:386` carries the reason decided in "Open decisions" (default plan: `NO_RETENTION`).
- AC6: All other `UpdateNodeInstanceConnection` call sites carry `NODE_INSTANCE_CONNECTIVITY_REASON_UNSPECIFIED` (default).
- AC7: Agent never emits `NODE_INSTANCE_CONNECTIVITY_REASON_AGENT_UPDATE` (cloud-derived).
- AC8: aclk-schemas submodule bumped to a commit containing PR #54.
- Verification: build with the bumped submodule succeeds; manual proto-to-json inspection through `src/aclk/schema-wrappers/proto_2_json.cc` shows the new fields populated correctly during the three lifecycle moments (startup connect, graceful disconnect, node-removed).

## Analysis

Sources checked:

- `src/libnetdata/exit/exit_initiated.{h,c}` — EXIT_REASON enum + bitmap helpers.
- `src/daemon/status-file.{h,c}` — `last_session_status.exit_reason` is file-static; needs accessor.
- `src/daemon/daemon-shutdown.c` — `netdata_cleanup_and_exit()` calls `exit_initiated_set(reason)` before ACLK shutdown, so `exit_initiated_get()` is populated by the time `aclk_graceful_disconnect()` runs.
- `src/aclk/aclk_tx_msgs.c`, `src/aclk/aclk.c` — agent connection update producers.
- `src/aclk/schema-wrappers/connection.{h,cc}`, `node_connection.{h,cc}`, `proto_2_json.cc`, `schema_wrapper_utils.{h,cc}` — message generation surface.
- `src/database/sqlite/sqlite_aclk_node.c`, `src/daemon/service.c`, `src/daemon/commands.c` — node-state update sites.
- aclk-schemas PR #54: diff, comment thread, commit history, latest reviews.

Current state:

- `update_agent_connection_t` has only `claim_id / reachable / session_id / lwt / capabilities`; no slot for exit reasons. Two stale TODO comments in the header reference unused `system_uptime` and `agent_uptime` proto fields — can fold in opportunistically.
- `node_instance_connection_t` has no slot for `reason`.
- `aclk_host_state_update()` / `aclk_update_node_instance_job()` / `send_node_update_with_wait()` all take `(host, live, queryable, ...)` and no reason parameter.

Risks:

- **Cross-repo coordination.** The agent change is unbuildable until PR #54 lands and the submodule is bumped. Coordinate with backend (papazach) for merge order; ideally schema merges first, agent PR follows immediately and bumps the submodule in the same change.
- **Mapping drift.** If a future `EXIT_REASON` is added without updating `AgentExitReason`, mapping must handle the missing value (silent skip + warn rather than crash). Add a static_assert / coverage test that every named `EXIT_REASON` value has a corresponding proto enum.
- **LWT divergence.** Cloud's interpretation of "empty exit_reasons on LWT" needs to be aligned (cross-team note in the PR).
- **Wrong reason on `commands.c:386`.** If backend wants a distinct enum for explicit unregister, we leak that into NO_RETENTION today. Mitigated by the Open decision below.

## Pre-Implementation Gate

Status: needs-user-decision (open decision on `commands.c:386` reason value, plus schema merge timing).

Problem / root-cause model:

- The agent currently provides no Cloud-visible reason for its disconnects (agent-level or per-node), so Cloud cannot suppress reachability notifications during updates and must wait the 60-day prune window after an Agent rotates a node out of its DB. PR #54 adds the schema fields; the agent must populate them.

Evidence reviewed:

- aclk-schemas PR #54 diff and full discussion thread (8 inline comments).
- Agent source paths cited above.
- `exit_initiated.h` 21 enum values vs proto's 21 values — 1:1 by name.

Affected contracts and surfaces:

- Proto: `agent.v1.UpdateAgentConnection`, `nodeinstance.v1.UpdateNodeInstanceConnection`.
- C API: `update_agent_connection_t`, `node_instance_connection_t`, `aclk_host_state_update()`, `aclk_update_node_instance_job()`, `send_node_update_with_wait()`.
- New accessor in `daemon/status-file.h`: `EXIT_REASON daemon_status_file_get_last_exit_reason(void);`.
- New helper in `connection.cc`: bitmap → repeated proto enum.
- Submodule: `src/aclk/aclk-schemas` bump.

Existing patterns to reuse:

- `ENUM_STR_MAP_DEFINE(EXIT_REASON)` / `BITMAP_STR_DEFINE_FUNCTIONS(EXIT_REASON, ...)` in `exit_initiated.c` — same pattern can be extended with a parallel `EXIT_REASON → AgentExitReason` table.
- `daemon_status_file_get_*` accessors in `status-file.{h,c}` — established pattern for file-static state.
- `proto_2_json.cc` already knows both message types, so debugging via JSON dump is already wired.

Risk and blast radius:

- Localized: ACLK message generation only. No on-disk format changes. Backward-compatible — Cloud must tolerate older agents that omit `exit_reasons` / `reason` (proto3 makes them implicitly default).
- Performance: negligible. One small array on each `UpdateAgentConnection`, one enum on each `UpdateNodeInstanceConnection`.

Sensitive data handling plan:

- No secrets, customer data, or PII are touched. Exit reasons are operational metadata. The SOW and code comments do not need redaction.

Implementation plan:

1. **Schema landing** (external, blocking). Coordinate with backend: post on PR #54, request `WIP:` removal, and either nudge `ktsaou`/`stelfrag` for approval or get explicit handoff. Goal: schema merges to aclk-schemas master.
2. **Submodule bump.** Update `src/aclk/aclk-schemas` to the merge commit. Confirm generated `.pb.h` / `.pb.cc` rebuild cleanly.
3. **AgentExitReason mapper.** Add a translator in `src/aclk/schema-wrappers/connection.cc` (or a small new `.cc` file) that walks a `EXIT_REASON` bitmap and appends one proto enum per set bit. Add a compile-time check that every named `EXIT_REASON` value is covered.
4. **Wire UpdateAgentConnection.**
   - Extend `update_agent_connection_t` with `EXIT_REASON exit_reasons;`.
   - Populate `exit_reasons` in `generate_update_agent_connection()` via the mapper.
   - `aclk_send_agent_connection_update(client, reachable=1)`: set `conn.exit_reasons = daemon_status_file_get_last_exit_reason()`.
   - `aclk_send_agent_connection_update(client, reachable=0)`: set `conn.exit_reasons = exit_initiated_get()`.
   - `aclk_generate_lwt()`: leave `exit_reasons` empty (document why with a brief code comment).
   - Add `daemon_status_file_get_last_exit_reason()` to `src/daemon/status-file.{h,c}`.
5. **Wire UpdateNodeInstanceConnection.**
   - Extend `node_instance_connection_t` with the reason (use a small project-local enum that maps to the proto values to avoid pulling proto headers into C).
   - Populate `reason` in `generate_node_instance_connection()`.
   - Extend signatures: `aclk_host_state_update()`, `aclk_update_node_instance_job()`, `send_node_update_with_wait()` to accept the reason.
   - Call sites:
     - `src/daemon/service.c:269` → `NO_RETENTION`.
     - `src/daemon/commands.c:386` → see Open decisions (default plan: `NO_RETENTION`).
     - All other call sites (`aclk_host_state_update_auto`, `aclk_send_node_instances`, etc.) → `UNSPECIFIED`.
6. **Self-tests.** Add a small unit test or a runtime invariant that every `EXIT_REASON` bit translates to a non-zero `AgentExitReason`.

Validation plan:

- Build agent with bumped submodule; no warnings on the new mapping code.
- Run a claimed agent against a staging Cloud and verify (via Cloud feed events or backend log inspection — coordinate with papazach):
  - Restart with a clean exit → next `UpdateAgentConnection(reachable=true)` carries `SIGTERM` (or whatever signal was used).
  - Restart with a forced crash (`kill -SEGV`) → next startup carries `SIGSEGV`.
  - Manual node prune via `netdatacli remove-stale-node` → cloud sees `UpdateNodeInstanceConnection(reason=NO_RETENTION)`.
- Cross-check with `proto_2_json.cc` debug dump on the wire.

Artifact impact plan:

- AGENTS.md: no expected update — the work is feature wiring, not a workflow change.
- Runtime project skills: no expected update.
- Specs: no expected update unless we want to record the agent-level exit reason contract in `.agents/sow/specs/`. Will revisit at close.
- End-user/operator docs: no public docs change. The exit reason is internal cloud telemetry, not surfaced to end users in this SOW.
- End-user/operator skills: no change.
- SOW lifecycle: this SOW is the single source of truth for the work; will close as `completed` once code + schema merge + submodule bump are landed in one agent PR.

Open decisions:

1. **Reason value for `commands.c:386` (`remove-stale-node` netdatacli unregister).**
   - Option A: `NO_RETENTION` — node has been removed from the agent's view, Cloud should treat it the same as a DB rotation. **Recommended.**
   - Option B: `UNSPECIFIED` — keep the explicit-unregister semantically distinct.
   - Option C: ask backend to add a third reason value (e.g., `EXPLICIT_REMOVAL`).
   - Will default to A unless the user or backend objects.
2. **Schema PR merge ordering.** Recommend schema merges first; agent PR opens immediately after with submodule bump + wiring in one commit. Confirm with backend that they are OK to merge with current title (drop `WIP:` first).

## Implications And Decisions

1. **AGENT_UPDATE on UpdateNodeInstanceConnection — cloud-derived (decided 2026-05-13).** Agent never emits this value; cloud derives it from agent-level `exit_reasons` containing `AGENT_EXIT_REASON_UPDATE`. Avoids a shutdown-time per-node fan-out that would slow graceful shutdown without giving cloud any signal it can't already infer.
2. **LWT carries empty `exit_reasons` — by design.** MQTT LWT is fixed at CONNECT time. Cloud's interpretation of an LWT-delivered message with empty exit_reasons must be aligned (cross-team note to be added on the schemas PR).
3. **Mapping is by name, not numeric value.** Implementation must use an explicit table; cannot cast bit position to proto enum.

## Plan

1. Drive PR #54 to merge: post on the PR offering to review on behalf of agent team, ask for `WIP:` removal. Risk: weeks of latency if reviewers are busy.
2. Bump submodule + wire schema fields in a single agent PR. Risk: small.
3. Staging-cloud validation. Risk: requires backend cooperation to confirm Cloud receives and surfaces the new fields.

## Execution Log

### 2026-05-13

- SOW drafted. Memory entry `project_aclk_exit_reasons_pr54.md` saved with the cloud-side-derivation decision.
- Submodule `src/aclk/aclk-schemas` checked out at PR #54 tip (`80a8daa8`, branch `pr-54`) — fetched via `refs/pull/54/head`.
- New `src/aclk/schema-wrappers/connectivity_reason.h`: standalone C-only header defining `NODE_CONNECTIVITY_REASON` so it can be pulled into `aclk.h` / `sqlite_aclk_node.h` (both transitively reach every translation unit through `daemon/common.h` and `database/rrd.h`) without dragging in proto/C++ headers.
- `src/aclk/schema-wrappers/connection.{h,cc}`: extended `update_agent_connection_t` with `EXIT_REASON exit_reasons`; new `exit_reason_bit_to_proto()` and `add_exit_reasons()` walk the bitmap and emit one `AgentExitReason` per set bit (mapping is by name/semantics, not by numeric value).
- `src/aclk/schema-wrappers/node_connection.{h,cc}`: extended `node_instance_connection_t` with `NODE_CONNECTIVITY_REASON reason`; cast to the proto enum in `generate_node_instance_connection()`.
- `src/aclk/aclk_tx_msgs.c`: `aclk_send_agent_connection_update()` now passes `daemon_status_file_get_last_exit_reason()` on reachable=true and `exit_initiated_get()` on reachable=false. `aclk_generate_lwt()` passes `EXIT_REASON_NONE` (commented). New include `daemon/status-file.h`.
- `src/aclk/aclk.{h,c}`: extended `aclk_host_state_update()` and `aclk_update_node_instance_job()` with a `NODE_CONNECTIVITY_REASON reason` parameter. `aclk_host_state_update_auto()` and `aclk_send_node_instances()` pass `UNSPECIFIED`.
- `src/database/sqlite/sqlite_aclk_node.{h,c}`: `send_node_update_with_wait()` extended with a reason parameter; passed through to `aclk_host_state_update()`.
- Call sites updated:
  - `src/daemon/service.c:272` (orphan archived host with zero retention) → `NO_RETENTION`.
  - `src/daemon/commands.c:388` (`netdatacli remove-stale-node`) → `NO_RETENTION` (per default decision recorded above).
- `src/daemon/status-file.{h,c}`: new accessor `daemon_status_file_get_last_exit_reason()`.
- Local build: `ninja netdata` succeeds end-to-end after the include-cycle fix. No warnings introduced by these changes (the unused-but-set warnings in `plugin_tc.c` are pre-existing).

## Validation

Pending implementation.

## Outcome

Pending.

## Lessons Extracted

Pending.

## Followup

None yet.

## Regression Log

None yet.
