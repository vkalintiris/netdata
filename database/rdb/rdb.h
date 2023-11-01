#ifndef RDB_H
#define RDB_H

#include "database/rrd.h"

#ifdef __cplusplus
extern "C" {
#endif

/*
 * STORAGE_METRIC_HANDLE
*/

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid);
STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg, RRDDIM *rd);
STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh);

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh);
bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *db_instance, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s);

time_t rdb_metric_oldest_time(STORAGE_METRIC_HANDLE *smh);
time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *smh);

/*
 * STORAGE_METRICS_GROUP
*/

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid);
void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg);

/*
 * STORAGE_COLLECT_HANDLE
*/

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh, uint32_t update_every, STORAGE_METRICS_GROUP *smg);

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags);

void rdb_store_metric_change_collection_frequency(STORAGE_COLLECT_HANDLE *sch, int update_every_s);

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch);
int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch);

/*
 * STORAGE_ENGINE_QUERY_HANDLE
*/

void rdb_load_metric_init(STORAGE_METRIC_HANDLE *smh, struct storage_engine_query_handle *seqh,
                          time_t start_time_s, time_t end_time_s, STORAGE_PRIORITY priority);

STORAGE_POINT rdb_load_metric_next(struct storage_engine_query_handle *seqh);

int rdb_query_is_finished(struct storage_engine_query_handle *handle);

void rdb_load_metric_finalize(struct storage_engine_query_handle *seqh);

/*
 * STORAGE_INSTANCE
*/

time_t rdb_global_first_time_s(STORAGE_INSTANCE *si);

uint64_t rdb_disk_space_used(STORAGE_INSTANCE *si);

void rdb_init();
void rdb_flush();
void rdb_fini();

#ifdef ENABLE_BENCHMARKS
int rdb_profile_main(int argc, char *argv[]);
extern STORAGE_INSTANCE *RDB_StorageInstance;
#endif

#ifdef ENABLE_TESTS
int rdb_tests_main(int argc, char *argv[]);
#endif

#ifdef __cplusplus
}
#endif

#endif /* RDB_H */
