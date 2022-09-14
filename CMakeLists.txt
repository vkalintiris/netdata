# SPDX-License-Identifier: GPL-3.0-or-later

cmake_minimum_required(VERSION 3.24)
project(netdata C CXX)

# -----------------------------------------------------------------------------
# O/S Detection

set(LINUX   False)
set(FREEBSD False)
set(MACOS   False)

if(${CMAKE_SYSTEM_NAME} MATCHES "Darwin")
    set(MACOS True)
elseif(${CMAKE_SYSTEM_NAME} MATCHES "FreeBSD")
    SET(FREEBSD True)
else()
    SET(LINUX True)
endif()

message(STATUS "system name: ${CMAKE_SYSTEM_NAME}")

# -----------------------------------------------------------------------------
# Toolchain

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 11)

find_program(CCACHE_FOUND ccache)
if(CCACHE_FOUND)
    message(STATUS "Enabling ccache for C/C++")
    set(CMAKE_C_COMPILER_LAUNCHER ccache)
    set(CMAKE_CXX_COMPILER_LAUNCHER ccache)
endif()

if(${CMAKE_C_COMPILER_ID} STREQUAL "GNU")
    SET(LIKELY_MACRO "__builtin_expect(!!(x), 1)")
    SET(UNLIKELY_MACRO "__builtin_expect(!!(x), 0)")
else()
    SET(LIKELY_MACRO "(x)")
    SET(UNLIKELY_MACRO "(x)")
endif()

if(${CMAKE_C_COMPILER_ID} STREQUAL "GNU")
    SET(ALWAYS_UNUSED_MACRO "__attribute__((unused))")
    SET(MAYBE_UNUSED_MACRO "__attribute__((unused))")
else()
    SET(ALWAYS_UNUSED_MACRO "")
    SET(MAYBE_UNUSED_MACRO "")
endif()

# -----------------------------------------------------------------------------
# Find required/optional packages

find_package(Threads REQUIRED)
find_package(PkgConfig REQUIRED)

include(cmake/aws.cmake)
include(cmake/jsonc.cmake)
include(cmake/libcrypto.cmake)
include(cmake/libcups.cmake)
include(cmake/libcurl.cmake)
include(cmake/liblz4.cmake)
include(cmake/libmongoc.cmake)
include(cmake/libssl.cmake)
include(cmake/libuv.cmake)
include(cmake/openssl.cmake)
include(cmake/protobuf.cmake)
include(cmake/pubsub.cmake)
include(cmake/snappy.cmake)
include(cmake/uuid.cmake)
include(cmake/zlib.cmake)
include(cmake/grpc.cmake)

if(LINUX)
    include(cmake/libcap.cmake)
    include(cmake/libipmimonitoring.cmake)
    include(cmake/libmnl.cmake)
    include(cmake/libnetfilter_acct.cmake)
    include(cmake/libxenstat.cmake)
    include(cmake/libelf.cmake)
endif()

if(MACOS)
    include(cmake/iokit.cmake)
    include(cmake/foundation.cmake)
endif()

set(NETDATA_COMMON_CFLAGS ${UUID_CFLAGS_OTHER}
                          ${ZLIB_CFLAGS_OTHER}
                          ${LIBUV_CFLAGS_OTHER}
                          ${LIBLZ4_CFLAGS_OTHER}
                          ${OPENSSL_CFLAGS_OTHER}
                          ${JSONC_CFLAGS_OTHER})

set(NETDATA_COMMON_LIBRARIES ${UUID_LIBRARIES}
                             ${ZLIB_LIBRARIES}
                             ${LIBUV_LIBRARIES}
                             ${LIBLZ4_LIBRARIES}
                             ${OPENSSL_LIBRARIES}
                             ${JSON_LIBRARIES})

set(NETDATA_COMMON_INCLUDE_DIRS ${UUID_INCLUDE_DIRS}
                                ${ZLIB_INCLUDE_DIRS}
                                ${LIBUV_INCLUDE_DIRS}
                                ${LIBLZ4_INCLUDE_DIRS}
                                ${OPENSSL_INCLUDE_DIRS}
                                ${JSON_INCLUDE_DIRS})

set(NETDATA_INCLUDE_DIR ${PROJECT_BINARY_DIR}/include)
set(NETDATA_COMMON_INCLUDE_DIRS ${NETDATA_COMMON_INCLUDE_DIRS} ${NETDATA_INCLUDE_DIR})

# -----------------------------------------------------------------------------
# Detect platform headers & types

