load("//bazel/build_settings:defs.bzl", "if_dbengine", "if_streaming_compression", "if_https", "if_aclk")

#
# aclk/
#

ACLK_ALWAYS_BUILD_HEADERS = [
    "aclk/aclk_rrdhost_state.h",
    "aclk/aclk_api.h",
    "aclk/aclk_proxy.h",
]

ACLK_ALWAYS_BUILD_SOURCES = [
    "aclk/aclk_api.c",
    "aclk/aclk_proxy.c",
]

ACLK_COMMON_HEADERS = [
    "aclk/aclk_collector_list.h",
]

ACLK_COMMON_SOURCES = [
    "aclk/aclk_collector_list.c",
]

ACLK_HEADERS = [
    "aclk/aclk.h",
    "aclk/aclk_util.h",
    "aclk/aclk_stats.h",
    "aclk/aclk_query.h",
    "aclk/aclk_query_queue.h",
    "aclk/aclk_otp.h",
    "aclk/aclk_tx_msgs.h",
    "aclk/aclk_rx_msgs.h",
    "aclk/https_client.h",
    "mqtt_websockets/src/include/mqtt_pal.h",
    "mqtt_websockets/src/include/mqtt_wss_client.h",
    "mqtt_websockets/src/include/mqtt_wss_log.h",
    "mqtt_websockets/src/include/ws_client.h",
    "mqtt_websockets/src/include/common_internal.h",
    "mqtt_websockets/src/include/endian_compat.h",
    "mqtt_websockets/c-rbuf/include/ringbuffer.h",
    "mqtt_websockets/c-rbuf/src/ringbuffer_internal.h",
    "mqtt_websockets/MQTT-C/include/mqtt.h",
    "mqtt_websockets/MQTT-C/include/mqtt_pal.h",
]

ACLK_SOURCES = [
    "aclk/aclk.c",
    "aclk/aclk_util.c",
    "aclk/aclk_stats.c",
    "aclk/aclk_query.c",
    "aclk/aclk_query_queue.c",
    "aclk/aclk_otp.c",
    "aclk/aclk_tx_msgs.c",
    "aclk/aclk_rx_msgs.c",
    "aclk/https_client.c",
    "mqtt_websockets/src/mqtt_wss_client.c",
    "mqtt_websockets/src/mqtt_wss_log.c",
    "mqtt_websockets/src/ws_client.c",
    "mqtt_websockets/c-rbuf/src/ringbuffer.c",
    "mqtt_websockets/MQTT-C/src/mqtt.c",
]

ACLK_NEW_CLOUD_PROTOCOL = [
    "aclk/aclk_charts_api.c",
    "aclk/aclk_charts_api.h",
    "aclk/aclk_alarm_api.c",
    "aclk/aclk_alarm_api.h",
    "aclk/schema-wrappers/connection.cc",
    "aclk/schema-wrappers/connection.h",
    "aclk/schema-wrappers/node_connection.cc",
    "aclk/schema-wrappers/node_connection.h",
    "aclk/schema-wrappers/node_creation.cc",
    "aclk/schema-wrappers/node_creation.h",
    "aclk/schema-wrappers/chart_stream.cc",
    # Required by default
    # "aclk/schema-wrappers/chart_stream.h",
    "aclk/schema-wrappers/chart_config.cc",
    "aclk/schema-wrappers/chart_config.h",
    "aclk/schema-wrappers/alarm_stream.cc",
    "aclk/schema-wrappers/alarm_stream.h",
    "aclk/schema-wrappers/alarm_config.cc",
    "aclk/schema-wrappers/alarm_config.h",
    "aclk/schema-wrappers/node_info.cc",
    "aclk/schema-wrappers/node_info.h",
    "aclk/schema-wrappers/schema_wrappers.h",
    "aclk/schema-wrappers/schema_wrapper_utils.cc",
    "aclk/schema-wrappers/schema_wrapper_utils.h",
]

#
# backends/
#

