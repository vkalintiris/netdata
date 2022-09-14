# SPDX-License-Identifier: GPL-3.0-or-later

set(STREAMING_PLUGIN_FILES rrdpush.c
                           rrdpush.h
                           compression.c
                           receiver.c
                           sender.c)

target_sources(netdata PRIVATE ${STREAMING_PLUGIN_FILES})
