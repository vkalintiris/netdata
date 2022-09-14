# SPDX-License-Identifier: GPL-3.0-or-later

if (NOT LINUX)
    message(STATUS "freeipmi.plugin: disabled")
    return()
endif()

if (NOT IPMI_FOUND)
    message(STATUS "freeipmi.plugin: disabled (requires libipmimonitoring)")
    return()
endif()

message(STATUS "freeipmi.plugin: enabled")

set(FREEIPMI_PLUGIN_FILES freeipmi_plugin.c)
add_executable(freeipmi.plugin ${FREEIPMI_PLUGIN_FILES})

target_compile_options(freeipmi.plugin PRIVATE ${NETDATA_COMMON_CFLAGS} ${IPMI_CFLAGS_OTHER})
target_include_directories(freeipmi.plugin PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS} ${IPMI_INCLUDE_DIRS})
target_link_libraries(freeipmi.plugin libnetdata ${NETDATA_COMMON_LIBRARIES} ${IPMI_LIBRARIES})

install(TARGETS freeipmi.plugin DESTINATION ${NETDATA_PLUGINS_DIR})
