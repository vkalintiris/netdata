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
STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *si);
STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh);
void rdb_metric_release(STORAGE_METRIC_HANDLE *smh);
bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *db_instance, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s);

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

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch);
int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch);

#ifdef __cplusplus
}
#endif

#endif /* RDB_H */