include(CheckIncludeFile)
include(CheckCSourceCompiles)
include(CheckLibraryExists)
include(CheckStructHasMember)
include(CheckSymbolExists)

check_include_file(time.h           HAVE_TIME_H)
check_include_file(unistd.h         HAVE_UNISTD_H)
check_include_file(sys/statfs.h     HAVE_SYS_STATFS_H)
check_include_file(sys/statvfs.h    HAVE_SYS_STATVFS_H)
check_include_file(inttypes.h       HAVE_INTTYPES_H)
check_include_file(stdint.h         HAVE_STDINT_H)
check_include_file(sys/mkdev.h      HAVE_SYS_MKDEV_H)
check_include_file(sys/sysmacros.h  HAVE_SYS_SYSMACROS_H)

if(HAVE_TIME_H)
    check_struct_has_member("struct timespec" tv_sec "time.h" HAVE_STRUCT_TIMESPEC)
endif()

if(HAVE_SYS_MKDEV_H)
    check_symbol_exists(major "sys/mkdev.h" MAJOR_IN_MKDEV)
endif()

if (HAVE_SYS_SYSMACROS_H)
    check_symbol_exists(major "sys/sysmacros.h" MAJOR_IN_SYSMACROS)
endif()

check_c_source_compiles("
  #define _GNU_SOURCE
  #include <string.h>
  int main() { char x = *strerror_r(0, &x, sizeof(x)); return 0; }
  " STRERROR_R_CHAR_P)

if(CRYPTO_FOUND)
    set(HAVE_CRYPTO True)
    FIND_LIBRARY(CRYPTO_LIBRARY_LOCATION NAMES crypto)
    check_library_exists(crypto X509_VERIFY_PARAM_set1_host ${CRYPTO_LIBRARY_LOCATION} HAVE_X509_VERIFY_PARAM_set1_host)
    if (HAVE_X509_VERIFY_PARAM_set1_host)
        set(HAVE_X509_VERIFY_PARAM_set1_host True)
    endif()
endif()

# -----------------------------------------------------------------------------
# Detect libbpf
set(ENABLE_PLUGIN_EBPF False)
if(LINUX AND EXISTS "${CMAKE_SOURCE_DIR}/externaldeps/libbpf/libbpf.a")
    message(STATUS "libbpf library found")
    # later we use:
    # ${ELF_LIBRARIES}
    # ${ELF_CFLAGS_OTHER}
    # ${ELF_INCLUDE_DIRS}
    if(ELF_LIBRARIES)
        list(APPEND NETDATA_COMMON_CFLAGS ${ELF_CFLAGS_OTHER})
        list(INSERT NETDATA_COMMON_LIBRARIES 0
            ${CMAKE_SOURCE_DIR}/externaldeps/libbpf/libbpf.a
            ${ELF_LIBRARIES})
        list(APPEND NETDATA_COMMON_INCLUDE_DIRS ${ELF_INCLUDE_DIRS})
        include_directories(BEFORE ${CMAKE_SOURCE_DIR}/externaldeps/libbpf/include ${CMAKE_SOURCE_DIR}/externaldeps/libbpf/include/uapi)
        set(ENABLE_PLUGIN_EBPF True)
        set(HAVE_LIBBPF True)
    else(ELF_LIBRARIES)
        message(STATUS "ebpf plugin: disabled (requires libelf)")
    endif(ELF_LIBRARIES)
endif()

# -----------------------------------------------------------------------------
# netdata files

set(NETDATA_USER "netdata" CACHE STRING "use this user to drop privileges")
set(NETDATA_CACHE_DIR "${CMAKE_INSTALL_PREFIX}/var/cache/netdata" CACHE PATH "cache dir")
set(NETDATA_CONFIG_DIR "${CMAKE_INSTALL_PREFIX}/etc/netdata" CACHE PATH "config dir")
set(NETDATA_LIBCONFIG_DIR "${CMAKE_INSTALL_PREFIX}/usr/lib/netdata/conf.d" CACHE PATH "libconfig dir")
set(NETDATA_LOG_DIR "${CMAKE_INSTALL_PREFIX}/var/log/netdata" CACHE PATH "log dir")
set(NETDATA_PLUGINS_DIR "${CMAKE_INSTALL_PREFIX}/usr/libexec/netdata/plugins.d" CACHE PATH "plugins dir")
set(NETDATA_WEB_DIR "${CMAKE_INSTALL_PREFIX}/usr/share/netdata/web" CACHE PATH "web dir")
set(NETDATA_VARLIB_DIR "${CMAKE_INSTALL_PREFIX}/var/lib/netdata" CACHE PATH "varlib dir")

set(NETDATA_COMMON_LIBRARIES ${NETDATA_COMMON_LIBRARIES} judy)

set(EBPF_PROCESS_PLUGIN_FILES
    collectors/ebpf.plugin/ebpf.c
    collectors/ebpf.plugin/ebpf.h
    collectors/ebpf.plugin/ebpf_cachestat.c
    collectors/ebpf.plugin/ebpf_cachestat.h
    collectors/ebpf.plugin/ebpf_dcstat.c
    collectors/ebpf.plugin/ebpf_dcstat.h
    collectors/ebpf.plugin/ebpf_disk.c
    collectors/ebpf.plugin/ebpf_disk.h
    collectors/ebpf.plugin/ebpf_fd.c
    collectors/ebpf.plugin/ebpf_fd.h
    collectors/ebpf.plugin/ebpf_hardirq.c
    collectors/ebpf.plugin/ebpf_hardirq.h
    collectors/ebpf.plugin/ebpf_mdflush.c
    collectors/ebpf.plugin/ebpf_mdflush.h
    collectors/ebpf.plugin/ebpf_mount.c
    collectors/ebpf.plugin/ebpf_mount.h
    collectors/ebpf.plugin/ebpf_filesystem.c
    collectors/ebpf.plugin/ebpf_filesystem.h
    collectors/ebpf.plugin/ebpf_oomkill.c
    collectors/ebpf.plugin/ebpf_oomkill.h
    collectors/ebpf.plugin/ebpf_process.c
    collectors/ebpf.plugin/ebpf_process.h
    collectors/ebpf.plugin/ebpf_shm.c
    collectors/ebpf.plugin/ebpf_shm.h
    collectors/ebpf.plugin/ebpf_socket.c
    collectors/ebpf.plugin/ebpf_socket.h
    collectors/ebpf.plugin/ebpf_softirq.c
    collectors/ebpf.plugin/ebpf_softirq.h
    collectors/ebpf.plugin/ebpf_sync.c
    collectors/ebpf.plugin/ebpf_sync.h
    collectors/ebpf.plugin/ebpf_swap.c
    collectors/ebpf.plugin/ebpf_swap.h
    collectors/ebpf.plugin/ebpf_vfs.c
    collectors/ebpf.plugin/ebpf_vfs.h
    collectors/ebpf.plugin/ebpf_apps.c
    collectors/ebpf.plugin/ebpf_apps.h
    collectors/ebpf.plugin/ebpf_cgroup.c
    collectors/ebpf.plugin/ebpf_cgroup.h)

set(ACLK_ALWAYS_BUILD
    aclk/aclk_rrdhost_state.h
    aclk/aclk_proxy.c
    aclk/aclk_proxy.h
    aclk/aclk.c
    aclk/aclk.h)

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

set(EXPORTING_ENGINE_FILES
    exporting/exporting_engine.c
    exporting/exporting_engine.h
    exporting/graphite/graphite.c
    exporting/graphite/graphite.h
    exporting/json/json.c
    exporting/json/json.h
    exporting/opentsdb/opentsdb.c
    exporting/opentsdb/opentsdb.h
    exporting/prometheus/prometheus.c
    exporting/prometheus/prometheus.h
    exporting/read_config.c
    exporting/clean_connectors.c
    exporting/init_connectors.c
    exporting/process_data.c
    exporting/check_filters.c
    exporting/send_data.c
    exporting/send_internal_metrics.c)

set(PROMETHEUS_REMOTE_WRITE_EXPORTING_FILES
    exporting/prometheus/remote_write/remote_write.c
    exporting/prometheus/remote_write/remote_write.h
    exporting/prometheus/remote_write/remote_write_request.cc
    exporting/prometheus/remote_write/remote_write_request.h)

set(KINESIS_EXPORTING_FILES
    exporting/aws_kinesis/aws_kinesis.c
    exporting/aws_kinesis/aws_kinesis.h
    exporting/aws_kinesis/aws_kinesis_put_record.cc
    exporting/aws_kinesis/aws_kinesis_put_record.h)

set(PUBSUB_EXPORTING_FILES
    exporting/pubsub/pubsub.c
    exporting/pubsub/pubsub.h
    exporting/pubsub/pubsub_publish.cc
    exporting/pubsub/pubsub_publish.h)

set(MONGODB_EXPORTING_FILES
    exporting/mongodb/mongodb.c
    exporting/mongodb/mongodb.h)

set(NETDATA_FILES collectors/all.h
                  ${EXPORTING_ENGINE_FILES})

set(NETDATACLI_FILES
        daemon/commands.h
        cli/cli.c
        cli/cli.h
        )

add_executable(netdata ${NETDATA_FILES})

# -----------------------------------------------------------------------------
# kinesis exporting connector

if(KINESIS_LIBRARIES AND AWS_CORE_LIBRARIES AND HAVE_AWS_EVENT_STREAM AND HAVE_AWS_COMMON AND HAVE_AWS_CHECKSUMS AND
   CRYPTO_LIBRARIES AND SSL_LIBRARIES AND CURL_LIBRARIES)
    set(ENABLE_EXPORTING_KINESIS True)
else()
    set(ENABLE_EXPORTING_KINESIS False)
endif()

if(ENABLE_EXPORTING_KINESIS)
    message(STATUS "kinesis exporting: enabled")
    target_sources(netdata PRIVATE ${KINESIS_EXPORTING_FILES})
    list(APPEND NETDATA_COMMON_LIBRARIES ${KINESIS_LIBRARIES} ${AWS_CORE_LIBRARIES}
                                         ${CRYPTO_LIBRARIES} ${SSL_LIBRARIES} ${CURL_LIBRARIES})
    list(APPEND NETDATA_COMMON_INCLUDE_DIRS ${KINESIS_INCLUDE_DIRS} ${AWS_CORE_INCLUDE_DIRS}
                                            ${CRYPTO_INCLUDE_DIRS} ${SSL_INCLUDE_DIRS} ${CURL_INCLUDE_DIRS})
    list(APPEND NETDATA_COMMON_CFLAGS ${CRYPTO_CFLAGS_OTHER} ${SSL_CFLAGS_OTHER} ${CURL_CFLAGS_OTHER})
else()
    message(STATUS "kinesis exporting: disabled (requires AWS SDK for C++)")
endif()

# -----------------------------------------------------------------------------
# Pub/Sub exporting connector

if(GRPC_LIBRARIES AND PUBSUB_LIBRARIES)
    SET(ENABLE_EXPORTING_PUBSUB True)
else()
    SET(ENABLE_EXPORTING_PUBSUB False)
endif()

if(ENABLE_EXPORTING_PUBSUB)
    message(STATUS "pubsub exporting connector: enabled")

    target_sources(netdata PRIVATE ${PUBSUB_EXPORTING_FILES})
    list(APPEND NETDATA_COMMON_LIBRARIES ${GRPC_LIBRARIES} ${PUBSUB_LIBRARIES})
    list(APPEND NETDATA_COMMON_INCLUDE_DIRS ${GRPC_INCLUDE_DIRS} ${PUBSUB_INCLUDE_DIRS})
    list(APPEND NETDATA_COMMON_CFLAGS ${GRPC_CFLAGS_OTHER} ${PUBSUB_CFLAGS_OTHER})
else()
    message(STATUS "pubsub exporting connector: disabled (requires grpc and googleapis)")
endif()

# -----------------------------------------------------------------------------
# prometheus remote write exporting connector

if(PROTOBUF_LIBRARIES AND SNAPPY_LIBRARIES)
    SET(ENABLE_EXPORTING_PROMETHEUS_REMOTE_WRITE True)
else()
    SET(ENABLE_EXPORTING_PROMETHEUS_REMOTE_WRITE False)
endif()

if(ENABLE_EXPORTING_PROMETHEUS_REMOTE_WRITE)
    message(STATUS "prometheus remote write exporting: enabled")

    find_package(Protobuf REQUIRED)

    function(PROTOBUF_REMOTE_WRITE_GENERATE_CPP SRCS HDRS)
        if(NOT ARGN)
            message(SEND_ERROR "Error: PROTOBUF_REMOTE_WRITE_GENERATE_CPP() called without any proto files")
            return()
        endif()

        set(${SRCS})
        set(${HDRS})
        foreach(FIL ${ARGN})
            get_filename_component(ABS_FIL ${FIL} ABSOLUTE)
            get_filename_component(DIR ${ABS_FIL} DIRECTORY)
            get_filename_component(FIL_WE ${FIL} NAME_WE)
            set(GENERATED_PB_CC "${DIR}/${FIL_WE}.pb.cc")
            set(GENERATED_PB_H "${DIR}/${FIL_WE}.pb.h")
            list(APPEND ${SRCS} ${GENERATED_PB_CC})
            list(APPEND ${HDRS} ${GENERATED_PB_H})
            add_custom_command(
            OUTPUT ${GENERATED_PB_CC}
                    ${GENERATED_PB_H}
            COMMAND  ${PROTOBUF_PROTOC_EXECUTABLE}
            ARGS -I=${CMAKE_SOURCE_DIR}/exporting/prometheus/remote_write --cpp_out=${CMAKE_SOURCE_DIR}/exporting/prometheus/remote_write ${ABS_FIL}
            DEPENDS ${ABS_FIL} ${PROTOBUF_PROTOC_EXECUTABLE}
            COMMENT "Running C++ protocol buffer compiler on ${FIL}"
            VERBATIM )
        endforeach()
        set_source_files_properties(${${SRCS}} ${${HDRS}} PROPERTIES GENERATED TRUE)
        set(${SRCS} ${${SRCS}} PARENT_SCOPE)
        set(${HDRS} ${${HDRS}} PARENT_SCOPE)
    endfunction()

    protobuf_remote_write_generate_cpp(PROTO_SRCS PROTO_HDRS exporting/prometheus/remote_write/remote_write.proto)


    target_sources(netdata PRIVATE ${PROMETHEUS_REMOTE_WRITE_EXPORTING_FILES} ${PROTO_SRCS} ${PROTO_HDRS})
    list(APPEND NETDATA_COMMON_LIBRARIES ${PROTOBUF_LIBRARIES} ${SNAPPY_LIBRARIES})
    list(APPEND NETDATA_COMMON_INCLUDE_DIRS ${PROTOBUF_INCLUDE_DIRS} ${SNAPPY_INCLUDE_DIRS} ${CMAKE_CURRENT_BINARY_DIR})
    list(APPEND NETDATA_COMMON_CFLAGS ${PROTOBUF_CFLAGS_OTHER} ${SNAPPY_CFLAGS_OTHER})
else()
    message(STATUS "prometheus remote write exporting: disabled (requires protobuf and snappy libraries)")
endif()

# -----------------------------------------------------------------------------
# mongodb exporting connector

if(MONGOC_LIBRARIES)
    message(STATUS "mongodb exporting: enabled")

    target_sources(netdata PRIVATE ${MONGODB_EXPORTING_FILES})
    list(APPEND NETDATA_COMMON_LIBRARIES ${MONGOC_LIBRARIES})
    list(APPEND NETDATA_COMMON_INCLUDE_DIRS ${MONGOC_INCLUDE_DIRS})
    list(APPEND NETDATA_COMMON_CFLAGS ${MONGOC_CFLAGS_OTHER})
else()
    message(STATUS "mongodb exporting: disabled (requires mongoc library)")
endif()

set(NETDATA_COMMON_LIBRARIES ${NETDATA_COMMON_LIBRARIES} m ${CMAKE_THREAD_LIBS_INIT})

find_package(Protobuf REQUIRED)

function(PROTOBUF_ACLK_GENERATE_CPP SRCS HDRS)
  if(NOT ARGN)
    message(SEND_ERROR "Error: PROTOBUF_ACLK_GENERATE_CPP() called without any proto files")
    return()
  endif()

  set(${SRCS})
  set(${HDRS})
  foreach(FIL ${ARGN})
    get_filename_component(ABS_FIL ${FIL} ABSOLUTE)
    get_filename_component(DIR ${ABS_FIL} DIRECTORY)
    get_filename_component(FIL_WE ${FIL} NAME_WE)
    set(GENERATED_PB_CC "${DIR}/${FIL_WE}.pb.cc")
    set(GENERATED_PB_H "${DIR}/${FIL_WE}.pb.h")
# cmake > 3.20 required :(
#    cmake_path(SET GENERATED_PB_CC "${DIR}")
#    cmake_path(SET GENERATED_PB_H "${DIR}")
#    cmake_path(APPEND GENERATED_PB_CC "${FIL_WE}.pb.cc")
#    cmake_path(APPEND GENERATED_PB_H "${FIL_WE}.pb.h")

    list(APPEND ${SRCS} ${GENERATED_PB_CC})
    list(APPEND ${HDRS} ${GENERATED_PB_H})
    add_custom_command(
      OUTPUT ${GENERATED_PB_CC}
             ${GENERATED_PB_H}
      COMMAND  ${PROTOBUF_PROTOC_EXECUTABLE}
      ARGS -I=${CMAKE_SOURCE_DIR}/aclk/aclk-schemas --cpp_out=${CMAKE_SOURCE_DIR}/aclk/aclk-schemas ${ABS_FIL}
      DEPENDS ${ABS_FIL} ${PROTOBUF_PROTOC_EXECUTABLE}
      COMMENT "Running C++ protocol buffer compiler on ${FIL}"
      VERBATIM )
  endforeach()
  set_source_files_properties(${${SRCS}} ${${HDRS}} PROPERTIES GENERATED TRUE)
  set(${SRCS} ${${SRCS}} PARENT_SCOPE)
  set(${HDRS} ${${HDRS}} PARENT_SCOPE)
endfunction()

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
PROTOBUF_ACLK_GENERATE_CPP(ACLK_PROTO_BUILT_SRCS ACLK_PROTO_BUILT_HDRS ${ACLK_PROTO_DEFS})

list(APPEND NETDATA_COMMON_LIBRARIES ${PROTOBUF_LIBRARIES})
list(APPEND NETDATA_COMMON_INCLUDE_DIRS ${PROTOBUF_INCLUDE_DIRS})
list(APPEND NETDATA_COMMON_CFLAGS ${PROTOBUF_CFLAGS_OTHER})

target_sources(netdata PRIVATE ${ACLK_ALWAYS_BUILD})
target_sources(netdata PRIVATE  ${ACLK_FILES} ${ACLK_PROTO_BUILT_SRCS} ${ACLK_PROTO_BUILT_HDRS})

include_directories(BEFORE ${CMAKE_SOURCE_DIR}/aclk/aclk-schemas)
include_directories(BEFORE ${CMAKE_SOURCE_DIR}/mqtt_websockets/MQTT-C/include)
include_directories(BEFORE ${CMAKE_SOURCE_DIR}/mqtt_websockets/src/include)
include_directories(BEFORE ${CMAKE_SOURCE_DIR}/mqtt_websockets/c-rbuf/include)

set(GIT_EXECUTABLE "git")
execute_process(COMMAND ${GIT_EXECUTABLE} describe
                WORKING_DIRECTORY ${PROJECT_SOURCE_DIR}
                OUTPUT_VARIABLE NETDATA_VERSION
                OUTPUT_STRIP_TRAILING_WHITESPACE)

if("${NETDATA_VERSION}" STREQUAL "")
    file(STRINGS "packaging/version" NETDATA_VERSION LIMIT_COUNT 1)
endif()

# -----------------------------------------------------------------------------
# netdata

target_compile_options(netdata PRIVATE ${NETDATA_COMMON_CFLAGS})
target_include_directories(netdata PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS})
target_link_libraries(netdata PRIVATE libnetdata ${NETDATA_COMMON_LIBRARIES})

