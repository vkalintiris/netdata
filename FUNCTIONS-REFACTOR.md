# Functions API Ergonomics Refactor

Notes on improving the C API ergonomics for the function call infrastructure
(`src/database/rrdfunctions*.c`).

## Problem 0: Misleading names — files and terms

The entire subsystem implements an **RPC/function-call dispatch mechanism**, but
the naming suggests it's related to RRD (Round Robin Database) data storage and
uses non-standard or made-up terminology.

### File names

| Current name | What it sounds like | What it actually does |
|---|---|---|
| `rrdfunctions-inflight.c` | RRD data in transit | Function call **dispatch**: lifecycle tracking, timeout, cancellation, progress |
| `rrdfunctions-inline.c` | C `inline` keyword optimization | Trampoline for **synchronous** function callbacks |
| `rrdfunctions-exporters.c` | Prometheus-style data exporters | **Serialization** of function metadata to JSON and streaming protocol |
| `rrdfunctions.c` | The "main" file? | Function **registration** — the per-host dictionary of available functions |

Proposed renames:

| Current | Proposed | Rationale |
|---|---|---|
| `rrdfunctions-inflight.c` | `rrdfunctions-dispatch.c` | This is the dispatch/execution engine |
| `rrdfunctions-inline.c` | `rrdfunctions-sync.c` | "Sync" is what it means — runs in caller's thread |
| `rrdfunctions-exporters.c` | `rrdfunctions-serialize.c` | Serializes metadata to JSON/streaming protocol; "exporter" means something else in monitoring |
| `rrdfunctions.c` | `rrdfunctions-registry.c` | It manages the per-host registry of available functions |

### Terms in the code

| Current term | Problem | What it means | Proposed |
|---|---|---|---|
| `rrd_` prefix | "RRD" = Round Robin Database. Unrelated to this subsystem. | Historical namespace artifact | Keep for now (too pervasive to change), but document that `rrd_function_*` means "netdata function API", not "RRD database function" |
| `inflight` | Aviation jargon, not standard software term | A function call currently being tracked/executed | `call` or `execution` — e.g., `struct rrd_function_call` |
| `inline` (in `rrd_function_add_inline`) | Suggests C `inline` keyword | Synchronous execution in caller's thread | `sync` — e.g., `rrd_function_add_sync` |
| `exporter` (in file name and function names) | In monitoring, "exporter" = data source (Prometheus exporter) | Serialization of function metadata | `serialize` or `format` |
| `progresser` | Made-up word | The callback that *requests* progress from the implementation (pull-side). Contrast with `progress.cb` which *receives* progress reports (push-side). | `progress_request` or `progress_poll` |
| `struct rrd_host_function` | "host function" — a mathematical function of the host? | A registered function definition with metadata | `struct rrd_function_def` or `struct rrd_registered_function` |
| `struct rrd_function_execute` | Reasonable but verbose | The context passed to a function when it's called | `struct rrd_function_call_args` or keep as-is |
| `rrd_function_run()` | "Run" is vague | Dispatches a function call (with routing, tracking, timeout) | `rrd_function_call()` or `rrd_function_dispatch()` |

### The two progress callbacks — confusing duality

The progress mechanism has two directions that use similar names:

- **Push (implementation → caller)**: `progress.cb` — the function implementation
  calls this to report "I've done 5 of 10 items". Type: `rrd_function_progress_cb_t`.

- **Pull (caller → implementation)**: `progresser.cb` / `register_progresser.cb` —
  the caller (e.g., `/api/v2/progress` endpoint) calls this to *ask* the
  implementation to report progress. Type: `rrd_function_progresser_cb_t`.

The "progresser" name is the main source of confusion. Proposed:

| Current | Proposed | Direction |
|---|---|---|
| `progress.cb` | `on_progress.cb` | push: implementation reports progress |
| `progresser.cb` | `request_progress.cb` | pull: caller asks for a progress update |
| `register_progresser.cb` | `register_progress_request.cb` | implementation registers how to be asked |

## Problem 1: `rrd_function_run()` — 16 positional parameters

This is the biggest issue. Current signature:

