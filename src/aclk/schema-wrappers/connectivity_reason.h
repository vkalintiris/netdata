// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ACLK_SCHEMA_WRAPPER_CONNECTIVITY_REASON_H
#define ACLK_SCHEMA_WRAPPER_CONNECTIVITY_REASON_H

// Mirrors nodeinstance.v1.NodeInstanceConnectivityReason. Kept in a separate
// header so it can be included from C-side headers (aclk.h, sqlite_aclk_node.h)
// without pulling in the proto/C++ machinery via node_connection.h.
//
// AGENT_UPDATE is reserved for cloud-side derivation (Cloud infers it from the
// agent-level UpdateAgentConnection.exit_reasons) and is intentionally not
// emitted by the agent — kept here so the C-side enum stays a faithful mirror
// of the proto enum.
typedef enum {
    NODE_CONNECTIVITY_REASON_UNSPECIFIED  = 0,
    NODE_CONNECTIVITY_REASON_NO_RETENTION = 1,
    NODE_CONNECTIVITY_REASON_AGENT_UPDATE = 2,
} NODE_CONNECTIVITY_REASON;

#endif /* ACLK_SCHEMA_WRAPPER_CONNECTIVITY_REASON_H */
