# SPDX-License-Identifier: GPL-3.0-or-later

message(STATUS "checks.plugin: enabled")

set(CHECKS_PLUGIN_FILES plugin_checks.c)
target_sources(netdata PRIVATE ${CHECKS_PLUGIN_FILES})