BACKENDS_HEADERS = [
    "backends/backends.h",
    "backends/graphite/graphite.h",
    "backends/json/json.h",
    "backends/opentsdb/opentsdb.h",
    "backends/prometheus/backend_prometheus.h",
]

BACKENDS_SOURCES = [
    "backends/backends.c",
    "backends/graphite/graphite.c",
    "backends/json/json.c",
    "backends/opentsdb/opentsdb.c",
    "backends/prometheus/backend_prometheus.c",
]

#
# claim/
#

CLAIM_HEADERS = [
    "claim/claim.h",
]

CLAIM_SOURCES = [
    "claim/claim.c",
]

#
# database/
#

DATABASE_HEADERS = [
    "database/rrdcalc.h",
    "database/rrdcalctemplate.h",
    "database/rrddimvar.h",
    "database/rrd.h",
    "database/rrdsetvar.h",
    "database/rrdvar.h",
    "database/sqlite/sqlite_functions.h",
    "database/sqlite/sqlite_aclk.h",
    "database/sqlite/sqlite_health.h",
    "database/sqlite/sqlite_aclk_node.h",
    "database/sqlite/sqlite_aclk_chart.h",
    "database/sqlite/sqlite_aclk_alert.h",
    "database/sqlite/sqlite3.h",
] + if_dbengine([
    "database/engine/rrdengine.h",
    "database/engine/rrddiskprotocol.h",
    "database/engine/datafile.h",
    "database/engine/journalfile.h",
    "database/engine/rrdenginelib.h",
    "database/engine/rrdengineapi.h",
    "database/engine/pagecache.h",
    "database/engine/rrdenglocking.h",
    "database/engine/metadata_log/metadatalog.h",
    "database/engine/metadata_log/metadatalogapi.h",
    "database/engine/metadata_log/logfile.h",
    "database/engine/metadata_log/metadatalogprotocol.h",
    "database/engine/metadata_log/metalogpluginsd.h",
    "database/engine/metadata_log/compaction.h",
])

DATABASE_SOURCES = [
    "database/rrdcalc.c",
    "database/rrdcalctemplate.c",
    "database/rrddim.c",
    "database/rrddimvar.c",
    "database/rrdfamily.c",
    "database/rrdhost.c",
    "database/rrdlabels.c",
    "database/rrd.c",
    "database/rrdset.c",
    "database/rrdsetvar.c",
    "database/rrdvar.c",
    "database/sqlite/sqlite_functions.c",
    "database/sqlite/sqlite_aclk.c",
    "database/sqlite/sqlite_health.c",
    "database/sqlite/sqlite_aclk_node.c",
    "database/sqlite/sqlite_aclk_chart.c",
    "database/sqlite/sqlite_aclk_alert.c",
    "database/sqlite/sqlite3.c",
] + if_dbengine([
    "database/engine/rrdengine.c",
    "database/engine/datafile.c",
    "database/engine/journalfile.c",
    "database/engine/rrdenginelib.c",
    "database/engine/rrdengineapi.c",
    "database/engine/pagecache.c",
    "database/engine/rrdenglocking.c",
    "database/engine/metadata_log/metadatalogapi.c",
    "database/engine/metadata_log/logfile.c",
    "database/engine/metadata_log/metalogpluginsd.c",
    "database/engine/metadata_log/compaction.c",
])

#
# exporting/
#

EXPORTING_HEADERS = [
    "exporting/exporting_engine.h",
    "exporting/graphite/graphite.h",
    "exporting/json/json.h",
    "exporting/opentsdb/opentsdb.h",
    "exporting/prometheus/prometheus.h",
]

EXPORTING_SOURCES = [
    "exporting/exporting_engine.c",
    "exporting/graphite/graphite.c",
    "exporting/json/json.c",
    "exporting/opentsdb/opentsdb.c",
    "exporting/prometheus/prometheus.c",
    "exporting/read_config.c",
    "exporting/clean_connectors.c",
    "exporting/init_connectors.c",
    "exporting/process_data.c",
    "exporting/check_filters.c",
    "exporting/send_data.c",
    "exporting/send_internal_metrics.c",
]

