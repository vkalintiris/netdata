// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_RDB_H
#define NETDATA_RDB_H

#include "database/rrd.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct rdb_instance rdb_instance_t;

rdb_instance_t *rdb_init();
void rdb_fini();

struct rdb_collect_handle {
    struct storage_collect_handle common; // has to be first item

    STORAGE_METRIC_HANDLE *db_metric_handle;
    RRDDIM *rd;
};

struct rdb_query_handle {
    STORAGE_METRIC_HANDLE *db_metric_handle;
    time_t dt;
    time_t next_timestamp;
    time_t last_timestamp;
    time_t slot_timestamp;
    size_t slot;
    size_t last_slot;
};

STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *db_instance);
STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *db_instance, uuid_t *uuid);
STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *db_metric_handle);
void rdb_metric_release(STORAGE_METRIC_HANDLE *db_metric_handle);

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *db_instance, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s);

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *db_instance, uuid_t *uuid);
void rdb_metrics_group_release(STORAGE_INSTANCE *db_instance, STORAGE_METRICS_GROUP *smg);

STORAGE_COLLECT_HANDLE *rdb_collect_init(STORAGE_METRIC_HANDLE *db_metric_handle, uint32_t update_every, STORAGE_METRICS_GROUP *smg);
void rdb_store_metric_change_collection_frequency(STORAGE_COLLECT_HANDLE *collection_handle, int update_every);
void rdb_collect_store_metric(STORAGE_COLLECT_HANDLE *collection_handle, usec_t point_in_time_ut, NETDATA_DOUBLE n,
                                 NETDATA_DOUBLE min_value,
                                 NETDATA_DOUBLE max_value,
                                 uint16_t count,
                                 uint16_t anomaly_count,
                                 SN_FLAGS flags);
void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *collection_handle);
int rdb_collect_finalize(STORAGE_COLLECT_HANDLE *collection_handle);

void rdb_query_init(STORAGE_METRIC_HANDLE *db_metric_handle, struct storage_engine_query_handle *handle, time_t start_time_s, time_t end_time_s, STORAGE_PRIORITY priority);
STORAGE_POINT rdb_query_next_metric(struct storage_engine_query_handle *handle);
int rdb_query_is_finished(struct storage_engine_query_handle *handle);
void rdb_query_finalize(struct storage_engine_query_handle *handle);
time_t rdb_query_latest_time_s(STORAGE_METRIC_HANDLE *db_metric_handle);
time_t rdb_query_oldest_time_s(STORAGE_METRIC_HANDLE *db_metric_handle);
time_t rdb_query_align_to_optimal_before(struct storage_engine_query_handle *rrddim_handle);

#ifdef __cplusplus
}
#endif

#endif // NETDATA_RDB_H
