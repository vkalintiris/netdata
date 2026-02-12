// SPDX-License-Identifier: GPL-3.0-or-later
//
// Fan-out function: dispatches a function call to all nodes that have it registered,
// collects their responses concurrently, and returns a combined JSON result.
//
// Usage:  fanout <function-name> [arguments...]
// Example: fanout otel-signal-viewer after:-3600 before:0
//
// The implementation uses two passes over rrdhost_root_index:
//  1. Count how many hosts have the target function (to pre-allocate the results array).
//  2. Dispatch rrd_function_run() in async mode (wait=false) to each of those hosts.
//
// A mutex+condvar is used to wait for all async callbacks to complete. If the timeout
// expires before all hosts respond, partial results are returned — nodes that didn't
// respond get code 504 (gateway timeout).
//
// JSON response format:
//  {
//    "status": 200,
//    "type": "fanout",
//    "function": "<target-function-name>",
//    "nodes_total": N,
//    "nodes_completed": M,
//    "nodes": [
//      { "hostname": "...", "machine_guid": "...", "node_id": "...",
//        "code": 200, "response": "<raw function output>" },
//      ...
//    ]
//  }

#include "function-fanout.h"

// Per-host result slot. Each carries a back-pointer to the shared fanout_state
// so the async callback can update completion counters.
struct fanout_result {
    struct fanout_state *state;
    RRDHOST *host;
    BUFFER *wb;
    int code;
    bool done;
};

// Shared state across all async function calls.
// The mutex protects 'completed' and individual result->done/code fields.
// The condvar is signaled when all results are in.
struct fanout_state {
    int total;
    int completed;
    netdata_mutex_t mutex;
    netdata_cond_t cond;
    struct fanout_result *results;
};

// Called by rrd_function_run() from arbitrary threads when a host's function completes.
// Thread-safe: all mutable state is protected by state->mutex.
static void fanout_result_callback(BUFFER *wb __maybe_unused, int code, void *data) {
    struct fanout_result *r = data;
    struct fanout_state *state = r->state;

    netdata_mutex_lock(&state->mutex);
    r->code = code;
    r->done = true;
    state->completed++;
    if(state->completed == state->total)
        netdata_cond_signal(&state->cond);
    netdata_mutex_unlock(&state->mutex);
}

