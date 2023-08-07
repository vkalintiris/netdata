set(LIBJUDY_PREV_FILES libnetdata/libjudy/src/JudyL/JudyLPrev.c
                       libnetdata/libjudy/src/JudyL/JudyLPrevEmpty.c)

set(LIBJUDY_NEXT_FILES libnetdata/libjudy/src/JudyL/JudyLNext.c
                       libnetdata/libjudy/src/JudyL/JudyLNextEmpty.c)

set(LIBJUDY_SOURCES libnetdata/libjudy/src/Judy.h
                    libnetdata/libjudy/src/JudyCommon/JudyMalloc.c
                    libnetdata/libjudy/src/JudyCommon/JudyPrivate.h
                    libnetdata/libjudy/src/JudyCommon/JudyPrivate1L.h
                    libnetdata/libjudy/src/JudyCommon/JudyPrivateBranch.h
                    libnetdata/libjudy/src/JudyL/JudyL.h
                    libnetdata/libjudy/src/JudyL/JudyLByCount.c
                    libnetdata/libjudy/src/JudyL/JudyLCascade.c
                    libnetdata/libjudy/src/JudyL/JudyLCount.c
                    libnetdata/libjudy/src/JudyL/JudyLCreateBranch.c
                    libnetdata/libjudy/src/JudyL/JudyLDecascade.c
                    libnetdata/libjudy/src/JudyL/JudyLDel.c
                    libnetdata/libjudy/src/JudyL/JudyLFirst.c
                    libnetdata/libjudy/src/JudyL/JudyLFreeArray.c
                    libnetdata/libjudy/src/JudyL/j__udyLGet.c
                    libnetdata/libjudy/src/JudyL/JudyLGet.c
                    libnetdata/libjudy/src/JudyL/JudyLInsArray.c
                    libnetdata/libjudy/src/JudyL/JudyLIns.c
                    libnetdata/libjudy/src/JudyL/JudyLInsertBranch.c
                    libnetdata/libjudy/src/JudyL/JudyLMallocIF.c
                    libnetdata/libjudy/src/JudyL/JudyLMemActive.c
                    libnetdata/libjudy/src/JudyL/JudyLMemUsed.c
                    libnetdata/libjudy/src/JudyL/JudyLTables.c
                    libnetdata/libjudy/src/JudyHS/JudyHS.c
                    ${LIBJUDY_PREV_FILES}
                    ${LIBJUDY_NEXT_FILES})

set(LIBNETDATA_FILES libnetdata/adaptive_resortable_list/adaptive_resortable_list.c
                     libnetdata/adaptive_resortable_list/adaptive_resortable_list.h
                     libnetdata/config/appconfig.c
                     libnetdata/config/appconfig.h
                     libnetdata/aral/aral.c
                     libnetdata/aral/aral.h
                     libnetdata/avl/avl.c
                     libnetdata/avl/avl.h
                     libnetdata/buffer/buffer.c
                     libnetdata/buffer/buffer.h
                     libnetdata/circular_buffer/circular_buffer.c
                     libnetdata/circular_buffer/circular_buffer.h
                     libnetdata/clocks/clocks.c
                     libnetdata/clocks/clocks.h
                     libnetdata/completion/completion.c
                     libnetdata/completion/completion.h
                     libnetdata/dictionary/dictionary.c
                     libnetdata/dictionary/dictionary.h
                     libnetdata/eval/eval.c
                     libnetdata/eval/eval.h
                     libnetdata/gorilla/gorilla.cc
                     libnetdata/gorilla/gorilla.h
                     libnetdata/health/health.c
                     libnetdata/health/health.h
                     libnetdata/july/july.c
                     libnetdata/july/july.h
                     libnetdata/inlined.h
                     libnetdata/json/json.c
                     libnetdata/json/json.h
                     libnetdata/json/jsmn.c
                     libnetdata/json/jsmn.h
                     libnetdata/libnetdata.c
                     libnetdata/libnetdata.h
                     libnetdata/locks/locks.c
                     libnetdata/locks/locks.h
                     libnetdata/log/log.c
                     libnetdata/log/log.h
                     libnetdata/os.c
                     libnetdata/os.h
                     libnetdata/onewayalloc/onewayalloc.c
                     libnetdata/onewayalloc/onewayalloc.h
                     libnetdata/popen/popen.c
                     libnetdata/popen/popen.h
                     libnetdata/procfile/procfile.c
                     libnetdata/procfile/procfile.h
                     libnetdata/required_dummies.h
                     libnetdata/socket/security.c
                     libnetdata/socket/security.h
                     libnetdata/simple_pattern/simple_pattern.c
                     libnetdata/simple_pattern/simple_pattern.h
                     libnetdata/socket/socket.c
                     libnetdata/socket/socket.h
                     libnetdata/statistical/statistical.c
                     libnetdata/statistical/statistical.h
                     libnetdata/storage_number/storage_number.c
                     libnetdata/storage_number/storage_number.h
                     libnetdata/string/string.c
                     libnetdata/string/string.h
                     libnetdata/threads/threads.c
                     libnetdata/threads/threads.h
                     libnetdata/url/url.c
                     libnetdata/url/url.h
                     libnetdata/string/utf8.h
                     libnetdata/worker_utilization/worker_utilization.c
                     libnetdata/worker_utilization/worker_utilization.h
                     libnetdata/http/http_defs.h
                     libnetdata/dyn_conf/dyn_conf.c
                     libnetdata/dyn_conf/dyn_conf.h)

