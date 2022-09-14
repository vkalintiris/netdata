# SPDX-License-Identifier: GPL-3.0-or-later

if(NOT LINUX)
    message(STATUS "diskspace.plugin: disabled")
    return()
endif()

message(STATUS "diskspace.plugin: enabled")

set(DISKSPACE_PLUGIN_FILES plugin_diskspace.c)
target_sources(netdata PRIVATE ${DISKSPACE_PLUGIN_FILES})
