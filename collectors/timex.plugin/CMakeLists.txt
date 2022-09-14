# SPDX-License-Identifier: GPL-3.0-or-later

if (NOT LINUX)
    message(STATUS "timex.plugin: disabled")
    return()
endif()

message(STATUS "timex.plugin: enabled")

set(TIMEX_PLUGIN_FILES plugin_timex.c)
target_sources(netdata PRIVATE ${TIMEX_PLUGIN_FILES})
