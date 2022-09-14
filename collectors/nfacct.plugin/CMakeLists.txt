# SPDX-License-Identifier: GPL-3.0-or-later

if (NOT LINUX)
    message(STATUS "nfacct.plugin: disabled")
    return()
endif()

if (NOT MNL_FOUND)
    message(STATUS "nfacct.plugin: disabled (requires libmnl)")
    return()
endif()

if (NOT NFACCT_FOUND)
    message(STATUS "nfacct.plugin: disabled (requires libnetfilter_acct)")
    return()
endif()

message(STATUS "nfacct.plugin: enabled")

set(NFACCT_PLUGIN_FILES plugin_nfacct.c)
add_executable(nfacct.plugin ${NFACCT_PLUGIN_FILES})

target_compile_options(nfacct.plugin PRIVATE ${NETDATA_COMMON_CFLAGS}
                       ${MNL_CFLAGS_OTHER} ${NFACCT_CFLAGS_OTHER})
target_include_directories(nfacct.plugin PRIVATE ${NETDATA_COMMON_INCLUDE_DIRS}
                           ${MNL_INCLUDE_DIRS} ${NFACCT_INCLUDE_DIRS})
target_link_libraries(nfacct.plugin libnetdata ${NETDATA_COMMON_LIBRARIES}
                      ${MNL_LIBRARIES} ${NFACCT_LIBRARIES})

install(TARGETS nfacct.plugin DESTINATION ${NETDATA_PLUGINS_DIR})
