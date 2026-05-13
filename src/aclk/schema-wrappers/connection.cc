// SPDX-License-Identifier: GPL-3.0-or-later

#include "src/aclk/aclk-schemas/proto/agent/v1/connection.pb.h"
#include "src/aclk/aclk-schemas/proto/agent/v1/disconnect.pb.h"
#include "connection.h"

#include "schema_wrapper_utils.h"

#include <sys/time.h>
#include <stdlib.h>

using namespace agent::v1;

// Maps a single EXIT_REASON bit to its AgentExitReason proto enum.
// The proto values are sequential (0..20) and do NOT match the EXIT_REASON
// bitmap positions (1<<0..1<<19), so mapping is by name/semantics, not by
// numeric value. Returns AGENT_EXIT_REASON_NONE if the bit is unknown.
static AgentExitReason exit_reason_bit_to_proto(EXIT_REASON bit) {
    switch (bit) {
        case EXIT_REASON_SIGBUS:          return AGENT_EXIT_REASON_SIGBUS;
        case EXIT_REASON_SIGSEGV:         return AGENT_EXIT_REASON_SIGSEGV;
        case EXIT_REASON_SIGFPE:          return AGENT_EXIT_REASON_SIGFPE;
        case EXIT_REASON_SIGILL:          return AGENT_EXIT_REASON_SIGILL;
        case EXIT_REASON_SIGABRT:         return AGENT_EXIT_REASON_SIGABRT;
        case EXIT_REASON_SIGSYS:          return AGENT_EXIT_REASON_SIGSYS;
        case EXIT_REASON_SIGXCPU:         return AGENT_EXIT_REASON_SIGXCPU;
        case EXIT_REASON_SIGXFSZ:         return AGENT_EXIT_REASON_SIGXFSZ;
        case EXIT_REASON_OUT_OF_MEMORY:   return AGENT_EXIT_REASON_OUT_OF_MEMORY;
        case EXIT_REASON_ALREADY_RUNNING: return AGENT_EXIT_REASON_ALREADY_RUNNING;
        case EXIT_REASON_FATAL:           return AGENT_EXIT_REASON_FATAL;
        case EXIT_REASON_API_QUIT:        return AGENT_EXIT_REASON_API_QUIT;
        case EXIT_REASON_CMD_EXIT:        return AGENT_EXIT_REASON_CMD_EXIT;
        case EXIT_REASON_SIGQUIT:         return AGENT_EXIT_REASON_SIGQUIT;
        case EXIT_REASON_SIGTERM:         return AGENT_EXIT_REASON_SIGTERM;
        case EXIT_REASON_SIGINT:          return AGENT_EXIT_REASON_SIGINT;
        case EXIT_REASON_SERVICE_STOP:    return AGENT_EXIT_REASON_SERVICE_STOP;
        case EXIT_REASON_SYSTEM_SHUTDOWN: return AGENT_EXIT_REASON_SYSTEM_SHUTDOWN;
        case EXIT_REASON_UPDATE:          return AGENT_EXIT_REASON_UPDATE;
        case EXIT_REASON_SHUTDOWN_TIMEOUT:return AGENT_EXIT_REASON_SHUTDOWN_TIMEOUT;
        case EXIT_REASON_NONE:            return AGENT_EXIT_REASON_NONE;
    }
    return AGENT_EXIT_REASON_NONE;
}

static void add_exit_reasons(UpdateAgentConnection &connupd, EXIT_REASON reasons) {
    if (reasons == EXIT_REASON_NONE)
        return;

    // Walk the bitmap; emit one proto enum per set bit. Unknown bits map to
    // AGENT_EXIT_REASON_NONE and are skipped (forward-compatibility for new
    // EXIT_REASON values that haven't been added to the proto yet).
    for (unsigned i = 0; i < sizeof(EXIT_REASON) * 8; i++) {
        EXIT_REASON bit = (EXIT_REASON)(1u << i);
        if (!(reasons & bit))
            continue;
        AgentExitReason proto_val = exit_reason_bit_to_proto(bit);
        if (proto_val != AGENT_EXIT_REASON_NONE)
            connupd.add_exit_reasons(proto_val);
    }
}

char *generate_update_agent_connection(size_t *len, const update_agent_connection_t *data)
{
    UpdateAgentConnection connupd;

    connupd.set_claim_id(data->claim_id);
    connupd.set_reachable(data->reachable);
    connupd.set_session_id(data->session_id);

    connupd.set_update_source((data->lwt) ? CONNECTION_UPDATE_SOURCE_LWT : CONNECTION_UPDATE_SOURCE_AGENT);

    add_exit_reasons(connupd, data->exit_reasons);

    struct timeval tv;
    gettimeofday(&tv, NULL);

    google::protobuf::Timestamp *timestamp = connupd.mutable_updated_at();
    timestamp->set_seconds(tv.tv_sec);
    timestamp->set_nanos(tv.tv_usec * 1000);

    if (data->capabilities) {
        const struct capability *capa = data->capabilities;
        while (capa->name) {
            aclk_lib::v1::Capability *proto_capa = connupd.add_capabilities();
            capability_set(proto_capa, capa);
            capa++;
        }
    }

    *len = PROTO_COMPAT_MSG_SIZE(connupd);
    char *msg = (char*)mallocz(*len);
    if (msg)
        connupd.SerializeToArray(msg, *len);

    return msg;
}

struct disconnect_cmd *parse_disconnect_cmd(const char *data, size_t len) {
    DisconnectReq req;
    struct disconnect_cmd *res;

    if (!req.ParseFromArray(data, len))
        return NULL;

    res = (struct disconnect_cmd *)callocz(1, sizeof(struct disconnect_cmd));

    if (!res)
        return NULL;

    res->reconnect_after_s = req.reconnect_after_seconds();
    res->permaban = req.permaban();
    res->error_code = req.error_code();
    if (req.error_description().c_str()) {
        res->error_description = strdupz(req.error_description().c_str());
        if (!res->error_description) {
            freez(res);
            return NULL;
        }
    }

    return res;
}