#
# libnetdata/
#

LIBNETDATA_HEADERS = [
    "libnetdata/adaptive_resortable_list/adaptive_resortable_list.h",
    "libnetdata/avl/avl.h",
    "libnetdata/buffer/buffer.h",
    "libnetdata/circular_buffer/circular_buffer.h",
    "libnetdata/clocks/clocks.h",
    "libnetdata/completion/completion.h",
    "libnetdata/config/appconfig.h",
    "libnetdata/dictionary/dictionary.h",
    #"libnetdata/ebpf/ebpf.h",
    "libnetdata/eval/eval.h",
    "libnetdata/health/health.h",
    "libnetdata/inlined.h",
    "libnetdata/json/jsmn.h",
    "libnetdata/json/json.h",
    "libnetdata/libnetdata.h",
    "libnetdata/locks/locks.h",
    "libnetdata/log/log.h",
    "libnetdata/os.h",
    "libnetdata/popen/popen.h",
    "libnetdata/procfile/procfile.h",
    "libnetdata/required_dummies.h",
    "libnetdata/simple_pattern/simple_pattern.h",
    "libnetdata/socket/security.h",
    "libnetdata/socket/socket.h",
    "libnetdata/statistical/statistical.h",
    "libnetdata/storage_number/storage_number.h",
    "libnetdata/string/utf8.h",
    "libnetdata/threads/threads.h",
    "libnetdata/url/url.h",
]

LIBNETDATA_SOURCES = [
    "libnetdata/adaptive_resortable_list/adaptive_resortable_list.c",
    "libnetdata/avl/avl.c",
    "libnetdata/buffer/buffer.c",
    "libnetdata/circular_buffer/circular_buffer.c",
    "libnetdata/completion/completion.c",
    "libnetdata/clocks/clocks.c",
    "libnetdata/config/appconfig.c",
    "libnetdata/dictionary/dictionary.c",
    #"libnetdata/ebpf/ebpf.c",
    "libnetdata/eval/eval.c",
    "libnetdata/health/health.c",
    "libnetdata/json/jsmn.c",
    "libnetdata/json/json.c",
    "libnetdata/libnetdata.c",
    "libnetdata/locks/locks.c",
    "libnetdata/log/log.c",
    "libnetdata/os.c",
    "libnetdata/popen/popen.c",
    "libnetdata/procfile/procfile.c",
    "libnetdata/simple_pattern/simple_pattern.c",
    "libnetdata/socket/security.c",
    "libnetdata/socket/socket.c",
    "libnetdata/statistical/statistical.c",
    "libnetdata/storage_number/storage_number.c",
    "libnetdata/threads/threads.c",
    "libnetdata/url/url.c",
]

#
# daemon/
#

DAEMON_HEADERS = [
    "daemon/buildinfo.h",
    "daemon/common.h",
    "daemon/daemon.h",
    "daemon/global_statistics.h",
    "daemon/analytics.h",
    "daemon/main.h",
    "daemon/signals.h",
    "daemon/static_threads.h",
    "daemon/commands.h",
    "daemon/unit_test.h",
]

DAEMON_SOURCES = [
    "daemon/buildinfo.c",
    "daemon/common.c",
    "daemon/daemon.c",
    "daemon/global_statistics.c",
    "daemon/analytics.c",
    "daemon/main.c",
    "daemon/signals.c",
    "daemon/static_threads.c",
    "daemon/static_threads_linux.c",
    "daemon/service.c",
    "daemon/commands.c",
    "daemon/unit_test.c",
]

#
# health/
#

HEALTH_HEADERS = [
    "health/health.h",
]

HEALTH_SOURCES = [
    "health/health.c",
    "health/health_config.c",
    "health/health_json.c",
    "health/health_log.c",
]

#
# ml/
#

ML_HEADERS = [
    "ml/ml.h",
]

ML_SOURCES = [
    "ml/ml-dummy.c",
]

#
# parser/
#

