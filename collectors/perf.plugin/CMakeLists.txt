# SPDX-License-Identifier: GPL-3.0-or-later

if (FREEBSD OR MAC_OS)
    message(STATUS "perf.plugin: disabled")
    return()
endif()

message(STATUS "perf.plugin: enabled")

set(PERF_PLUGIN_FILES perf_plugin.c)

add_executable(perf.plugin ${PERF_PLUGIN_FILES})

target_compile_options(perf.plugin PRIVATE ${NETDATA_COMMON_CFLAGS})
target_include_directories(perf.plugin PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS})
target_link_libraries(perf.plugin libnetdata ${NETDATA_COMMON_LIBRARIES})

install(TARGETS perf.plugin DESTINATION ${NETDATA_PLUGINS_DIR})
