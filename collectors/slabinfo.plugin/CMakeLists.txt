# SPDX-License-Identifier: GPL-3.0-or-later

if (FREEBSD OR MAC_OS)
    message(STATUS "slabinfo.plugin: disabled")
    return()
endif()

message(STATUS "slabinfo.plugin: enabled")
set(ENABLE_SLABINFO True)
set(SLABINFO_PLUGIN_FILES slabinfo.c)

add_executable(slabinfo.plugin ${SLABINFO_PLUGIN_FILES})

target_compile_options(slabinfo.plugin PRIVATE ${NETDATA_COMMON_CFLAGS})
target_include_directories(slabinfo.plugin PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS})
target_link_libraries(slabinfo.plugin libnetdata ${NETDATA_COMMON_LIBRARIES})

install(TARGETS slabinfo.plugin DESTINATION ${NETDATA_PLUGINS_DIR})
