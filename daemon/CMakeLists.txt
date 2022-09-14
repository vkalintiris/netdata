# SPDX-License-Identifier: GPL-3.0-or-later

set(DAEMON_FILES buildinfo.c
                 buildinfo.h
                 common.c
                 common.h
                 daemon.c
                 daemon.h
                 global_statistics.c
                 global_statistics.h
                 analytics.c
                 analytics.h
                 main.c
                 main.h
                 signals.c
                 signals.h
                 service.c
                 static_threads.c
                 static_threads.h
                 commands.c
                 commands.h
                 unit_test.c
                 unit_test.h)

if(LINUX)
    list(APPEND DAEMON_FILES static_threads_linux.c)
elseif(MAC_OS)
    list(APPEND DAEMON_FILES static_threads_macos.c)
elseif(FREEBSD)
    list(APPEND DAEMON_FILES static_threads_freebsd.c)
endif()

target_sources(netdata PRIVATE ${DAEMON_FILES})
