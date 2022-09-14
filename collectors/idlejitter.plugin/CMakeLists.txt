# SPDX-License-Identifier: GPL-3.0-or-later

message(STATUS "idlejitter.plugin: enabled")

set(IDLEJITTER_PLUGIN_FILES plugin_idlejitter.c)
target_sources(netdata PRIVATE ${IDLEJITTER_PLUGIN_FILES})
