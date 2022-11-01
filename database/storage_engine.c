// SPDX-License-Identifier: GPL-3.0-or-later

#include "storage_engine.h"
#include "ram/rrddim_mem.h"
#ifdef ENABLE_DBENGINE
#include "engine/rrdengineapi.h"
#endif

STORAGE_METRIC_HANDLE *se_metric_get(RRD_MEMORY_MODE mode,
                                     STORAGE_INSTANCE *db_instance,
                                     uuid_t *uuid,
                                     STORAGE_METRICS_GROUP *smg)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_metric_get(db_instance, uuid, smg);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_metric_get(db_instance, uuid, smg);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

STORAGE_METRIC_HANDLE *se_metric_get_or_create(RRD_MEMORY_MODE mode,
                                               RRDDIM *rd,
                                               STORAGE_INSTANCE *db_instance,
                                               STORAGE_METRICS_GROUP *smg)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_metric_get_or_create(rd, db_instance, smg);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_metric_get_or_create(rd, db_instance, smg);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

STORAGE_METRIC_HANDLE *se_metric_dup(RRD_MEMORY_MODE mode,
                                     STORAGE_METRIC_HANDLE *db_metric_handle)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_metric_dup(db_metric_handle);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_metric_dup(db_metric_handle);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

void se_metric_release(RRD_MEMORY_MODE mode, STORAGE_METRIC_HANDLE *db_metric_handle)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            rrddim_metric_release(db_metric_handle);
            return;
        case RRD_MEMORY_MODE_DBENGINE:
            rrdeng_metric_release(db_metric_handle);
            return;
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

STORAGE_COLLECT_HANDLE *se_store_metric_init(RRD_MEMORY_MODE mode,
                                             STORAGE_METRIC_HANDLE *db_metric_handle,
                                             uint32_t update_every)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_collect_init(db_metric_handle, update_every);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_store_metric_init(db_metric_handle, update_every);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

void se_store_metric_next(RRD_MEMORY_MODE mode, STORAGE_COLLECT_HANDLE *collection_handle,
                          usec_t point_in_time_ut, NETDATA_DOUBLE n,
                          NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                          uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            rrddim_collect_store_metric(collection_handle, point_in_time_ut, n, min_value, max_value, count, anomaly_count, flags);
            break;
        case RRD_MEMORY_MODE_DBENGINE:
            rrdeng_store_metric_next(collection_handle, point_in_time_ut, n, min_value, max_value, count, anomaly_count, flags);
            break;
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

void se_store_metric_flush_current_page(RRD_MEMORY_MODE mode, STORAGE_COLLECT_HANDLE *collection_handle)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            rrddim_store_metric_flush(collection_handle);
            break;
        case RRD_MEMORY_MODE_DBENGINE:
            rrdeng_store_metric_flush_current_page(collection_handle);
            break;
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

int se_collect_finalize(RRD_MEMORY_MODE mode, STORAGE_COLLECT_HANDLE *collection_handle)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_collect_finalize(collection_handle);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_store_metric_finalize(collection_handle);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

void se_store_metric_change_collection_frequency(RRD_MEMORY_MODE mode,
                                                 STORAGE_COLLECT_HANDLE *collection_handle,
                                                 int update_every)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            rrddim_store_metric_change_collection_frequency(collection_handle, update_every);
            break;
        case RRD_MEMORY_MODE_DBENGINE:
            rrdeng_store_metric_change_collection_frequency(collection_handle, update_every);
            break;
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

STORAGE_METRICS_GROUP *se_metrics_group_get(RRD_MEMORY_MODE mode,
                                            STORAGE_INSTANCE *db_instance,
                                            uuid_t *uuid)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_metrics_group_get(db_instance, uuid);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_metrics_group_get(db_instance, uuid);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

void se_metrics_group_release(RRD_MEMORY_MODE mode,
                              STORAGE_INSTANCE *db_instance,
                              STORAGE_METRICS_GROUP *smg __maybe_unused)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_metrics_group_release(db_instance, smg);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_metrics_group_release(db_instance, smg);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

void se_query_init(RRD_MEMORY_MODE mode,
                   STORAGE_METRIC_HANDLE *db_metric_handle,
                   struct storage_engine_query_handle *handle,
                   time_t start_time, time_t end_time)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            rrddim_query_init(db_metric_handle, handle, start_time, end_time);
            break;
        case RRD_MEMORY_MODE_DBENGINE:
            rrdeng_load_metric_init(db_metric_handle, handle, start_time, end_time);
            break;
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}


STORAGE_POINT se_query_next_metric(RRD_MEMORY_MODE mode, struct storage_engine_query_handle *handle)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_query_next_metric(handle);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_load_metric_next(handle);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

int se_query_is_finished(RRD_MEMORY_MODE mode, struct storage_engine_query_handle *handle)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_query_is_finished(handle);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_load_metric_is_finished(handle);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

void se_query_finalize(RRD_MEMORY_MODE mode, struct storage_engine_query_handle *handle)
{
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            rrddim_query_finalize(handle);
            break;
        case RRD_MEMORY_MODE_DBENGINE:
            rrdeng_load_metric_finalize(handle);
            break;
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

time_t se_metric_latest_time(RRD_MEMORY_MODE mode, STORAGE_METRIC_HANDLE *db_metric_handle) {
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_query_latest_time(db_metric_handle);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_metric_latest_time(db_metric_handle);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}

time_t se_metric_oldest_time(RRD_MEMORY_MODE mode, STORAGE_METRIC_HANDLE *db_metric_handle) {
    switch (mode) {
        case RRD_MEMORY_MODE_NONE:
        case RRD_MEMORY_MODE_RAM:
        case RRD_MEMORY_MODE_MAP:
        case RRD_MEMORY_MODE_SAVE:
        case RRD_MEMORY_MODE_ALLOC:
            return rrddim_query_oldest_time(db_metric_handle);
        case RRD_MEMORY_MODE_DBENGINE:
            return rrdeng_metric_oldest_time(db_metric_handle);
        default:
            fatal("Invalid memory mode: %d", mode);
    }
}
