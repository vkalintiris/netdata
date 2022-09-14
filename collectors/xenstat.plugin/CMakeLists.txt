# SPDX-License-Identifier: GPL-3.0-or-later

if (NOT LINUX)
    message(STATUS "xenstat.plugin: disabled")
    return()
endif()

if (NOT XENSTAT_FOUND)
    message(STATUS "xenstat.plugin: disabled (requires libxenstat)")
    return()
endif()

message(STATUS "xenstat.plugin: enabled")

set(XENSTAT_PLUGIN_FILES xenstat_plugin.c)
add_executable(xenstat.plugin ${XENSTAT_PLUGIN_FILES})

target_compile_options(xenstat.plugin PRIVATE ${NETDATA_COMMON_CFLAGS} ${XENSTAT_CFLAGS_OTHER})
target_include_directories(xenstat.plugin PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS} ${XENSTAT_INCLUDE_DIRS})
target_link_libraries(xenstat.plugin libnetdata ${NETDATA_COMMON_LIBRARIES} ${XENSTAT_LIBRARIES})

install(TARGETS xenstat.plugin DESTINATION ${NETDATA_PLUGINS_DIR})
