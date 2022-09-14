# SPDX-License-Identifier: GPL-3.0-or-later

if(NOT LINUX)
    message(STATUS "tc.plugin: disabled")
    return()
endif()

message(STATUS "tc.plugin: enabled")

set(TC_PLUGIN_FILES plugin_tc.c)
target_sources(netdata PRIVATE ${TC_PLUGIN_FILES})
