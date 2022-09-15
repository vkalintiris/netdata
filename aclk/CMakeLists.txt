# SPDX-License-Identifier: GPL-3.0-or-later

set(ACLK_ALWAYS_BUILD
    aclk/aclk_rrdhost_state.h
    aclk/aclk_proxy.c
    aclk/aclk_proxy.h
    aclk/aclk.c
    aclk/aclk.h)

target_sources(netdata PRIVATE  ${ACLK_ALWAYS_BUILD})

set(ACLK_FILES
    aclk/aclk_util.c
    aclk/aclk_util.h
    aclk/aclk_stats.c
    aclk/aclk_stats.h
    aclk/aclk_query.c
    aclk/aclk_query.h
    aclk/aclk_query_queue.c
    aclk/aclk_query_queue.h
    aclk/aclk_otp.c
    aclk/aclk_otp.h
    aclk/aclk_tx_msgs.c
    aclk/aclk_tx_msgs.h
    aclk/aclk_rx_msgs.c
    aclk/aclk_rx_msgs.h
    aclk/https_client.c
    aclk/https_client.h
    aclk/aclk_charts_api.c
    aclk/aclk_charts_api.h
    aclk/aclk_alarm_api.c
    aclk/aclk_alarm_api.h
    aclk/aclk_contexts_api.c
    aclk/aclk_contexts_api.h
    mqtt_websockets/src/mqtt_wss_client.c
    mqtt_websockets/src/include/mqtt_wss_client.h
    mqtt_websockets/src/mqtt_wss_log.c
    mqtt_websockets/src/include/mqtt_wss_log.h
    mqtt_websockets/src/ws_client.c
    mqtt_websockets/src/include/ws_client.h
    mqtt_websockets/src/mqtt_ng.c
    mqtt_websockets/src/include/mqtt_ng.h
    mqtt_websockets/src/common_public.c
    mqtt_websockets/src/include/common_public.h
    mqtt_websockets/src/include/common_internal.h
    mqtt_websockets/c-rbuf/src/ringbuffer.c
    mqtt_websockets/c-rbuf/include/ringbuffer.h
    mqtt_websockets/c-rbuf/src/ringbuffer_internal.h
    mqtt_websockets/MQTT-C/src/mqtt.c
    mqtt_websockets/MQTT-C/include/mqtt.h
    aclk/schema-wrappers/connection.cc
    aclk/schema-wrappers/connection.h
    aclk/schema-wrappers/node_connection.cc
    aclk/schema-wrappers/node_connection.h
    aclk/schema-wrappers/node_creation.cc
    aclk/schema-wrappers/node_creation.h
    aclk/schema-wrappers/chart_stream.cc
    aclk/schema-wrappers/chart_stream.h
    aclk/schema-wrappers/chart_config.cc
    aclk/schema-wrappers/chart_config.h
    aclk/schema-wrappers/alarm_stream.cc
    aclk/schema-wrappers/alarm_stream.h
    aclk/schema-wrappers/alarm_config.cc
    aclk/schema-wrappers/alarm_config.h
    aclk/schema-wrappers/node_info.cc
    aclk/schema-wrappers/node_info.h
    aclk/schema-wrappers/capability.cc
    aclk/schema-wrappers/capability.h
    aclk/schema-wrappers/proto_2_json.cc
    aclk/schema-wrappers/proto_2_json.h
    aclk/schema-wrappers/context_stream.cc
    aclk/schema-wrappers/context_stream.h
    aclk/schema-wrappers/context.cc
    aclk/schema-wrappers/context.h
    aclk/schema-wrappers/schema_wrappers.h
    aclk/schema-wrappers/schema_wrapper_utils.cc
    aclk/schema-wrappers/schema_wrapper_utils.h)

target_sources(netdata PRIVATE  ${ACLK_FILES})

find_package(Protobuf)

set(Protobuf_DEBUG On)

if(False)
set(ACLK_PROTO_DEFS
    aclk/aclk-schemas/proto/aclk/v1/lib.proto
    aclk/aclk-schemas/proto/agent/v1/disconnect.proto
    aclk/aclk-schemas/proto/agent/v1/connection.proto
    aclk/aclk-schemas/proto/alarm/v1/config.proto
    aclk/aclk-schemas/proto/alarm/v1/stream.proto
    aclk/aclk-schemas/proto/chart/v1/config.proto
    aclk/aclk-schemas/proto/chart/v1/dimension.proto
    aclk/aclk-schemas/proto/chart/v1/instance.proto
    aclk/aclk-schemas/proto/chart/v1/stream.proto
    aclk/aclk-schemas/proto/nodeinstance/connection/v1/connection.proto
    aclk/aclk-schemas/proto/nodeinstance/create/v1/creation.proto
    aclk/aclk-schemas/proto/nodeinstance/info/v1/info.proto
    aclk/aclk-schemas/proto/context/v1/context.proto
    aclk/aclk-schemas/proto/context/v1/stream.proto
)
endif()

set(AGENT_V1_CONNECTION_PROTO aclk/aclk-schemas/proto/agent/v1/connection.proto)
protobuf_generate_cpp(AGENT_V1_CONNECTION_SRC AGENT_V1_CONNECTION_HDR ${AGENT_V1_CONNECTION_PROTO})
target_sources(netdata PRIVATE ${AGENT_V1_CONNECTION_SRC} ${AGENT_V1_CONNECTION_HDR})

set(NODEINSTANCE_V1_CONNECTION_PROTO aclk/aclk-schemas/proto/nodeinstance/connection/v1/connection.proto)
protobuf_generate_cpp(NODEINSTANCE_V1_CONNECTION_SRC NODEINSTANCE_V1_CONNECTION_HDR ${NODEINSTANCE_V1_CONNECTION_PROTO})
target_sources(netdata PRIVATE ${NODEINSTANCE_V1_CONNECTION_SRC} ${NODEINSTANCE_V1_CONNECTION_HDR})

# protobuf_generate_cpp(ACLK_PROTO_BUILT_SRCS ACLK_PROTO_BUILT_HDRS ${ACLK_PROTO_DEFS})
# target_sources(netdata PRIVATE ${ACLK_PROTO_BUILT_SRCS} ${ACLK_PROTO_BUILT_HDRS})

include_directories(BEFORE ${CMAKE_SOURCE_DIR}/aclk/aclk-schemas)
include_directories(BEFORE ${CMAKE_SOURCE_DIR}/mqtt_websockets/MQTT-C/include)
include_directories(BEFORE ${CMAKE_SOURCE_DIR}/mqtt_websockets/src/include)
include_directories(BEFORE ${CMAKE_SOURCE_DIR}/mqtt_websockets/c-rbuf/include)

list(APPEND NETDATA_COMMON_LIBRARIES ${PROTOBUF_LIBRARIES})
list(APPEND NETDATA_COMMON_INCLUDE_DIRS ${PROTOBUF_INCLUDE_DIRS})
list(APPEND NETDATA_COMMON_CFLAGS ${PROTOBUF_CFLAGS_OTHER})
