// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ACLK_SCHEMA_WRAPPER_CONNECTION_H
#define ACLK_SCHEMA_WRAPPER_CONNECTION_H

#include "capability.h"
#include "libnetdata/libnetdata.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    const char *claim_id;
    unsigned int reachable:1;

    int64_t session_id;

    unsigned int lwt:1;

    const struct capability *capabilities;

    // Bitmap of EXIT_REASON values to expand into UpdateAgentConnection.exit_reasons.
    // - On reachable=true (initial connect): the previous run's reasons, loaded
    //   from the status file via daemon_status_file_get_last_exit_reason().
    // - On reachable=false (graceful disconnect): the current run's reasons
    //   via exit_initiated_get().
    // - On LWT: always EXIT_REASON_NONE (MQTT LWT is fixed at CONNECT time,
    //   before any exit reason is known).
    EXIT_REASON exit_reasons;

// TODO in future optional fields
// > 15 optional fields:
// How long the system was running until connection (only applicable when reachable=true)
//    google.protobuf.Duration system_uptime = 15;
// How long the netdata agent was running until connection (only applicable when reachable=true)
//    google.protobuf.Duration agent_uptime = 16;


} update_agent_connection_t;

char *generate_update_agent_connection(size_t *len, const update_agent_connection_t *data);

struct disconnect_cmd {
    uint64_t reconnect_after_s;
    int permaban;
    uint32_t error_code;
    char *error_description;
};

struct disconnect_cmd *parse_disconnect_cmd(const char *data, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* ACLK_SCHEMA_WRAPPER_CONNECTION_H */