if(LINUX)
    target_link_libraries(netdata PRIVATE rt)
elseif(MACOS)
    target_link_libraries(netdata PRIVATE ${IOKIT} ${FOUNDATION})
endif()

# -----------------------------------------------------------------------------
# netdatacli

add_executable(netdatacli ${NETDATACLI_FILES})
target_link_libraries (netdatacli libnetdata ${NETDATA_COMMON_LIBRARIES})
target_include_directories(netdatacli PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS})
target_compile_options(netdatacli PRIVATE ${NETDATA_COMMON_CFLAGS})

# -----------------------------------------------------------------------------
# ebpf_process.plugin

if(ENABLE_PLUGIN_EBPF)
    message(STATUS "ebpf.plugin: enabled")
    add_executable(ebpf.plugin ${EBPF_PROCESS_PLUGIN_FILES})
    target_link_libraries (ebpf.plugin libnetdata ${NETDATA_COMMON_LIBRARIES})
    target_include_directories(ebpf.plugin PUBLIC ${NETDATA_COMMON_INCLUDE_DIRS})
    target_compile_options(ebpf.plugin PUBLIC ${NETDATA_COMMON_CFLAGS})
else()
    message(STATUS "ebpf.plugin: disabled")
endif()

# -----------------------------------------------------------------------------
# Unit tests