if(ENABLE_EBPF)
    list(APPEND LIBNETDATA_FILES libnetdata/ebpf/ebpf.c
                                 libnetdata/ebpf/ebpf.h)
endif()

set(DAEMON_FILES daemon/buildinfo.c
                 daemon/buildinfo.h
                 daemon/common.c
                 daemon/common.h
                 daemon/daemon.c
                 daemon/daemon.h
                 daemon/event_loop.c
                 daemon/event_loop.h
                 daemon/global_statistics.c
                 daemon/global_statistics.h
                 daemon/analytics.c
                 daemon/analytics.h
                 daemon/main.c
                 daemon/main.h
                 daemon/signals.c
                 daemon/signals.h
                 daemon/service.c
                 daemon/static_threads.c
                 daemon/static_threads.h
                 daemon/commands.c
                 daemon/commands.h
                 daemon/pipename.c
                 daemon/pipename.h
                 daemon/unit_test.c
                 daemon/unit_test.h)

set(API_PLUGIN_FILES web/api/web_api.c
                     web/api/web_api.h
                     web/api/web_api_v1.c
                     web/api/web_api_v1.h
                     web/api/web_api_v2.c
                     web/api/web_api_v2.h
                     web/api/badges/web_buffer_svg.c
                     web/api/badges/web_buffer_svg.h
                     web/api/exporters/allmetrics.c
                     web/api/exporters/allmetrics.h
                     web/api/exporters/shell/allmetrics_shell.c
                     web/api/exporters/shell/allmetrics_shell.h
                     web/api/queries/rrdr.c
                     web/api/queries/rrdr.h
                     web/api/queries/query.c
                     web/api/queries/query.h
                     web/api/queries/average/average.c
                     web/api/queries/average/average.h
                     web/api/queries/countif/countif.c
                     web/api/queries/countif/countif.h
                     web/api/queries/incremental_sum/incremental_sum.c
                     web/api/queries/incremental_sum/incremental_sum.h
                     web/api/queries/max/max.c
                     web/api/queries/max/max.h
                     web/api/queries/min/min.c
                     web/api/queries/min/min.h
                     web/api/queries/sum/sum.c
                     web/api/queries/sum/sum.h
                     web/api/queries/median/median.c
                     web/api/queries/median/median.h
                     web/api/queries/percentile/percentile.c
                     web/api/queries/percentile/percentile.h
                     web/api/queries/stddev/stddev.c
                     web/api/queries/stddev/stddev.h
                     web/api/queries/ses/ses.c
                     web/api/queries/ses/ses.h
                     web/api/queries/des/des.c
                     web/api/queries/des/des.h
                     web/api/queries/trimmed_mean/trimmed_mean.c
                     web/api/queries/trimmed_mean/trimmed_mean.h
                     web/api/queries/weights.c
                     web/api/queries/weights.h
                     web/api/formatters/rrd2json.c
                     web/api/formatters/rrd2json.h
                     web/api/formatters/csv/csv.c
                     web/api/formatters/csv/csv.h
                     web/api/formatters/json/json.c
                     web/api/formatters/json/json.h
                     web/api/formatters/ssv/ssv.c
                     web/api/formatters/ssv/ssv.h
                     web/api/formatters/value/value.c
                     web/api/formatters/value/value.h
                     web/api/formatters/json_wrapper.c
                     web/api/formatters/json_wrapper.h
                     web/api/formatters/charts2json.c
                     web/api/formatters/charts2json.h
                     web/api/formatters/rrdset2json.c
                     web/api/formatters/rrdset2json.h
                     web/api/health/health_cmdapi.c
                     web/rtc/webrtc.c
                     web/rtc/webrtc.h)

