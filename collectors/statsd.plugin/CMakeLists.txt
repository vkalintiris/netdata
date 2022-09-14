# SPDX-License-Identifier: GPL-3.0-or-later

if(NOT LINUX)
    message(STATUS "statsd.plugin: disabled")
    return()
endif()

message(STATUS "statd.plugin: enabled")

set(STATSD_PLUGIN_FILES statsd.c)
target_sources(netdata PRIVATE ${STATSD_PLUGIN_FILES})