int function_fanout(BUFFER *wb, const char *function, BUFFER *payload, const char *source) {
    // skip "fanout" prefix to get the target command
    const char *target_cmd = function;
    while(*target_cmd && !isspace((uint8_t)*target_cmd))
        target_cmd++;
    while(*target_cmd && isspace((uint8_t)*target_cmd))
        target_cmd++;

    if(!*target_cmd) {
        buffer_flush(wb);
        wb->content_type = CT_APPLICATION_JSON;
        buffer_json_initialize(wb, "\"", "\"", 0, true, BUFFER_JSON_OPTIONS_DEFAULT);
        buffer_json_member_add_uint64(wb, "status", HTTP_RESP_BAD_REQUEST);
        buffer_json_member_add_string(wb, "error", "Usage: fanout <function> [arguments...]");
        buffer_json_member_add_string(wb, "help", RRDFUNCTIONS_FANOUT_HELP);
        buffer_json_finalize(wb);
        return HTTP_RESP_BAD_REQUEST;
    }

    // extract target function name (first word of target_cmd)
    char target_function_name[256];
    {
        const char *s = target_cmd;
        size_t i = 0;
        while(*s && !isspace((uint8_t)*s) && i < sizeof(target_function_name) - 1)
            target_function_name[i++] = *s++;
        target_function_name[i] = '\0';
    }

    // first pass: count hosts that have the target function
    int count = 0;
    {
        RRDHOST *host;
        dfe_start_read(rrdhost_root_index, host) {
            if(rrd_function_available(host, target_function_name))
                count++;
        }
        dfe_done(host);
    }

    if(count == 0) {
        buffer_flush(wb);
        wb->content_type = CT_APPLICATION_JSON;
        buffer_json_initialize(wb, "\"", "\"", 0, true, BUFFER_JSON_OPTIONS_DEFAULT);
        buffer_json_member_add_uint64(wb, "status", HTTP_RESP_NOT_FOUND);
        buffer_json_member_add_string(wb, "error", "No hosts have the requested function");
        buffer_json_member_add_string(wb, "function", target_function_name);
        buffer_json_finalize(wb);
        return HTTP_RESP_NOT_FOUND;
    }

    // allocate state
    struct fanout_state state = {
        .total = count,
        .completed = 0,
        .results = callocz(count, sizeof(struct fanout_result)),
    };
    netdata_mutex_init(&state.mutex);
    netdata_cond_init(&state.cond);

    // second pass: dispatch function calls
    int timeout_s = 120;
    int idx = 0;
    {
        RRDHOST *host;
        dfe_start_read(rrdhost_root_index, host) {
            if(!rrd_function_available(host, target_function_name))
                continue;

            struct fanout_result *r = &state.results[idx];
            r->state = &state;
            r->host = host;
            r->wb = buffer_create(4096, NULL);
            r->code = 0;
            r->done = false;

            rrd_function_run(
                host, r->wb, timeout_s,
                HTTP_ACCESS_ALL, target_cmd,
                false, NULL,
                fanout_result_callback, r,
                NULL, NULL,
                NULL, NULL,
                payload, source, false);

            idx++;
        }
        dfe_done(host);
    }

    // wait for all results with timeout
    usec_t deadline_ut = now_realtime_usec() + (usec_t)timeout_s * USEC_PER_SEC;
    netdata_mutex_lock(&state.mutex);
    while(state.completed < state.total) {
        usec_t now_ut = now_realtime_usec();
        if(now_ut >= deadline_ut)
            break;

        uint64_t remaining_ns = (deadline_ut - now_ut) * NSEC_PER_USEC;
        if(remaining_ns > 100 * NSEC_PER_MSEC)
            remaining_ns = 100 * NSEC_PER_MSEC; // poll every 100ms

        netdata_cond_timedwait(&state.cond, &state.mutex, remaining_ns);
    }
    netdata_mutex_unlock(&state.mutex);

    // build the JSON response
    buffer_flush(wb);
    wb->content_type = CT_APPLICATION_JSON;
    buffer_json_initialize(wb, "\"", "\"", 0, true, BUFFER_JSON_OPTIONS_DEFAULT);

    buffer_json_member_add_uint64(wb, "status", HTTP_RESP_OK);
    buffer_json_member_add_string(wb, "type", "fanout");
    buffer_json_member_add_string(wb, "function", target_function_name);
    buffer_json_member_add_int64(wb, "nodes_total", state.total);
    buffer_json_member_add_int64(wb, "nodes_completed", state.completed);

    buffer_json_member_add_array(wb, "nodes");
    for(int i = 0; i < state.total; i++) {
        struct fanout_result *r = &state.results[i];

        buffer_json_add_array_item_object(wb);
        buffer_json_member_add_string(wb, "hostname", rrdhost_hostname(r->host));
        buffer_json_member_add_string(wb, "machine_guid", r->host->machine_guid);

        if(!UUIDiszero(r->host->node_id))
            buffer_json_member_add_uuid(wb, "node_id", r->host->node_id.uuid);

        if(r->done) {
            buffer_json_member_add_int64(wb, "code", r->code);
            if(buffer_strlen(r->wb) > 0)
                buffer_json_member_add_string(wb, "response", buffer_tostring(r->wb));
            else
                buffer_json_member_add_string(wb, "response", "");
        }
        else {
            buffer_json_member_add_int64(wb, "code", HTTP_RESP_GATEWAY_TIMEOUT);
            buffer_json_member_add_string(wb, "response", "timeout waiting for response");
        }

        buffer_json_object_close(wb);
    }
    buffer_json_array_close(wb);

    buffer_json_finalize(wb);

    // cleanup
    for(int i = 0; i < state.total; i++)
        buffer_free(state.results[i].wb);

    freez(state.results);
    netdata_mutex_destroy(&state.mutex);
    netdata_cond_destroy(&state.cond);

    return HTTP_RESP_OK;
}