set(EXPORTING_ENGINE_FILES exporting/exporting_engine.c
                           exporting/exporting_engine.h
                           exporting/graphite/graphite.c
                           exporting/graphite/graphite.h
                           exporting/json/json.c
                           exporting/json/json.h
                           exporting/opentsdb/opentsdb.c
                           exporting/opentsdb/opentsdb.h
                           exporting/prometheus/prometheus.c
                           exporting/prometheus/prometheus.h
                           exporting/read_config.c
                           exporting/clean_connectors.c
                           exporting/init_connectors.c
                           exporting/process_data.c
                           exporting/check_filters.c
                           exporting/send_data.c
                           exporting/send_internal_metrics.c)

set(HEALTH_PLUGIN_FILES health/health.c
                        health/health.h
                        health/health_config.c
                        health/health_json.c
                        health/health_log.c)

set(IDLEJITTER_PLUGIN_FILES collectors/idlejitter.plugin/plugin_idlejitter.c)


if(ENABLE_ML)
        set(ML_FILES ml/ad_charts.h
                     ml/ad_charts.cc
                     ml/Config.cc
                     ml/dlib/dlib/all/source.cpp
                     ml/ml.h
                     ml/ml.cc
                     ml/ml-private.h)
else()
        set(ML_FILES ml/ml.h
                     ml/ml-dummy.c)
endif()

set(PLUGINSD_PLUGIN_FILES collectors/plugins.d/plugins_d.c
                          collectors/plugins.d/plugins_d.h
                          collectors/plugins.d/pluginsd_parser.c
                          collectors/plugins.d/pluginsd_parser.h)

set(RRD_PLUGIN_FILES database/contexts/api_v1.c
                     database/contexts/api_v2.c
                     database/contexts/context.c
                     database/contexts/instance.c
                     database/contexts/internal.h
                     database/contexts/metric.c
                     database/contexts/query_scope.c
                     database/contexts/query_target.c
                     database/contexts/rrdcontext.c
                     database/contexts/rrdcontext.h
                     database/contexts/worker.c
                     database/rrdcalc.c
                     database/rrdcalc.h
                     database/rrdcalctemplate.c
                     database/rrdcalctemplate.h
                     database/rrddim.c
                     database/rrddimvar.c
                     database/rrddimvar.h
                     database/rrdfamily.c
                     database/rrdfunctions.c
                     database/rrdfunctions.h
                     database/rrdhost.c
                     database/rrdlabels.c
                     database/rrd.c
                     database/rrd.h
                     database/rrdset.c
                     database/rrdsetvar.c
                     database/rrdsetvar.h
                     database/rrdvar.c
                     database/rrdvar.h
                     database/storage_engine.c
                     database/storage_engine.h
                     database/ram/rrddim_mem.c
                     database/ram/rrddim_mem.h
                     database/sqlite/sqlite_metadata.c
                     database/sqlite/sqlite_metadata.h
                     database/sqlite/sqlite_functions.c
                     database/sqlite/sqlite_functions.h
                     database/sqlite/sqlite_context.c
                     database/sqlite/sqlite_context.h
                     database/sqlite/sqlite_db_migration.c
                     database/sqlite/sqlite_db_migration.h
                     database/sqlite/sqlite_aclk.c
                     database/sqlite/sqlite_aclk.h
                     database/sqlite/sqlite_health.c
                     database/sqlite/sqlite_health.h
                     database/sqlite/sqlite_aclk_node.c
                     database/sqlite/sqlite_aclk_node.h
                     database/sqlite/sqlite_aclk_alert.c
                     database/sqlite/sqlite_aclk_alert.h
                     database/sqlite/sqlite3.c
                     database/sqlite/sqlite3.h
                     database/engine/rrdengine.c
                     database/engine/rrdengine.h
                     database/engine/rrddiskprotocol.h
                     database/engine/datafile.c
                     database/engine/datafile.h
                     database/engine/journalfile.c
                     database/engine/journalfile.h
                     database/engine/rrdenginelib.c
                     database/engine/rrdenginelib.h
                     database/engine/rrdengineapi.c
                     database/engine/rrdengineapi.h
                     database/engine/pagecache.c
                     database/engine/pagecache.h
                     database/engine/cache.c
                     database/engine/cache.h
                     database/engine/metric.c
                     database/engine/metric.h
                     database/engine/pdc.c
                     database/engine/pdc.h
                     database/KolmogorovSmirnovDist.c
                     database/KolmogorovSmirnovDist.h)

