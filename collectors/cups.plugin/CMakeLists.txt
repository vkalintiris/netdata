# SPDX-License-Identifier: GPL-3.0-or-later

if (NOT CUPS_FOUND)
    message(STATUS "cups.plugin: disabled (requires cups library)")
    return()
endif()

message(STATUS "cups.plugin: enabled")

set(CUPS_PLUGIN_FILES cups_plugin.c)
add_executable(cups.plugin ${CUPS_PLUGIN_FILES})

target_compile_options(cups.plugin PRIVATE ${NETDATA_COMMON_CFLAGS} ${CUPS_CFLAGS_OTHER})
target_include_directories(cups.plugin PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS} ${CUPS_INCLUDE_DIRS})
target_link_libraries(cups.plugin libnetdata ${NETDATA_COMMON_LIBRARIES} ${CUPS_LIBRARIES})

install(TARGETS cups.plugin DESTINATION ${NETDATA_PLUGINS_DIR})
