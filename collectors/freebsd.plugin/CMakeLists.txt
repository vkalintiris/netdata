# SPDX-License-Identifier: GPL-3.0-or-later

if(NOT FREEBSD)
    return()
endif()

set(FREEBSD_PLUGIN_FILES plugin_freebsd.c
                         plugin_freebsd.h
                         freebsd_sysctl.c
                         freebsd_getmntinfo.c
                         freebsd_getifaddrs.c
                         freebsd_devstat.c
                         freebsd_kstat_zfs.c
                         freebsd_ipfw.c)

target_sources(netdata PRIVATE ${FREEBSD_PLUGIN_FILES})