PARSER_HEADERS = [
    "parser/parser.h",
]

PARSER_SOURCES = [
    "parser/parser.c",
]

#
# plugin/cgroups.plugin/
#

PLUGIN_CGROUPS_HEADERS = [
    'collectors/cgroups.plugin/sys_fs_cgroup.h',
]

PLUGIN_CGROUPS_SOURCES = [
    'collectors/cgroups.plugin/sys_fs_cgroup.c',
]

#
# plugin/checks.plugin/
#

PLUGIN_CHECKS_SOURCES = [
    "collectors/checks.plugin/plugin_checks.c",
]

#
# plugin/diskspace.plugin/
#

PLUGIN_DISKSPACE_SOURCES = [
    'collectors/diskspace.plugin/plugin_diskspace.c',
]

#
# plugin/idlejitter.plugin/
#

PLUGIN_IDLEJITTER_SOURCES = [
    "collectors/idlejitter.plugin/plugin_idlejitter.c",
]

#
# plugin/plugins.d/
#

PLUGIN_PLUGINSD_HEADERS = [
    "collectors/plugins.d/plugins_d.h",
    "collectors/plugins.d/pluginsd_parser.h",
]

PLUGIN_PLUGINSD_SOURCES = [
    "collectors/plugins.d/plugins_d.c",
    "collectors/plugins.d/pluginsd_parser.c",
]

#
# plugin/proc.plugin/
#

PLUGIN_PROC_HEADERS = [
    "collectors/proc.plugin/plugin_proc.h",
    "collectors/proc.plugin/proc_pressure.h",
    "collectors/proc.plugin/proc_self_mountinfo.h",
    "collectors/proc.plugin/zfs_common.h",
]

PLUGIN_PROC_SOURCES = [
    "collectors/proc.plugin/ipc.c",
    "collectors/proc.plugin/plugin_proc.c",
    "collectors/proc.plugin/proc_diskstats.c",
    "collectors/proc.plugin/proc_mdstat.c",
    "collectors/proc.plugin/proc_interrupts.c",
    "collectors/proc.plugin/proc_softirqs.c",
    "collectors/proc.plugin/proc_loadavg.c",
    "collectors/proc.plugin/proc_meminfo.c",
    "collectors/proc.plugin/proc_pagetypeinfo.c",
    "collectors/proc.plugin/proc_pressure.c",
    "collectors/proc.plugin/proc_net_dev.c",
    "collectors/proc.plugin/proc_net_wireless.c",
    "collectors/proc.plugin/proc_net_ip_vs_stats.c",
    "collectors/proc.plugin/proc_net_netstat.c",
    "collectors/proc.plugin/proc_net_rpc_nfs.c",
    "collectors/proc.plugin/proc_net_rpc_nfsd.c",
    "collectors/proc.plugin/proc_net_snmp.c",
    "collectors/proc.plugin/proc_net_snmp6.c",
    "collectors/proc.plugin/proc_net_sctp_snmp.c",
    "collectors/proc.plugin/proc_net_sockstat.c",
    "collectors/proc.plugin/proc_net_sockstat6.c",
    "collectors/proc.plugin/proc_net_softnet_stat.c",
    "collectors/proc.plugin/proc_net_stat_conntrack.c",
    "collectors/proc.plugin/proc_net_stat_synproxy.c",
    "collectors/proc.plugin/proc_self_mountinfo.c",
    "collectors/proc.plugin/zfs_common.c",
    "collectors/proc.plugin/proc_spl_kstat_zfs.c",
    "collectors/proc.plugin/proc_stat.c",
    "collectors/proc.plugin/proc_sys_kernel_random_entropy_avail.c",
    "collectors/proc.plugin/proc_vmstat.c",
    "collectors/proc.plugin/proc_uptime.c",
    "collectors/proc.plugin/sys_kernel_mm_ksm.c",
    "collectors/proc.plugin/sys_block_zram.c",
    "collectors/proc.plugin/sys_devices_system_edac_mc.c",
    "collectors/proc.plugin/sys_devices_system_node.c",
    "collectors/proc.plugin/sys_fs_btrfs.c",
    "collectors/proc.plugin/sys_class_power_supply.c",
    "collectors/proc.plugin/sys_class_infiniband.c",

]