if(UNIT_TESTING)
    message(STATUS "Looking for CMocka which is required for unit testing")
    find_package(CMocka REQUIRED)
    include(CTest)

if(BUILD_TESTING)
    add_executable(str2ld_testdriver libnetdata/tests/test_str2ld.c)
    target_link_libraries(str2ld_testdriver libnetdata ${NETDATA_COMMON_LIBRARIES} ${CMOCKA_LIBRARIES})
    add_test(NAME test_str2ld COMMAND str2ld_testdriver)

    add_executable(storage_number_testdriver libnetdata/storage_number/tests/test_storage_number.c)
    target_link_libraries(storage_number_testdriver libnetdata ${NETDATA_COMMON_LIBRARIES} ${CMOCKA_LIBRARIES})
    add_test(NAME test_storage_number COMMAND storage_number_testdriver)

    set(EXPORTING_ENGINE_TEST_FILES
        exporting/tests/test_exporting_engine.c
        exporting/tests/test_exporting_engine.h
        exporting/tests/exporting_fixtures.c
        exporting/tests/exporting_doubles.c
        exporting/tests/netdata_doubles.c
        exporting/tests/system_doubles.c
        database/rrdlabels.c
        database/rrdvar.c
        )
    set(TEST_NAME exporting_engine)
    set(PROMETHEUS_REMOTE_WRITE_LINK_OPTIONS)
    set(KINESIS_LINK_OPTIONS)
    set(PUBSUB_LINK_OPTIONS)
    set(MONGODB_LINK_OPTIONS)
