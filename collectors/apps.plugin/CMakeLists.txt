# SPDX-License-Identifier: GPL-3.0-or-later
# This file is only used for development (netdata in Clion)
# It can build netdata, but you are on your own...

if (MAC_OS)
    message(STATUS "apps.plugin: disabled")
    return()
endif()

message(STATUS "apps.plugin: enabled")
set(APPS_PLUGIN_FILES apps_plugin.c)

add_executable(apps.plugin ${APPS_PLUGIN_FILES})

target_compile_options(apps.plugin PRIVATE ${NETDATA_COMMON_CFLAGS} ${CAP_CFLAGS_OTHER})
target_include_directories(apps.plugin PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS} ${CAP_INCLUDE_DIRS})
target_link_libraries(apps.plugin libnetdata ${NETDATA_COMMON_LIBRARIES} ${CAP_LIBRARIES})

install(TARGETS apps.plugin DESTINATION ${NETDATA_PLUGINS_DIR})
install(FILES apps_groups.conf DESTINATION ${NETDATA_LIBCONFIG_DIR})