#
# plugin/statsd.plugin/
#

PLUGIN_STATSD_SOURCES = [
    "collectors/statsd.plugin/statsd.c",
]

#
# plugin/tc.plugin/
#

PLUGIN_TC_SOURCES = [
    'collectors/tc.plugin/plugin_tc.c',
]

#
# plugin/timex.plugin/
#

PLUGIN_TIMEX_SOURCES = [
    'collectors/timex.plugin/plugin_timex.c',
]

#
# registry/
#

REGISTRY_HEADERS = [
    "registry/registry.h",
    "registry/registry_internals.h",
    "registry/registry_machine.h",
    "registry/registry_person.h",
    "registry/registry_url.h",
]

REGISTRY_SOURCES = [
    "registry/registry.c",
    "registry/registry_db.c",
    "registry/registry_init.c",
    "registry/registry_internals.c",
    "registry/registry_log.c",
    "registry/registry_machine.c",
    "registry/registry_person.c",
    "registry/registry_url.c",
]

#
# spawn/
#

SPAWN_HEADERS = [
    "spawn/spawn.h",
]

SPAWN_SOURCES = [
    "spawn/spawn.c",
    "spawn/spawn_server.c",
    "spawn/spawn_client.c",
]

#
# streaming/
#

STREAMING_HEADERS = [
    "streaming/rrdpush.h",
]

STREAMING_SOURCES = [
    "streaming/rrdpush.c",
    "streaming/compression.c",
    "streaming/sender.c",
    "streaming/receiver.c",
]

#
# web/
#

WEB_HEADERS = [
    "web/api/badges/web_buffer_svg.h",
    "web/api/exporters/allmetrics.h",
    "web/api/exporters/shell/allmetrics_shell.h",
    "web/api/queries/average/average.h",
    "web/api/queries/des/des.h",
    "web/api/queries/incremental_sum/incremental_sum.h",
    "web/api/queries/max/max.h",
    "web/api/queries/median/median.h",
    "web/api/queries/min/min.h",
    "web/api/queries/query.h",
    "web/api/queries/rrdr.h",
    "web/api/queries/ses/ses.h",
    "web/api/queries/stddev/stddev.h",
    "web/api/queries/sum/sum.h",
    "web/api/formatters/rrd2json.h",
    "web/api/formatters/csv/csv.h",
    "web/api/formatters/json/json.h",
    "web/api/formatters/ssv/ssv.h",
    "web/api/formatters/value/value.h",
    "web/api/formatters/json_wrapper.h",
    "web/api/formatters/charts2json.h",
    "web/api/formatters/rrdset2json.h",
    "web/api/health/health_cmdapi.h",
    "web/api/web_api_v1.h",
    "web/server/web_client.h",
    "web/server/web_client_cache.h",
    "web/server/web_server.h",
    "web/server/static/static-threaded.h",
]

WEB_SOURCES = [
    "web/api/badges/web_buffer_svg.c",
    "web/api/exporters/allmetrics.c",
    "web/api/exporters/shell/allmetrics_shell.c",
    "web/api/queries/average/average.c",
    "web/api/queries/des/des.c",
    "web/api/queries/incremental_sum/incremental_sum.c",
    "web/api/queries/max/max.c",
    "web/api/queries/median/median.c",
    "web/api/queries/min/min.c",
    "web/api/queries/query.c",
    "web/api/queries/rrdr.c",
    "web/api/queries/ses/ses.c",
    "web/api/queries/stddev/stddev.c",
    "web/api/queries/sum/sum.c",
    "web/api/formatters/rrd2json.c",
    "web/api/formatters/csv/csv.c",
    "web/api/formatters/json/json.c",
    "web/api/formatters/ssv/ssv.c",
    "web/api/formatters/value/value.c",
    "web/api/formatters/json_wrapper.c",
    "web/api/formatters/charts2json.c",
    "web/api/formatters/rrdset2json.c",
    "web/api/health/health_cmdapi.c",
    "web/api/web_api_v1.c",
    "web/server/web_client.c",
    "web/server/web_client_cache.c",
    "web/server/web_server.c",
    "web/server/static/static-threaded.c",
]