if(ENABLE_EXPORTING_PROMETHEUS_REMOTE_WRITE)
    list(APPEND EXPORTING_ENGINE_FILES ${PROMETHEUS_REMOTE_WRITE_EXPORTING_FILES} ${PROTO_SRCS} ${PROTO_HDRS})
    list(
        APPEND PROMETHEUS_REMOTE_WRITE_LINK_OPTIONS
        -Wl,--wrap=init_write_request
        -Wl,--wrap=add_host_info
        -Wl,--wrap=add_label
        -Wl,--wrap=add_metric
    )
endif()
if(ENABLE_EXPORTING_KINESIS)
    list(APPEND EXPORTING_ENGINE_FILES ${KINESIS_EXPORTING_FILES})
    list(
        APPEND KINESIS_LINK_OPTIONS
        -Wl,--wrap=aws_sdk_init
        -Wl,--wrap=kinesis_init
        -Wl,--wrap=kinesis_put_record
        -Wl,--wrap=kinesis_get_result
    )
endif()
if(ENABLE_EXPORTING_PUBSUB)
    list(APPEND EXPORTING_ENGINE_FILES ${PUBSUB_EXPORTING_FILES})
    list(
        APPEND PUBSUB_LINK_OPTIONS
        -Wl,--wrap=pubsub_init
        -Wl,--wrap=pubsub_add_message
        -Wl,--wrap=pubsub_publish
        -Wl,--wrap=pubsub_get_result
    )