set(REGISTRY_PLUGIN_FILES registry/registry.c
                          registry/registry.h
                          registry/registry_db.c
                          registry/registry_init.c
                          registry/registry_internals.c
                          registry/registry_internals.h
                          registry/registry_log.c
                          registry/registry_machine.c
                          registry/registry_machine.h
                          registry/registry_person.c
                          registry/registry_person.h)

set(STATSD_PLUGIN_FILES collectors/statsd.plugin/statsd.c)

set(STREAMING_PLUGIN_FILES streaming/rrdpush.c
                           streaming/rrdpush.h
                           streaming/compression.c
                           streaming/receiver.c
                           streaming/sender.c
                           streaming/replication.c
                           streaming/replication.h)

set(WEB_PLUGIN_FILES web/server/web_client.c
                     web/server/web_client.h
                     web/server/web_server.c
                     web/server/web_server.h
                     web/server/static/static-threaded.c
                     web/server/static/static-threaded.h
                     web/server/web_client_cache.c
                     web/server/web_client_cache.h)

set(CLAIM_PLUGIN_FILES claim/claim.c
                       claim/claim.h)

set(SPAWN_PLUGIN_FILES spawn/spawn.c
                       spawn/spawn_server.c
                       spawn/spawn_client.c
                       spawn/spawn.h)

set(ACLK_ALWAYS_BUILD aclk/aclk_rrdhost_state.h
                      aclk/aclk_proxy.c
                      aclk/aclk_proxy.h
                      aclk/aclk.c
                      aclk/aclk.h
                      aclk/aclk_capas.c
                      aclk/aclk_capas.h)

set(TIMEX_PLUGIN_FILES collectors/timex.plugin/plugin_timex.c)

set(PROFILE_PLUGIN_FILES collectors/profile.plugin/plugin_profile.cc)

set(CGROUPS_PLUGIN_FILES collectors/cgroups.plugin/sys_fs_cgroup.c
                         collectors/cgroups.plugin/sys_fs_cgroup.h)

set(DISKSPACE_PLUGIN_FILES collectors/diskspace.plugin/plugin_diskspace.c)

