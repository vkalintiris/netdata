# SPDX-License-Identifier: GPL-3.0-or-later

set(PLUGINSD_PLUGIN_FILES plugins_d.c
                          plugins_d.h
                          pluginsd_parser.c
                          pluginsd_parser.h)

target_sources(netdata PRIVATE ${PLUGINSD_PLUGIN_FILES})