endif()
if(MONGOC_LIBRARIES)
    list(APPEND EXPORTING_ENGINE_FILES ${MONGODB_EXPORTING_FILES})
    list(
        APPEND MONGODB_LINK_OPTIONS
        -Wl,--wrap=mongoc_init
        -Wl,--wrap=mongoc_uri_new_with_error
        -Wl,--wrap=mongoc_uri_get_option_as_int32
        -Wl,--wrap=mongoc_uri_set_option_as_int32
        -Wl,--wrap=mongoc_client_new_from_uri
        -Wl,--wrap=mongoc_client_set_appname
        -Wl,--wrap=mongoc_client_get_collection
        -Wl,--wrap=mongoc_uri_destroy
        -Wl,--wrap=mongoc_collection_insert_many
    )
endif()
    add_executable(${TEST_NAME}_testdriver ${EXPORTING_ENGINE_TEST_FILES} ${EXPORTING_ENGINE_FILES})
    target_compile_options(
        ${TEST_NAME}_testdriver
        PRIVATE
        -DUNIT_TESTING
    )
    target_include_directories(
        ${TEST_NAME}_testdriver
        PUBLIC
        ${CMAKE_CURRENT_BINARY_DIR}
        ${NETDATA_COMMON_INCLUDE_DIRS}
    )
    target_link_options(
        ${TEST_NAME}_testdriver
        PRIVATE
        -Wl,--wrap=read_exporting_config
        -Wl,--wrap=init_connectors
        -Wl,--wrap=mark_scheduled_instances
        -Wl,--wrap=rrdhost_is_exportable
        -Wl,--wrap=rrdset_is_exportable
        -Wl,--wrap=exporting_calculate_value_from_stored_data
        -Wl,--wrap=prepare_buffers
        -Wl,--wrap=send_internal_metrics
        -Wl,--wrap=now_realtime_sec
        -Wl,--wrap=uv_thread_set_name_np
        -Wl,--wrap=uv_thread_create
        -Wl,--wrap=uv_mutex_lock
        -Wl,--wrap=uv_mutex_unlock
        -Wl,--wrap=uv_cond_signal
        -Wl,--wrap=uv_cond_wait
        -Wl,--wrap=strdupz
        -Wl,--wrap=info_int
        -Wl,--wrap=recv
        -Wl,--wrap=send
        -Wl,--wrap=connect_to_one_of
        -Wl,--wrap=create_main_rusage_chart
        -Wl,--wrap=send_main_rusage
        -Wl,--wrap=simple_connector_end_batch
        ${PROMETHEUS_REMOTE_WRITE_LINK_OPTIONS}
        ${KINESIS_LINK_OPTIONS}
        ${PUBSUB_LINK_OPTIONS}
        ${MONGODB_LINK_OPTIONS}
    )
    target_link_libraries(${TEST_NAME}_testdriver libnetdata ${NETDATA_COMMON_LIBRARIES} ${CMOCKA_LIBRARIES})
    add_test(NAME test_${TEST_NAME} COMMAND ${TEST_NAME}_testdriver)

    set(WEB_API_TEST_FILES
        web/api/tests/web_api.c
        web/server/web_client.c
        )
    add_executable(web_api_testdriver ${WEB_API_TEST_FILES})
    target_link_options(
        web_api_testdriver
        PRIVATE
        -Wl,--wrap=rrdhost_find_by_hostname
        -Wl,--wrap=finished_web_request_statistics
        -Wl,--wrap=config_get
        -Wl,--wrap=web_client_api_request_v1
        -Wl,--wrap=rrdhost_find_by_guid
        -Wl,--wrap=rrdset_find_byname
        -Wl,--wrap=sql_create_host_by_uuid
        -Wl,--wrap=rrdset_find
        -Wl,--wrap=rrdpush_receiver_thread_spawn
        -Wl,--wrap=debug_int
        -Wl,--wrap=error_int
        -Wl,--wrap=info_int
        -Wl,--wrap=fatal_int
    )
    target_link_libraries(web_api_testdriver libnetdata ${NETDATA_COMMON_LIBRARIES} ${CMOCKA_LIBRARIES})