#
# dummy
#

DUMMY_HEADERS = [
    "collectors/all.h",
    "collectors/freebsd.plugin/plugin_freebsd.h",
    "collectors/macos.plugin/plugin_macos.h",
    "aclk/schema-wrappers/chart_stream.h",
]

#
# proto
#

proto_library(
    name = "aclk_protos",
    srcs = [
        "aclk/aclk-schemas/proto/aclk/v1/lib.proto",
        "aclk/aclk-schemas/proto/alarm/v1/stream.proto",
        "aclk/aclk-schemas/proto/alarm/v1/config.proto",
        "aclk/aclk-schemas/proto/chart/v1/stream.proto",
        "aclk/aclk-schemas/proto/chart/v1/config.proto",
        "aclk/aclk-schemas/proto/chart/v1/instance.proto",
        "aclk/aclk-schemas/proto/chart/v1/dimension.proto",
        "aclk/aclk-schemas/proto/nodeinstance/info/v1/info.proto",
        "aclk/aclk-schemas/proto/nodeinstance/connection/v1/connection.proto",
        "aclk/aclk-schemas/proto/nodeinstance/create/v1/creation.proto",
        "aclk/aclk-schemas/proto/agent/v1/disconnect.proto",
        "aclk/aclk-schemas/proto/agent/v1/connection.proto",
    ],
    deps = [
        "@com_google_protobuf//:duration_proto",
        "@com_google_protobuf//:timestamp_proto",
    ],
    strip_import_prefix = "aclk/aclk-schemas",
)

cc_proto_library(
    name = "aclk_cc_protos",
    deps = [":aclk_protos"],
)

NETDATA_SOURCES = DUMMY_HEADERS

NETDATA_SOURCES += ACLK_ALWAYS_BUILD_HEADERS + ACLK_ALWAYS_BUILD_SOURCES
NETDATA_SOURCES += if_aclk(ACLK_COMMON_HEADERS + ACLK_COMMON_SOURCES + ACLK_HEADERS + ACLK_SOURCES + ACLK_NEW_CLOUD_PROTOCOL)

NETDATA_SOURCES += BACKENDS_HEADERS + BACKENDS_SOURCES
NETDATA_SOURCES += CLAIM_HEADERS + CLAIM_SOURCES
NETDATA_SOURCES += DATABASE_HEADERS + DATABASE_SOURCES
NETDATA_SOURCES += EXPORTING_HEADERS + EXPORTING_SOURCES
NETDATA_SOURCES += LIBNETDATA_HEADERS + LIBNETDATA_SOURCES
NETDATA_SOURCES += DAEMON_HEADERS + DAEMON_SOURCES
NETDATA_SOURCES += HEALTH_HEADERS + HEALTH_SOURCES
NETDATA_SOURCES += ML_HEADERS + ML_SOURCES
NETDATA_SOURCES += PARSER_HEADERS + PARSER_SOURCES
NETDATA_SOURCES += PLUGIN_CGROUPS_HEADERS + PLUGIN_CGROUPS_SOURCES
NETDATA_SOURCES += PLUGIN_CHECKS_SOURCES
NETDATA_SOURCES += PLUGIN_IDLEJITTER_SOURCES
NETDATA_SOURCES += PLUGIN_DISKSPACE_SOURCES
NETDATA_SOURCES += PLUGIN_TC_SOURCES
NETDATA_SOURCES += PLUGIN_TIMEX_SOURCES
NETDATA_SOURCES += PLUGIN_PLUGINSD_HEADERS + PLUGIN_PLUGINSD_SOURCES
NETDATA_SOURCES += PLUGIN_PROC_HEADERS + PLUGIN_PROC_SOURCES
NETDATA_SOURCES += PLUGIN_STATSD_SOURCES
NETDATA_SOURCES += REGISTRY_HEADERS + REGISTRY_SOURCES
NETDATA_SOURCES += SPAWN_HEADERS + SPAWN_SOURCES
NETDATA_SOURCES += STREAMING_HEADERS + STREAMING_SOURCES
NETDATA_SOURCES += WEB_HEADERS + WEB_SOURCES

