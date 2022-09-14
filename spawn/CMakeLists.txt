# SPDX-License-Identifier: GPL-3.0-or-later

set(SPAWN_PLUGIN_FILES spawn.c
                       spawn.h
                       spawn_server.c
                       spawn_client.c)

target_sources(netdata PRIVATE ${SPAWN_PLUGIN_FILES})