```c
int rrd_function_run(RRDHOST *host, BUFFER *result_wb, int timeout_s,
                     HTTP_ACCESS user_access, const char *cmd,
                     bool wait, const char *transaction,
                     rrd_function_result_callback_t result_cb, void *result_cb_data,
                     rrd_function_progress_cb_t progress_cb, void *progress_cb_data,
                     rrd_function_is_cancelled_cb_t is_cancelled_cb, void *is_cancelled_cb_data,
                     BUFFER *payload, const char *source, bool allow_restricted);
```

Most callers pass NULL for 6-8 of the parameters. Swap two NULLs and you get a
silent miscompile. The `bool wait` and `bool allow_restricted` are positional
booleans — `true, NULL, NULL, ...` conveys zero meaning at the call site.

Examples of current caller pain:

```c
// MCP registry — 8 NULLs in a row
rrd_function_run(host, response, 10, auth.access, info_function,
    true, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, false);

// dyncfg echo — which NULL is which?
rrd_function_run(host, e->wb, 10, HTTP_ACCESS_ALL, buf,
    false, NULL, dyncfg_echo_cb, e, NULL, NULL, NULL, NULL,
    NULL, string2str(df->dyncfg.source), false);

// web API — the only caller that uses most params
rrd_function_run(host, wb, timeout, w->user_auth.access, function,
    true, transaction, NULL, NULL,
    web_client_progress_functions_update, NULL,
    web_client_interrupt_callback, w, w->payload,
    buffer_tostring(source), false);
```

### Fix: C99 compound literal with designated initializers

Replace the 16-param function with a struct:

```c
struct rrd_function_request {
    RRDHOST *host;              // required
    BUFFER *wb;                 // required
    const char *function;       // required
    int timeout;
    HTTP_ACCESS access;
    BUFFER *payload;
    const char *source;
    const char *transaction;
    bool wait;
    bool allow_restricted;

    rrd_function_result_callback_t result_cb;
    void *result_cb_data;
    rrd_function_progress_cb_t progress_cb;
    void *progress_cb_data;
    rrd_function_is_cancelled_cb_t is_cancelled_cb;
    void *is_cancelled_cb_data;
};

int rrd_function_run(struct rrd_function_request req);
```

The same call sites become self-documenting:

```c
// MCP registry — clean, no NULLs
int code = rrd_function_run((struct rrd_function_request){
    .host = host,
    .wb = response,
    .function = info_function,
    .timeout = 10,
    .access = auth.access,
    .wait = true,
});

// dyncfg echo — obvious what each field means
rrd_function_run((struct rrd_function_request){
    .host = host,
    .wb = e->wb,
    .function = buf,
    .timeout = 10,
    .access = HTTP_ACCESS_ALL,
    .result_cb = dyncfg_echo_cb,
    .result_cb_data = e,
    .source = string2str(df->dyncfg.source),
});

// web API — still clear even with all the callbacks
rrd_function_run((struct rrd_function_request){
    .host = host,
    .wb = wb,
    .function = function,
    .timeout = timeout,
    .access = w->user_auth.access,
    .wait = true,
    .transaction = transaction,
    .progress_cb = web_client_progress_functions_update,
    .is_cancelled_cb = web_client_interrupt_callback,
    .is_cancelled_cb_data = w,
    .payload = w->payload,
    .source = buffer_tostring(source),
});
```

Omitted fields are zero/NULL/false by C99 guarantee. No positional ambiguity.
The compiler catches typos in field names.

## Problem 2: "MUST call result.cb" is a comment, not enforcement

Every async function implementer must remember:

```c
if(rfe->result.cb)
    rfe->result.cb(rfe->result.wb, code, rfe->result.data);
```

...on every single return path. The inline wrapper (`rrdfunctions-inline.c:23`)
does it automatically for sync functions, but async implementers are on their own.

### Fix: helper functions that make the intent explicit

