# SPDX-License-Identifier: GPL-3.0-or-later

set(HEALTH_PLUGIN_FILES health.c
                        health.h
                        health_config.c
                        health_json.c
                        health_log.c)

target_sources(netdata PRIVATE ${HEALTH_PLUGIN_FILES})
