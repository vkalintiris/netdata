# SPDX-License-Identifier: GPL-3.0-or-later

set(WEB_PLUGIN_FILES web_client.c
                     web_client.h
                     web_server.c
                     web_server.h
                     static/static-threaded.c
                     static/static-threaded.h
                     web_client_cache.c
                     web_client_cache.h)

target_sources(netdata PRIVATE ${WEB_PLUGIN_FILES})