set(PROC_PLUGIN_FILES collectors/proc.plugin/ipc.c
                      collectors/proc.plugin/plugin_proc.c
                      collectors/proc.plugin/plugin_proc.h
                      collectors/proc.plugin/proc_sys_fs_file_nr.c
                      collectors/proc.plugin/proc_diskstats.c
                      collectors/proc.plugin/proc_mdstat.c
                      collectors/proc.plugin/proc_interrupts.c
                      collectors/proc.plugin/proc_softirqs.c
                      collectors/proc.plugin/proc_loadavg.c
                      collectors/proc.plugin/proc_meminfo.c
                      collectors/proc.plugin/proc_pagetypeinfo.c
                      collectors/proc.plugin/proc_net_dev.c
                      collectors/proc.plugin/proc_net_wireless.c
                      collectors/proc.plugin/proc_net_ip_vs_stats.c
                      collectors/proc.plugin/proc_net_netstat.c
                      collectors/proc.plugin/proc_net_rpc_nfs.c
                      collectors/proc.plugin/proc_net_rpc_nfsd.c
                      collectors/proc.plugin/proc_net_sctp_snmp.c
                      collectors/proc.plugin/proc_net_sockstat.c
                      collectors/proc.plugin/proc_net_sockstat6.c
                      collectors/proc.plugin/proc_net_softnet_stat.c
                      collectors/proc.plugin/proc_net_stat_conntrack.c
                      collectors/proc.plugin/proc_net_stat_synproxy.c
                      collectors/proc.plugin/proc_self_mountinfo.c
                      collectors/proc.plugin/proc_self_mountinfo.h
                      collectors/proc.plugin/zfs_common.c
                      collectors/proc.plugin/zfs_common.h
                      collectors/proc.plugin/proc_spl_kstat_zfs.c
                      collectors/proc.plugin/proc_stat.c
                      collectors/proc.plugin/proc_sys_kernel_random_entropy_avail.c
                      collectors/proc.plugin/proc_vmstat.c
                      collectors/proc.plugin/proc_uptime.c
                      collectors/proc.plugin/proc_pressure.c
                      collectors/proc.plugin/proc_pressure.h
                      collectors/proc.plugin/sys_kernel_mm_ksm.c
                      collectors/proc.plugin/sys_block_zram.c
                      collectors/proc.plugin/sys_devices_system_edac_mc.c
                      collectors/proc.plugin/sys_devices_system_node.c
                      collectors/proc.plugin/sys_class_infiniband.c
                      collectors/proc.plugin/sys_fs_btrfs.c
                      collectors/proc.plugin/sys_class_power_supply.c
                      collectors/proc.plugin/sys_devices_pci_aer.c)

set(TC_PLUGIN_FILES collectors/tc.plugin/plugin_tc.c)

set(NETDATACLI_FILES daemon/commands.h
                     daemon/pipename.c
                     daemon/pipename.h
                     cli/cli.c
                     cli/cli.h)

set(NETDATA_FILES collectors/all.h
                  ${DAEMON_FILES}
                  ${API_PLUGIN_FILES}
                  ${EXPORTING_ENGINE_FILES}
                  ${HEALTH_PLUGIN_FILES}
                  ${IDLEJITTER_PLUGIN_FILES}
                  ${ML_FILES}
                  ${PLUGINSD_PLUGIN_FILES}
                  ${RRD_PLUGIN_FILES}
                  ${REGISTRY_PLUGIN_FILES}
                  ${STATSD_PLUGIN_FILES}
                  ${STREAMING_PLUGIN_FILES}
                  ${WEB_PLUGIN_FILES}
                  ${CLAIM_PLUGIN_FILES}
                  ${SPAWN_PLUGIN_FILES}
                  ${ACLK_ALWAYS_BUILD}
                  ${TIMEX_PLUGIN_FILES}
                  ${PROFILE_PLUGIN_FILES})

# linux only
list(APPEND NETDATA_FILES daemon/static_threads_linux.c
                          ${CGROUPS_PLUGIN_FILES}
                          ${DISKSPACE_PLUGIN_FILES}
                          ${PROC_PLUGIN_FILES}
                          ${TC_PLUGIN_FILES})

set(MQTT_WEBSOCKETS_FILES mqtt_websockets/src/mqtt_wss_client.c
                          mqtt_websockets/src/include/mqtt_wss_client.h
                          mqtt_websockets/src/mqtt_wss_log.c
                          mqtt_websockets/src/include/mqtt_wss_log.h
                          mqtt_websockets/src/ws_client.c
                          mqtt_websockets/src/include/ws_client.h
                          mqtt_websockets/src/mqtt_ng.c
                          mqtt_websockets/src/include/mqtt_ng.h
                          mqtt_websockets/src/common_public.c
                          mqtt_websockets/src/include/common_public.h
                          mqtt_websockets/src/include/common_internal.h
                          mqtt_websockets/c-rbuf/src/ringbuffer.c
                          mqtt_websockets/c-rbuf/include/ringbuffer.h
                          mqtt_websockets/c-rbuf/src/ringbuffer_internal.h
                          mqtt_websockets/c_rhash/src/c_rhash.c
                          mqtt_websockets/c_rhash/include/c_rhash.h
                          mqtt_websockets/c_rhash/src/c_rhash_internal.h)