```c
static inline void rrd_function_result_send(struct rrd_function_execute *rfe, int code) {
    if(rfe->result.cb)
        rfe->result.cb(rfe->result.wb, code, rfe->result.data);
}

static inline void rrd_function_report_progress(struct rrd_function_execute *rfe, size_t done, size_t total) {
    if(rfe->progress.cb)
        rfe->progress.cb(rfe->transaction, rfe->progress.data, done, total);
}

static inline bool rrd_function_is_cancelled(struct rrd_function_execute *rfe) {
    return rfe->is_cancelled.cb && rfe->is_cancelled.cb(rfe->is_cancelled.data);
}
```

Fanout's call sites go from:

```c
if(rfe->result.cb)
    rfe->result.cb(wb, code, rfe->result.data);
return code;
```

to:

```c
rrd_function_result_send(rfe, code);
return code;
```

Eliminates the NULL check boilerplate and makes it greppable — you can search
for functions that *don't* call `rrd_function_result_send` to find bugs.

## Problem 3: Two separate registration APIs

`rrd_function_add_inline()` exists solely to wrap a simple
`(BUFFER *wb, const char *function, BUFFER *payload, const char *source)`
callback into the full `struct rrd_function_execute` interface via a trampoline
in `rrdfunctions-inline.c`. This means implementers have to choose between two
different signatures upfront, and converting between them (as we did with fanout)
requires touching the header, the implementation, and the registration site.

### Fix: deprecate `rrd_function_add_inline`, use one path

With the helpers above, writing an async-style function is almost as simple as
inline:

```c
int function_fanout(struct rrd_function_execute *rfe, void *data) {
    BUFFER *wb = rfe->result.wb;
    // ... do work ...
    rrd_function_result_send(rfe, HTTP_RESP_OK);
    return HTTP_RESP_OK;
}
```

That's barely more boilerplate than the inline signature. One registration path
(`rrd_function_add`), one callback type (`rrd_function_execute_cb_t`).

## Affected call sites

### `rrd_function_run()` — 15 callers

| Location | Layer |
|----------|-------|
| `src/web/api/v1/api_v1_function.c:39` | Web API v1 |
| `src/web/api/v1/api_v1_config.c:84` | Web API v1 |
| `src/web/api/functions/function-bearer_get_token.c:76` | Web API |
| `src/web/api/functions/function-fanout.c:209` | Web API |
| `src/streaming/stream-sender-execute.c:81` | Streaming |
| `src/web/mcp/mcp-tools-execute-function.c:2218` | MCP |
| `src/web/mcp/mcp-tools-execute-function-registry.c:331` | MCP |
| `src/daemon/dyncfg/dyncfg-echo.c:93,126,161` | dyncfg |
| `src/daemon/dyncfg/dyncfg-unittest.c:469` | dyncfg |

### `rrd_function_add_inline()` — 10 registrations

| Location | Function |
|----------|----------|
| `src/web/api/functions/functions.c` (4 calls) | streaming, api-calls, bearer, cardinality |
| `src/collectors/diskspace.plugin/plugin_diskspace.c:1111` | mount-points |
| `src/collectors/proc.plugin/proc_diskstats.c:1410` | block-devices |
| `src/collectors/proc.plugin/proc_net_dev.c:1724` | network-interfaces |
| `src/collectors/cgroups.plugin/sys_fs_cgroup.c:1427,1433` | containers-vms, systemd-services |

## Implementation order

1. **File renames** (low risk, high clarity) — rename the four core files.
   Mechanical `git mv` + update `#include` paths and `CMakeLists.txt`.
2. **Term renames** — rename `progresser` → `request_progress`,
   `struct rrd_function_inflight` → `struct rrd_function_call`, etc.
   Mechanical find-and-replace within `src/database/rrdfunctions*`.
3. **Request struct** (biggest API impact) — change `rrd_function_run`
   signature, update all 15 callers. Purely mechanical, no behavioral change.
4. **Helper functions** — add `rrd_function_result_send`,
   `rrd_function_report_progress`, `rrd_function_is_cancelled` to
   `rrdfunctions.h`. Update existing async implementers.
5. **Unified registration** — migrate `rrd_function_add_inline` callers to
   `rrd_function_add` with the full callback signature, then remove the
   sync wrapper.