cc_binary(
    name = "netdata",
    srcs = NETDATA_SOURCES ,
    copts = [
        "-I netdata/mqtt_websockets/src/include",
        "-I netdata/mqtt_websockets/c-rbuf/include",
        "-I netdata/mqtt_websockets/MQTT-C/include",
        "-I netdata",
    ],
    linkopts = [
        '-lm',
    ],
    deps = [
        "//third_party/projects/libuv:libuv",
        "//third_party/projects/lz4:lz4",
        "//third_party/projects/util-linux:util-linux",
        "//third_party/projects/openssl:openssl",
        "//third_party/projects/json-c:json-c",
        "@zlib//:zlib",
    ] + [
        '//bazel/build_settings:macro-definitions'
    ] + if_dbengine([
        "//third_party/projects/judy:judy"
    ]) + if_aclk([
        ":aclk_cc_protos"
    ]),
    defines = [
        # Assume these exist on Linux.
        "HAVE_ACCEPT4",
        "HAVE_CLOCK_GETTIME",
        "HAVE_CLOCKID_T",
        "HAVE_STRUCT_TIMESPEC",
        "HAVE_GETPRIORITY",
        "HAVE_NICE",
        "HAVE_PTHREAD_GETNAME_NP",
        "HAVE_RECVMMSG",
        "HAVE_SETNS",
        "HAVE_STRERROR_R",

        "HAVE_SCHED_GETSCHEDULER",
        "HAVE_SCHED_SETSCHEDULER",
        "HAVE_SCHED_GET_PRIORITY_MIN",
        "HAVE_SCHED_GET_PRIORITY_MAX",

        "MAJOR_IN_SYSMACROS",
        "STRERROR_R_CHAR_P",

        # Assume the are supported in Linux toolchains.
        "HAVE_FUNC_ATTRIBUTE_FORMAT",
        "HAVE_FUNC_ATTRIBUTE_MALLOC",
        "HAVE_FUNC_ATTRIBUTE_NOINLINE",
        "HAVE_FUNC_ATTRIBUTE_NORETURN",
        "HAVE_FUNC_ATTRIBUTE_RETURNS_NONNULL",
        "HAVE_FUNC_ATTRIBUTE_WARN_UNUSED_RESULT",
        "HAVE_C___ATOMIC",

        # Provided by our Linux toolchains.
        "HAVE_CRYPTO",
        "HAVE_C_MALLINFO",
        "HAVE_C_MALLOPT",

        # Linux headers.
        "HAVE_NETINET_IN_H",
        "HAVE_RESOLVE_H",
        "HAVE_NETDB_H",
        "HAVE_SYS_PRCTL_H",
        "HAVE_SYS_STAT_H",
        "HAVE_SYS_VFS_H",
        "HAVE_SYS_STATFS_H",
        "HAVE_LINUX_MAGIC_H",
        "HAVE_SYS_MOUNT_H",
        "HAVE_SYS_STATVFS_H",
        "HAVE_INTTYPES_H",
        "HAVE_STDINT_H",

        "_GNU_SOURCE",

        "STORAGE_WITH_MATH",
        "NETDATA_WITH_ZLIB",
        "ENABLE_JSONC",

        'CONFIGURE_COMMAND="\\"trololol\\""',
        'VERSION="\\"123\\""',
    ] +
    if_dbengine(["ENABLE_DBENGINE"]) +
    if_streaming_compression(["ENABLE_COMPRESSION"]) +
    if_https(["ENABLE_HTTPS"]),
    visibility = [
        '//visibility:public',
    ],
)