set(ACLK_PROTO_DEFS aclk/aclk-schemas/proto/aclk/v1/lib.proto
                    aclk/aclk-schemas/proto/agent/v1/disconnect.proto
                    aclk/aclk-schemas/proto/agent/v1/connection.proto
                    aclk/aclk-schemas/proto/alarm/v1/config.proto
                    aclk/aclk-schemas/proto/alarm/v1/stream.proto
                    aclk/aclk-schemas/proto/nodeinstance/connection/v1/connection.proto
                    aclk/aclk-schemas/proto/nodeinstance/create/v1/creation.proto
                    aclk/aclk-schemas/proto/nodeinstance/info/v1/info.proto
                    aclk/aclk-schemas/proto/context/v1/context.proto
                    aclk/aclk-schemas/proto/context/v1/stream.proto
                    aclk/aclk-schemas/proto/agent/v1/cmds.proto)

set(ACLK_FILES aclk/aclk_util.c
               aclk/aclk_util.h
               aclk/aclk_stats.c
               aclk/aclk_stats.h
               aclk/aclk_query.c
               aclk/aclk_query.h
               aclk/aclk_query_queue.c
               aclk/aclk_query_queue.h
               aclk/aclk_otp.c
               aclk/aclk_otp.h
               aclk/aclk_tx_msgs.c
               aclk/aclk_tx_msgs.h
               aclk/aclk_rx_msgs.c
               aclk/aclk_rx_msgs.h
               aclk/https_client.c
               aclk/https_client.h
               aclk/aclk_alarm_api.c
               aclk/aclk_alarm_api.h
               aclk/aclk_contexts_api.c
               aclk/aclk_contexts_api.h
               aclk/schema-wrappers/connection.cc
               aclk/schema-wrappers/connection.h
               aclk/schema-wrappers/node_connection.cc
               aclk/schema-wrappers/node_connection.h
               aclk/schema-wrappers/node_creation.cc
               aclk/schema-wrappers/node_creation.h
               aclk/schema-wrappers/alarm_stream.cc
               aclk/schema-wrappers/alarm_stream.h
               aclk/schema-wrappers/alarm_config.cc
               aclk/schema-wrappers/alarm_config.h
               aclk/schema-wrappers/node_info.cc
               aclk/schema-wrappers/node_info.h
               aclk/schema-wrappers/capability.cc
               aclk/schema-wrappers/capability.h
               aclk/schema-wrappers/proto_2_json.cc
               aclk/schema-wrappers/proto_2_json.h
               aclk/schema-wrappers/context_stream.cc
               aclk/schema-wrappers/context_stream.h
               aclk/schema-wrappers/context.cc
               aclk/schema-wrappers/context.h
               aclk/schema-wrappers/schema_wrappers.h
               aclk/schema-wrappers/schema_wrapper_utils.cc
               aclk/schema-wrappers/schema_wrapper_utils.h
               aclk/schema-wrappers/agent_cmds.cc
               aclk/schema-wrappers/agent_cmds.h
               aclk/helpers/mqtt_wss_pal.h
               aclk/helpers/ringbuffer_pal.h)

set(DEBUGFS_PLUGIN_FILES collectors/debugfs.plugin/debugfs_plugin.c
                         collectors/debugfs.plugin/debugfs_plugin.h
                         collectors/debugfs.plugin/debugfs_extfrag.c
                         collectors/debugfs.plugin/debugfs_zswap.c
                         collectors/debugfs.plugin/sys_devices_virtual_powercap.c)

set(APPS_PLUGIN_FILES collectors/apps.plugin/apps_plugin.c)

set(FREEIPMI_PLUGIN_FILES collectors/freeipmi.plugin/freeipmi_plugin.c)

set(NFACCT_PLUGIN_FILES collectors/nfacct.plugin/plugin_nfacct.c)