#    add_test(NAME test_web_api COMMAND web_api_testdriver)

    set(VALID_URLS_TEST_FILES
        web/api/tests/valid_urls.c
        web/server/web_client.c
        )
    add_executable(valid_urls_testdriver ${VALID_URLS_TEST_FILES})
    target_link_options(
        valid_urls_testdriver
        PRIVATE
        -Wl,--wrap=rrdhost_find_by_hostname
        -Wl,--wrap=finished_web_request_statistics
        -Wl,--wrap=config_get
        -Wl,--wrap=web_client_api_request_v1
        -Wl,--wrap=rrdhost_find_by_guid
        -Wl,--wrap=rrdset_find_byname
        -Wl,--wrap=sql_create_host_by_uuid
        -Wl,--wrap=rrdset_find
        -Wl,--wrap=rrdpush_receiver_thread_spawn
        -Wl,--wrap=debug_int
        -Wl,--wrap=error_int
        -Wl,--wrap=info_int
        -Wl,--wrap=fatal_int
        -Wl,--wrap=mysendfile
        -DREMOVE_MYSENDFILE
    )
    target_link_libraries(valid_urls_testdriver libnetdata ${NETDATA_COMMON_LIBRARIES} ${CMOCKA_LIBRARIES})
#    add_test(NAME test_valid_urls COMMAND valid_urls_testdriver)

    set(CGROUPS_TEST_FILES
        collectors/cgroups.plugin/sys_fs_cgroup.c
        collectors/cgroups.plugin/sys_fs_cgroup.h
        collectors/cgroups.plugin/tests/test_cgroups_plugin.c
        collectors/cgroups.plugin/tests/test_cgroups_plugin.h
        collectors/cgroups.plugin/tests/test_doubles.c
        database/rrdlabels.c
        database/rrd.h
    )
    add_executable(cgroups_testdriver ${CGROUPS_TEST_FILES})
    target_link_libraries(cgroups_testdriver libnetdata ${NETDATA_COMMON_LIBRARIES} ${CMOCKA_LIBRARIES})
    add_test(NAME test_cgroups COMMAND cgroups_testdriver)

    set_target_properties(
        str2ld_testdriver
        storage_number_testdriver
        exporting_engine_testdriver
        web_api_testdriver
        valid_urls_testdriver
        cgroups_testdriver
        PROPERTIES RUNTIME_OUTPUT_DIRECTORY tests
    )

endif()
endif()

set(ENABLE_ML True)

add_subdirectory(libnetdata)
add_subdirectory(claim)
add_subdirectory(collectors)
add_subdirectory(daemon)
add_subdirectory(database)
add_subdirectory(health)
add_subdirectory(ml)
add_subdirectory(parser)
add_subdirectory(registry)
add_subdirectory(spawn)
add_subdirectory(streaming)
add_subdirectory(web)

configure_file(
    libnetdata/config.h.cmake
    ${NETDATA_INCLUDE_DIR}/config.h
)

install(TARGETS netdata DESTINATION bin)
