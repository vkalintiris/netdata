# SPDX-License-Identifier: GPL-3.0-or-later

if(NOT MAC_OS)
    return()
endif()

set(MACOS_PLUGIN_FILES plugin_macos.c
                       plugin_macos.h
                       macos_sysctl.c
                       macos_mach_smi.c
                       macos_fw.c)

target_sources(netdata PRIVATE MACOS_PLUGIN_FILES)
