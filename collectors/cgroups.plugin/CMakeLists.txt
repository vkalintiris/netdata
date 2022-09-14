# SPDX-License-Identifier: GPL-3.0-or-later

if (NOT LINUX)
    message(STATUS "cgroup-network: disabled")
    return()
endif()

message(STATUS "cgroup-network: enabled")

set(CGROUP_NETWORK_FILES cgroup-network.c)
add_executable(cgroup-network ${CGROUP_NETWORK_FILES})

target_compile_options(cgroup-network PRIVATE ${NETDATA_COMMON_CFLAGS})
target_include_directories(cgroup-network PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS})
target_link_libraries(cgroup-network libnetdata ${NETDATA_COMMON_LIBRARIES})

install(TARGETS cgroup-network DESTINATION ${NETDATA_PLUGINS_DIR})
install(PROGRAMS cgroup-network-helper.sh DESTINATION ${NETDATA_PLUGINS_DIR})
