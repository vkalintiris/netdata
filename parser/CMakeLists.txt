# SPDX-License-Identifier: GPL-3.0-or-later

set(PARSER_PLUGIN_FILES parser.c parser.h)
target_sources(netdata PRIVATE ${PARSER_PLUGIN_FILES})
